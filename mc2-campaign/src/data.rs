// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The static game data the campaign state refers to.
//!
//! [`GameData`] is built once from the shipped files (through `mc2_formats::Vfs`) and then only
//! read: the component catalogue, the chassis and designer variants, the pilot roster as the
//! game ships it, the campaign scripts and every purchase list a campaign can reach. The
//! mutable campaign state ([`crate::Logistics`]) holds an `Arc<GameData>` and never re-reads a
//! file, so it can run (and be tested) without a data directory.
//!
//! Load path, with the C++ it follows:
//! - components: `LogisticsData::initComponents` ([`ComponentCatalog::load`]);
//! - chassis and designer variants: `LogisticsData::initVariants`
//!   (`code/logisticsdata.cpp:222-351`), driven by `data/art/buildings.csv`;
//! - pilots: `LogisticsData::initPilots` and `LogisticsPilot::init`;
//! - campaign scripts: `LogisticsMissionInfo::init` via `mc2_sim_data::campaign`;
//! - purchase lists: `updateAvailability` / `appendAvailability` read these files by path; here
//!   they are loaded ahead of time, keyed by [`purchase_key`].

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use mc2_formats::csv::CsvFile;
use mc2_formats::ini::FitIni;
use mc2_formats::pak::PakFile;
use mc2_formats::vfs::{normalize_name, Vfs};
use mc2_sim_data::campaign::{Campaign, PurchaseList, MISSION_DIR};
use mc2_sim_data::mech::load_chassis;
use mc2_sim_data::mission::{Action, Mission};
use mc2_sim_data::objtype::OBJECT_PAK_PATH;
use mc2_sim_data::pilot::{load_roster, NUM_SPECIALTY_SKILLS};

use crate::component::ComponentCatalog;
use crate::variant::{Chassis, VariantSpec};

/// `artPath` + `buildings.csv`: the table that lists every mech chassis.
pub const BUILDINGS_CSV: &str = "data/art/buildings.csv";
/// `objectPath`, where the chassis CSVs live.
pub const OBJECT_DIR: &str = "data/objects/";
/// Campaign names the game ships.
pub const SHIPPED_CAMPAIGNS: [&str; 2] = ["campaign", "tutorial"];

/// Designer variants in load order: chassis index and the variant as the CSV lists it.
pub type DesignerVariants = Vec<(usize, VariantSpec)>;

/// `IDS_VEHICLE_DESCRIPTION_0`: the encyclopedia text of a vehicle whose FIT names none
/// (`LogisticsVehicle::init`, `code/logisticsvariant.cpp:1031`).
pub const IDS_VEHICLE_DESCRIPTION_0: i64 = 45200;

/// A vehicle the Mechlopedia lists (`LogisticsVehicle`, read by `LogisticsData::addVehicle` from
/// the `VEHICLE` rows of `buildings.csv` and their packets of `object2.pak`).
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleRecord {
    /// `AppearanceName` of `[ObjectType]`: the shape the preview shows.
    pub file_name: String,
    /// `DescIndex` of `[General]`: the string id of the name.
    pub name_id: i64,
    /// `CurTonnage`.
    pub max_weight: f32,
    /// `HouseID`, `-1` when the FIT names none.
    pub house_id: i64,
    /// `EncyclopediaID`, [`IDS_VEHICLE_DESCRIPTION_0`] when the FIT names none.
    pub encyclopedia_id: i64,
    /// `MaxVelocity` of `[VehicleDynamics]`.
    pub speed: f32,
    /// The five locations' `MaxArmorPoints` plus `CurInternalStructure` (`baseArmor`).
    pub base_armor: i64,
    /// Catalogue indices ([`ComponentCatalog::find`]) of the weapons, in `Item:4...` order.
    pub components: Vec<usize>,
    /// The `buildings.csv` scale.
    pub scale: f32,
}

impl VehicleRecord {
    /// `LogisticsChassis::getArmorClass`: the string id of the armor class.
    pub fn armor_class_string_id(&self) -> u32 {
        crate::variant::armor_class_string_id(self.base_armor)
    }

    /// `LogisticsChassis::getDisplaySpeed`: `-1` for speeds not in the table.
    pub fn display_speed(&self) -> i32 {
        crate::variant::display_speed(self.speed)
    }
}

