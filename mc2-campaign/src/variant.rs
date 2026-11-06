// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mech chassis and loadout variants, with the legality rules of the mech lab.
//!
//! Port of `code/logisticsvariant.{h,cpp}` (`LogisticsChassis`, `LogisticsVariant`) and
//! `code/logisticsmech.h` accessors that forward to the variant. Weight, heat, armor and cost
//! arithmetic keeps the C++ integer/float mixing (`retVal += float` truncates each step, the
//! weight is a `float` converted with truncation), and `can_add_component` keeps the order of the
//! checks, because the error a caller sees depends on it.

use mc2_sim_data::component::ComponentForm;

use crate::component::ComponentCatalog;
use crate::error::{LogisticsError, LogisticsResult};

/// `weightClasses[]` in `logisticsvariant.cpp`: class boundaries in tons.
pub const WEIGHT_CLASSES: [i32; 5] = [0, 35, 55, 75, 100];

/// First string id of the weight class names (`IDS_VERY_LIGHT`, `resource.h`).
pub const IDS_VERY_LIGHT: u32 = 27006;
/// `IDS_VEHICLE_CLASS`.
pub const IDS_VEHICLE_CLASS: u32 = 27005;
/// First of the five armor class strings (`IDS_ARMOR0`).
pub const IDS_ARMOR0: u32 = 45138;
/// `IDS_HOTKEY1`, the first of the three range band strings.
pub const IDS_HOTKEY1: u32 = 43200;

/// A variant holds at most this many components (`components[54]` in the header).
pub const MAX_VARIANT_COMPONENTS: usize = 54;

/// Where a component sits on the mech (`LOGISTICS_COMPONENT_LOCATION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Location {
    /// `HEAD`: ECM and sensors.
    Head = 0,
    /// `CHEST`: everything placed on the lab grid.
    Chest = 1,
    /// `LEGS`: jump jets.
    Legs = 2,
}

/// The chassis facts the logistics layer reads (`LogisticsChassis`).
#[derive(Debug, Clone, PartialEq)]
pub struct Chassis {
    /// Index in load order (`chassisID++` in `initVariants`).
    pub index: usize,
    /// `LogisticsChassis::ID`: the index with the house id in bits 8-15.
    pub id: u32,
    /// Lower-case path of the chassis CSV, as the C++ keeps it (`data/objects/<name>.csv`).
    pub file_name: String,
    /// `maxWeight`, tons.
    pub max_weight: f32,
    /// `chassisNameID`: string id of the chassis name.
    pub chassis_name_id: i64,
    /// `baseCost`.
    pub base_cost: i64,
    /// `maxHeat`: heat the chassis sheds without heat sinks.
    pub max_heat: i64,
    /// `baseArmor`.
    pub base_armor: i64,
    /// `maxArmor`.
    pub max_armor: i64,
    /// `speed`.
    pub speed: f32,
    /// `scale` (mechlopedia), from `buildings.csv`.
    pub scale: f32,
    /// `iconPictureIndex`.
    pub icon_picture_index: i64,
    /// `iconFileNames[3]` (small, medium, large).
    pub icon_file_names: [String; 3],
    /// `componentAreaWidth`: lab grid width.
    pub component_area_width: i64,
    /// `componentAreaHeight`: lab grid height.
    pub component_area_height: i64,
    /// `canHaveAdvSensor`.
    pub can_have_adv_sensor: bool,
    /// `canHaveOptics`.
    pub can_have_optics: bool,
    /// `canHaveGlobalSensor`.
    pub can_have_global_sensor: bool,
    /// `canHaveECM`.
    pub can_have_ecm: bool,
    /// `canHaveActiveProbe`.
    pub can_have_active_probe: bool,
    /// `canHaveJumpJets`.
    pub can_have_jump_jets: bool,
    /// `canHaveExtendedSensor`.
    pub can_have_extended_sensor: bool,
    /// `encyclopediaID`.
    pub encyclopedia_id: i64,
    /// `helpID`.
    pub help_id: i64,
    /// `houseID`.
    pub house_id: i64,
    /// `fitID`: object type number (`buildings.csv` column 5).
    pub fit_id: i64,
}

/// Weight class of a chassis, in `IDS_VERY_LIGHT` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeightClass {
    /// `IDS_VERY_LIGHT`.
    VeryLight = 0,
    /// `IDS_LIGHT_MECH`.
    Light = 1,
    /// `IDS_MEDIUM_MECH`.
    Medium = 2,
    /// `IDS_HEAVY_MECH`.
    Heavy = 3,
    /// `IDS_VERY_HEAVY`.
    VeryHeavy = 4,
    /// `IDS_EXTREME`.
    Extreme = 5,
}

impl WeightClass {
    /// The string-table id of the class name.
    pub fn string_id(self) -> u32 {
        IDS_VERY_LIGHT + self as u32
    }
}

/// `LogisticsChassis::getArmorClass` over a `baseArmor`: the string id of the armor class.
pub fn armor_class_string_id(base_armor: i64) -> u32 {
    if base_armor < 96 {
        IDS_ARMOR0
    } else if base_armor < 113 {
        IDS_ARMOR0 + 1
    } else if base_armor < 195 {
        IDS_ARMOR0 + 2
    } else if base_armor < 304 {
        IDS_ARMOR0 + 3
    } else {
        IDS_ARMOR0 + 4
    }
}

