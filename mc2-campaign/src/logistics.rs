// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The campaign and logistics state: inventory, force group, variants, pilots, purchases,
//! availability and the mission cycle.
//!
//! Port of `code/logisticsdata.{h,cpp}` (`LogisticsData`) with the parts of `logisticsmech.cpp`
//! (`LogisticsMech`) it drives. Naming follows the C++ in snake case, so a reader can put the two
//! side by side. Deviations are deliberate and listed in `rust/docs/porting-log.md`; the main
//! ones:
//! - Pilots are identified by the profile file stem, not the localized display name.
//! - The pointer identity the C++ relies on (variants, mechs, pilots) is a key or an index.
//! - Nothing here does I/O: static data comes from [`GameData`], saves go through
//!   [`crate::save`].
//! - Multiplayer (`MPlayer`), the encyclopedia lists (`getVehicles`, `getBuildings`) and the
//!   `.var` import/export are not ported.

use std::collections::BTreeMap;
use std::sync::Arc;

use mc2_sim_data::component::ComponentForm;

use crate::data::{purchase_key, GameData};
use crate::error::{LogisticsError, LogisticsResult};
use crate::interface::{
    ForceMech, LoadSpec, LoadType, LoadoutComponent, MissionOptions, MissionResult, MoverReport,
    ObjectiveStatus, Outcome, RosterPilot, SalvageCandidate, SupportEnables,
};
use crate::mission_info::{MissionInfo, FALLBACK_PURCHASE_FILE};
use crate::pilot::Pilot;
use crate::variant::{Chassis, Ctx, Variant};

/// Highest slot of the force group the code accepts (`addMechToForceGroup`: `slot > 12` fails).
pub const MAX_FORCE_GROUP_SLOT: i32 = 12;
/// Single-player drop limit in mechs (`maxUnits` in `canAddMechToForceGroup`).
pub const MAX_FORCE_GROUP_MECHS: usize = 12;
/// Size of the `available[]` array in `updateAvailability` (component ids index it).
const AVAILABLE_LEN: usize = 255;

/// Handle of a variant in the arena (stable while the variant lives).
pub type VariantKey = u32;
/// Handle of a mech in the inventory (stable while the mech lives).
pub type MechKey = u32;

/// A mech in the inventory (`LogisticsMech`).
#[derive(Debug, Clone, PartialEq)]
pub struct Mech {
    /// Identity within the inventory.
    pub key: MechKey,
    /// `ID`: instance number in the top byte over the variant id.
    pub id: i32,
    /// The variant the mech is built to.
    pub variant: VariantKey,
    /// Index into the pilot roster.
    pub pilot: Option<usize>,
    /// `forceGroup`: `0` when not in the force group, else the slot.
    pub force_group: i32,
    /// `baseColor`, `highlightColor1`, `highlightColor2`.
    pub colors: [u32; 3],
}

impl Mech {
    /// `getInstanceID`.
    pub fn instance_id(&self) -> i32 {
        self.id >> 24
    }
}

/// What a mission completion did (see [`Logistics::apply_outcome`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutcomeEffect {
    /// The mission counted as won: its force left the inventory, survivors came back, the
    /// campaign advanced.
    pub completed: bool,
    /// The campaign has no further missions.
    pub campaign_over: bool,
}

/// One entry of the salvage list (`SalvageListItem`).
#[derive(Debug, Clone, PartialEq)]
pub struct SalvageOffer {
    /// Index into [`Outcome::salvage_candidates`].
    pub candidate: usize,
    /// The variant the candidate maps to.
    pub variant: VariantKey,
    /// Its name.
    pub variant_name: String,
    /// What recovering it costs: 80% of the variant cost, truncated. This is what ticking charges
    /// (`decrementCBills(costToSalvage)` takes an int).
    pub cost: i32,
    /// `costToSalvage` as the float it is: whether the box is enabled compares this, not `cost`
    /// (`SalvageListItem::update`: `costToSalvage > CBills`).
    pub exact_cost: f32,
    /// Tonnage shown with the entry.
    pub tonnage: f32,
    /// The wreck's paint scheme (`getPaintScheme`).
    pub paint_scheme: [u32; 3],
    /// `(current armour, maximum armour, body location destroyed)` per armour location, as the damage colours of the wreck's icon read them.
    pub armor: [(f32, f32, bool); 11],
}

/// The campaign state (`LogisticsData`).
#[derive(Debug, Clone)]
pub struct Logistics {
    pub(crate) data: Arc<GameData>,

    pub(crate) variants: BTreeMap<VariantKey, Variant>,
    pub(crate) variant_list: Vec<VariantKey>,
    pub(crate) next_variant_key: VariantKey,
    /// `LogisticsChassis::refCount` per chassis.
    pub(crate) chassis_refs: Vec<u32>,

    pub(crate) inventory: Vec<Mech>,
    pub(crate) next_mech_key: MechKey,

    pub(crate) pilots: Vec<Pilot>,
    /// Availability of each catalogue component (`LogisticsComponent::bAvailable`).
    pub(crate) component_available: Vec<bool>,

    pub(crate) mission: MissionInfo,

    /// `resourcePoints`: the in-mission support budget of the selected mission.
    pub(crate) resource_points: i64,
    /// `rpJustAdded`.
    pub(crate) rp_just_added: bool,
    pub(crate) new_mechs: bool,
    pub(crate) new_pilots: bool,
    pub(crate) new_weapons: bool,

    pub(crate) currently_modified: Option<MechKey>,
    pub(crate) old_variant: Option<VariantKey>,

    /// `MissionResults::FirstTimeResults`, saved with the campaign.
    pub(crate) first_time_results: bool,
    /// `MissionBegin::FirstTimePurchase`: the tutorial's purchase screen has not been left yet.
    pub(crate) first_time_purchase: bool,
    /// `MissionBegin::FirstTimeMechLab`: the tutorial's mech lab has not been left yet.
    pub(crate) first_time_mech_lab: bool,
}