/// A building the Mechlopedia lists (`LogisticsData::Building`, `addBuilding`).
#[derive(Debug, Clone, PartialEq)]
pub struct BuildingRecord {
    /// `AppearanceName` of `[ObjectType]`.
    pub file_name: String,
    /// `BuildingName`: the string id of the name.
    pub name_id: i64,
    /// `EncyclopediaID` of `[ObjectType]`.
    pub encyclopedia_id: i64,
    /// `DmgLevel`, shown as the building's weight.
    pub weight: u32,
    /// The turret's `WeaponType`, `WeaponType1`...`WeaponType3`; `0` for none (and for any
    /// building that is not a turret).
    pub component_ids: [i64; 4],
    /// The `buildings.csv` scale.
    pub scale: f32,
}

/// Failure while building [`GameData`].
#[derive(Debug)]
pub enum DataError {
    /// A required file is not in the data root or any archive.
    Missing(String),
    /// A file could not be read or parsed.
    Format(String),
    /// A `mc2-sim-data` loader failed.
    SimData(mc2_sim_data::SimDataError),
}

/// Result alias for data loading.
pub type Result<T> = std::result::Result<T, DataError>;

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataError::Missing(name) => write!(f, "missing file {name}"),
            DataError::Format(detail) => write!(f, "{detail}"),
            DataError::SimData(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DataError {}

impl From<mc2_sim_data::SimDataError> for DataError {
    fn from(e: mc2_sim_data::SimDataError) -> Self {
        DataError::SimData(e)
    }
}

impl From<mc2_formats::FormatError> for DataError {
    fn from(e: mc2_formats::FormatError) -> Self {
        DataError::Format(e.to_string())
    }
}

/// The key a purchase list is stored under: the path as the game would open it, lower-cased with
/// `/` separators. Campaign entries give a stem (`Purchase01`), mission actions give a path
/// (`data\missions\PurBonus08.fit`); both reduce to the same key form.
pub fn purchase_key(path_or_stem: &str) -> String {
    let mut key = normalize_name(path_or_stem);
    if !key.contains('/') {
        key = format!("{MISSION_DIR}{key}");
    }
    if !key.ends_with(".fit") {
        key.push_str(".fit");
    }
    key
}

/// A pilot as `LogisticsPilot::init` builds it, before any mission has changed it.
#[derive(Debug, Clone, PartialEq)]
pub struct PilotTemplate {
    /// `id`: 1-based line number in `pilots.csv` (lines that fail to load still use up a number).
    pub id: u32,
    /// `fileName`: the warrior profile stem from `pilots.csv`.
    pub file_name: String,
    /// `descIndex`: string id of the display name.
    pub name_id: i64,
    /// `pilotAudio`.
    pub audio: String,
    /// `pilotVideo`.
    pub video: String,
    /// `PictureIndex`.
    pub photo_index: i64,
    /// `FlavorText`: string id.
    pub flavor_text_id: i64,
    /// Portrait file (`artPath` + `Picture`).
    pub icon_file: String,
    /// `Skills.Gunnery`.
    pub gunnery: f32,
    /// `Skills.Piloting`.
    pub piloting: f32,
    /// `SpecialtySkills` (value 1 means the pilot has the skill).
    pub specialty_skills: [bool; NUM_SPECIALTY_SKILLS],
}

/// Everything static the campaign needs.
#[derive(Debug, Clone)]
pub struct GameData {
    /// Component catalogue.
    pub catalog: ComponentCatalog,
    /// Mech chassis in load order (index = `LogisticsChassis` build number).
    pub chassis: Vec<Chassis>,
    /// Designer variants in load order: chassis index and the variant as the CSV lists it.
    pub designer_variants: DesignerVariants,
    /// Pilots in `pilots.csv` order.
    pub pilots: Vec<PilotTemplate>,
    /// Campaign scripts by lower-case name (`campaign`, `tutorial`).
    pub campaigns: BTreeMap<String, Arc<Campaign>>,
    /// Purchase lists by [`purchase_key`].
    pub purchase_lists: BTreeMap<String, PurchaseList>,
    /// Vehicles in `buildings.csv` order (`LogisticsData::vehicles`).
    pub vehicles: Vec<VehicleRecord>,
    /// Buildings in `buildings.csv` order (`LogisticsData::buildings`).
    pub buildings: Vec<BuildingRecord>,
}

impl GameData {
    /// An empty data set, for tests that fill in what they need.
    pub fn empty() -> GameData {
        GameData {
            catalog: ComponentCatalog::default(),
            chassis: Vec::new(),
            designer_variants: Vec::new(),
            pilots: Vec::new(),
            campaigns: BTreeMap::new(),
            purchase_lists: BTreeMap::new(),
            vehicles: Vec::new(),
            buildings: Vec::new(),
        }
    }

    /// Loads the shipped data: components, chassis and variants, pilots, the `campaign` and
    /// `tutorial` scripts, and every purchase list they can reach (the lists the missions name,
    /// plus the bonus lists that objective actions unlock).
    pub fn load(vfs: &mut Vfs) -> Result<GameData> {
        let catalog = ComponentCatalog::load(vfs)?;
        let (chassis, designer_variants) = load_chassis_and_variants(vfs, &catalog)?;
        let pilots = load_pilots(vfs)?;
        let (vehicles, buildings) = load_vehicles_and_buildings(vfs, &catalog)?;

        let mut data = GameData {
            catalog,
            chassis,
            designer_variants,
            pilots,
            campaigns: BTreeMap::new(),
            purchase_lists: BTreeMap::new(),
            vehicles,
            buildings,
        };
        for name in SHIPPED_CAMPAIGNS {
            data.add_campaign(vfs, name)?;
        }
        Ok(data)
    }

    /// Loads a campaign script and the purchase lists reachable from it (the list each mission
    /// names, the fallback `purchase.fit`, and every bonus list an objective action names).
    pub fn add_campaign(&mut self, vfs: &mut Vfs, name: &str) -> Result<()> {
        let campaign = Campaign::load(vfs, name)?;

        let mut wanted: Vec<String> = vec![purchase_key("purchase")];
        let mut bonus_paths: Vec<String> = Vec::new();
        for (_, mission) in campaign.missions() {
            if !mission.purchase_file.is_empty() {
                wanted.push(purchase_key(&mission.purchase_file));
            }
            if mission.settings.is_none() {
                continue;
            }
            let path = format!("{MISSION_DIR}{}.fit", mission.file_name);
            if let Ok(m) = Mission::load_named(vfs, &path) {
                for team in &m.teams {
                    for objective in &team.objectives {
                        for action in objective.actions.iter().chain(&objective.failure_actions) {
                            if let Action::MakeNewTechnologyAvailable { purchase_file_name } =
                                action
                            {
                                bonus_paths.push(purchase_file_name.clone());
                            }
                        }
                    }
                }
            }
        }
        wanted.extend(bonus_paths.iter().map(|p| purchase_key(p)));

        for key in wanted {
            if self.purchase_lists.contains_key(&key) {
                continue;
            }
            if let Some(mut fit) = vfs.read_fit(&key)? {
                let list = PurchaseList::from_fit(&key, &mut fit)?;
                self.purchase_lists.insert(key, list);
            }
        }
        self.campaigns
            .insert(name.to_ascii_lowercase(), Arc::new(campaign));
        Ok(())
    }

    /// Adds a purchase list under [`purchase_key`] of `name`.
    pub fn add_purchase_list(&mut self, name: &str, list: PurchaseList) {
        self.purchase_lists.insert(purchase_key(name), list);
    }

    /// The purchase list for a stem or path, when it was loaded.
    pub fn purchase_list(&self, name: &str) -> Option<&PurchaseList> {
        self.purchase_lists.get(&purchase_key(name))
    }

    /// The index of the chassis whose CSV stem is `stem` (ignoring case).
    pub fn chassis_by_stem(&self, stem: &str) -> Option<usize> {
        self.chassis
            .iter()
            .position(|c| c.stem().eq_ignore_ascii_case(stem))
    }
}

/// `LogisticsData::initVariants` for the rows of `buildings.csv`. The chassis block and
/// the variants come from the `mc2-sim-data` chassis loader.
fn load_chassis_and_variants(
    vfs: &mut Vfs,
    catalog: &ComponentCatalog,
) -> Result<(Vec<Chassis>, DesignerVariants)> {
    let bytes = vfs
        .read(BUILDINGS_CSV)?
        .ok_or_else(|| DataError::Missing(BUILDINGS_CSV.to_string()))?;
    let buildings = CsvFile::from_bytes(bytes)?;

    let mut chassis: Vec<Chassis> = Vec::new();
    let mut specs: DesignerVariants = Vec::new();

    // Column 4: BUILDING, TREE, MECH or VEHICLE. An empty cell ends the table.
    for i in 1u32.. {
        let Ok(Some(kind)) = buildings.read_string(i, 4, 256) else {
            break;
        };
        // Process MECH, VEHICLE, and BUILDING rows
        let kind_upper = kind.to_uppercase();
        if kind_upper != "MECH" && kind_upper != "VEHICLE" && kind_upper != "BUILDING" {
            continue;
        }
        let scale = buildings.read_f32(i, 11).unwrap_or(0.0);
        let Ok(Some(name)) = buildings.read_string(i, 1, 256) else {
            continue;
        };
        let fit_id = buildings.read_i64(i, 5).unwrap_or(0);

        // Only MECH entries have chassis CSVs; VEHICLE and BUILDING are in the pak file
        if kind_upper == "MECH" {
            let stem = name.to_ascii_lowercase();
            let sim = load_chassis(vfs, &stem)?;
            let path = format!("{OBJECT_DIR}{stem}.csv");

            let index = chassis.len();
            chassis.push(Chassis::from_sim(&sim, path, index, fit_id, scale));
            for v in &sim.variants {
                specs.push((index, VariantSpec::from_sim(v, catalog)));
            }
        }
        // VEHICLE and BUILDING entries are loaded from the pak file elsewhere
        // (currently unported, so we skip them)
    }
    Ok((chassis, specs))
}

/// The `VEHICLE` and other non-`MECH` rows of `buildings.csv` that carry a scale, with their
/// packets of `object2.pak` (`LogisticsData::initVariants`, `addVehicle`, `addBuilding`,
/// `code/logisticsdata.cpp:268-293`, `342-360`, `2506-2570`).
fn load_vehicles_and_buildings(
    vfs: &mut Vfs,
    catalog: &ComponentCatalog,
) -> Result<(Vec<VehicleRecord>, Vec<BuildingRecord>)> {
    let bytes = vfs
        .read(BUILDINGS_CSV)?
        .ok_or_else(|| DataError::Missing(BUILDINGS_CSV.to_string()))?;
    let table = CsvFile::from_bytes(bytes)?;
    let pak_bytes = vfs
        .read(OBJECT_PAK_PATH)?
        .ok_or_else(|| DataError::Missing(OBJECT_PAK_PATH.to_string()))?;
    let pak = PakFile::from_bytes(pak_bytes)?;

    let mut vehicles = Vec::new();
    let mut buildings = Vec::new();
    for i in 1u32.. {
        let Ok(Some(kind)) = table.read_string(i, 4, 256) else {
            break;
        };
        let scale = table.read_f32(i, 11).unwrap_or(0.0);
        if kind.eq_ignore_ascii_case("MECH") || scale == 0.0 {
            continue;
        }
        let Some(fit_id) = table.read_i64(i, 5) else {
            continue;
        };
        let Some(mut fit) = usize::try_from(fit_id)
            .ok()
            .filter(|&n| n < pak.packet_count())
            .and_then(|n| pak.read_packet(n).ok())
            .filter(|packet| !packet.is_empty())
            .and_then(|packet| FitIni::from_bytes(packet).ok())
        else {
            continue;
        };
        if kind.eq_ignore_ascii_case("VEHICLE") {
            if let Some(v) = read_vehicle(&mut fit, catalog, scale) {
                vehicles.push(v);
            }
        } else if let Some(b) = read_building(&mut fit, scale) {
            buildings.push(b);
        }
    }
    Ok((vehicles, buildings))
}

/// `LogisticsVehicle::init` (`code/logisticsvariant.cpp:1006-1077`).
fn read_vehicle(fit: &mut FitIni, catalog: &ComponentCatalog, scale: f32) -> Option<VehicleRecord> {
    fit.seek_block("ObjectType").ok()?;
    let file_name = fit.read_string("AppearanceName", 255).unwrap_or_default();
    fit.seek_block("General").ok()?;
    let name_id = fit.read_i64("DescIndex").unwrap_or(0);
    let max_weight = fit.read_f32("CurTonnage").unwrap_or(0.0);
    let house_id = fit.read_i64("HouseID").unwrap_or(-1);
    let encyclopedia_id = fit
        .read_i64("EncyclopediaID")
        .unwrap_or(IDS_VEHICLE_DESCRIPTION_0);
    fit.seek_block("VehicleDynamics").ok()?;
    let speed = fit.read_f32("MaxVelocity").unwrap_or(0.0);

    // A location the FIT lacks leaves the previous reading in `pts`, which is added again.
    let mut base_armor = 0i64;
    let mut pts = 0i64;
    for part in ["Front", "Left", "Right", "Rear", "Turret"] {
        if fit.seek_block(part).is_ok() {
            pts = i64::from(fit.read_u8("MaxArmorPoints").unwrap_or(pts as u8));
            base_armor += pts;
            pts = i64::from(fit.read_u8("CurInternalStructure").unwrap_or(pts as u8));
            base_armor += pts;
        } else {
            base_armor += 2 * pts;
        }
    }

    let mut components = Vec::new();
    fit.seek_block("InventoryInfo").ok()?;
    let weapons = fit.read_u8("NumWeapons").unwrap_or(0);
    for item in 4..4 + i32::from(weapons) {
        if fit.seek_block(&format!("Item:{item}")).is_err() {
            continue;
        }
        let Ok(master_id) = fit.read_u8("MasterID") else {
            continue;
        };
        if let Some(index) = catalog.find(i32::from(master_id)) {
            components.push(index);
        }
    }
    Some(VehicleRecord {
        file_name,
        name_id,
        max_weight,
        house_id,
        encyclopedia_id,
        speed,
        base_armor,
        components,
        scale,
    })
}

/// `LogisticsData::addBuilding`'s block reading (`code/logisticsdata.cpp:2517-2561`).
fn read_building(fit: &mut FitIni, scale: f32) -> Option<BuildingRecord> {
    fit.seek_block("ObjectType").ok()?;
    let file_name = fit.read_string("AppearanceName", 63).unwrap_or_default();
    let encyclopedia_id = fit.read_i64("EncyclopediaID").unwrap_or(0);
    let mut is_turret = false;
    if fit.seek_block("BuildingData").is_err() && fit.seek_block("GateData").is_err() {
        if fit.seek_block("TurretData").is_ok() {
            is_turret = true;
        } else if fit.seek_block("General").is_err() {
            // the original asserts here; the building is left out
            return None;
        }
    }
    let name_id = fit.read_i64("BuildingName").unwrap_or(0);
    let weight = fit.read_u32("DmgLevel").unwrap_or(0);
    let mut component_ids = [0i64; 4];
    if is_turret {
        for (i, id) in component_ids.iter_mut().enumerate() {
            let name = if i == 0 {
                "WeaponType".to_string()
            } else {
                format!("WeaponType{i}")
            };
            *id = fit.read_i64(&name).unwrap_or(0);
        }
    }
    Some(BuildingRecord {
        file_name,
        name_id,
        encyclopedia_id,
        weight,
        component_ids,
        scale,
    })
}

/// `LogisticsData::initPilots` over `pilots.csv`, through the `mc2-sim-data` roster loader.
fn load_pilots(vfs: &mut Vfs) -> Result<Vec<PilotTemplate>> {
    let roster = load_roster(vfs)?;
    Ok(roster
        .pilots
        .iter()
        .map(|p| PilotTemplate {
            id: p.id,
            file_name: p.file_name.clone(),
            name_id: p.desc_index,
            audio: p.audio.clone().unwrap_or_default(),
            video: p.video.clone().unwrap_or_default(),
            photo_index: p.photo_index,
            flavor_text_id: p.flavor_text,
            icon_file: p.icon_file.clone(),
            gunnery: f32::from(p.gunnery),
            piloting: f32::from(p.piloting),
            specialty_skills: p.specialty,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purchase_keys_accept_stems_and_windows_paths() {
        assert_eq!(purchase_key("Purchase01"), "data/missions/purchase01.fit");
        assert_eq!(
            purchase_key("data\\missions\\PurBonus08.fit"),
            "data/missions/purbonus08.fit"
        );
        assert_eq!(
            purchase_key("purchase_All"),
            "data/missions/purchase_all.fit"
        );
    }
}