/// `LogisticsChassis::getDisplaySpeed` over a `speed`: `-1` for speeds not in the table.
pub fn display_speed(speed: f32) -> i32 {
    match speed as i64 {
        5 => 17,
        10 => 34,
        15 => 54,
        19 => 65,
        23 => 86,
        27 => 97,
        31 => 119,
        35 => 130,
        _ => -1,
    }
}

impl Chassis {
    /// File stem of the chassis CSV (`_splitpath` base name in `updateAvailability`).
    pub fn stem(&self) -> &str {
        let name = self.file_name.rsplit(['/', '\\']).next().unwrap_or("");
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }

    /// `LogisticsChassis::init` weight class: the highest boundary the weight exceeds.
    pub fn weight_class(&self) -> WeightClass {
        let mut class = 0usize;
        for i in (0..5).rev() {
            if self.max_weight > WEIGHT_CLASSES[i] as f32 {
                class = i + 1;
                break;
            }
        }
        match class {
            0 => WeightClass::VeryLight,
            1 => WeightClass::Light,
            2 => WeightClass::Medium,
            3 => WeightClass::Heavy,
            4 => WeightClass::VeryHeavy,
            _ => WeightClass::Extreme,
        }
    }

    /// `getArmorClass`: string id of the armor class.
    pub fn armor_class_string_id(&self) -> u32 {
        armor_class_string_id(self.base_armor)
    }

    /// `getDisplaySpeed`: the speed shown in the lab, `-1` for speeds not in the table.
    pub fn display_speed(&self) -> i32 {
        display_speed(self.speed)
    }

    /// `getSensorID`: component id of the sensor this chassis can carry, `-1` for none.
    pub fn sensor_id(&self) -> i32 {
        if self.can_have_adv_sensor {
            15
        } else if self.can_have_extended_sensor {
            14
        } else if self.can_have_global_sensor {
            17
        } else {
            -1
        }
    }

    /// `getECM`: component id of the ECM, `-1` when the chassis cannot carry one.
    pub fn ecm_id(&self) -> i32 {
        if self.can_have_ecm {
            38
        } else {
            -1
        }
    }

    /// `LogisticsChassis::init` from the chassis CSV as `mc2-sim-data` parsed it. Cells the C++
    /// cannot read count as zero or false (`CSVFile::readLong` and friends return `NO_ERR` with
    /// a zero value). The speed is the CSV's `MaxRunSpeed`, which the sim loader reads as a
    /// byte; the C++ reads a float, and the shipped values are whole numbers.
    pub fn from_sim(
        sim: &mc2_sim_data::mech::Chassis,
        file_name: String,
        chassis_index: usize,
        fit_id: i64,
        scale: f32,
    ) -> Chassis {
        let house_id = sim.house_id.unwrap_or(0);
        let id = (chassis_index as u32) | (((house_id << 8) & 0x0000_ff00) as u32);

        // `readString` into a shared buffer: a missing cell leaves the previous text.
        let mut icon_file_names: [String; 3] = Default::default();
        let mut buffer = String::new();
        for (slot, icon) in icon_file_names.iter_mut().zip(&sim.icon_files) {
            if let Some(s) = icon {
                buffer = s.clone();
            }
            *slot = buffer.clone();
        }

        Chassis {
            index: chassis_index,
            id,
            file_name,
            max_weight: sim.tonnage,
            chassis_name_id: sim.descriptor_id.map_or(0, i64::from),
            base_cost: sim.base_cost.unwrap_or(0),
            max_heat: sim.heat_index.unwrap_or(0),
            base_armor: sim.total_armor.unwrap_or(0),
            max_armor: sim.max_armor.unwrap_or(0),
            speed: f32::from(sim.max_run_speed),
            scale,
            icon_picture_index: sim.icon_picture_index.map_or(0, i64::from),
            icon_file_names,
            component_area_width: sim.logistics_width.unwrap_or(0),
            component_area_height: sim.logistics_height.unwrap_or(0),
            can_have_adv_sensor: sim.allow.advanced_sensor,
            can_have_optics: sim.allow.optics,
            can_have_global_sensor: sim.allow.global_sensor,
            can_have_ecm: sim.allow.ecm,
            can_have_active_probe: sim.allow.active_probe,
            can_have_jump_jets: sim.allow.jump_jets,
            can_have_extended_sensor: sim.allow.extended_sensor,
            encyclopedia_id: sim.abbr_index.unwrap_or(0),
            help_id: sim.help_id.unwrap_or(0),
            house_id,
            fit_id,
        }
    }
}

/// Everything a variant method needs to look at besides itself.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    /// The component catalogue.
    pub catalog: &'a ComponentCatalog,
    /// The variant's chassis.
    pub chassis: &'a Chassis,
}

/// A component placed in a variant (`LogisticsChassis::ComponentInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlacedComponent {
    /// Catalogue index of the component.
    pub component: usize,
    /// Lab grid column; `-2` for jump jets and `-3` for ECM and sensors.
    pub x: i64,
    /// Lab grid row; `-2` for jump jets and `-3` for ECM and sensors.
    pub y: i64,
    /// Where the component sits.
    pub location: Location,
}

/// A mech loadout (`LogisticsVariant`).
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    /// `ID`: bits 0-7 chassis, bits 8-15 house, bits 16-23 number among the chassis' variants.
    pub id: u32,
    /// `fileID`: the variant number inside the chassis CSV (`0` for player variants).
    pub file_id: i32,
    /// Index of the chassis.
    pub chassis: usize,
    /// `variantName`.
    pub name: String,
    /// The components, in the order they were added.
    pub components: Vec<PlacedComponent>,
    /// `availableToUser`: set by the purchase lists.
    pub available_to_user: bool,
    /// `bDesignerMech`: shipped with the game; player variants are not.
    pub designer: bool,
    /// `bHidden`: hidden designer variants are never offered.
    pub hidden: bool,
}