/// `S_stricmp(a, b) == 0` for ASCII names.
fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Logistics {
    // ------------------------------------------------------------------------------------
    // construction

    /// `LogisticsData::init`: the component, pilot and variant tables, then the `campaign`
    /// script with its first mission selected.
    pub fn new(data: Arc<GameData>) -> LogisticsResult<Logistics> {
        Self::with_campaign(data, "campaign")
    }

    /// Like [`new`](Self::new) for another campaign script (`tutorial`).
    pub fn with_campaign(data: Arc<GameData>, campaign: &str) -> LogisticsResult<Logistics> {
        let campaign_data = data
            .campaigns
            .get(&campaign.to_ascii_lowercase())
            .cloned()
            .ok_or(LogisticsError::InvalidFileName)?;
        let mission = MissionInfo::new(&campaign.to_ascii_lowercase(), &campaign_data);

        let mut l = Logistics {
            variants: BTreeMap::new(),
            variant_list: Vec::new(),
            next_variant_key: 1,
            chassis_refs: vec![0; data.chassis.len()],
            inventory: Vec::new(),
            next_mech_key: 1,
            pilots: Vec::new(),
            component_available: vec![false; data.catalog.len()],
            mission,
            resource_points: 0,
            rp_just_added: false,
            new_mechs: false,
            new_pilots: false,
            new_weapons: false,
            currently_modified: None,
            old_variant: None,
            first_time_results: true,
            first_time_purchase: true,
            first_time_mech_lab: true,
            data,
        };
        l.init_pilots();
        l.init_variants();
        l.select_first_available_mission()?;
        Ok(l)
    }

    /// The `setCurrentMission(missionNames[0])` that `init` and `startNewCampaign` end with.
    fn select_first_available_mission(&mut self) -> LogisticsResult<()> {
        let names = self.mission.available_missions();
        match names.first() {
            Some(first) => {
                let first = first.clone();
                self.set_current_mission(&first)
            }
            None => Err(LogisticsError::InvalidId),
        }
    }

    /// `initPilots`: the roster as shipped.
    fn init_pilots(&mut self) {
        self.pilots = self.data.pilots.iter().map(Pilot::from_template).collect();
    }

    /// `initVariants`: builds the designer variants.
    fn init_variants(&mut self) {
        let data = Arc::clone(&self.data);
        for (chassis_index, spec) in &data.designer_variants {
            let chassis = &data.chassis[*chassis_index];
            let ctx = Ctx {
                catalog: &data.catalog,
                chassis,
            };
            let id = self.next_variant_id(chassis);
            let v = Variant::from_spec(&ctx, spec, id);
            let key = self.alloc_variant(v);
            self.variant_list.push(key);
        }
    }

    /// `(refCount << 16) | chassis.ID`, then `refCount++`.
    fn next_variant_id(&mut self, chassis: &Chassis) -> u32 {
        let count = &mut self.chassis_refs[chassis.index];
        let id = (*count << 16) | chassis.id;
        *count += 1;
        id
    }

    fn alloc_variant(&mut self, v: Variant) -> VariantKey {
        let key = self.next_variant_key;
        self.next_variant_key += 1;
        self.variants.insert(key, v);
        key
    }

    /// The `delete` of a `LogisticsVariant`: frees the arena slot and gives back the chassis
    /// reference count.
    fn free_variant(&mut self, key: VariantKey) {
        if let Some(v) = self.variants.remove(&key) {
            let refs = &mut self.chassis_refs[v.chassis];
            *refs = refs.saturating_sub(1);
        }
    }

    // ------------------------------------------------------------------------------------
    // read access

    /// The static game data.
    pub fn data(&self) -> &Arc<GameData> {
        &self.data
    }

    /// The campaign position and balance.
    pub fn mission_info(&self) -> &MissionInfo {
        &self.mission
    }

    /// `LogisticsData::getCurrentBigVideo`: take the group's pre-operation cutscene once.
    pub fn take_current_big_video(&mut self) -> Option<String> {
        self.mission.take_current_big_video()
    }

    /// All mechs in the inventory, in order.
    pub fn inventory(&self) -> &[Mech] {
        &self.inventory
    }

    /// The pilot roster.
    pub fn pilots(&self) -> &[Pilot] {
        &self.pilots
    }

    /// A variant by key.
    pub fn variant(&self, key: VariantKey) -> Option<&Variant> {
        self.variants.get(&key)
    }

    /// The variants in list order (designer variants first, then player variants in the order
    /// they were saved).
    pub fn variants(&self) -> impl Iterator<Item = (VariantKey, &Variant)> + '_ {
        self.variant_list
            .iter()
            .filter_map(|k| self.variants.get(k).map(|v| (*k, v)))
    }

    /// A mech by key.
    pub fn mech(&self, key: MechKey) -> Option<&Mech> {
        self.inventory.iter().find(|m| m.key == key)
    }

    fn mech_index(&self, key: MechKey) -> Option<usize> {
        self.inventory.iter().position(|m| m.key == key)
    }

    /// The `Ctx` for evaluating a variant.
    pub fn ctx_for<'a>(&'a self, v: &Variant) -> Ctx<'a> {
        Ctx {
            catalog: &self.data.catalog,
            chassis: &self.data.chassis[v.chassis],
        }
    }

    /// The chassis of a variant: its name id and its tonnage, as the mech list rows show them.
    pub fn variant_chassis(&self, key: VariantKey) -> Option<&Chassis> {
        let v = self.variants.get(&key)?;
        self.data.chassis.get(v.chassis)
    }

    /// `getCost` of a variant.
    pub fn variant_cost(&self, key: VariantKey) -> Option<i32> {
        let v = self.variants.get(&key)?;
        Some(v.cost(&self.ctx_for(v)))
    }

    /// `LogisticsMech::getCost`.
    pub fn mech_cost(&self, key: MechKey) -> Option<i32> {
        self.variant_cost(self.mech(key)?.variant)
    }

    /// `LogisticsMech::getMaxWeight` (chassis tons).
    pub fn mech_max_weight(&self, key: MechKey) -> Option<f32> {
        let v = self.variants.get(&self.mech(key)?.variant)?;
        Some(self.data.chassis[v.chassis].max_weight)
    }

    /// Name of a mech's variant (`LogisticsMech::getName`).
    pub fn mech_name(&self, key: MechKey) -> Option<&str> {
        let v = self.variants.get(&self.mech(key)?.variant)?;
        Some(v.name.as_str())
    }

    /// `getResourcePoints`: the in-mission support budget.
    pub fn resource_points(&self) -> i64 {
        self.resource_points
    }

    /// `setResourcePoints`.
    pub fn set_resource_points(&mut self, rp: i64) {
        self.resource_points = rp;
        self.rp_just_added = false;
    }

    /// `addResourcePoints`.
    pub fn add_resource_points(&mut self, amount: i64) {
        self.resource_points += amount;
        self.rp_just_added = true;
    }

    /// `decrementResourcePoints`.
    pub fn decrement_resource_points(&mut self, amount: i64) {
        self.resource_points -= amount;
    }

    /// `rpJustAdded`.
    pub fn rp_just_added(&self) -> bool {
        self.rp_just_added
    }

    /// `getCBills`.
    pub fn c_bills(&self) -> i64 {
        self.mission.c_bills
    }

    /// `addCBills`.
    pub fn add_c_bills(&mut self, amount: i64) {
        self.mission.c_bills += amount;
    }

    /// `decrementCBills`.
    pub fn decrement_c_bills(&mut self, amount: i64) {
        self.mission.c_bills -= amount;
    }

    /// `newMechsAvailable`.
    pub fn new_mechs_available(&self) -> bool {
        self.new_mechs
    }
    /// `newPilotsAvailable`.
    pub fn new_pilots_available(&self) -> bool {
        self.new_pilots
    }
    /// `newWeaponsAvailable`.
    pub fn new_weapons_available(&self) -> bool {
        self.new_weapons
    }
    /// `setNewMechsAcknowledged`.
    pub fn set_new_mechs_acknowledged(&mut self) {
        self.new_mechs = false;
    }
    /// `setNewPilotsAcknowledged`.
    pub fn set_new_pilots_acknowledged(&mut self) {
        self.new_pilots = false;
    }
    /// `LogisticsData::getVideoShown`: the current mission's video has played once.
    /// CROSS-CRATE: accessor for the logistics screens (U2a).
    pub fn video_shown(&self) -> bool {
        self.mission.video_shown()
    }
    /// `LogisticsData::setVideoShown`. CROSS-CRATE: accessor for the logistics screens (U2a).
    pub fn set_video_shown(&mut self) {
        self.mission.set_video_shown();
    }
    /// `setNewWeaponsAcknowledged`.
    pub fn set_new_weapons_acknowledged(&mut self) {
        self.new_weapons = false;
    }

    /// `MissionResults::FirstTimeResults`.
    pub fn first_time_results(&self) -> bool {
        self.first_time_results
    }
    /// Sets `MissionResults::FirstTimeResults`.
    pub fn set_first_time_results(&mut self, v: bool) {
        self.first_time_results = v;
    }

    /// `MissionBegin::FirstTimePurchase`.
    pub fn first_time_purchase(&self) -> bool {
        self.first_time_purchase
    }
    /// Sets `MissionBegin::FirstTimePurchase`.
    pub fn set_first_time_purchase(&mut self, v: bool) {
        self.first_time_purchase = v;
    }

    /// `MissionBegin::FirstTimeMechLab`.
    pub fn first_time_mech_lab(&self) -> bool {
        self.first_time_mech_lab
    }
    /// Sets `MissionBegin::FirstTimeMechLab`.
    pub fn set_first_time_mech_lab(&mut self, v: bool) {
        self.first_time_mech_lab = v;
    }

    // ------------------------------------------------------------------------------------
    // components and availability

    /// `getComponent`: catalogue index for a component id.
    pub fn component_index(&self, component_id: i32) -> Option<usize> {
        self.data.catalog.find(component_id)
    }

    /// Whether a catalogue component is currently available (`isAvailable`).
    pub fn component_available(&self, index: usize) -> bool {
        self.component_available
            .get(index)
            .copied()
            .unwrap_or(false)
    }

    /// `getAvailableComponents`: catalogue indices of the available components.
    pub fn available_components(&self) -> Vec<usize> {
        (0..self.data.catalog.len())
            .filter(|&i| self.component_available[i])
            .collect()
    }

    /// `getAllComponents`: every catalogue index.
    pub fn all_components(&self) -> Vec<usize> {
        (0..self.data.catalog.len()).collect()
    }

    /// `updateAvailability`: recomputes which components, mechs and pilots can be had from the
    /// current mission's purchase list and the bonus lists unlocked so far. Sets the
    /// "new mechs / pilots / weapons" flags the screens show as notices.
    pub fn update_availability(&mut self) -> LogisticsResult<()> {
        let data = Arc::clone(&self.data);
        self.new_weapons = false;

        let mut purchase_name = self.mission.current_purchase_file().to_ascii_lowercase();
        if purchase_name.is_empty() {
            purchase_name = FALLBACK_PURCHASE_FILE.to_string();
        }

        let mut old_mech_available_count = 0;
        let mut new_mech_available_count = 0;
        let old_pilot_available_count = self.pilots.iter().filter(|p| p.available).count();
        for p in &mut self.pilots {
            p.available = false;
        }

        // make sure it is around and can be opened
        let Some(list) = data.purchase_list(&purchase_name) else {
            return Err(LogisticsError::NoPurchaseFile);
        };

        // read in available components
        let mut available = [false; AVAILABLE_LEN];
        let all = list.all_components;
        if all {
            available.fill(true);
        } else {
            for &c in &list.components {
                if let Ok(i) = usize::try_from(c) {
                    if i < AVAILABLE_LEN {
                        available[i] = true;
                    }
                }
            }
        }

        // go through the component list and set them
        for (i, c) in data.catalog.iter().enumerate() {
            let listed = usize::try_from(c.id)
                .ok()
                .filter(|&id| id < AVAILABLE_LEN)
                .is_some_and(|id| available[id]);
            if listed || all {
                if !self.component_available[i] {
                    self.new_weapons = true;
                }
                self.component_available[i] = true;
            }
        }

        let additional: Vec<String> = self.mission.additional_purchase_files.clone();

        // reset all variants to unavailable
        let keys = self.variant_list.clone();
        for key in &keys {
            let v = self.variants.get_mut(key).expect("listed variant exists");
            if v.is_available() && (v.id >> 16) == 0 {
                old_mech_available_count += 1;
            }
            v.set_available(false);
        }

        for file in &additional {
            self.append_availability(file, &mut available);
        }

        // go through the component list again, and unset the unlisted ones
        for (i, c) in data.catalog.iter().enumerate() {
            let listed = usize::try_from(c.id)
                .ok()
                .filter(|&id| id < AVAILABLE_LEN)
                .is_some_and(|id| available[id]);
            if !listed {
                self.component_available[i] = false;
            }
        }

        // go through each variant, and see if it is available
        for chassis_stem in &list.mechs {
            for key in &keys {
                let v = &self.variants[key];
                if !same_name(data.chassis[v.chassis].stem(), chassis_stem) {
                    continue;
                }
                // every component of the variant must be available
                let all_listed = v.components.iter().all(|p| {
                    let id = data.catalog.get(p.component).map_or(-1, |c| c.id);
                    usize::try_from(id)
                        .ok()
                        .filter(|&i| i < AVAILABLE_LEN)
                        .is_some_and(|i| available[i])
                });
                if all_listed {
                    let v = self.variants.get_mut(key).expect("listed variant exists");
                    v.set_available(true);
                    if (v.id >> 16) == 0 {
                        new_mech_available_count += 1;
                    }
                }
            }
        }

        self.new_mechs = new_mech_available_count != old_mech_available_count;

        // add new pilots
        for name in &list.pilots {
            for p in &mut self.pilots {
                if same_name(&p.file_name, name) {
                    p.available = true;
                }
            }
        }
        let new_pilot_available_count = self.pilots.iter().filter(|p| p.available).count();
        self.new_pilots = new_pilot_available_count > old_pilot_available_count;

        Ok(())
    }

    /// `appendAvailability`: adds what a bonus purchase list offers. Reads the list's components
    /// whether or not it says `AllComponents` (the C++ ignores the flag here), then pilots, then
    /// mechs; for mechs only the first variant of the named chassis is considered.
    fn append_availability(&mut self, file_name: &str, available: &mut [bool; AVAILABLE_LEN]) {
        let data = Arc::clone(&self.data);
        // a list that cannot be opened adds nothing
        let Some(list) = data.purchase_list(file_name) else {
            return;
        };

        for &c in &list.components {
            if let Ok(id) = usize::try_from(c) {
                if id < AVAILABLE_LEN {
                    available[id] = true;
                }
            }
            if let Some(index) = data.catalog.find(c as i32) {
                if !self.component_available[index] {
                    self.new_weapons = true;
                }
                self.component_available[index] = true;
            }
        }

        for name in &list.pilots {
            for p in &mut self.pilots {
                if same_name(&p.file_name, name) {
                    p.available = true;
                    self.new_pilots = true;
                }
            }
        }

        for chassis_stem in &list.mechs {
            let keys = self.variant_list.clone();
            for key in keys {
                let v = &self.variants[&key];
                if !same_name(data.chassis[v.chassis].stem(), chassis_stem) {
                    continue;
                }
                let all_listed = v
                    .components
                    .iter()
                    .all(|p| self.component_available[p.component]);
                if all_listed {
                    self.variants
                        .get_mut(&key)
                        .expect("listed variant exists")
                        .set_available(true);
                }
                break;
            }
        }
    }

    // ------------------------------------------------------------------------------------
    // variants

    /// `getVariant(const char*)`: the first variant with this name (ignoring case).
    pub fn variant_by_name(&self, name: &str) -> Option<VariantKey> {
        self.variants()
            .find(|(_, v)| same_name(&v.name, name))
            .map(|(k, _)| k)
    }

    /// `getVariant(int ID)`: by variant id; also finds the variant being edited when `id` is the
    /// edited mech's id.
    pub fn variant_by_id(&self, id: i32) -> Option<VariantKey> {
        let wanted = (id & 0x00ff_ffff) as u32;
        if let Some((k, _)) = self.variants().find(|(_, v)| v.id == wanted) {
            return Some(k);
        }
        let modified = self.mech(self.currently_modified?)?;
        (id == modified.id).then_some(modified.variant)
    }

    /// `getVariant(const char* pCSVFileName, int VariantNum)`.
    pub fn variant_by_chassis_file(&self, csv_name: &str, variant_num: u32) -> Option<VariantKey> {
        let lower = csv_name.to_ascii_lowercase();
        self.variants()
            .find(|(_, v)| {
                self.data.chassis[v.chassis].file_name.contains(&lower)
                    && ((v.variant_id() >> 16) & 0xff) == variant_num
            })
            .map(|(k, _)| k)
    }

    /// `getPurchasableMechs`: available variants, in list order.
    pub fn purchasable_mechs(&self) -> Vec<VariantKey> {
        self.variants()
            .filter(|(_, v)| v.is_available())
            .map(|(k, _)| k)
            .collect()
    }

    /// `getChassisVariants`: the variants of a chassis.
    pub fn chassis_variants(&self, chassis: usize) -> Vec<VariantKey> {
        self.variants()
            .filter(|(_, v)| v.chassis == chassis)
            .map(|(k, _)| k)
            .collect()
    }

    /// `getPlayerVariantNames`: names of the variants the player made.
    pub fn player_variant_names(&self) -> Vec<String> {
        self.variants()
            .filter(|(_, v)| !v.designer)
            .map(|(_, v)| v.name.clone())
            .collect()
    }

    /// `getEncyclopediaMechs`: designer variants whose name contains "Prime".
    pub fn encyclopedia_mechs(&self) -> Vec<VariantKey> {
        self.variants()
            .filter(|(_, v)| v.name.contains("Prime") && v.designer)
            .map(|(k, _)| k)
            .collect()
    }

    /// `getHelicopters`: designer variants of the chassis with no variants before them (number
    /// 0 in their chassis) that are not "Prime".
    pub fn helicopters(&self) -> Vec<VariantKey> {
        self.variants()
            .filter(|(_, v)| {
                ((v.variant_id() >> 16) & 0xff) == 0 && !v.name.contains("Prime") && v.designer
            })
            .map(|(k, _)| k)
            .collect()
    }

    /// `getVehicles`: the vehicles of `buildings.csv` with their `object2.pak` records.
    pub fn vehicles(&self) -> &[crate::data::VehicleRecord] {
        &self.data.vehicles
    }

    /// `getBuildings`: the buildings of `buildings.csv` with their `object2.pak` records.
    pub fn buildings(&self) -> &[crate::data::BuildingRecord] {
        &self.data.buildings
    }

    /// `removeVariant`: deletes a player variant by name. A variant being edited, one an
    /// inventory mech uses, or an unknown name is refused.
    pub fn remove_variant(&mut self, name: &str) -> LogisticsResult<()> {
        // `EString::operator==` is case sensitive.
        let editing_name = self
            .currently_modified
            .and_then(|m| self.mech_name(m))
            .is_some_and(|n| n == name);
        let old_name = self
            .old_variant
            .and_then(|k| self.variants.get(&k))
            .is_some_and(|v| v.name == name);
        if editing_name || old_name {
            return Err(LogisticsError::Failed);
        }

        let Some(key) = self.variant_by_name(name) else {
            return Err(LogisticsError::InvalidVariant);
        };
        if self.inventory.iter().any(|m| m.variant == key) {
            return Err(LogisticsError::VariantInUse);
        }
        self.variant_list.retain(|k| *k != key);
        self.free_variant(key);
        Ok(())
    }

    // ------------------------------------------------------------------------------------
    // inventory

    /// `createInstanceID`: one more than the highest instance number among the mechs of this
    /// variant.
    pub fn create_instance_id(&self, variant: VariantKey) -> i32 {
        let Some(v) = self.variants.get(&variant) else {
            return 0;
        };
        let mut count = -1;
        for m in &self.inventory {
            let mv = &self.variants[&m.variant];
            if v.variant_id() == mv.id {
                count = count.max(m.instance_id());
            }
        }
        count + 1
    }

    fn push_mech(&mut self, variant: VariantKey, count: i32) -> usize {
        let vid = self.variants[&variant].id;
        let key = self.next_mech_key;
        self.next_mech_key += 1;
        self.inventory.push(Mech {
            key,
            id: (count << 24) | vid as i32,
            variant,
            pilot: None,
            force_group: 0,
            colors: [u32::MAX; 3],
        });
        self.inventory.len() - 1
    }

    /// `LogisticsMech::setPilot`: the old pilot is released, the new one marked used.
    pub fn set_mech_pilot(&mut self, mech: MechKey, pilot: Option<usize>) {
        let Some(i) = self.mech_index(mech) else {
            return;
        };
        self.set_pilot_at(i, pilot);
    }

    fn set_pilot_at(&mut self, i: usize, pilot: Option<usize>) {
        if let Some(old) = self.inventory[i].pilot {
            self.pilots[old].used = false;
        }
        self.inventory[i].pilot = pilot;
        if let Some(p) = pilot {
            self.pilots[p].used = true;
        }
    }

    /// The destructor of `LogisticsMech`: frees the pilot, drops the mech.
    fn delete_mech_at(&mut self, i: usize) {
        if let Some(p) = self.inventory[i].pilot {
            self.pilots[p].used = false;
        }
        self.inventory.remove(i);
    }

    /// `addMechToInventory(pVar, pPilot, ForceGroup, bSubtractPts)`: used for salvage and for
    /// units that came back from a mission.
    pub fn add_mech_to_inventory_with_pilot(
        &mut self,
        variant: VariantKey,
        pilot: Option<usize>,
        force_group: i32,
        subtract_pts: bool,
    ) -> Option<MechKey> {
        if !self.variants.contains_key(&variant) {
            return None;
        }
        let count = self.create_instance_id(variant);
        let i = self.push_mech(variant, count);
        self.inventory[i].force_group = force_group;
        if let Some(p) = pilot {
            if !self.pilots[p].dead {
                self.set_pilot_at(i, Some(p));
            }
        }
        if force_group > -1 && force_group < 13 {
            self.inventory[i].force_group = force_group;
        }
        if subtract_pts {
            let cost = self.mech_cost(self.inventory[i].key).unwrap_or(0);
            self.decrement_c_bills(i64::from(cost));
        }
        Some(self.inventory[i].key)
    }

    /// `addMechToInventory(pVar, addToForceGroup, pPilot, colors)`: puts a mech in the bay,
    /// optionally straight into a force group slot. A dead pilot is not assigned.
    pub fn add_mech_to_inventory(
        &mut self,
        variant: VariantKey,
        add_to_force_group: i32,
        pilot: Option<usize>,
        colors: [u32; 3],
    ) -> Option<MechKey> {
        if !self.variants.contains_key(&variant) {
            return None; // the C++ asserts "couldn't add the mech to the inventory"
        }
        let count = self.create_instance_id(variant);
        let i = self.push_mech(variant, count);
        if add_to_force_group > -1 && add_to_force_group < 13 {
            self.inventory[i].force_group = add_to_force_group;
        }
        if let Some(p) = pilot {
            if !self.pilots[p].dead {
                self.set_pilot_at(i, Some(p));
            }
        }
        self.inventory[i].colors = colors;
        Some(self.inventory[i].key)
    }

    /// `getMech(MechName, pilotName)`: the first mech with this variant name whose pilot (by
    /// file stem) matches; with `None` the first without a pilot.
    pub fn mech_by_name_and_pilot(
        &self,
        mech_name: &str,
        pilot_file: Option<&str>,
    ) -> Option<MechKey> {
        for m in &self.inventory {
            if !same_name(&self.variants[&m.variant].name, mech_name) {
                continue;
            }
            match pilot_file {
                None => {
                    if m.pilot.is_none() {
                        return Some(m.key);
                    }
                }
                Some(name) => {
                    if let Some(p) = m.pilot {
                        if same_name(&self.pilots[p].file_name, name) {
                            return Some(m.key);
                        }
                    }
                }
            }
        }
        None
    }

    /// `getMech(int ID)`.
    pub fn mech_by_id(&self, id: i32) -> Option<MechKey> {
        self.inventory.iter().find(|m| m.id == id).map(|m| m.key)
    }

    /// `removeMechFromInventory`: deletes the mech found by name and pilot, if any.
    pub fn remove_mech_from_inventory(&mut self, mech_name: &str, pilot_file: Option<&str>) {
        if let Some(key) = self.mech_by_name_and_pilot(mech_name, pilot_file) {
            let i = self.mech_index(key).expect("key was just found");
            self.delete_mech_at(i);
        }
    }

    /// `removeMechsInForceGroup`: deletes the mechs that went on the mission, last first.
    pub fn remove_mechs_in_force_group(&mut self) {
        let mut i = self.inventory.len();
        while i > 0 {
            i -= 1;
            if self.inventory[i].force_group != 0 {
                self.delete_mech_at(i);
            }
        }
    }

    /// `clearInventory`.
    pub fn clear_inventory(&mut self) {
        while !self.inventory.is_empty() {
            let last = self.inventory.len() - 1;
            self.delete_mech_at(last);
        }
    }

    /// `getVariantsInInventory`: how many mechs are of this variant, optionally counting those
    /// in the force group.
    pub fn variants_in_inventory(&self, variant: VariantKey, include_force_group: bool) -> usize {
        self.inventory
            .iter()
            .filter(|m| m.variant == variant && (m.force_group == 0 || include_force_group))
            .count()
    }

    // ------------------------------------------------------------------------------------
    // purchase and sale

    /// `canPurchaseMech`.
    pub fn can_purchase_mech(&self, variant: VariantKey) -> LogisticsResult<()> {
        let Some(cost) = self.variant_cost(variant) else {
            return Err(LogisticsError::InvalidId);
        };
        if self.mission.c_bills - i64::from(cost) >= 0 {
            Ok(())
        } else {
            Err(LogisticsError::NotEnoughResourcePoints)
        }
    }

    /// `purchaseMech`: buys a mech of the variant for C-Bills.
    pub fn purchase_mech(&mut self, variant: VariantKey) -> LogisticsResult<MechKey> {
        let Some(cost) = self.variant_cost(variant) else {
            return Err(LogisticsError::Failed);
        };
        if self.mission.c_bills - i64::from(cost) >= 0 {
            let count = self.create_instance_id(variant);
            let i = self.push_mech(variant, count);
            self.mission.c_bills -= i64::from(cost);
            Ok(self.inventory[i].key)
        } else {
            Err(LogisticsError::NotEnoughResourcePoints)
        }
    }

    /// `sellMech`: sells the last mech in the bay that is built like `mech` and is not in the
    /// force group (not necessarily `mech` itself), for its full cost.
    pub fn sell_mech(&mut self, mech: MechKey) -> LogisticsResult<()> {
        let Some(variant) = self.mech(mech).map(|m| m.variant) else {
            return Err(LogisticsError::Failed);
        };
        for i in (0..self.inventory.len()).rev() {
            if self.inventory[i].force_group != 0 {
                continue;
            }
            if self.inventory[i].variant == variant {
                let cost = self.variant_cost(variant).unwrap_or(0);
                self.set_pilot_at(i, None);
                self.delete_mech_at(i);
                self.mission.c_bills += i64::from(cost);
                return Ok(());
            }
        }
        Err(LogisticsError::Failed)
    }

    // ------------------------------------------------------------------------------------
    // force group

    /// `getForceGroup`: the mechs with a force group slot, in inventory order, at most twelve.
    pub fn force_group(&self) -> Vec<MechKey> {
        self.inventory
            .iter()
            .filter(|m| m.force_group != 0)
            .take(MAX_FORCE_GROUP_MECHS)
            .map(|m| m.key)
            .collect()
    }

    /// `getInventory`.
    pub fn inventory_keys(&self) -> Vec<MechKey> {
        self.inventory.iter().map(|m| m.key).collect()
    }

    /// `getCurrentDropWeight`: tons in the force group.
    pub fn current_drop_weight(&self) -> i64 {
        let mut total: i64 = 0;
        for m in &self.inventory {
            if m.force_group != 0 {
                let w = self.mech_max_weight(m.key).unwrap_or(0.0);
                total = (total as f32 + w) as i64;
            }
        }
        total
    }

    /// `getMaxDropWeight`.
    pub fn max_drop_weight(&self) -> i64 {
        self.mission.current_drop_weight()
    }

    /// `canAddMechToForceGroup`: fewer than twelve in the group and the weight fits.
    pub fn can_add_mech_to_force_group(&self, mech: MechKey) -> bool {
        let Some(weight) = self.mech_max_weight(mech) else {
            return false;
        };
        let fg_count = self.inventory.iter().filter(|m| m.force_group != 0).count();
        if fg_count >= MAX_FORCE_GROUP_MECHS {
            return false;
        }
        (weight + self.current_drop_weight() as f32) <= self.max_drop_weight() as f32
    }

    /// `getMechWithoutForceGroup`: the mech itself when it is not in the group, else another
    /// of the same variant that is not.
    pub fn mech_without_force_group(&self, mech: MechKey) -> Option<MechKey> {
        let m = self.mech(mech)?;
        if m.force_group == 0 {
            return Some(m.key);
        }
        self.inventory
            .iter()
            .find(|o| o.variant == m.variant && o.force_group == 0)
            .map(|o| o.key)
    }

    /// `addMechToForceGroup(pMech, slot)`.
    pub fn add_mech_to_force_group(&mut self, mech: MechKey, slot: i32) -> LogisticsResult<()> {
        let Some(i) = self.mech_index(mech) else {
            return Err(LogisticsError::Failed);
        };
        if slot > MAX_FORCE_GROUP_SLOT {
            return Err(LogisticsError::Failed);
        }
        if self.inventory[i].force_group == 0 {
            self.inventory[i].force_group = slot;
            return Ok(());
        }
        // find another of the same variant
        if let Some(other) = self.mech_without_force_group(mech) {
            let j = self.mech_index(other).expect("key from the inventory");
            self.inventory[j].force_group = slot;
            return Ok(());
        }
        Err(LogisticsError::Failed)
    }

    /// `removeMechFromForceGroup(int slot)`: takes the mech in that slot out, with its pilot.
    pub fn remove_slot_from_force_group(&mut self, slot: i32) -> LogisticsResult<()> {
        for i in 0..self.inventory.len() {
            if self.inventory[i].force_group == slot {
                self.inventory[i].force_group = 0;
                self.set_pilot_at(i, None);
                return Ok(());
            }
        }
        Err(LogisticsError::Failed)
    }

    /// `removeMechFromForceGroup(pMech, bRemovePilot)`. When `mech` is not in the group, another
    /// mech of its variant that is gets removed; in that case a requested pilot removal applies
    /// to `mech`, as in the original.
    pub fn remove_mech_from_force_group(
        &mut self,
        mech: MechKey,
        remove_pilot: bool,
    ) -> LogisticsResult<()> {
        let Some(i) = self.mech_index(mech) else {
            return Err(LogisticsError::Failed);
        };
        if self.inventory[i].force_group != 0 {
            self.inventory[i].force_group = 0;
            if remove_pilot {
                self.set_pilot_at(i, None);
            }
            return Ok(());
        }
        // find a similar one
        let variant = self.inventory[i].variant;
        if let Some(j) = self
            .inventory
            .iter()
            .position(|o| o.variant == variant && o.force_group != 0)
        {
            self.inventory[j].force_group = 0;
            if remove_pilot {
                self.set_pilot_at(i, None);
            }
            return Ok(());
        }
        Err(LogisticsError::Failed)
    }

    /// `removeDeadWeight`: after the drop limit changed, empties slots 12 down to 1 until the
    /// force fits.
    pub fn remove_dead_weight(&mut self) {
        let max = self.max_drop_weight();
        let mut cur = self.current_drop_weight();
        let mut i = 12;
        while cur > max {
            let _ = self.remove_slot_from_force_group(i);
            i -= 1;
            cur = self.current_drop_weight();
            if i == 0 {
                break;
            }
        }
    }

    // ------------------------------------------------------------------------------------
    // pilots

    /// `getPilotCount`.
    pub fn pilot_count(&self) -> usize {
        self.pilots.len()
    }

    /// `getPilot(name)`: an available pilot first, then any, by profile stem.
    pub fn pilot_index(&self, file_name: &str) -> Option<usize> {
        self.pilots
            .iter()
            .position(|p| p.is_available() && same_name(&p.file_name, file_name))
            .or_else(|| {
                self.pilots
                    .iter()
                    .position(|p| same_name(&p.file_name, file_name))
            })
    }

    /// `getFirstAvailablePilot`: the first pilot no inventory mech uses.
    pub fn first_available_pilot(&self) -> Option<usize> {
        (0..self.pilots.len()).find(|&i| {
            !self.inventory.iter().any(|m| {
                m.pilot
                    .is_some_and(|p| self.pilots[p].id == self.pilots[i].id)
            })
        })
    }

    /// `setPilotUnused`.
    pub fn set_pilot_unused(&mut self, file_name: &str) {
        if let Some(p) = self
            .pilots
            .iter_mut()
            .find(|p| same_name(&p.file_name, file_name))
        {
            p.used = false;
        }
    }

    /// The use flags `PilotReadyScreen::begin` rebuilds: every pilot is cleared, then the pilot of
    /// each mech that has a force group is marked used again. A pilot kept on a mech outside the
    /// force group is free, so the ready room lists it.
    pub fn rebuild_pilot_use(&mut self) {
        for p in &mut self.pilots {
            p.used = false;
        }
        for i in 0..self.inventory.len() {
            if self.inventory[i].force_group == 0 {
                continue;
            }
            if let Some(p) = self.inventory[i].pilot {
                self.pilots[p].used = true;
            }
        }
    }

    /// The pilots the ready screen lists: available and not yet assigned.
    pub fn unassigned_pilots(&self) -> Vec<usize> {
        (0..self.pilots.len())
            .filter(|&i| !self.pilots[i].used && self.pilots[i].is_available())
            .collect()
    }

    /// `comparePilots`: positive when `p1` is the better choice. Used pilots lose; pilots on
    /// inventory mechs lose to ones that are not; then rank, gunnery, piloting.
    ///
    /// A quirk of the source is kept: after `p1.rank > p2.rank` the C++ tests `p2.rank < p1.rank`,
    /// which can never be true, so a lower rank does not decide the result; the comparison goes
    /// on to gunnery and piloting.
    pub fn compare_pilots(&self, p1: usize, p2: usize, _weight: i64) -> i32 {
        let (a, b) = (&self.pilots[p1], &self.pilots[p2]);
        if a.used {
            return -1;
        } else if b.used {
            return 1;
        }
        for m in &self.inventory {
            if let Some(p) = m.pilot {
                if self.pilots[p].id == a.id {
                    return -1;
                } else if self.pilots[p].id == b.id {
                    return 1;
                }
            }
        }
        if a.rank > b.rank {
            return 1;
        }
        if b.gunnery > a.gunnery {
            return -1;
        }
        if a.gunnery > b.gunnery {
            return 1;
        }
        if a.piloting > b.piloting {
            return 1;
        }
        if b.piloting > a.piloting {
            return -1;
        }
        0
    }

    fn candidate_pilots(&self, in_use: &dyn Fn(&Pilot) -> bool) -> Vec<usize> {
        (0..self.pilots.len())
            .filter(|&i| self.pilots[i].is_available() && !in_use(&self.pilots[i]))
            .collect()
    }

    /// `getBestPilot`: picks the best free pilot for a mech of `mech_weight` tons and marks them
    /// used. `in_use` says which pilots the running mission already has
    /// (`MechWarrior::warriorInUse`). Returns the pilot's index.
    pub fn best_pilot(
        &mut self,
        mech_weight: i64,
        in_use: &dyn Fn(&Pilot) -> bool,
    ) -> Option<usize> {
        let mut list = self.candidate_pilots(in_use);
        let count = list.len();

        for i in 1..count {
            let cur = list[i];
            for j in 0..i {
                if self.compare_pilots(cur, list[j], mech_weight) > 0 && j != i {
                    list[i] = list[j];
                    list[j] = cur;
                    break;
                }
            }
        }

        for &p in &list {
            if self.pilots[p].is_available() && !self.pilots[p].used {
                self.pilots[p].used = true;
                return Some(p);
            }
        }
        // "We're out of pilots, shouldn't be here"
        let first = *list.first()?;
        self.pilots[first].used = true;
        Some(first)
    }

    /// `gotPilotsLeft`.
    pub fn got_pilots_left(&self, in_use: &dyn Fn(&Pilot) -> bool) -> bool {
        self.candidate_pilots(in_use)
            .into_iter()
            .any(|p| self.pilots[p].is_available() && !self.pilots[p].used)
    }

    // ------------------------------------------------------------------------------------
    // mech lab

    /// `setMechToModify`: starts editing a mech. The edit works on a copy of the mech's
    /// variant, which the mech points at until the edit is accepted or cancelled.
    pub fn set_mech_to_modify(&mut self, mech: MechKey) -> LogisticsResult<VariantKey> {
        let Some(i) = self.mech_index(mech) else {
            return Err(LogisticsError::Failed);
        };
        let old = self.inventory[i].variant;
        self.currently_modified = Some(mech);
        self.old_variant = Some(old);

        // copy constructor: same chassis, availability, components, name and file id; a new id;
        // not a designer variant; not hidden
        let src = self.variants[&old].clone();
        let data = Arc::clone(&self.data);
        let id = self.next_variant_id(&data.chassis[src.chassis]);
        let copy = Variant {
            id,
            designer: false,
            hidden: false,
            ..src
        };
        let new_key = self.alloc_variant(copy);
        self.mech_set_variant(i, new_key);
        Ok(new_key)
    }

    /// `LogisticsMech::setVariant`: the mech gets a new variant and a new instance number. The
    /// variant is assigned first, so the instance count `createInstanceID` takes includes the
    /// mech itself: every re-assignment to a variant the mech already has raises its number.
    fn mech_set_variant(&mut self, i: usize, variant: VariantKey) {
        self.inventory[i].variant = variant;
        let vid = self.variants[&variant].id;
        let instance = self.create_instance_id(variant);
        self.inventory[i].id = (instance << 24) | vid as i32;
    }

    /// `getMechToModify`.
    pub fn mech_to_modify(&self) -> Option<MechKey> {
        self.currently_modified
    }

    /// The variant being edited (the copy the lab works on).
    pub fn variant_being_edited(&self) -> Option<VariantKey> {
        self.mech(self.currently_modified?).map(|m| m.variant)
    }

    /// The variant the edit started from.
    pub fn original_variant(&self) -> Option<VariantKey> {
        self.old_variant
    }

    /// `canAddComponent` on the variant being edited. `x` and `y` follow
    /// [`Variant::can_add_component`].
    pub fn lab_can_add_component(
        &self,
        component: usize,
        x: &mut i64,
        y: &mut i64,
    ) -> LogisticsResult<()> {
        let key = self.variant_being_edited().ok_or(LogisticsError::Failed)?;
        let v = &self.variants[&key];
        v.can_add_component(&self.ctx_for(v), component, x, y)
    }

    /// `addComponent` on the variant being edited.
    pub fn lab_add_component(
        &mut self,
        component: usize,
        x: &mut i64,
        y: &mut i64,
    ) -> LogisticsResult<()> {
        let key = self.variant_being_edited().ok_or(LogisticsError::Failed)?;
        self.variant_add_component(key, component, x, y)
    }

    /// `removeComponent` on the variant being edited.
    pub fn lab_remove_component(&mut self, x: i64, y: i64) -> LogisticsResult<()> {
        let key = self.variant_being_edited().ok_or(LogisticsError::Failed)?;
        let data = Arc::clone(&self.data);
        let v = self.variants.get_mut(&key).expect("variant exists");
        let ctx = Ctx {
            catalog: &data.catalog,
            chassis: &data.chassis[v.chassis],
        };
        v.remove_component(&ctx, x, y)
    }

    /// `LogisticsVariant::addComponent` on any variant in the arena.
    pub fn variant_add_component(
        &mut self,
        key: VariantKey,
        component: usize,
        x: &mut i64,
        y: &mut i64,
    ) -> LogisticsResult<()> {
        let data = Arc::clone(&self.data);
        let v = self.variants.get_mut(&key).ok_or(LogisticsError::Failed)?;
        let ctx = Ctx {
            catalog: &data.catalog,
            chassis: &data.chassis[v.chassis],
        };
        v.add_component(&ctx, component, x, y)
    }

    /// The lab's "swap variant" (`*pVariant = *pNewVariant` then restoring the name): the edited
    /// variant takes the loadout of another one of the same chassis. It gets a new id (the
    /// chassis reference count goes up) and is no designer variant.
    pub fn lab_copy_loadout_from(&mut self, source: VariantKey) -> LogisticsResult<()> {
        let key = self.variant_being_edited().ok_or(LogisticsError::Failed)?;
        let src = self
            .variants
            .get(&source)
            .cloned()
            .ok_or(LogisticsError::InvalidVariant)?;
        let data = Arc::clone(&self.data);
        let id = self.next_variant_id(&data.chassis[src.chassis]);
        let target = self.variants.get_mut(&key).expect("variant exists");
        let name = target.name.clone();
        let hidden = target.hidden;
        *target = Variant {
            id,
            name,
            designer: false,
            hidden,
            ..src
        };
        Ok(())
    }

    /// The lab's cost check when the player presses accept: the change in cost against the
    /// available C-Bills. `Err(NotEnoughResourcePoints)` when the new loadout costs more than
    /// the player has to cover.
    pub fn lab_can_afford(&self) -> LogisticsResult<()> {
        let (Some(new), Some(old)) = (self.variant_being_edited(), self.old_variant) else {
            return Err(LogisticsError::Failed);
        };
        let new_cost = self.variant_cost(new).unwrap_or(0) as f32;
        let old_cost = self.variant_cost(old).unwrap_or(0) as f32;
        if new_cost - old_cost > self.mission.c_bills as f32 {
            Err(LogisticsError::NotEnoughResourcePoints)
        } else {
            Ok(())
        }
    }

    // CROSS-CRATE: lab screen queries (mc2-ui `mech_lab`), additive; the rules stay here.

    /// `MechLabScreen::canRemoveComponent` for the cell `(x, y)` of the variant being edited.
    pub fn lab_can_remove_component(&self, x: i64, y: i64) -> bool {
        let Some(key) = self.variant_being_edited() else {
            return false;
        };
        let v = &self.variants[&key];
        v.can_remove_component(&self.ctx_for(v), x, y)
    }

    /// `*pCurVar == *pVariant` in the lab: the variant called `name` has the same loadout (and
    /// chassis and availability) as the one being edited.
    pub fn lab_matches_variant(&self, name: &str) -> bool {
        let (Some(edited), Some(other)) = (self.variant_being_edited(), self.variant_by_name(name))
        else {
            return false;
        };
        let (a, b) = (&self.variants[&edited], &self.variants[&other]);
        a.equivalent(b, &self.ctx_for(a), &self.ctx_for(b))
    }

    /// `LogisticsVariant::allComponentsAvailable`.
    pub fn variant_all_components_available(&self, key: VariantKey) -> bool {
        self.variants
            .get(&key)
            .is_some_and(|v| v.all_components_available(|i| self.component_available(i)))
    }

    /// `canReplaceVariant`: whether saving the edit under `name` may replace a variant of that
    /// name. It may not when another inventory mech with that name uses a different variant,
    /// when more than one mech carries the name, or when a designer variant has it.
    pub fn can_replace_variant(&self, name: &str) -> bool {
        let mut name_count = 0;
        for m in &self.inventory {
            if same_name(&self.variants[&m.variant].name, name) {
                name_count += 1;
                if Some(m.variant) != self.old_variant && Some(m.key) != self.currently_modified {
                    return false;
                }
            }
        }
        if name_count > 1 {
            return false;
        }
        for (_, v) in self.variants() {
            if v.designer && same_name(&v.name, name) {
                return false;
            }
        }
        true
    }

    /// `canDeleteVariant`.
    pub fn can_delete_variant(&self, name: &str) -> bool {
        if self.variant_by_name(name).is_none() {
            return false;
        }
        if !self.can_replace_variant(name) {
            return false;
        }
        let editing = self
            .currently_modified
            .and_then(|m| self.mech_name(m))
            .is_some_and(|n| n == name);
        let old = self
            .old_variant
            .and_then(|k| self.variants.get(&k))
            .is_some_and(|v| v.name == name);
        !(editing || old)
    }

    /// `acceptMechModifications(name)`: keeps the edited loadout under `name`. The old
    /// loadout's price is refunded and the new one charged; the old variant is deleted when no
    /// other mech uses it and it has the same name; variants already called `name` are removed.
    pub fn accept_mech_modifications(&mut self, name: &str) -> LogisticsResult<()> {
        let Some(edited_mech) = self.currently_modified else {
            return Err(LogisticsError::Failed);
        };
        let Some(i) = self.mech_index(edited_mech) else {
            return Err(LogisticsError::Failed);
        };
        let edited_variant = self.inventory[i].variant;
        let old = self.old_variant;

        // Hardening: the original deletes every variant with this name, even one that other
        // inventory mechs still use, and leaves them dangling. Refuse before changing anything.
        let in_use_elsewhere = self.variant_list.iter().any(|k| {
            same_name(&self.variants[k].name, name)
                && self
                    .inventory
                    .iter()
                    .any(|m| m.variant == *k && m.key != edited_mech)
        });
        if in_use_elsewhere {
            return Err(LogisticsError::NameInInventory);
        }

        let mut found = false;
        if let Some(o) = old {
            let refund = self.variant_cost(o).unwrap_or(0);
            self.mission.c_bills += i64::from(refund);
        }
        if old.is_some_and(|o| self.variants[&o].designer) {
            found = true;
        } else {
            for m in &self.inventory {
                if Some(m.variant) == old && m.key != edited_mech {
                    found = true;
                }
            }
        }

        if !found {
            if let Some(o) = old {
                if self.variant_list.contains(&o) && same_name(&self.variants[&o].name, name) {
                    self.variant_list.retain(|k| *k != o);
                    self.free_variant(o);
                }
            }
        }

        // now get rid of variants with this name
        let clash: Vec<VariantKey> = self
            .variant_list
            .iter()
            .copied()
            .filter(|k| same_name(&self.variants[k].name, name))
            .collect();
        for k in clash {
            self.variant_list.retain(|x| *x != k);
            self.free_variant(k);
        }

        self.variants
            .get_mut(&edited_variant)
            .expect("edited variant exists")
            .name = name.to_string();
        self.variant_list.push(edited_variant);
        let cost = self.variant_cost(edited_variant).unwrap_or(0);
        self.mission.c_bills -= i64::from(cost);

        self.currently_modified = None;
        self.old_variant = None;
        Ok(())
    }

    /// `acceptMechModificationsUseOldVariant(name)`: the edit turned out equal to an existing
    /// variant called `name`; the mech uses that one. Refunds the old price and charges the
    /// existing variant's.
    pub fn accept_mech_modifications_use_old_variant(&mut self, name: &str) -> LogisticsResult<()> {
        let Some(edited_mech) = self.currently_modified else {
            return Err(LogisticsError::Failed);
        };
        let Some(i) = self.mech_index(edited_mech) else {
            return Err(LogisticsError::Failed);
        };
        // (the original asserts when the name is unknown, after the refund; check first)
        let Some(existing) = self.variant_by_name(name) else {
            return Err(LogisticsError::InvalidVariant);
        };
        if let Some(o) = self.old_variant {
            let refund = self.variant_cost(o).unwrap_or(0);
            self.mission.c_bills += i64::from(refund);
        }
        let edited_variant = self.inventory[i].variant;
        self.mech_set_variant(i, existing);
        self.free_variant(edited_variant);
        let cost = self.variant_cost(existing).unwrap_or(0);
        self.mission.c_bills -= i64::from(cost);

        self.currently_modified = None;
        self.old_variant = None;
        Ok(())
    }

    /// `cancelMechModfications`: drops the copy; the mech goes back to its variant.
    pub fn cancel_mech_modifications(&mut self) -> LogisticsResult<()> {
        let Some(edited_mech) = self.currently_modified else {
            return Err(LogisticsError::Failed);
        };
        let Some(i) = self.mech_index(edited_mech) else {
            return Err(LogisticsError::Failed);
        };
        let edited_variant = self.inventory[i].variant;
        if let Some(old) = self.old_variant {
            self.mech_set_variant(i, old);
        }
        self.free_variant(edited_variant);
        self.old_variant = None;
        self.currently_modified = None;
        Ok(())
    }

    // ------------------------------------------------------------------------------------
    // campaign and missions

    /// `startNewCampaign`: empties the inventory, resets the pilots, starts the script with its
    /// first mission selected. Player-made variants are kept, as in the original.
    pub fn start_new_campaign(&mut self, campaign: &str) -> LogisticsResult<()> {
        let key = campaign.to_ascii_lowercase();
        let script = self
            .data
            .campaigns
            .get(&key)
            .cloned()
            .ok_or(LogisticsError::InvalidFileName)?;
        // `inventory.Clear()` without freeing the mechs; pilots are rebuilt anyway.
        self.inventory.clear();
        self.resource_points = 0;
        self.init_pilots();
        self.mission = MissionInfo::new(&key, &script);
        self.currently_modified = None;
        self.old_variant = None;
        self.select_first_available_mission()
    }

    /// `setCurrentMission`: selects a mission of the current group, then refreshes availability
    /// from its purchase list, loads its resource points and drops mechs that no longer fit.
    pub fn set_current_mission(&mut self, mission_name: &str) -> LogisticsResult<()> {
        self.mission.set_next_mission(mission_name)?;
        // a missing purchase file is reported by the C++ (a pause) and the rest goes on
        let _ = self.update_availability();
        self.resource_points = self.mission.current_rp();
        self.remove_dead_weight();
        Ok(())
    }

    /// `LogisticsData::setSingleMission`: a one-mission game on a map of the player's choice.
    /// The campaign is replaced by [`MissionInfo::new_single`], player-made variants and the
    /// inventory are cleared, the pilots start over, and availability is read from
    /// `data/missions/purchase.fit` (the entry names no purchase file). `settings` are the map's
    /// `[MissionSettings]` (`mc2_sim_data::campaign::MissionSettings::from_fit`).
    pub fn set_single_mission(
        &mut self,
        mission_file_name: &str,
        settings: &mc2_sim_data::campaign::MissionSettings,
        raw_drop_weight: Option<f32>,
    ) {
        self.mission = MissionInfo::new_single(mission_file_name, settings, raw_drop_weight);
        self.clear_player_variants();
        self.clear_inventory();
        self.currently_modified = None;
        self.old_variant = None;
        self.init_pilots();
        let _ = self.update_availability();
    }

    /// `clearVariants`: deletes the variants the player made.
    pub fn clear_player_variants(&mut self) {
        let doomed: Vec<VariantKey> = self
            .variant_list
            .iter()
            .copied()
            .filter(|k| !self.variants[k].designer)
            .collect();
        for k in doomed.into_iter().rev() {
            self.variant_list.retain(|x| *x != k);
            self.free_variant(k);
        }
    }

    /// `getAvailableMissions`.
    pub fn available_missions(&self) -> Vec<String> {
        self.mission.available_missions()
    }

    /// `getCurrentMissions`.
    pub fn current_missions(&self) -> Vec<String> {
        self.mission.current_missions()
    }

    /// `getMissionAvailable`.
    pub fn mission_available(&self, mission_name: &str) -> bool {
        self.mission.mission_available(mission_name)
    }

    /// `getCurrentMission`.
    pub fn current_mission(&self) -> &str {
        &self.mission.current_mission_name
    }

    /// `getLastMission`.
    pub fn last_mission(&self) -> &str {
        &self.mission.last_mission_name
    }

    /// `campaignOver`.
    pub fn campaign_over(&self) -> bool {
        self.mission.campaign_over()
    }

    /// `addNewBonusPurchaseFile`: a mission objective unlocked a bonus purchase list.
    pub fn add_new_bonus_purchase_file(&mut self, file_name: &str) {
        self.mission.add_bonus_purchase_file(file_name);
    }

    /// `skipLogistics`.
    pub fn skip_logistics(&self) -> bool {
        self.mission.skip_logistics()
    }

    /// `showChooseMission`.
    pub fn show_choose_mission(&self) -> bool {
        self.mission.show_choose_mission()
    }

    /// `skipPilotReview`.
    pub fn skip_pilot_review(&self) -> bool {
        self.mission.skip_pilot_review()
    }

    /// `skipSalvageScreen`.
    pub fn skip_salvage_screen(&self) -> bool {
        self.mission.skip_salvage_screen()
    }

    /// `skipPurchasing`.
    pub fn skip_purchasing(&self) -> bool {
        self.mission.skip_purchasing()
    }

    /// The `canHave*` flags of the current mission.
    pub fn support_enables(&self) -> SupportEnables {
        SupportEnables {
            salvage_craft: self.mission.can_have_salvage_craft(),
            repair_truck: self.mission.can_have_repair_truck(),
            scout_copter: self.mission.can_have_scout_copter(),
            artillery_piece: self.mission.can_have_artillery_piece(),
            air_strike: self.mission.can_have_air_strike(),
            sensor_strike: self.mission.can_have_sensor_strike(),
            mine_layer: self.mission.can_have_mine_layer(),
        }
    }

    // ------------------------------------------------------------------------------------
    // the mission hand-over

    /// What `Logistics::beginMission` hands the mission: the current mission, how the force is
    /// chosen, and (for a logistics start) the force-group mechs that have a pilot.
    pub fn load_spec(&self, options: MissionOptions) -> LoadSpec {
        let load_type = if self.skip_logistics() {
            LoadType::SpQuickstart
        } else {
            LoadType::SpLogistics
        };
        let mut force = Vec::new();
        if load_type == LoadType::SpLogistics {
            for key in self.force_group() {
                let m = self.mech(key).expect("force group key is in the inventory");
                let Some(pilot) = m.pilot else {
                    continue; // mechs without a pilot do not deploy
                };
                let p = &self.pilots[pilot];
                let v = &self.variants[&m.variant];
                let chassis = &self.data.chassis[v.chassis];
                let ctx = self.ctx_for(v);
                let components = if v.designer {
                    Vec::new()
                } else {
                    v.components
                        .iter()
                        .map(|c| LoadoutComponent {
                            component_id: ctx.catalog.get(c.component).map_or(0, |k| k.id),
                            x: c.x,
                            y: c.y,
                        })
                        .collect()
                };
                force.push(ForceMech {
                    variant_name: v.name.clone(),
                    chassis_file: chassis.file_name.clone(),
                    object_number: chassis.fit_id,
                    variant_id: if v.designer { v.file_id } else { 0 },
                    designer: v.designer,
                    components,
                    pilot_file: p.file_name.clone(),
                    gunnery: p.gunnery,
                    piloting: p.piloting,
                    specialty_skills: p.specialty_skills,
                    force_group: m.force_group,
                    colors: [
                        options.base_color,
                        options.highlight_color,
                        options.highlight_color,
                    ],
                });
            }
        }
        LoadSpec {
            mission: self.mission.current_mission_name.clone(),
            load_type,
            difficulty: options.difficulty,
            unlimited_ammo: options.unlimited_ammo,
            resource_points: self.resource_points,
            drop_weight_limit: self.max_drop_weight(),
            force,
            abl_script: self
                .mission
                .current_abl_script_name()
                .unwrap_or("NONE")
                .to_string(),
            support: self.support_enables(),
            // `gotPilotsLeft`/`getBestPilot` read `pilots` (`initPilots` on first use) whatever the load type: a quickstart-type campaign
            // mission (the tutorial, which skips logistics) still has its recovery craft pilots.
            roster: self
                .pilots
                .iter()
                .map(|p| RosterPilot {
                    file_name: p.file_name.clone(),
                    available: p.is_available(),
                    used: p.used,
                    in_inventory: self
                        .inventory
                        .iter()
                        .any(|m| m.pilot.is_some_and(|i| self.pilots[i].id == p.id)),
                    rank: p.rank,
                    gunnery: p.gunnery,
                    piloting: p.piloting,
                    specialty_skills: p.specialty_skills,
                })
                .collect(),
        }
    }

    /// What `Logistics::start(log_RESULTS)` does with a finished mission. Bonus purchase lists
    /// the mission unlocked are recorded whatever the result. A big win removes the deployed
    /// force from the inventory and calls [`set_mission_completed`](Self::set_mission_completed);
    /// every other result leaves the campaign where it was.
    pub fn apply_outcome(&mut self, outcome: &Outcome) -> LogisticsResult<OutcomeEffect> {
        for file in &outcome.unlocked_purchase_files {
            self.add_new_bonus_purchase_file(file);
        }
        if outcome.result != MissionResult::PlayerWinBig {
            return Ok(OutcomeEffect {
                completed: false,
                campaign_over: self.campaign_over(),
            });
        }
        self.remove_mechs_in_force_group();
        self.set_mission_completed(outcome)?;
        Ok(OutcomeEffect {
            completed: true,
            campaign_over: self.campaign_over(),
        })
    }

    /// `setMissionCompleted`: marks the mission done, pays the successful objectives, brings the
    /// surviving mechs back into the bay (force group slots 1.. in reverse roster order),
    /// recovers mechs picked up during the mission, drops destroyed and disabled ones, and
    /// updates every pilot that flew.
    pub fn set_mission_completed(&mut self, outcome: &Outcome) -> LogisticsResult<()> {
        self.mission.set_mission_complete();
        self.rp_just_added = false;

        // first set all pilots as not just dead
        for p in &mut self.pilots {
            p.used = false;
        }

        for o in &outcome.objectives {
            if o.status == ObjectiveStatus::Successful {
                self.add_c_bills(i64::from(o.resource_points));
            }
        }

        // go find out which pilots died, and who came home
        let mission_id = self.mission.current_mission_id();
        let mut force_group_count = 1;
        for mover in outcome.movers.iter().rev() {
            // must check that we ever linked up with the mech
            if !(mover.on_gui && mover.is_battle_mech && mover.home_commander && !mover.is_air) {
                continue;
            }
            // `pMover->getPilot()->getName()` always names someone; a mover without a roster
            // pilot gets a name that matches nobody.
            let pilot_file = Some(mover.pilot_file.as_deref().unwrap_or(""));
            let existing = self.mech_by_name_and_pilot(&mover.variant_name, pilot_file);
            let pilot = pilot_file.and_then(|f| self.pilot_index(f));

            if existing.is_some() {
                if mover.destroyed || mover.disabled {
                    self.remove_mech_from_inventory(&mover.variant_name, pilot_file);
                } else {
                    self.remove_mech_from_inventory(&mover.variant_name, pilot_file);
                    if let Some(var) = self.variant_by_name(&mover.variant_name) {
                        self.add_mech_to_inventory(
                            var,
                            force_group_count,
                            pilot,
                            mover.paint_scheme,
                        );
                        force_group_count += 1;
                        if let Some(p) = pilot {
                            self.pilots[p].used = true;
                        }
                    }
                }
            } else if !mover.destroyed && !mover.disabled {
                // mech was recovered during the mission
                if let Some(var) = self.variant_by_name(&mover.variant_name) {
                    self.add_mech_to_inventory(var, force_group_count, pilot, mover.paint_scheme);
                    force_group_count += 1;
                    if let Some(p) = pilot {
                        self.pilots[p].used = true;
                    }
                }
                // else: "couldn't find the variant of a salvaged mech"
            }

            if let Some(p) = pilot {
                self.pilots[p].update(&mover.pilot, mission_id)?;
            }
        }
        Ok(())
    }

    /// `Logistics::initializeLogData`: after a mission that skips logistics, the units of the
    /// mission file become the inventory, one force group each. Note the original reads the
    /// paint scheme in a different order here than in `setMissionCompleted`
    /// (`getPaintScheme(highlight1, highlight2, base)`); the stored colors follow it.
    pub fn initialize_log_data(&mut self, roster: &[MoverReport]) {
        self.remove_mechs_in_force_group();
        for mover in roster.iter().rev() {
            if !mover.is_battle_mech {
                continue;
            }
            let pilot = mover
                .pilot_file
                .as_deref()
                .and_then(|f| self.pilot_index(f));
            let [highlight1, highlight2, base] = mover.paint_scheme;
            if let Some(var) = self.variant_by_name(&mover.variant_name) {
                self.add_mech_to_inventory(var, 1, pilot, [base, highlight1, highlight2]);
            }
        }
    }

    // ------------------------------------------------------------------------------------
    // salvage

    /// The salvage list the screen shows after a mission: candidates the campaign knows a
    /// variant for, cheapest first (ties in candidate order; the original's `qsort` leaves ties
    /// unspecified), with the price of recovering each.
    pub fn salvage_offers(&self, candidates: &[SalvageCandidate]) -> Vec<SalvageOffer> {
        let mut offers: Vec<SalvageOffer> = Vec::new();
        for (i, c) in candidates.iter().enumerate() {
            let Some(var) = self.variant_by_name(&c.variant_name) else {
                continue;
            };
            let exact_cost = 0.8f32 * self.variant_cost(var).unwrap_or(0) as f32;
            offers.push(SalvageOffer {
                candidate: i,
                variant: var,
                variant_name: self.variants[&var].name.clone(),
                cost: exact_cost as i32,
                exact_cost,
                tonnage: c.tonnage,
                paint_scheme: c.paint_scheme,
                armor: c.armor,
            });
        }
        offers.sort_by_key(|o| self.variant_cost(o.variant).unwrap_or(0));
        offers
    }

    /// Ticking a salvage box charges its price (`SalvageListItem::handleMessage`). Refused when
    /// the player cannot cover it.
    pub fn check_salvage(&mut self, cost: i32) -> LogisticsResult<()> {
        if i64::from(cost) > self.mission.c_bills {
            return Err(LogisticsError::NotEnoughResourcePoints);
        }
        self.decrement_c_bills(i64::from(cost));
        Ok(())
    }

    /// Unticking a salvage box gives the price back.
    pub fn uncheck_salvage(&mut self, cost: i32) {
        self.add_c_bills(i64::from(cost));
    }

    /// `SalvageMechScreen::updateSalvage`: the ticked mechs join the bay (no pilot, not in the
    /// force group; the price was charged when they were ticked).
    pub fn finish_salvage(&mut self, variants: &[VariantKey]) {
        for &v in variants {
            self.add_mech_to_inventory_with_pilot(v, None, 0, false);
        }
    }

    /// Ticks the given offers in order and then adds the mechs: the whole salvage screen in
    /// one call. Stops at the first offer the player cannot pay for (earlier ones stay charged
    /// and are added), returning the error.
    pub fn salvage(&mut self, offers: &[SalvageOffer]) -> LogisticsResult<Vec<MechKey>> {
        let mut taken = Vec::new();
        let mut result = Ok(());
        for o in offers {
            match self.check_salvage(o.cost) {
                Ok(()) => taken.push(o.variant),
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        let before = self.inventory.len();
        self.finish_salvage(&taken);
        let keys: Vec<MechKey> = self.inventory[before..].iter().map(|m| m.key).collect();
        result.map(|()| keys)
    }

    /// Weight of a chassis class etc. are available from [`Chassis`]; this is the chassis of a
    /// variant.
    pub fn chassis_of(&self, variant: VariantKey) -> Option<&Chassis> {
        self.variants
            .get(&variant)
            .map(|v| &self.data.chassis[v.chassis])
    }

    /// Whether the component at `index` is a heat sink (helper for screens).
    pub fn is_heat_sink(&self, index: usize) -> bool {
        self.data
            .catalog
            .get(index)
            .is_some_and(|c| c.form == ComponentForm::HeatSink)
    }
}

/// The key a purchase file name is stored under, exposed for callers that unlock lists.
pub fn purchase_list_key(name: &str) -> String {
    purchase_key(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::{MoverReport, ObjectiveReport};
    use crate::pilot::PilotReport;
    use crate::testdata;
    use crate::variant::test_support::{ECM, HEATSINK, JUMPJET, LASER, PPC};

    fn fresh() -> Logistics {
        Logistics::new(testdata::data()).unwrap()
    }

    fn v(l: &Logistics, name: &str) -> VariantKey {
        l.variant_by_name(name)
            .unwrap_or_else(|| panic!("no variant {name}"))
    }

    fn buy(l: &mut Logistics, name: &str) -> MechKey {
        let k = v(l, name);
        l.purchase_mech(k).unwrap()
    }

    fn mover(variant: &str, pilot: &str, destroyed: bool, disabled: bool) -> MoverReport {
        MoverReport {
            on_gui: true,
            is_battle_mech: true,
            home_commander: true,
            is_air: false,
            destroyed,
            disabled,
            variant_name: variant.into(),
            pilot_file: Some(pilot.into()),
            paint_scheme: [1, 2, 3],
            pilot: PilotReport::default(),
        }
    }

    #[test]
    fn a_new_campaign_selects_the_first_mission_and_applies_its_purchase_list() {
        let l = fresh();
        assert_eq!(l.current_mission(), "a");
        assert_eq!(
            l.c_bills(),
            80_500,
            "script C-Bills plus the mission's additional C-Bills"
        );
        assert_eq!(l.resource_points(), 1000);
        assert_eq!(l.max_drop_weight(), 200);
        let names: Vec<_> = l
            .purchasable_mechs()
            .iter()
            .map(|k| l.variant(*k).unwrap().name.clone())
            .collect();
        assert_eq!(
            names,
            vec!["Alpha Prime"],
            "the hidden variant is never offered"
        );
        let comps: Vec<_> = l
            .available_components()
            .iter()
            .map(|&i| l.data().catalog.get(i).unwrap().id)
            .collect();
        assert_eq!(comps, vec![10, 12, 13]);
        let pilots: Vec<_> = l
            .pilots()
            .iter()
            .filter(|p| p.available)
            .map(|p| p.file_name.as_str())
            .collect();
        assert_eq!(pilots, vec!["p1", "p2"]);
        assert!(l.new_weapons_available() && l.new_mechs_available() && l.new_pilots_available());
    }

    #[test]
    fn availability_flags_clear_when_nothing_changes() {
        let mut l = fresh();
        l.update_availability().unwrap();
        assert!(!l.new_weapons_available(), "no component became available");
        assert!(!l.new_mechs_available(), "same number of first variants");
        assert!(!l.new_pilots_available(), "no more pilots than before");
        l.set_new_mechs_acknowledged();
        l.set_new_pilots_acknowledged();
        l.set_new_weapons_acknowledged();
    }

    #[test]
    fn a_missing_purchase_file_is_reported_and_leaves_pilots_unavailable() {
        let mut l = fresh();
        l.mission.groups[0].missions[0].purchase_file_name = "data/missions/nothere.fit".into();
        assert_eq!(l.update_availability(), Err(LogisticsError::NoPurchaseFile));
        assert!(
            l.pilots().iter().all(|p| !p.available),
            "reset happens before the open"
        );
        // an empty name falls back to purchase.fit
        l.mission.groups[0].missions[0].purchase_file_name = String::new();
        l.update_availability().unwrap();
        assert!(l.pilots().iter().all(|p| !p.available));
        assert_eq!(
            l.available_components().len(),
            1,
            "purchase.fit lists the laser"
        );
    }

    #[test]
    fn mech_lists_in_the_main_file_consider_every_variant_but_bonus_lists_only_the_first() {
        // mission c lists beta: both Beta variants are checked against the available components
        let mut l = fresh();
        l.mission.current_stage = 1;
        l.mission.current_mission = -1;
        l.set_current_mission("c").unwrap();
        let names = |l: &Logistics| -> Vec<String> {
            l.purchasable_mechs()
                .iter()
                .map(|k| l.variant(*k).unwrap().name.clone())
                .collect()
        };
        assert_eq!(
            names(&l),
            vec!["Beta Prime"],
            "Beta Wide needs the big gun, which is not listed"
        );

        // mission a does not list beta; the bonus list does, but only its first variant counts
        let mut l = fresh();
        l.add_new_bonus_purchase_file("data\\missions\\Bonus.fit");
        l.update_availability().unwrap();
        assert_eq!(
            names(&l),
            vec!["Alpha Prime"],
            "Beta Wide is first and unavailable"
        );
        assert!(
            l.component_available(l.component_index(11).unwrap()),
            "the bonus gave the PPC"
        );
        assert!(l.pilots()[2].available, "and pilot p3");
        // an unknown bonus file adds nothing and breaks nothing
        l.add_new_bonus_purchase_file("data\\missions\\nothing.fit");
        l.update_availability().unwrap();
        assert_eq!(l.available_components().len(), 4);
    }

    #[test]
    fn purchase_and_sale_move_c_bills_by_the_full_cost() {
        let mut l = fresh();
        let prime = v(&l, "Alpha Prime");
        let cost = l.variant_cost(prime).unwrap();
        assert_eq!(cost, 100_000 + 1000 + 1000 + 800);
        assert_eq!(
            l.can_purchase_mech(prime),
            Err(LogisticsError::NotEnoughResourcePoints)
        );
        assert_eq!(
            l.purchase_mech(prime),
            Err(LogisticsError::NotEnoughResourcePoints)
        );
        l.add_c_bills(200_000);
        l.can_purchase_mech(prime).unwrap();
        let m = l.purchase_mech(prime).unwrap();
        assert_eq!(l.c_bills(), 280_500 - i64::from(cost));
        assert_eq!(l.mech(m).unwrap().instance_id(), 0);
        let m2 = l.purchase_mech(prime).unwrap();
        assert_eq!(
            l.mech(m2).unwrap().instance_id(),
            1,
            "instance numbers count up per variant"
        );
        assert_eq!(
            l.mech(m2).unwrap().id & 0x00ff_ffff,
            l.variant(prime).unwrap().id as i32
        );
        assert_eq!(l.variants_in_inventory(prime, false), 2);

        // selling a mech takes the *last* spare mech of that variant
        l.add_mech_to_force_group(m2, 1).unwrap();
        l.sell_mech(m).unwrap();
        assert_eq!(l.inventory().len(), 1);
        assert_eq!(l.c_bills(), 280_500 - i64::from(cost));
        assert_eq!(l.variants_in_inventory(prime, true), 1);
        // the deployed one cannot be sold
        assert_eq!(l.sell_mech(m2), Err(LogisticsError::Failed));
        assert_eq!(l.sell_mech(9999), Err(LogisticsError::Failed));
        assert_eq!(l.purchase_mech(99_999), Err(LogisticsError::Failed));
    }

    #[test]
    fn force_group_rules_follow_drop_weight_and_the_twelve_mech_limit() {
        let mut l = fresh();
        l.add_c_bills(10_000_000);
        let prime = v(&l, "Alpha Prime"); // 50 tons
        let mechs: Vec<_> = (0..5).map(|_| l.purchase_mech(prime).unwrap()).collect();
        // limit 200: four fit, the fifth does not
        for (i, m) in mechs.iter().take(4).enumerate() {
            assert!(l.can_add_mech_to_force_group(*m));
            l.add_mech_to_force_group(*m, i as i32 + 1).unwrap();
        }
        assert_eq!(l.current_drop_weight(), 200);
        assert!(!l.can_add_mech_to_force_group(mechs[4]));
        // slot numbers above 12 are refused
        assert_eq!(
            l.add_mech_to_force_group(mechs[4], 13),
            Err(LogisticsError::Failed)
        );

        // raise the limit: the twelve-mech cap bites
        l.mission.groups[0].missions[0].drop_weight = 10_000;
        for _ in 0..8 {
            l.purchase_mech(prime).unwrap();
        }
        let spare: Vec<_> = l
            .inventory()
            .iter()
            .filter(|m| m.force_group == 0)
            .map(|m| m.key)
            .collect();
        for (i, m) in spare.iter().take(8).enumerate() {
            assert!(l.can_add_mech_to_force_group(*m), "slot {}", i + 5);
            l.add_mech_to_force_group(*m, i as i32 + 5).unwrap();
        }
        assert_eq!(l.force_group().len(), 12);
        let last = l
            .inventory()
            .iter()
            .find(|m| m.force_group == 0)
            .unwrap()
            .key;
        assert!(!l.can_add_mech_to_force_group(last));
    }

    #[test]
    fn adding_a_deployed_mech_again_takes_a_spare_of_the_same_variant() {
        let mut l = fresh();
        l.add_c_bills(1_000_000);
        let a = buy(&mut l, "Alpha Prime");
        let b = buy(&mut l, "Alpha Prime");
        l.add_mech_to_force_group(a, 1).unwrap();
        l.add_mech_to_force_group(a, 2).unwrap();
        assert_eq!(l.mech(b).unwrap().force_group, 2, "the spare took slot 2");
        assert_eq!(
            l.add_mech_to_force_group(a, 3),
            Err(LogisticsError::Failed),
            "no spare left"
        );
        assert_eq!(l.mech_without_force_group(a), None);
    }

    #[test]
    fn removing_from_the_force_group_has_the_original_pilot_quirk() {
        let mut l = fresh();
        l.add_c_bills(1_000_000);
        let a = buy(&mut l, "Alpha Prime");
        let b = buy(&mut l, "Alpha Prime");
        l.set_mech_pilot(a, Some(0));
        l.set_mech_pilot(b, Some(1));
        assert!(l.pilots()[0].used && l.pilots()[1].used);
        l.add_mech_to_force_group(b, 1).unwrap();
        // `a` is spare: asking to remove `a` removes the similar deployed `b`, but the pilot that
        // is released is a's
        l.remove_mech_from_force_group(a, true).unwrap();
        assert_eq!(l.mech(b).unwrap().force_group, 0);
        assert_eq!(l.mech(b).unwrap().pilot, Some(1), "b keeps its pilot");
        assert_eq!(l.mech(a).unwrap().pilot, None, "a lost its pilot");
        assert!(!l.pilots()[0].used);
        // removing the mech that is deployed releases its own pilot when asked
        l.add_mech_to_force_group(b, 1).unwrap();
        l.remove_mech_from_force_group(b, false).unwrap();
        assert_eq!(l.mech(b).unwrap().pilot, Some(1));
        l.add_mech_to_force_group(b, 4).unwrap();
        l.remove_slot_from_force_group(4).unwrap();
        assert_eq!(
            l.mech(b).unwrap().pilot,
            None,
            "by slot, the pilot always goes"
        );
        assert_eq!(
            l.remove_slot_from_force_group(4),
            Err(LogisticsError::Failed)
        );
        assert_eq!(
            l.remove_mech_from_force_group(9999, true),
            Err(LogisticsError::Failed)
        );
    }

    #[test]
    fn choosing_a_lighter_mission_empties_force_slots_from_the_top() {
        let mut l = fresh();
        l.add_c_bills(10_000_000);
        let prime = v(&l, "Alpha Prime");
        for slot in 1..=4 {
            let m = l.purchase_mech(prime).unwrap();
            l.add_mech_to_force_group(m, slot).unwrap();
        }
        assert_eq!(l.current_drop_weight(), 200);
        l.mission.groups[0].missions[1].drop_weight = 120;
        l.set_current_mission("b").unwrap();
        assert_eq!(l.max_drop_weight(), 120);
        // slots 12 down to 1 are tried: 4 and 3 go, 100 tons is left
        let slots: Vec<_> = l.inventory().iter().map(|m| m.force_group).collect();
        assert_eq!(slots, vec![1, 2, 0, 0]);
        assert_eq!(l.current_drop_weight(), 100);
        assert_eq!(
            l.c_bills(),
            10_080_500 - 4 * i64::from(l.variant_cost(prime).unwrap()),
            "no C-Bill change"
        );
    }

    #[test]
    fn pilot_choice_prefers_free_pilots_then_rank_then_skill() {
        let mut l = fresh();
        l.mission.groups[0].missions[0].purchase_file_name = "data/missions/P_b.fit".into();
        l.update_availability().unwrap();
        // p1 40/40 green, p2 55/50 regular, p3 60/62 regular (avg 61 -> veteran)
        assert_eq!(l.pilots()[2].rank, crate::pilot::RANK_VETERAN);
        let best = l.best_pilot(50, &|_| false).unwrap();
        assert_eq!(l.pilots()[best].file_name, "p3");
        assert!(l.pilots()[best].used, "the pick is marked used");
        let second = l.best_pilot(50, &|_| false).unwrap();
        assert_eq!(l.pilots()[second].file_name, "p2");
        // a pilot the running mission already has is skipped
        // p1 is free, but the running mission has p1: the candidates are p2 and p3, both used, so
        // the first of them is returned anyway ("we're out of pilots, shouldn't be here")
        let third = l.best_pilot(50, &|p| p.file_name == "p1").unwrap();
        assert_eq!(third, 1);
        assert!(l.got_pilots_left(&|_| false), "p1 is free");
        assert!(!l.got_pilots_left(&|p| p.file_name == "p1"));
        l.pilots[0].used = true;
        assert!(!l.got_pilots_left(&|_| false));
        l.set_pilot_unused("p2");
        assert!(l.got_pilots_left(&|_| false));
        assert!(!l.got_pilots_left(&|p| p.file_name == "p2"));
        assert_eq!(
            l.first_available_pilot(),
            Some(0),
            "nobody is in a mech yet"
        );
        assert_eq!(
            l.compare_pilots(1, 0, 50),
            1,
            "p2 beats p1 on gunnery at equal rank"
        );
    }

    #[test]
    fn the_second_rank_comparison_of_compare_pilots_never_decides() {
        let mut l = fresh();
        l.mission.groups[0].missions[0].purchase_file_name = "data/missions/P_b.fit".into();
        l.update_availability().unwrap();
        // p4 is 70/75 elite but not in the list; compare anyway: lower rank, better gunnery
        l.pilots[0].rank = 3;
        l.pilots[0].gunnery = 10.0;
        l.pilots[1].rank = 1;
        l.pilots[1].gunnery = 90.0;
        assert_eq!(l.compare_pilots(0, 1, 0), 1, "higher rank wins");
        // the lower-ranked side is not marked down for rank; gunnery decides, so p2 (90) beats p1
        assert_eq!(
            l.compare_pilots(1, 0, 0),
            1,
            "rank 1 < 3 does not decide; gunnery 90 > 10"
        );
        l.pilots[1].gunnery = 5.0;
        assert_eq!(l.compare_pilots(1, 0, 0), -1, "gunnery 5 < 10 -> -1");
        l.pilots[1].gunnery = 10.0;
        l.pilots[1].piloting = 99.0;
        assert_eq!(
            l.compare_pilots(1, 0, 0),
            1,
            "equal gunnery, better piloting"
        );
    }

    #[test]
    fn pilots_in_the_bay_rank_below_free_ones() {
        let mut l = fresh();
        l.add_c_bills(1_000_000);
        let a = buy(&mut l, "Alpha Prime");
        l.set_mech_pilot(a, Some(1));
        l.pilots[1].used = false; // assigned in the bay but not flagged used
        assert_eq!(l.compare_pilots(1, 0, 0), -1, "p2 is in a mech, p1 is free");
        assert_eq!(l.compare_pilots(0, 1, 0), 1);
        assert_eq!(l.first_available_pilot(), Some(0));
    }

    // ----- mech lab

    fn lab_setup() -> (Logistics, MechKey) {
        let mut l = fresh();
        l.add_c_bills(1_000_000);
        let m = buy(&mut l, "Alpha Prime");
        (l, m)
    }

    #[test]
    fn editing_works_on_a_copy_until_accepted() {
        let (mut l, m) = lab_setup();
        let original = l.mech(m).unwrap().variant;
        let before = l.c_bills();
        let old_cost = l.variant_cost(original).unwrap();

        let copy = l.set_mech_to_modify(m).unwrap();
        assert_ne!(copy, original);
        assert_eq!(l.mech(m).unwrap().variant, copy);
        assert_eq!(l.original_variant(), Some(original));
        assert!(!l.variant(copy).unwrap().designer);
        assert_eq!(l.variant(copy).unwrap().name, "Alpha Prime");
        assert_eq!(
            l.variant(copy).unwrap().id >> 16,
            2,
            "the copy takes the next number on the chassis"
        );
        assert_eq!(
            l.variant_by_id(l.mech(m).unwrap().id),
            Some(copy),
            "found through the edited mech"
        );

        // add a PPC to the copy: heat 2+2+4 = 8 <= 20
        assert!(
            !l.component_available(l.component_index(11).unwrap()),
            "no PPC on mission a"
        );
        let heatsink = l.component_index(12).unwrap();
        let (mut x, mut y) = (-1, -1);
        l.lab_can_add_component(heatsink, &mut x, &mut y).unwrap();
        l.lab_add_component(heatsink, &mut x, &mut y).unwrap();
        l.lab_can_afford().unwrap();
        l.accept_mech_modifications("My Alpha").unwrap();

        let new_cost = l.variant_cost(copy).unwrap();
        assert_eq!(new_cost, old_cost + 600);
        assert_eq!(
            l.c_bills(),
            before + i64::from(old_cost) - i64::from(new_cost)
        );
        assert_eq!(l.mech_name(m), Some("My Alpha"));
        assert_eq!(l.player_variant_names(), vec!["My Alpha".to_string()]);
        assert_eq!(l.mech_to_modify(), None);
        // the designer variant is untouched, and the new one is in the list after it
        assert!(l.variant(original).unwrap().designer);
        assert_eq!(l.variant_by_name("my alpha"), Some(copy));
    }

    #[test]
    fn cancelling_restores_the_mech_and_frees_the_copy() {
        let (mut l, m) = lab_setup();
        let original = l.mech(m).unwrap().variant;
        let id = l.mech(m).unwrap().id;
        let refs = l.chassis_refs.clone();
        let copy = l.set_mech_to_modify(m).unwrap();
        assert_eq!(l.chassis_refs[0], refs[0] + 1);
        l.cancel_mech_modifications().unwrap();
        assert!(l.variant(copy).is_none());
        assert_eq!(l.mech(m).unwrap().variant, original);
        // The mech counts itself when it is assigned a variant: instance 0 became 1 with the
        // copy and 2 on the way back.
        assert_eq!(l.mech(m).unwrap().id, id + (2 << 24));
        assert_eq!(l.chassis_refs, refs);
        assert_eq!(l.cancel_mech_modifications(), Err(LogisticsError::Failed));
        assert_eq!(
            l.accept_mech_modifications("x"),
            Err(LogisticsError::Failed)
        );
    }

    #[test]
    fn accepting_under_the_old_name_replaces_an_unused_player_variant() {
        let (mut l, m) = lab_setup();
        l.set_mech_to_modify(m).unwrap();
        l.accept_mech_modifications("Mine").unwrap();
        let first = l.mech(m).unwrap().variant;
        // edit it again and keep the name: the old player variant goes away
        l.set_mech_to_modify(m).unwrap();
        let hs = l.component_index(12).unwrap();
        let (mut x, mut y) = (-1, -1);
        l.lab_add_component(hs, &mut x, &mut y).unwrap();
        l.accept_mech_modifications("MINE").unwrap();
        assert!(l.variant(first).is_none(), "replaced");
        assert_eq!(l.player_variant_names(), vec!["MINE".to_string()]);

        // under a different name the old one stays, unused, in the list
        let current = l.mech(m).unwrap().variant;
        l.set_mech_to_modify(m).unwrap();
        l.accept_mech_modifications("Renamed").unwrap();
        assert!(l.variant(current).is_some());
        assert_eq!(
            l.player_variant_names(),
            vec!["MINE".to_string(), "Renamed".to_string()]
        );
        // and can be deleted since no mech uses it
        assert!(l.can_delete_variant("MINE"));
        l.remove_variant("mine").unwrap();
        assert!(l.variant(current).is_none());
        assert_eq!(l.player_variant_names(), vec!["Renamed".to_string()]);
    }

    #[test]
    fn name_checks_protect_designer_variants_and_shared_names() {
        let (mut l, m) = lab_setup();
        let m2 = buy(&mut l, "Alpha Prime");
        l.set_mech_to_modify(m).unwrap();
        // a designer variant's name cannot be taken
        assert!(!l.can_replace_variant("Alpha Prime"));
        assert!(l.can_replace_variant("Fresh Name"));
        l.accept_mech_modifications("Shared").unwrap();
        // two mechs now share nothing yet; edit the second to the same variant name
        l.set_mech_to_modify(m2).unwrap();
        assert!(
            !l.can_replace_variant("Shared"),
            "the other mech carries that name"
        );
        assert_eq!(
            l.accept_mech_modifications("Shared"),
            Err(LogisticsError::NameInInventory),
            "refused instead of deleting a variant another mech uses"
        );
        assert_eq!(l.mech_to_modify(), Some(m2), "the edit is still open");
        l.cancel_mech_modifications().unwrap();
        // a variant in use by a mech cannot be removed
        assert_eq!(
            l.remove_variant("Shared"),
            Err(LogisticsError::VariantInUse)
        );
        assert_eq!(
            l.remove_variant("nothing"),
            Err(LogisticsError::InvalidVariant)
        );
        assert!(!l.can_delete_variant("Shared"));
        assert!(!l.can_delete_variant("nothing"));
        // a variant under edit cannot be removed either
        l.set_mech_to_modify(m).unwrap();
        assert_eq!(l.remove_variant("Shared"), Err(LogisticsError::Failed));
        // (m2 still uses the designer variant)
        assert_eq!(
            l.remove_variant("Alpha Prime"),
            Err(LogisticsError::VariantInUse)
        );
    }

    #[test]
    fn using_an_equal_existing_variant_swaps_it_in_with_a_refund() {
        let (mut l, m) = lab_setup();
        let designer = l.mech(m).unwrap().variant;
        let before = l.c_bills();
        l.set_mech_to_modify(m).unwrap();
        // the edit is identical to the designer variant
        let copy = l.variant_being_edited().unwrap();
        let ctx_eq = {
            let a = l.variant(copy).unwrap();
            let b = l.variant(designer).unwrap();
            a.equivalent(b, &l.ctx_for(a), &l.ctx_for(b))
        };
        assert!(ctx_eq);
        l.accept_mech_modifications_use_old_variant("Alpha Prime")
            .unwrap();
        assert_eq!(l.mech(m).unwrap().variant, designer);
        assert_eq!(l.c_bills(), before, "refund and charge cancel");
        assert!(l.variant(copy).is_none());
        // unknown name: nothing changes
        l.set_mech_to_modify(m).unwrap();
        assert_eq!(
            l.accept_mech_modifications_use_old_variant("Nope"),
            Err(LogisticsError::InvalidVariant)
        );
        assert_eq!(l.c_bills(), before);
    }

    #[test]
    fn the_lab_refuses_what_the_money_cannot_cover() {
        let (mut l, m) = lab_setup();
        l.set_mech_to_modify(m).unwrap();
        let hs = l.component_index(12).unwrap();
        let (mut x, mut y) = (-1, -1);
        l.lab_add_component(hs, &mut x, &mut y).unwrap();
        l.lab_can_afford().unwrap();
        l.mission.c_bills = 599; // the sink costs 600
        assert_eq!(
            l.lab_can_afford(),
            Err(LogisticsError::NotEnoughResourcePoints)
        );
        l.mission.c_bills = 600;
        l.lab_can_afford().unwrap();
    }

    #[test]
    fn lab_edits_use_the_variant_rules() {
        let (mut l, m) = lab_setup();
        l.set_mech_to_modify(m).unwrap();
        let jj = l.component_index(13).unwrap();
        let (mut x, mut y) = (-1, -1);
        assert_eq!(
            l.lab_can_add_component(jj, &mut x, &mut y),
            Err(LogisticsError::OnlyOneJumpJetAllowed),
            "Alpha Prime already has jump jets"
        );
        l.lab_remove_component(-2, -2).unwrap();
        l.lab_add_component(jj, &mut x, &mut y).unwrap();
        assert_eq!((x, y), (-2, -2));
        // copying the loadout of another variant of the chassis
        let hidden = v(&l, "Alpha Hidden");
        l.lab_copy_loadout_from(hidden).unwrap();
        let edited = l.variant_being_edited().unwrap();
        assert_eq!(l.variant(edited).unwrap().components.len(), 1);
        assert_eq!(
            l.variant(edited).unwrap().name,
            "Alpha Prime",
            "the name stays"
        );
        assert!(!l.variant(edited).unwrap().designer);
    }

    // ----- missions

    #[test]
    fn completing_a_mission_brings_survivors_home_and_pays_the_objectives() {
        let mut l = fresh();
        l.add_c_bills(1_000_000);
        let alpha = buy(&mut l, "Alpha Prime");
        let spare = buy(&mut l, "Alpha Prime");
        let _ = spare;
        l.set_mech_pilot(alpha, Some(0));
        l.add_mech_to_force_group(alpha, 1).unwrap();
        let before = l.c_bills();

        let outcome = Outcome {
            result: MissionResult::PlayerWinBig,
            objectives: vec![
                ObjectiveReport {
                    status: ObjectiveStatus::Successful,
                    resource_points: 10_000,
                },
                ObjectiveReport {
                    status: ObjectiveStatus::Failed,
                    resource_points: 99_999,
                },
                ObjectiveReport {
                    status: ObjectiveStatus::Undetermined,
                    resource_points: 77_777,
                },
                ObjectiveReport {
                    status: ObjectiveStatus::Successful,
                    resource_points: 2_500,
                },
            ],
            unlocked_purchase_files: vec!["data\\missions\\Bonus.fit".into()],
            // roster order: a survivor, a destroyed one, a disabled one, a copter, a stranger
            movers: vec![
                mover("Alpha Prime", "p1", false, false),
                mover("Alpha Prime", "p2", true, false),
                mover("Alpha Prime", "p3", false, true),
                MoverReport {
                    is_air: true,
                    ..mover("Alpha Prime", "p4", false, false)
                },
                MoverReport {
                    on_gui: false,
                    ..mover("Alpha Prime", "p4", false, false)
                },
            ],
            mechs: vec![],
            mission_time: 0.0,
        };
        let effect = l.apply_outcome(&outcome).unwrap();
        assert!(effect.completed && !effect.campaign_over);

        assert_eq!(
            l.c_bills(),
            before + 12_500 + 500,
            "objectives plus the mission's additional C-Bills"
        );
        // the deployed mech left and came back; the spare stayed
        assert_eq!(l.inventory().len(), 2);
        let back = l.inventory().iter().find(|m| m.force_group != 0).unwrap();
        assert_eq!(back.force_group, 1);
        assert_eq!(back.pilot, Some(0));
        assert_eq!(back.colors, [1, 2, 3]);
        assert!(l.pilots()[0].used && l.pilots()[0].missions_completed == 1);
        // a destroyed or disabled mech still has its pilot updated; air units and movers the
        // player never controlled are ignored
        assert_eq!(l.pilots()[1].missions_completed, 1);
        assert_eq!(l.pilots()[2].missions_completed, 1);
        assert_eq!(l.pilots()[3].missions_completed, 0);
        assert_eq!(
            l.mission_info().additional_purchase_files,
            vec!["data\\missions\\Bonus.fit".to_string()]
        );
        assert_eq!(l.mission_info().last_mission_name, "a");
        assert_eq!(
            l.mission_info().current_mission_name,
            "c",
            "the group quota is 1: on to group 1"
        );
    }

    #[test]
    fn survivors_take_slots_in_reverse_roster_order_and_an_unknown_variant_is_skipped() {
        let mut l = fresh();
        let outcome = Outcome {
            movers: vec![
                mover("Alpha Prime", "p1", false, false),
                mover("No Such Variant", "p2", false, false),
                mover("Alpha Prime", "p3", false, false),
            ],
            ..Outcome::won()
        };
        l.apply_outcome(&outcome).unwrap();
        let slots: Vec<_> = l
            .inventory()
            .iter()
            .map(|m| {
                (
                    m.force_group,
                    l.pilots()[m.pilot.unwrap()].file_name.clone(),
                )
            })
            .collect();
        assert_eq!(slots, vec![(1, "p3".to_string()), (2, "p1".to_string())]);
        // p2's mech had no variant to come back as, but the pilot still flew
        assert_eq!(l.pilots()[1].missions_completed, 1);
    }

    #[test]
    fn a_dead_pilot_keeps_the_mech_they_came_home_in_for_this_step() {
        let mut l = fresh();
        let mut m = mover("Alpha Prime", "p1", false, false);
        m.pilot.status = crate::pilot::WarriorStatus::Dead;
        l.apply_outcome(&Outcome {
            movers: vec![m],
            ..Outcome::won()
        })
        .unwrap();
        let back = &l.inventory()[0];
        assert_eq!(
            back.pilot,
            Some(0),
            "the pilot was alive when the mech was re-added"
        );
        assert!(l.pilots()[0].dead && l.pilots()[0].just_died);
        // a later add refuses the dead pilot
        let var = v(&l, "Alpha Prime");
        let k = l.add_mech_to_inventory(var, 0, Some(0), [0; 3]).unwrap();
        assert_eq!(l.mech(k).unwrap().pilot, None);
    }

    #[test]
    fn a_lost_mission_changes_nothing_but_the_bonus_lists() {
        let mut l = fresh();
        l.add_c_bills(1_000_000);
        let a = buy(&mut l, "Alpha Prime");
        l.add_mech_to_force_group(a, 1).unwrap();
        let before = l.to_save();
        let outcome = Outcome {
            result: MissionResult::PlayerLostBig,
            unlocked_purchase_files: vec!["bonus".into()],
            objectives: vec![ObjectiveReport {
                status: ObjectiveStatus::Successful,
                resource_points: 5000,
            }],
            ..Outcome::won()
        };
        let effect = l.apply_outcome(&outcome).unwrap();
        assert!(!effect.completed);
        let mut after = l.to_save();
        assert_eq!(after.additional_purchase_files, vec!["bonus".to_string()]);
        after.additional_purchase_files.clear();
        assert_eq!(after, before);
        assert_eq!(l.current_mission(), "a", "the same mission is played again");
    }

    #[test]
    fn the_campaign_ends_after_the_last_group() {
        let mut l = fresh();
        for expected in ["c", "e"] {
            let _ = expected;
        }
        // a -> group 1 (c, d hidden mandatory) -> ... -> e -> over
        for name in ["a", "c", "d", "e"] {
            assert_eq!(l.current_mission(), name);
            l.apply_outcome(&Outcome::won()).unwrap();
            if !l.campaign_over() {
                l.begin_mission_selection().unwrap();
            }
        }
        assert!(l.campaign_over());
        assert!(l.mission_info().final_video() == "FINAL");
        assert!(l.available_missions().is_empty());
    }

    #[test]
    fn load_spec_lists_piloted_force_mechs_and_custom_loadouts() {
        let mut l = fresh();
        l.add_c_bills(1_000_000);
        let designer = buy(&mut l, "Alpha Prime");
        let custom = buy(&mut l, "Alpha Prime");
        let unpiloted = buy(&mut l, "Alpha Prime");
        l.set_mech_pilot(designer, Some(0));
        l.set_mech_pilot(custom, Some(1));
        l.mission.groups[0].missions[0].drop_weight = 1000;
        for (i, m) in [designer, custom, unpiloted].into_iter().enumerate() {
            l.add_mech_to_force_group(m, i as i32 + 1).unwrap();
        }
        l.set_mech_to_modify(custom).unwrap();
        l.accept_mech_modifications("Custom One").unwrap();

        let spec = l.load_spec(MissionOptions {
            difficulty: 2,
            unlimited_ammo: false,
            base_color: 11,
            highlight_color: 22,
        });
        assert_eq!(spec.mission, "a");
        assert_eq!(spec.load_type, LoadType::SpLogistics);
        assert_eq!((spec.difficulty, spec.unlimited_ammo), (2, false));
        assert_eq!(spec.resource_points, 1000);
        assert_eq!(spec.drop_weight_limit, 1000);
        assert_eq!(spec.abl_script, "NONE");
        assert!(spec.support.air_strike && !spec.support.artillery_piece);
        assert_eq!(spec.force.len(), 2, "the mech without a pilot stays home");
        let f0 = &spec.force[0];
        assert_eq!(f0.variant_name, "Alpha Prime");
        assert!(f0.designer && f0.components.is_empty());
        assert_eq!(f0.variant_id, 0);
        assert_eq!(f0.pilot_file, "p1");
        assert_eq!((f0.gunnery, f0.piloting), (40.0, 40.0));
        assert_eq!(f0.chassis_file, "data/objects/alpha.csv");
        assert_eq!(
            f0.colors,
            [11, 22, 22],
            "the preference colors, not the bay mech's"
        );
        assert_eq!(f0.object_number, 5);
        let f1 = &spec.force[1];
        assert_eq!(f1.variant_name, "Custom One");
        assert!(!f1.designer);
        assert_eq!(f1.variant_id, 0);
        let ids: Vec<_> = f1.components.iter().map(|c| c.component_id).collect();
        assert_eq!(ids, vec![10, 10, 13]);
        assert_eq!(f1.components[2].x, -2);
    }

    #[test]
    fn a_mission_that_skips_logistics_loads_with_its_own_force() {
        let mut l = fresh();
        l.mission.groups[0].missions[0].play_logistics = false;
        let spec = l.load_spec(MissionOptions::default());
        assert_eq!(spec.load_type, LoadType::SpQuickstart);
        assert!(spec.force.is_empty());
    }

    #[test]
    fn initialize_log_data_adds_the_roster_with_swapped_paint_order() {
        let mut l = fresh();
        let mut roster = vec![
            mover("Alpha Prime", "p1", false, false),
            mover("Alpha Prime", "p2", false, false),
        ];
        roster[0].paint_scheme = [10, 20, 30];
        l.initialize_log_data(&roster);
        // reverse roster order; colours (base, h1, h2) = (third, first, second) of the paint scheme
        assert_eq!(l.inventory().len(), 2);
        assert_eq!(l.pilots()[l.inventory()[1].pilot.unwrap()].file_name, "p1");
        assert_eq!(l.inventory()[1].colors, [30, 10, 20]);
        assert!(l.inventory().iter().all(|m| m.force_group == 1));
        // running it again clears the old force first
        l.initialize_log_data(&roster[..1]);
        assert_eq!(l.inventory().len(), 1);
    }

    #[test]
    fn salvage_prices_are_eighty_percent_and_ticking_checks_the_balance() {
        let mut l = fresh();
        let candidates = vec![
            SalvageCandidate {
                variant_name: "Alpha Prime".into(),
                tonnage: 50.0,
                paint_scheme: [0; 3],
                armor: [(0.0, 0.0, false); 11],
            },
            SalvageCandidate {
                variant_name: "Unknown".into(),
                tonnage: 10.0,
                paint_scheme: [0; 3],
                armor: [(0.0, 0.0, false); 11],
            },
            SalvageCandidate {
                variant_name: "alpha hidden".into(),
                tonnage: 50.0,
                paint_scheme: [0; 3],
                armor: [(0.0, 0.0, false); 11],
            },
        ];
        let offers = l.salvage_offers(&candidates);
        assert_eq!(offers.len(), 2, "unknown variants cannot be recovered");
        // cheapest first: the hidden alpha has only a laser
        assert_eq!(offers[0].variant_name, "Alpha Hidden");
        assert_eq!(offers[0].cost, ((100_000 + 1000) as f32 * 0.8) as i32);
        assert_eq!(
            offers[1].cost,
            ((100_000 + 1000 + 1000 + 800) as f32 * 0.8) as i32
        );

        l.mission.c_bills = i64::from(offers[0].cost);
        assert_eq!(
            l.check_salvage(offers[1].cost),
            Err(LogisticsError::NotEnoughResourcePoints)
        );
        l.check_salvage(offers[0].cost).unwrap();
        assert_eq!(l.c_bills(), 0);
        let before = l.inventory().len();
        l.finish_salvage(&[offers[0].variant]);
        assert_eq!(l.inventory().len(), before + 1);
        assert_eq!(l.inventory()[before].pilot, None);
        l.uncheck_salvage(5);
        assert_eq!(l.c_bills(), 5);
        // the whole screen at once stops at the first unaffordable pick
        l.mission.c_bills = i64::from(offers[0].cost);
        let err = l.salvage(&offers).unwrap_err();
        assert_eq!(err, LogisticsError::NotEnoughResourcePoints);
        assert_eq!(l.inventory().len(), before + 2, "the first pick was added");
    }

    #[test]
    fn selection_screen_presses_the_current_mission_and_lists_the_group() {
        let mut l = fresh();
        let choices = l.begin_mission_selection().unwrap();
        assert_eq!(choices.len(), 2);
        assert_eq!(
            choices[0],
            crate::results::MissionChoice {
                name: "a".into(),
                available: true,
                selected: true
            }
        );
        let after = l.choose_mission("b").unwrap();
        assert!(after[1].selected && !after[0].selected);
        assert_eq!(l.mission_info().current_mission, 1);
        assert_eq!(l.choose_mission("zzz"), Err(LogisticsError::InvalidId));
        // completed missions are listed but disabled
        l.apply_outcome(&Outcome::won()).unwrap();
        assert_eq!(l.current_mission(), "c");
        let choices = l.begin_mission_selection().unwrap();
        assert_eq!(
            choices.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["c"]
        );
    }

    #[test]
    fn the_results_flow_runs_salvage_then_pilot_review_then_autosaves() {
        use crate::results::ResultsStage;
        let mut l = fresh();
        l.add_c_bills(1_000_000);
        let a = buy(&mut l, "Alpha Prime");
        l.set_mech_pilot(a, Some(1));
        l.add_mech_to_force_group(a, 1).unwrap();
        let mut m = mover("Alpha Prime", "p2", false, false);
        m.pilot = PilotReport {
            gunnery_rank: 55.0,
            piloting_rank: 50.0,
            gunnery_points: 100.0,
            piloting_points: 2.0,
            rank: 1,
            ..PilotReport::default()
        };
        let mut dead = mover("Alpha Prime", "p1", true, false);
        dead.pilot.status = crate::pilot::WarriorStatus::Dead;
        dead.pilot.gunnery_rank = 40.0;
        dead.pilot.piloting_rank = 40.0;
        let outcome = Outcome {
            movers: vec![m, dead],
            mechs: vec![
                crate::interface::MechSummary::wreck("Alpha Prime", 50.0),
                crate::interface::MechSummary {
                    disabled: false,
                    ..crate::interface::MechSummary::wreck("Alpha Prime", 50.0)
                },
                crate::interface::MechSummary {
                    destroyed: true,
                    ..crate::interface::MechSummary::wreck("Alpha Prime", 50.0)
                },
                crate::interface::MechSummary {
                    move_level: 2,
                    ..crate::interface::MechSummary::wreck("Alpha Prime", 50.0)
                },
            ],
            ..Outcome::won()
        };
        assert_eq!(
            outcome.salvage_candidates().len(),
            1,
            "only the disabled, intact, ground mech"
        );
        l.apply_outcome(&outcome).unwrap();

        let mut flow = l.begin_results(&outcome);
        assert_eq!(flow.stage(), ResultsStage::Salvage);
        assert!(
            flow.end(&l).is_none(),
            "no autosave before the salvage is done"
        );
        assert_eq!(flow.offers().len(), 1);
        let cb = l.c_bills();
        assert!(flow.offer_enabled(&l, 0));
        assert_eq!(flow.toggle_salvage(&mut l, 0), Ok(true));
        assert_eq!(
            flow.toggle_salvage(&mut l, 5),
            Err(LogisticsError::InvalidId)
        );
        assert_eq!(l.c_bills(), cb - i64::from(flow.offers()[0].cost));
        let added = flow.finish_salvage(&mut l);
        assert_eq!(added.len(), 1);
        assert_eq!(flow.stage(), ResultsStage::PilotReview);
        assert!(
            flow.end(&l).is_none(),
            "no autosave during the pilot review"
        );
        let review = flow.pilot_review().unwrap();
        assert_eq!(review.killed, vec![0]);
        assert_eq!(review.active.len(), 1);
        let p = &review.active[0];
        assert_eq!(p.pilot, 1);
        assert_eq!(p.gain, (56.0 - 55.0, 53.0 - 50.0));
        assert!(!p.promoted, "avg 54.5 stays regular");
        flow.finish_pilot_review(&mut l);
        assert!(flow.is_done());
        assert!(!l.pilots()[0].just_died);
        assert_eq!(flow.end(&l).unwrap().mission(), "a");
        // nothing more to do after done
        assert!(flow.finish_salvage(&mut l).is_empty());
        assert_eq!(flow.toggle_salvage(&mut l, 0), Err(LogisticsError::Failed));
    }

    #[test]
    fn the_last_mission_and_single_missions_have_no_results_screens() {
        use crate::results::ResultsStage;
        // end of the campaign: the final video, no screens
        let mut l = fresh();
        for _ in 0..4 {
            l.apply_outcome(&Outcome::won()).unwrap();
            if !l.campaign_over() {
                l.begin_mission_selection().unwrap();
            }
        }
        assert!(l.campaign_over());
        let flow = l.begin_results(&Outcome::won());
        assert_eq!(flow.stage(), ResultsStage::Done);
        assert!(flow.pilot_review().is_none() && flow.offers().is_empty());
        assert!(flow.end(&l).is_none(), "no autosave either");
    }

    #[test]
    fn a_single_mission_replaces_the_campaign_and_clears_the_bay() {
        use crate::results::ResultsStage;
        use mc2_sim_data::campaign::MissionSettings;
        let (mut l, m) = lab_setup();
        l.set_mech_to_modify(m).unwrap();
        l.accept_mech_modifications("Keeper").unwrap();
        l.pilots[0].used = true;
        l.add_new_bonus_purchase_file("bonus");

        let settings = MissionSettings {
            resource_points: Some(7000),
            drop_weight_limit: 190,
            enable_air_strike: false,
            ..MissionSettings::default()
        };
        l.set_single_mission("solo", &settings, Some(190.5));

        let mi = l.mission_info();
        assert!(mi.single_mission && mi.is_single_mission());
        assert_eq!(l.current_mission(), "solo");
        // no AdditionalCBills in the file: 1200 * DropWeightLimit + 100000
        assert_eq!(l.c_bills(), 328_600);
        assert_eq!(l.max_drop_weight(), 190);
        assert_eq!(mi.current_operation_file(), Some("MCL_CM_Op1_1.fit"));
        assert!(l.inventory().is_empty());
        assert!(
            l.player_variant_names().is_empty(),
            "player variants are cleared"
        );
        assert!(
            l.pilots().iter().all(|p| !p.used && !p.available),
            "purchase.fit lists no pilots"
        );
        // availability comes from purchase.fit (the entry names no purchase file)
        assert_eq!(l.available_components().len(), 1);
        assert!(!l.support_enables().air_strike);
        // picking a mission is the multiplayer branch of the original: refused
        assert_eq!(
            l.set_current_mission("solo"),
            Err(LogisticsError::NotYetImplemented)
        );
        assert!(l.mission_info().additional_purchase_files.is_empty());

        // an AdditionalCBills of the file is used as it is
        let rich = MissionSettings {
            additional_cbills: Some(55_000),
            ..settings.clone()
        };
        l.set_single_mission("solo", &rich, None);
        assert_eq!(l.c_bills(), 55_000);

        // saved and loaded back
        let text = l.save_to_string().unwrap();
        let back = Logistics::load_from_str(testdata::data(), &text)
            .unwrap()
            .logistics;
        assert_eq!(back.to_save(), l.to_save());
        assert!(back.mission_info().single_mission);

        // winning it ends the game; the results flow does not start
        l.apply_outcome(&Outcome::won()).unwrap();
        assert!(l.campaign_over());
        let flow = l.begin_results(&Outcome::won());
        assert_eq!(flow.stage(), ResultsStage::Done);
        assert!(flow.end(&l).is_none(), "single missions are not autosaved");
    }

    #[test]
    fn mission_flags_skip_the_result_screens() {
        use crate::results::ResultsStage;
        let mut l = fresh();
        // `skipSalvageScreen` looks at the *previous mission* in the sense of `getPreviousMission`:
        // with the new group's first mission selected, that is the last mission of the group
        // before (b here), not necessarily the one just played.
        l.mission.groups[0].missions[1].play_salvage = false;
        l.mission.groups[0].missions[1].play_pilot_promotion = false;
        l.apply_outcome(&Outcome::won()).unwrap();
        let flow = l.begin_results(&Outcome::won());
        assert_eq!(flow.stage(), ResultsStage::Done);
        assert!(flow.pilot_review().is_none());
    }

    #[test]
    fn promotions_are_reported_when_a_skill_gain_crosses_a_rank() {
        let mut l = fresh();
        l.pilots[1].gunnery = 49.0; // p2: 49/49 green
        l.pilots[1].piloting = 49.0;
        l.pilots[1].rank = 0;
        let mut m = mover("Alpha Prime", "p2", false, false);
        m.pilot.gunnery_rank = 49.0;
        m.pilot.piloting_rank = 49.0;
        m.pilot.gunnery_points = 400.0; // +4
        m.pilot.piloting_points = 3.0; // +1 free +3 = +4
        l.apply_outcome(&Outcome {
            movers: vec![m],
            ..Outcome::won()
        })
        .unwrap();
        let mut flow = l.begin_results(&Outcome::won());
        let review = flow.pilot_review().unwrap();
        assert!(review.active[0].promoted, "49 -> 53 average crosses 50");
        assert_eq!(l.pilots()[1].rank, crate::pilot::RANK_REGULAR);
        flow.finish_salvage(&mut l);
    }

    #[test]
    fn helpers_filter_variant_lists() {
        let l = fresh();
        assert_eq!(l.chassis_variants(0).len(), 2);
        assert_eq!(
            l.encyclopedia_mechs().len(),
            2,
            "Alpha Prime and Beta Prime"
        );
        let _ = (HEATSINK, ECM, PPC, JUMPJET, LASER);
        assert!(l.variant_by_chassis_file("alpha", 0).is_some());
        assert!(l.variant_by_chassis_file("alpha", 1).is_some());
        assert_eq!(l.variant_by_chassis_file("alpha", 5), None);
        assert_eq!(l.pilot_index("P2"), Some(1));
        assert_eq!(l.pilot_index("nobody"), None);
        assert_eq!(l.unassigned_pilots(), vec![0, 1]);
        assert_eq!(l.all_components().len(), 8);
        assert!(l.is_heat_sink(2));
    }

    #[test]
    fn start_new_campaign_resets_progress_but_keeps_player_variants() {
        let (mut l, m) = lab_setup();
        l.set_mech_to_modify(m).unwrap();
        l.accept_mech_modifications("Keeper").unwrap();
        l.apply_outcome(&Outcome::won()).unwrap();
        assert_eq!(l.mission_info().current_stage, 1);
        l.start_new_campaign("campaign").unwrap();
        assert_eq!(l.mission_info().current_stage, 0);
        assert_eq!(l.current_mission(), "a");
        assert!(l.inventory().is_empty());
        assert_eq!(l.c_bills(), 80_500);
        assert_eq!(l.player_variant_names(), vec!["Keeper".to_string()]);
        assert!(l
            .pilots()
            .iter()
            .all(|p| !p.used && p.missions_completed == 0));
        assert_eq!(
            l.start_new_campaign("nope"),
            Err(LogisticsError::InvalidFileName)
        );
    }
}