/// `components[i].component` for use in loops.
fn comp<'a>(ctx: &Ctx<'a>, p: &PlacedComponent) -> &'a crate::component::Component {
    ctx.catalog
        .get(p.component)
        .expect("placed component index is valid for the catalogue")
}

impl Variant {
    /// `LogisticsVariant(const LogisticsChassis*, bool isDesignerMech)`: an empty variant that is
    /// available to the user. `id` is `(refCount << 16) | chassis.id`, assigned by the caller.
    pub fn new_empty(chassis: &Chassis, id: u32, designer: bool) -> Variant {
        Variant {
            id,
            file_id: 0,
            chassis: chassis.index,
            name: String::new(),
            components: Vec::new(),
            available_to_user: true,
            designer,
            hidden: false,
        }
    }

    /// `getVariantID`: the id without the top byte.
    pub fn variant_id(&self) -> u32 {
        self.id & 0x00ff_ffff
    }

    /// `isAvailable`.
    pub fn is_available(&self) -> bool {
        self.available_to_user && !self.hidden
    }

    /// `canDelete`: only player variants can be deleted.
    pub fn can_delete(&self) -> bool {
        !self.designer
    }

    /// `setAvailable`, including the hard-coded exception for "Renard's Atlas".
    pub fn set_available(&mut self, available: bool) {
        if self.name.eq_ignore_ascii_case("Renard's Atlas") {
            self.available_to_user = false;
        } else {
            self.available_to_user = available;
        }
    }

    /// `allComponentsAvailable` (with the same Renard exception). `available` says whether the
    /// component at a catalogue index is currently available.
    pub fn all_components_available(&self, available: impl Fn(usize) -> bool) -> bool {
        if self.name.eq_ignore_ascii_case("Renard's Atlas") {
            return false;
        }
        self.components.iter().all(|p| available(p.component))
    }

    /// `operator==`: same chassis name id, availability and component sequence.
    pub fn equivalent(&self, other: &Variant, ctx: &Ctx<'_>, other_ctx: &Ctx<'_>) -> bool {
        if ctx.chassis.chassis_name_id != other_ctx.chassis.chassis_name_id {
            return false;
        }
        if self.available_to_user != other.available_to_user {
            return false;
        }
        if self.components.len() != other.components.len() {
            return false;
        }
        self.components
            .iter()
            .zip(&other.components)
            .all(|(a, b)| comp(ctx, a).id == comp(other_ctx, b).id)
    }

    /// `getCost`: chassis base cost plus the components.
    pub fn cost(&self, ctx: &Ctx<'_>) -> i32 {
        let mut cost = ctx.chassis.base_cost as i32;
        for p in &self.components {
            cost += comp(ctx, p).cost;
        }
        cost
    }

    /// `getWeight`: 20% of the chassis weight plus 7 tons (armor and unlisted parts), plus the
    /// components, truncated.
    pub fn weight(&self, ctx: &Ctx<'_>) -> i32 {
        let mut base: f32 = (f64::from(ctx.chassis.max_weight) * 0.20 + 7.0) as f32;
        for p in &self.components {
            base += comp(ctx, p).weight;
        }
        base as i32
    }

    /// `getJumpRange`: 5 with jump jets, else 0 ("entirely made up" in the source).
    pub fn jump_range(&self, ctx: &Ctx<'_>) -> i32 {
        if self.has_form(ctx, ComponentForm::JumpJet) {
            5
        } else {
            0
        }
    }

    /// `getMaxJumpRange`.
    pub fn max_jump_range(&self) -> i32 {
        5
    }

    /// `getHeat`: total heat of the components, truncated after each addition.
    pub fn heat(&self, ctx: &Ctx<'_>) -> i32 {
        let mut heat = 0i32;
        for p in &self.components {
            heat = (heat as f32 + comp(ctx, p).heat) as i32;
        }
        heat
    }

    /// `getMaxHeat`: chassis heat capacity plus the dissipation of the heat sinks.
    pub fn max_heat(&self, ctx: &Ctx<'_>) -> i32 {
        let mut heat = 0i32;
        for p in &self.components {
            let c = comp(ctx, p);
            if c.form == ComponentForm::HeatSink {
                heat = (heat as f32 + c.damage) as i32;
            }
        }
        heat + ctx.chassis.max_heat as i32
    }

    /// `getArmor`: base armor plus 32 per armor plating.
    pub fn armor(&self, ctx: &Ctx<'_>) -> i32 {
        let mut armor = ctx.chassis.base_armor as i32;
        for p in &self.components {
            if comp(ctx, p).form == ComponentForm::Bulk {
                armor += 32;
            }
        }
        armor
    }

    /// `getSpeed`: the chassis speed, truncated.
    pub fn speed(&self, ctx: &Ctx<'_>) -> i32 {
        ctx.chassis.speed as i32
    }

    /// `getDisplaySpeed`.
    pub fn display_speed(&self, ctx: &Ctx<'_>) -> i32 {
        ctx.chassis.display_speed()
    }

    fn has_form(&self, ctx: &Ctx<'_>, form: ComponentForm) -> bool {
        self.components.iter().any(|p| comp(ctx, p).form == form)
    }

    /// `getComponentAtLocation`: the component whose grid rectangle covers `(x, y)`.
    /// `(-1, -1)` never matches.
    pub fn component_at(&self, ctx: &Ctx<'_>, x: i64, y: i64) -> Option<usize> {
        if x == -1 && y == -1 {
            return None;
        }
        for (i, p) in self.components.iter().enumerate() {
            let c = comp(ctx, p);
            for j in 0..i64::from(c.height) {
                for k in 0..i64::from(c.width) {
                    if p.x + k == x && p.y + j == y {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    /// `getCompAtLocation`: the component at a grid cell with its origin, or `None`.
    pub fn comp_at_location(
        &self,
        ctx: &Ctx<'_>,
        x: i64,
        y: i64,
    ) -> Option<(PlacedComponent, i64, i64)> {
        self.component_at(ctx, x, y).map(|i| {
            (
                self.components[i],
                self.components[i].x,
                self.components[i].y,
            )
        })
    }

    /// `getComponentLocation`: where `component` sits. When `(x, y)` names a component the
    /// result is that component's origin, whatever `component` is; otherwise the first component
    /// with the same catalogue index is used. `Err(ComponentNotFound)` when neither exists.
    pub fn component_location(
        &self,
        ctx: &Ctx<'_>,
        component: usize,
        x: i64,
        y: i64,
    ) -> LogisticsResult<(i64, i64)> {
        if let Some(i) = self.component_at(ctx, x, y) {
            return Ok((self.components[i].x, self.components[i].y));
        }
        self.components
            .iter()
            .find(|p| p.component == component)
            .map(|p| (p.x, p.y))
            .ok_or(LogisticsError::ComponentNotFound)
    }

    /// `canAddComponent`. Pass `(-1, -1)` to have the grid searched; the cell found is written
    /// back to `x` and `y`. The order of the checks is the C++ order.
    pub fn can_add_component(
        &self,
        ctx: &Ctx<'_>,
        component: usize,
        x: &mut i64,
        y: &mut i64,
    ) -> LogisticsResult<()> {
        let c = comp(
            ctx,
            &PlacedComponent {
                component,
                x: 0,
                y: 0,
                location: Location::Chest,
            },
        );
        // "weight no longer matters": COMPONENT_TOO_HEAVY is never returned.

        if self.heat(ctx) as f32 + c.heat > self.max_heat(ctx) as f32 {
            return Err(LogisticsError::ComponentTooHot);
        }

        if c.form == ComponentForm::JumpJet {
            if !ctx.chassis.can_have_jump_jets {
                return Err(LogisticsError::JumpJetsNotAllowed);
            } else if self.has_form(ctx, ComponentForm::JumpJet) {
                return Err(LogisticsError::OnlyOneJumpJetAllowed);
            } else if (*x != -1 && *x != -2) || (*y != -1 && *y != -2) {
                return Err(LogisticsError::ComponentSlotFull);
            }
            return Ok(());
        } else if *x == -2 && *y == -2 {
            // trying to put something illegal in the jump jet slot
            return Err(LogisticsError::ComponentSlotFull);
        }

        let area_w = ctx.chassis.component_area_width;
        let area_h = ctx.chassis.component_area_height;

        if *x != -1 && *y != -1 {
            for i in 0..i64::from(c.width) {
                for j in 0..i64::from(c.height) {
                    if self.component_at(ctx, *x + i, *y + j).is_some()
                        || *x + i >= area_w
                        || j + *y >= area_h
                    {
                        return Err(LogisticsError::ComponentSlotFull);
                    }
                }
            }
        } else {
            let mut j = 0;
            while j < area_h && *x == -1 {
                let mut i = 0;
                while i < area_w && *x == -1 {
                    if self.component_at(ctx, i, j).is_none() {
                        let mut fits = true;
                        for l in 0..i64::from(c.height) {
                            for k in 0..i64::from(c.width) {
                                if self.component_at(ctx, i + k, j + l).is_some()
                                    || i + k >= area_w
                                    || j + l >= area_h
                                {
                                    fits = false;
                                    break;
                                }
                            }
                        }
                        if fits {
                            *x = i;
                            *y = j;
                        }
                    }
                    i += 1;
                }
                j += 1;
            }
            if *x == -1 || *y == -1 {
                return Err(LogisticsError::AddComponentFailed);
            }
        }

        if c.form == ComponentForm::Bulk && self.armor(ctx) as i64 + 32 > ctx.chassis.max_armor {
            return Err(LogisticsError::NoMoreArmor);
        }

        if c.form == ComponentForm::Sensor {
            if !ctx.chassis.can_have_adv_sensor {
                return Err(LogisticsError::SensorsNotAllowed);
            } else if self.has_form(ctx, ComponentForm::Sensor) {
                return Err(LogisticsError::OnlyOneSensorAllowed);
            }
        }

        if c.form == ComponentForm::Ecm {
            if !ctx.chassis.can_have_ecm {
                return Err(LogisticsError::EcmNotAllowed);
            } else if self.has_form(ctx, ComponentForm::Ecm) {
                return Err(LogisticsError::OnlyOneEcmAllowed);
            }
        }

        Ok(())
    }

    /// `addComponent(int idFromFitFile, long& x, long& y)`: places a component without the
    /// legality checks. Jump jets go to the legs (`-2,-2`), ECM and sensors to the head
    /// (`-3,-3`); everything else needs a free spot on the grid (searched when `(-1, -1)`).
    /// Returns `false` when it cannot be placed.
    pub fn add_component_unchecked(
        &mut self,
        ctx: &Ctx<'_>,
        component: usize,
        x: &mut i64,
        y: &mut i64,
    ) -> bool {
        let Some(c) = ctx.catalog.get(component) else {
            return false;
        };
        if self.components.len() >= MAX_VARIANT_COMPONENTS {
            // The C++ array holds 54 entries and does not check; refuse instead of overflowing.
            return false;
        }

        let location;
        if c.form == ComponentForm::JumpJet {
            location = Location::Legs;
            *x = -2;
            *y = -2;
        } else if c.form == ComponentForm::Ecm || c.form == ComponentForm::Sensor {
            location = Location::Head;
            *x = -3;
            *y = -3;
        } else {
            let width = i64::from(c.width);
            let height = i64::from(c.height);
            let area_w = ctx.chassis.component_area_width;
            let area_h = ctx.chassis.component_area_height;

            if *x == -1 && *y == -1 {
                let mut j = 0;
                while j < area_h && *x == -1 {
                    let mut i = 0;
                    while i < area_w && *x == -1 {
                        let mut fits = true;
                        for l in 0..height {
                            for k in 0..width {
                                if self.component_at(ctx, i + k, j + l).is_some()
                                    || i + k >= area_w
                                    || j + l >= area_h
                                {
                                    fits = false;
                                    break;
                                }
                            }
                        }
                        if fits {
                            *x = i;
                            *y = j;
                        }
                        i += 1;
                    }
                    j += 1;
                }
            }

            if *x == -1 && *y == -1 {
                return false;
            }

            if *x > -1 {
                for i in 0..width {
                    for j in 0..height {
                        if self.component_at(ctx, *x + i, *y + j).is_some() {
                            return false;
                        }
                    }
                }
            }
            location = Location::Chest;
        }

        self.components.push(PlacedComponent {
            component,
            x: *x,
            y: *y,
            location,
        });
        true
    }

    /// `addComponent(LogisticsComponent*, long& x, long& y)`: legality check, then placement.
    pub fn add_component(
        &mut self,
        ctx: &Ctx<'_>,
        component: usize,
        x: &mut i64,
        y: &mut i64,
    ) -> LogisticsResult<()> {
        self.can_add_component(ctx, component, x, y)?;
        if self.add_component_unchecked(ctx, component, x, y) {
            Ok(())
        } else {
            Err(LogisticsError::AddComponentFailed)
        }
    }

    /// `removeComponent(x, y)`: removes the component covering the cell. A heat sink can only go
    /// while the remaining capacity still covers the heat.
    pub fn remove_component(&mut self, ctx: &Ctx<'_>, x: i64, y: i64) -> LogisticsResult<()> {
        let Some(i) = self.component_at(ctx, x, y) else {
            return Err(LogisticsError::Failed);
        };
        if !self.can_remove_component(ctx, x, y) {
            return Err(LogisticsError::InsufficientHeat);
        }
        self.components.remove(i);
        Ok(())
    }

    /// `MechLabScreen::canRemoveComponent`: a heat sink can only go while the remaining capacity
    /// still covers the heat. Anything else, or an empty cell, can go.
    pub fn can_remove_component(&self, ctx: &Ctx<'_>, x: i64, y: i64) -> bool {
        let Some(i) = self.component_at(ctx, x, y) else {
            return true;
        };
        let c = comp(ctx, &self.components[i]);
        !(c.form == ComponentForm::HeatSink
            && (self.max_heat(ctx) as f32 - c.damage) < self.heat(ctx) as f32)
    }

    /// `getOptimalRangeString`: the range band (0 short, 1 medium, 2 long) that carries the most
    /// damage per second, as an offset from `IDS_HOTKEY1`. Without weapons the first band.
    pub fn optimal_range_band(&self, ctx: &Ctx<'_>) -> usize {
        let mut range_damage = [0f32; 3];
        let mut max_damage = -1f32;
        for p in &self.components {
            let c = comp(ctx, p);
            if c.is_weapon() {
                let mut rating = c.damage;
                if c.recycle_time != 0.0 {
                    rating = c.damage / c.recycle_time;
                }
                let band = c.range_type as usize;
                if band < 3 {
                    range_damage[band] += rating;
                    if range_damage[band] > max_damage {
                        max_damage = range_damage[band];
                    }
                }
            }
        }
        for (i, d) in range_damage.iter().enumerate() {
            if *d == max_damage {
                return i;
            }
        }
        0
    }

    /// `LogisticsVariant::init` for one variant of a mech CSV, from the components already
    /// looked up in the file ([`VariantSpec`]). Components are added from the largest footprint
    /// to the smallest (height 4 down to 1, width 3 down to 1), in file order within a size, so
    /// the big ones claim their cells first.
    pub fn from_spec(ctx: &Ctx<'_>, spec: &VariantSpec, id: u32) -> Variant {
        let mut v = Variant {
            id,
            file_id: spec.file_id,
            chassis: ctx.chassis.index,
            name: spec.name.clone(),
            components: Vec::new(),
            available_to_user: false,
            designer: true,
            hidden: spec.hidden,
        };
        for height in (1..=4).rev() {
            for width in (1..=3).rev() {
                for (index, x, y) in &spec.components {
                    let c = ctx
                        .catalog
                        .get(*index)
                        .expect("spec component is in the catalogue");
                    if c.width == width && c.height == height {
                        let (mut px, mut py) = (*x, *y);
                        let _ = v.add_component_unchecked(ctx, *index, &mut px, &mut py);
                    }
                }
            }
        }
        v
    }
}

/// One designer variant as the chassis CSV lists it, before it is built.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantSpec {
    /// Variant number inside the CSV (`fileID`).
    pub file_id: i32,
    /// Variant name (row `23 + 97 * n`, column 2).
    pub name: String,
    /// `bHidden` (row `21 + 97 * n`, column 4).
    pub hidden: bool,
    /// Components found in rows `26 + 97n .. 96 + 97n`: catalogue index, `x`, `y`.
    pub components: Vec<(usize, i64, i64)>,
}

impl VariantSpec {
    /// `LogisticsVariant::init` (`logisticsvariant.cpp:255-310`) up to the point where components
    /// are added, from a variant as `mc2-sim-data` parsed it. The logistics reader looks at 71
    /// inventory rows (26..96); the simulation loads 72. A row names a component by `CompBas#`
    /// (255 is empty; a cell the C++ would read as 0 finds no component), with the lab position
    /// in `MechBay X` and `Y` (cells the C++ cannot read count as 0).
    pub fn from_sim(
        variant: &mc2_sim_data::mech::ChassisVariant,
        catalog: &ComponentCatalog,
    ) -> VariantSpec {
        let mut components = Vec::new();
        for row in variant.items.iter().take(71) {
            let component_id = row.comp_bas.map_or(0, i64::from);
            if component_id != 0xff {
                if let Some(index) = catalog.find(component_id as i32) {
                    components.push((
                        index,
                        row.mechbay_x.unwrap_or(0),
                        row.mechbay_y.unwrap_or(0),
                    ));
                }
            }
        }
        VariantSpec {
            file_id: variant.index as i32,
            name: variant.name.clone(),
            hidden: variant.hidden,
            components,
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::component::test_component;

    /// Catalogue indices of the synthetic components.
    pub const LASER: usize = 0; // 1x1, 2 heat
    pub const PPC: usize = 1; // 2x2, 4 heat
    pub const HEATSINK: usize = 2; // 1x1, dissipates 1
    pub const JUMPJET: usize = 3;
    pub const BULK: usize = 4;
    pub const ECM: usize = 5;
    pub const SENSOR: usize = 6;
    pub const BIG_GUN: usize = 7; // 2x3

    pub fn catalog() -> ComponentCatalog {
        use ComponentForm as F;
        ComponentCatalog::from_components(vec![
            test_component(10, F::WeaponEnergy, (1.0, 2.0, 3.0), 1000, (1, 1)),
            test_component(11, F::WeaponEnergy, (7.0, 4.0, 10.0), 5000, (2, 2)),
            test_component(12, F::HeatSink, (1.0, 0.0, 1.0), 600, (1, 1)),
            test_component(13, F::JumpJet, (2.0, 0.0, 0.0), 800, (1, 1)),
            test_component(14, F::Bulk, (1.0, 0.0, 0.0), 400, (1, 1)),
            test_component(15, F::Ecm, (1.0, 0.0, 0.0), 900, (1, 1)),
            test_component(16, F::Sensor, (1.0, 0.0, 0.0), 700, (1, 1)),
            test_component(17, F::WeaponBallistic, (10.0, 1.0, 20.0), 9000, (2, 3)),
        ])
    }

    pub fn chassis() -> Chassis {
        Chassis {
            index: 0,
            id: 0x0200,
            file_name: "data/objects/testmech.csv".into(),
            max_weight: 50.0,
            chassis_name_id: 777,
            base_cost: 100_000,
            max_heat: 20,
            base_armor: 100,
            max_armor: 164,
            speed: 15.0,
            scale: 1.0,
            icon_picture_index: 0,
            icon_file_names: Default::default(),
            component_area_width: 4,
            component_area_height: 3,
            can_have_adv_sensor: true,
            can_have_optics: false,
            can_have_global_sensor: false,
            can_have_ecm: true,
            can_have_active_probe: false,
            can_have_jump_jets: true,
            can_have_extended_sensor: false,
            encyclopedia_id: 1,
            help_id: 2,
            house_id: 2,
            fit_id: 5,
        }
    }

    pub fn empty_variant(c: &Chassis) -> Variant {
        let mut v = Variant::new_empty(c, 0x0001_0200, false);
        v.name = "Mine".into();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn auto(v: &mut Variant, ctx: &Ctx<'_>, c: usize) -> LogisticsResult<(i64, i64)> {
        let (mut x, mut y) = (-1, -1);
        v.add_component(ctx, c, &mut x, &mut y)?;
        Ok((x, y))
    }

    #[test]
    fn weight_class_boundaries_are_strict() {
        let mut c = chassis();
        for (w, class) in [
            (0.0, WeightClass::VeryLight),
            (20.0, WeightClass::Light),
            (35.0, WeightClass::Light),
            (36.0, WeightClass::Medium),
            (55.0, WeightClass::Medium),
            (56.0, WeightClass::Heavy),
            (75.0, WeightClass::Heavy),
            (76.0, WeightClass::VeryHeavy),
            (100.0, WeightClass::VeryHeavy),
            (101.0, WeightClass::Extreme),
        ] {
            c.max_weight = w;
            assert_eq!(c.weight_class(), class, "{w} tons");
        }
        assert_eq!(WeightClass::Medium.string_id(), 27008);
    }

    #[test]
    fn auto_placement_walks_the_grid_row_by_row() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        assert_eq!(auto(&mut v, &ctx, LASER).unwrap(), (0, 0));
        assert_eq!(auto(&mut v, &ctx, LASER).unwrap(), (1, 0));
        // A 2x2 PPC cannot start at (2,0)? It can: cells (2,0),(3,0),(2,1),(3,1) are free.
        assert_eq!(auto(&mut v, &ctx, PPC).unwrap(), (2, 0));
        // Next 2x2: row 1 has (0,1),(1,1) free, so it fits at (0,1).
        assert_eq!(auto(&mut v, &ctx, PPC).unwrap(), (0, 1));
        // Only (2,2) and (3,2) are left: no room for another 2x2.
        assert_eq!(
            v.can_add_component(&ctx, PPC, &mut -1, &mut -1),
            Err(LogisticsError::AddComponentFailed)
        );
    }

    #[test]
    fn heat_gate_comes_before_everything_else() {
        let cat = catalog();
        let mut ch = chassis();
        ch.max_heat = 6;
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        // max heat 6: laser (2) + laser (2) + laser (2) = 6 fits, a fourth does not.
        for _ in 0..3 {
            auto(&mut v, &ctx, LASER).unwrap();
        }
        assert_eq!(v.heat(&ctx), 6);
        assert_eq!(v.max_heat(&ctx), 6);
        assert_eq!(
            auto(&mut v, &ctx, LASER),
            Err(LogisticsError::ComponentTooHot)
        );
        // Even a jump jet is refused first by the heat rule only if it adds heat; it does not.
        assert!(auto(&mut v, &ctx, JUMPJET).is_ok());
        // A heat sink raises the cap by its dissipation (damage 1).
        auto(&mut v, &ctx, HEATSINK).unwrap();
        assert_eq!(v.max_heat(&ctx), 7);
        auto(&mut v, &ctx, LASER).unwrap_err(); // 8 > 7
    }

    #[test]
    fn jump_jet_rules() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        assert_eq!(auto(&mut v, &ctx, JUMPJET).unwrap(), (-2, -2));
        assert_eq!(v.components[0].location, Location::Legs);
        assert_eq!(
            auto(&mut v, &ctx, JUMPJET),
            Err(LogisticsError::OnlyOneJumpJetAllowed)
        );

        let mut no_jj = chassis();
        no_jj.can_have_jump_jets = false;
        let ctx2 = Ctx {
            catalog: &cat,
            chassis: &no_jj,
        };
        let w = empty_variant(&no_jj);
        assert_eq!(
            w.can_add_component(&ctx2, JUMPJET, &mut -1, &mut -1),
            Err(LogisticsError::JumpJetsNotAllowed)
        );

        // Jump jets cannot be dropped on a grid cell.
        let w = empty_variant(&ch);
        assert_eq!(
            w.can_add_component(&ctx, JUMPJET, &mut 1, &mut 1),
            Err(LogisticsError::ComponentSlotFull)
        );
        // ...and nothing else may go in the jump jet slot.
        assert_eq!(
            w.can_add_component(&ctx, LASER, &mut -2, &mut -2),
            Err(LogisticsError::ComponentSlotFull)
        );
    }

    #[test]
    fn sensor_and_ecm_are_one_each_and_go_to_the_head() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        assert_eq!(auto(&mut v, &ctx, SENSOR).unwrap(), (-3, -3));
        assert_eq!(
            auto(&mut v, &ctx, SENSOR),
            Err(LogisticsError::OnlyOneSensorAllowed)
        );
        assert_eq!(auto(&mut v, &ctx, ECM).unwrap(), (-3, -3));
        assert_eq!(
            auto(&mut v, &ctx, ECM),
            Err(LogisticsError::OnlyOneEcmAllowed)
        );
        assert_eq!(v.components[0].location, Location::Head);

        let mut limited = chassis();
        limited.can_have_adv_sensor = false;
        limited.can_have_ecm = false;
        let ctx2 = Ctx {
            catalog: &cat,
            chassis: &limited,
        };
        let w = empty_variant(&limited);
        assert_eq!(
            w.can_add_component(&ctx2, SENSOR, &mut -1, &mut -1),
            Err(LogisticsError::SensorsNotAllowed)
        );
        assert_eq!(
            w.can_add_component(&ctx2, ECM, &mut -1, &mut -1),
            Err(LogisticsError::EcmNotAllowed)
        );
    }

    #[test]
    fn armor_plating_stops_at_max_armor() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        assert_eq!(v.armor(&ctx), 100);
        auto(&mut v, &ctx, BULK).unwrap();
        assert_eq!(v.armor(&ctx), 132);
        auto(&mut v, &ctx, BULK).unwrap();
        assert_eq!(v.armor(&ctx), 164); // exactly max_armor is allowed
        assert_eq!(auto(&mut v, &ctx, BULK), Err(LogisticsError::NoMoreArmor));
    }

    #[test]
    fn explicit_placement_is_bounds_and_overlap_checked() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        let (mut x, mut y) = (3, 0);
        // 2x2 at column 3 runs off the 4-wide grid.
        assert_eq!(
            v.add_component(&ctx, PPC, &mut x, &mut y),
            Err(LogisticsError::ComponentSlotFull)
        );
        let (mut x, mut y) = (2, 1);
        v.add_component(&ctx, PPC, &mut x, &mut y).unwrap();
        let (mut x, mut y) = (3, 2);
        assert_eq!(
            v.add_component(&ctx, LASER, &mut x, &mut y),
            Err(LogisticsError::ComponentSlotFull),
            "(3,2) is covered by the PPC at (2,1)"
        );
        let (mut x, mut y) = (3, 0);
        v.add_component(&ctx, LASER, &mut x, &mut y).unwrap();
        // Grid height is 3: a 2x3 gun does not fit below row 0 here.
        let (mut x, mut y) = (0, 1);
        assert_eq!(
            v.add_component(&ctx, BIG_GUN, &mut x, &mut y),
            Err(LogisticsError::ComponentSlotFull)
        );
    }

    #[test]
    fn removing_a_heat_sink_keeps_the_mech_cool() {
        let cat = catalog();
        let mut ch = chassis();
        ch.max_heat = 6;
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        let (hx, hy) = auto(&mut v, &ctx, HEATSINK).unwrap(); // cap 7
        for _ in 0..3 {
            auto(&mut v, &ctx, LASER).unwrap(); // 6 heat
        }
        // Removing the only sink leaves cap 6 against heat 6: allowed (the test is `<`).
        let (hx2, hy2) = auto(&mut v, &ctx, HEATSINK).unwrap(); // second sink, cap 8
        auto(&mut v, &ctx, LASER).unwrap(); // heat 8, cap 8
        assert_eq!((v.heat(&ctx), v.max_heat(&ctx)), (8, 8));
        assert_eq!(
            v.remove_component(&ctx, hx2, hy2),
            Err(LogisticsError::InsufficientHeat),
            "cap would drop to 7 under heat 8"
        );
        // Taking a laser away first makes room.
        let (lx, ly) = (v.components[2].x, v.components[2].y);
        v.remove_component(&ctx, lx, ly).unwrap();
        v.remove_component(&ctx, hx2, hy2).unwrap();
        v.remove_component(&ctx, hx, hy).unwrap();
        assert_eq!(v.max_heat(&ctx), 6);
    }

    #[test]
    fn removal_shifts_the_list_and_reports_missing_cells() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        auto(&mut v, &ctx, LASER).unwrap();
        auto(&mut v, &ctx, BULK).unwrap();
        auto(&mut v, &ctx, JUMPJET).unwrap();
        assert_eq!(v.remove_component(&ctx, 3, 2), Err(LogisticsError::Failed));
        v.remove_component(&ctx, 0, 0).unwrap();
        assert_eq!(v.components.len(), 2);
        assert_eq!(v.components[0].component, BULK);
        // Jump jets are found at (-2,-2).
        v.remove_component(&ctx, -2, -2).unwrap();
        assert_eq!(v.components.len(), 1);
    }

    #[test]
    fn cost_weight_and_derived_numbers() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        assert_eq!(v.cost(&ctx), 100_000);
        assert_eq!(v.weight(&ctx), 17); // 50 * 0.2 + 7
        auto(&mut v, &ctx, PPC).unwrap();
        auto(&mut v, &ctx, JUMPJET).unwrap();
        assert_eq!(v.cost(&ctx), 100_000 + 5000 + 800);
        assert_eq!(v.weight(&ctx), 17 + 7 + 2);
        assert_eq!(v.jump_range(&ctx), 5);
        assert_eq!(v.speed(&ctx), 15);
        assert_eq!(v.display_speed(&ctx), 54);
    }

    #[test]
    fn variant_equality_ignores_names_and_ids() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut a = empty_variant(&ch);
        let mut b = empty_variant(&ch);
        b.name = "Other".into();
        b.id += 0x10000;
        auto(&mut a, &ctx, LASER).unwrap();
        assert!(!a.equivalent(&b, &ctx, &ctx));
        auto(&mut b, &ctx, LASER).unwrap();
        assert!(a.equivalent(&b, &ctx, &ctx));
        b.available_to_user = false;
        assert!(!a.equivalent(&b, &ctx, &ctx));
    }

    #[test]
    fn renards_atlas_is_never_available() {
        let ch = chassis();
        let mut v = empty_variant(&ch);
        v.set_available(true);
        assert!(v.available_to_user);
        v.name = "Renard's Atlas".into();
        v.set_available(true);
        assert!(!v.available_to_user);
        assert!(!v.all_components_available(|_| true));
    }

    #[test]
    fn from_spec_adds_large_components_first() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        // A laser listed before a PPC, both with auto (-1,-1)... file rows carry real
        // coordinates; use them: laser at (0,0), PPC at (0,1)? The PPC is added first.
        let spec = VariantSpec {
            file_id: 2,
            name: "Spec".into(),
            hidden: false,
            components: vec![(LASER, 0, 0), (PPC, 0, 0), (JUMPJET, 0, 0)],
        };
        let v = Variant::from_spec(&ctx, &spec, 0x0002_0200);
        // PPC (2x2) goes first and takes (0,0); the laser at (0,0) then collides and is dropped.
        assert_eq!(v.components.len(), 2);
        assert_eq!(v.components[0].component, PPC);
        assert_eq!(v.components[1].component, JUMPJET);
        assert!(v.designer);
        assert!(!v.available_to_user);
        assert_eq!(v.file_id, 2);
    }

    #[test]
    fn optimal_range_prefers_the_band_with_most_damage() {
        let (cat, ch) = (catalog(), chassis());
        let ctx = Ctx {
            catalog: &cat,
            chassis: &ch,
        };
        let mut v = empty_variant(&ch);
        assert_eq!(v.optimal_range_band(&ctx), 0);
        auto(&mut v, &ctx, LASER).unwrap();
        assert_eq!(v.optimal_range_band(&ctx), 0);
    }
}
