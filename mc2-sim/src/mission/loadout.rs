// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The starting loadout of a mover: armor, internal structure and inventory as `BattleMech::init(DWORD variantNum)`
//! (mech.cpp:1376-1925) and `GroundVehicle::init(FitIniFile*)` (gvehicl.cpp:985-1260) leave them.
//!
//! Only the state the frame-0 digest prints (armor, internal structure, inventory) and the indices other lanes look up
//! (`cockpit`, `engine`, `lifeSupport`, `sensor`, `ecm`, `probe`, `jumpJets`, `gyro`, the actuators, jump jet count, the body
//! locations' `CASE` and `totalSpaces`, `ItemLocationToInvLocation`) are built here. What those functions derive afterwards from
//! combat tables (`calcAmmoTotals`, `calcFireRanges`, `calcCV`, `setThreatRating`, `calcWeaponEffectiveness`, the weapon node `facing`
//! that comes from the appearance, `effectiveness`, `chassisClass`, `cBills`) is [`crate::combat::init_mover_stats`]'s.
//! `Loadout::critical` holds the seven indices in the order listed above, 255 for "none".
//!
//! Three passes read the chassis inventory (mech.cpp:1474-1670): everything but weapons and ammo first, then weapons, then ammo; each
//! installed item gets the next free slot, so the final order is "other, weapons, ammo" regardless of the CSV order, and the critical
//! slots of the body locations are mapped through `ItemLocationToInvLocation` (CSV item index -> slot).

// Struct updates stay: lane C adds fields to `InventoryItem` (start amount, ammo index, effectiveness).
#![allow(clippy::needless_update)]

use crate::combat::{BodyLocationState, InventoryItem, Loadout, MoverStats};
use mc2_sim_data::component::{ComponentForm, MasterComponentList};
use mc2_sim_data::mech::chassis::{Chassis, ChassisVariant};
use mc2_sim_data::mech::layout::{BodyLocation, NUM_ARMOR_LOCATIONS, NUM_BODY_LOCATIONS};
use mc2_sim_data::mech::vehicle::GroundVehicleObject;

/// Armor points one BULK component adds (`ARMOR_POINTS_PER_NEWARMOR`, mech.cpp:284).
const ARMOR_POINTS_PER_NEWARMOR: f32 = 32.0;
/// Shares of the bulk armor per armor location (mech.cpp:285-295), in `MECH_ARMOR_LOCATION_*` order.
const ARMOR_BULK_PERCENT: [f32; NUM_ARMOR_LOCATIONS] = [
    0.0, 0.17, 0.125, 0.125, 0.12, 0.12, 0.13, 0.13, 0.03, 0.02, 0.02,
];
/// `IS_DAMAGE_*` (mover.h:105).
const IS_DAMAGE_NONE: u8 = 0;
const IS_DAMAGE_PARTIAL: u8 = 1;
const IS_DAMAGE_DESTROYED: u8 = 2;
/// Index of "none" in the critical component indices (`cockpit = 255`, ...).
pub const NO_COMPONENT: u8 = 255;

/// Positions inside [`Loadout::critical`].
pub mod crit {
    /// `cockpit`.
    pub const COCKPIT: usize = 0;
    /// `engine`.
    pub const ENGINE: usize = 1;
    /// `lifeSupport`.
    pub const LIFE_SUPPORT: usize = 2;
    /// `sensor`.
    pub const SENSOR: usize = 3;
    /// `ecm`.
    pub const ECM: usize = 4;
    /// `probe`.
    pub const PROBE: usize = 5;
    /// `jumpJets`.
    pub const JUMP_JETS: usize = 6;
}

/// A mech ready to be put into a slot.
#[derive(Clone, Debug)]
pub struct MechBuild {
    /// Armor, internal structure, inventory.
    pub loadout: Loadout,
    /// `tonnage` (CSV row 3 column 5).
    pub tonnage: f32,
    /// `maxMoveSpeed` (CSV row 7 column 5).
    pub max_move_speed: f32,
    /// `numJumpJets`: 5 once a jump jet component is installed (mech.cpp:1534).
    pub num_jump_jets: u8,
    /// `variantName`.
    pub variant_name: String,
    /// `variantID` (the variant actually loaded; a missing variant falls back to 0).
    pub variant_id: u32,
    /// `numArmorComponents` (BULK components, mech.cpp:1544).
    pub num_armor_components: u32,
}

fn is_other_form(f: ComponentForm) -> bool {
    use ComponentForm::*;
    matches!(
        f,
        JumpJet
            | Cockpit
            | Sensor
            | LifeSupport
            | Gyroscope
            | PowerAmplifier
            | Ecm
            | Probe
            | Jammer
            | Case
            | Actuator
            | Engine
            | HeatSink
    )
}

fn is_weapon_form(f: ComponentForm) -> bool {
    use ComponentForm::*;
    matches!(f, Weapon | WeaponEnergy | WeaponBallistic | WeaponMissile)
}

/// `BattleMech::init(DWORD variantNum)`.
pub fn build_mech(
    chassis: &Chassis,
    variant_num: u32,
    components: &MasterComponentList,
) -> Result<MechBuild, String> {
    // mech.cpp:1405-1413: a missing variant falls back to 0 (with a PAUSE the shipped game would never show)
    let variant: &ChassisVariant = chassis
        .variants
        .get(variant_num as usize)
        .or_else(|| chassis.variants.first())
        .ok_or_else(|| format!("chassis {} has no variant", chassis.file_name))?;
    let variant_id = variant.index;

    let comp = |master_id: u8| {
        components.get(i32::from(master_id)).ok_or_else(|| {
            format!(
                "{}: component {master_id} is not in compbas.csv",
                chassis.file_name
            )
        })
    };

    // armor (mech.cpp:1425-1488): maxArmor row 18, curArmor row 115 + 97 * variant
    let mut armor: Vec<(f32, u8)> = Vec::with_capacity(NUM_ARMOR_LOCATIONS);
    for i in 0..NUM_ARMOR_LOCATIONS {
        let max = chassis.base_armor[i].unwrap_or(0);
        let cur = variant.current_armor[i].unwrap_or(0);
        armor.push((f32::from(cur), max));
    }

    // inventory, three passes
    let mut inventory: Vec<InventoryItem> = Vec::new();
    let mut item_to_slot = vec![NO_COMPONENT; variant.items.len()];
    let mut critical = vec![NO_COMPONENT; 7];
    let mut num_jump_jets = 0u8;
    let mut gyro = NO_COMPONENT;
    let mut actuator = [NO_COMPONENT; 4];
    let mut anti_missile: Vec<u8> = Vec::new();
    let special = components.special();
    let mut num_armor_components = 0u32;
    let (mut num_other, mut num_weapons, mut num_ammos) = (0u8, 0u8, 0u8);

    for (idx, item) in variant.items.iter().enumerate() {
        if !item.is_installed() {
            continue;
        }
        let master = item.comp_bas.unwrap_or(0);
        let c = comp(master)?;
        let Some(form) = c.form else { continue };
        if is_other_form(form) {
            if form == ComponentForm::JumpJet {
                num_jump_jets = 5;
            }
            num_other += 1;
            item_to_slot[idx] = inventory.len() as u8;
            inventory.push(InventoryItem {
                master_id: master,
                health: c.health,
                disabled: false,
                amount: 1,
                ready_time: 0.0,
                body_location: 255,
                ..InventoryItem::default()
            });
        } else if form == ComponentForm::Bulk {
            // armor component: not stored with the others
            num_armor_components += 1;
        }
    }
    for (idx, item) in variant.items.iter().enumerate() {
        if !item.is_installed() {
            continue;
        }
        let master = item.comp_bas.unwrap_or(0);
        let c = comp(master)?;
        let Some(form) = c.form else { continue };
        if is_weapon_form(form) {
            num_weapons += 1;
            item_to_slot[idx] = inventory.len() as u8;
            inventory.push(InventoryItem {
                master_id: master,
                health: c.health,
                disabled: false,
                amount: 1,
                // `facing = appearance->getWeaponNode(type)`: lane F
                ready_time: 0.0,
                body_location: 255,
                ..InventoryItem::default()
            });
        }
    }
    for (idx, item) in variant.items.iter().enumerate() {
        if !item.is_installed() {
            continue;
        }
        let master = item.comp_bas.unwrap_or(0);
        let c = comp(master)?;
        if c.form == Some(ComponentForm::Ammo) {
            num_ammos += 1;
            let per_ton = match c.stats {
                mc2_sim_data::component::ComponentStats::Ammo { ammo_per_ton, .. } => ammo_per_ton,
                _ => 0,
            };
            item_to_slot[idx] = inventory.len() as u8;
            inventory.push(InventoryItem {
                master_id: master,
                health: c.health,
                disabled: false,
                amount: per_ton as i16,
                ready_time: 0.0,
                body_location: 255,
                ..InventoryItem::default()
            });
        }
    }

    // body locations (mech.cpp:1672-1862)
    let mut body: Vec<BodyLocationState> = Vec::with_capacity(NUM_BODY_LOCATIONS);
    for loc in BodyLocation::ALL {
        let li = loc.index();
        let cur = variant.current_internal[li].unwrap_or(0);
        let max = chassis.base_internal[li];
        let damage_level = f32::from(cur) / f32::from(max);
        let damage_state = if damage_level <= 0.0 {
            IS_DAMAGE_DESTROYED
        } else if damage_level <= 0.5 {
            IS_DAMAGE_PARTIAL
        } else {
            IS_DAMAGE_NONE
        };
        let mut spaces: Vec<(u8, bool)> = Vec::new();
        let mut total_spaces = 0i32;
        for slot in variant.slots(loc) {
            // the C++ maps the CSV item index through ItemLocationToInvLocation (0xff where unmapped)
            let mapped = match slot {
                Some(v) => item_to_slot
                    .get(*v as usize)
                    .copied()
                    .unwrap_or(NO_COMPONENT),
                None => NO_COMPONENT,
            };
            spaces.push((mapped, false));
            if mapped < 255 {
                if usize::from(mapped) >= inventory.len() {
                    continue; // "Sage has filled them all in now"
                }
                let inv = &mut inventory[usize::from(mapped)];
                inv.body_location = li as u8;
                let c = comp(inv.master_id)?;
                // The following line assumes the "new" crit hit system...
                total_spaces += i32::from(c.size);
                match c.form {
                    Some(ComponentForm::Cockpit) => critical[crit::COCKPIT] = mapped,
                    Some(ComponentForm::JumpJet) => critical[crit::JUMP_JETS] = mapped,
                    Some(ComponentForm::Sensor) => critical[crit::SENSOR] = mapped,
                    Some(ComponentForm::Engine) => critical[crit::ENGINE] = mapped,
                    Some(ComponentForm::LifeSupport) => critical[crit::LIFE_SUPPORT] = mapped,
                    Some(ComponentForm::Ecm) => critical[crit::ECM] = mapped,
                    Some(ComponentForm::Probe) => critical[crit::PROBE] = mapped,
                    Some(ComponentForm::Gyroscope) => gyro = mapped,
                    Some(ComponentForm::WeaponBallistic) => {
                        let m = i32::from(inv.master_id);
                        if m == special.clan_anti_missile || m == special.inner_sphere_anti_missile
                        {
                            anti_missile.push(mapped);
                        }
                    }
                    Some(ComponentForm::Actuator) => {
                        let m = i32::from(inv.master_id);
                        if m == special.arm_actuator {
                            if loc == BodyLocation::LeftArm {
                                actuator[0] = mapped;
                            } else if loc == BodyLocation::RightArm {
                                actuator[1] = mapped;
                            }
                        } else if m == special.leg_actuator {
                            if loc == BodyLocation::LeftLeg {
                                actuator[2] = mapped;
                            } else if loc == BodyLocation::RightLeg {
                                actuator[3] = mapped;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        body.push(BodyLocationState {
            cur_internal_structure: f32::from(cur),
            max_internal_structure: max,
            damage_state,
            critical_spaces: spaces,
            // ALL MC2 mechs have CASE everywhere.
            case: true,
            total_spaces,
        });
    }

    // BULK armor (mech.cpp:1890-1955): every location grows by its share and is refilled
    if num_armor_components > 0 {
        let total = ARMOR_POINTS_PER_NEWARMOR * num_armor_components as f32;
        for (i, a) in armor.iter_mut().enumerate() {
            let add = ARMOR_BULK_PERCENT[i] * total;
            // maxArmor is an unsigned char in the C++ array; the float sum is stored back truncated
            let max = (f32::from(a.1) + add) as u8;
            a.1 = max;
            a.0 = f32::from(max);
        }
    }

    Ok(MechBuild {
        loadout: Loadout {
            armor,
            body,
            inventory,
            ammo_totals: Vec::new(),
            counts: (num_weapons, num_other, num_ammos),
            critical,
            stats: MoverStats {
                gyro,
                actuator,
                anti_missile,
                chassis_br: chassis.chassis_br.unwrap_or(0),
                num_armor_components: num_armor_components as i32,
                ..MoverStats::default()
            },
            tail: Vec::new(),
            item_to_inv: item_to_slot,
        },
        tonnage: chassis.tonnage,
        max_move_speed: f32::from(chassis.max_run_speed),
        num_jump_jets,
        variant_name: variant.name.clone(),
        variant_id,
        num_armor_components,
    })
}

/// A ground vehicle ready to be put into a slot.
#[derive(Clone, Debug)]
pub struct VehicleBuild {
    /// Armor (5 locations), internal structure and inventory.
    pub loadout: Loadout,
    /// `tonnage` (`CurTonnage`).
    pub tonnage: f32,
    /// `maxMoveSpeed` (the dynamics' speed).
    pub max_move_speed: f32,
    /// `accel` is set to `maxVelocity * 5` by `MoverDynamics::init`.
    pub acceleration: f32,
}

/// `GroundVehicle::init(FitIniFile*)` for the packet data and `components`.
pub fn build_vehicle(
    v: &GroundVehicleObject,
    components: &MasterComponentList,
) -> Result<VehicleBuild, String> {
    let comp = |master_id: u8| {
        components
            .get(i32::from(master_id))
            .ok_or_else(|| format!("vehicle component {master_id} is not in compbas.csv"))
    };
    let mut inventory: Vec<InventoryItem> = Vec::new();
    let mut critical = vec![NO_COMPONENT; 7];
    let other = usize::from(v.num_other);
    let weapons = usize::from(v.num_weapons);
    for (n, &master) in v.items.iter().enumerate() {
        let c = comp(master)?;
        let item = if n < other {
            match c.form {
                Some(ComponentForm::Cockpit) => critical[crit::COCKPIT] = n as u8,
                Some(ComponentForm::Sensor) => critical[crit::SENSOR] = n as u8,
                Some(ComponentForm::Engine) => critical[crit::ENGINE] = n as u8,
                Some(ComponentForm::LifeSupport) => critical[crit::LIFE_SUPPORT] = n as u8,
                Some(ComponentForm::Ecm) => critical[crit::ECM] = n as u8,
                Some(ComponentForm::Probe) => critical[crit::PROBE] = n as u8,
                _ => {}
            }
            InventoryItem {
                master_id: master,
                health: c.health,
                disabled: false,
                amount: 1,
                ready_time: 0.0,
                body_location: 255,
                ..InventoryItem::default()
            }
        } else if n < other + weapons {
            InventoryItem {
                master_id: master,
                health: c.health,
                disabled: false,
                amount: 1,
                facing: 1,
                ready_time: 0.0,
                body_location: 255,
                ..InventoryItem::default()
            }
        } else {
            let per_ton = match c.stats {
                mc2_sim_data::component::ComponentStats::Ammo { ammo_per_ton, .. } => ammo_per_ton,
                _ => 0,
            };
            InventoryItem {
                master_id: master,
                health: c.health,
                disabled: false,
                amount: per_ton as i16,
                ready_time: 0.0,
                body_location: 255,
                ..InventoryItem::default()
            }
        };
        inventory.push(item);
    }

    let mut armor: Vec<(f32, u8)> = Vec::new();
    let mut body: Vec<BodyLocationState> = Vec::new();
    for p in &v.parts {
        let damage_level = f32::from(p.cur_internal) / f32::from(p.max_internal);
        let damage_state = if damage_level == 0.0 {
            IS_DAMAGE_DESTROYED
        } else if damage_level <= 0.5 {
            IS_DAMAGE_PARTIAL
        } else {
            IS_DAMAGE_NONE
        };
        body.push(BodyLocationState {
            cur_internal_structure: f32::from(p.cur_internal),
            max_internal_structure: p.max_internal,
            damage_state,
            critical_spaces: Vec::new(),
            case: false,
            total_spaces: 0,
        });
        armor.push((f32::from(p.cur_armor), p.max_armor));
    }
    // refit, recover and mine layer vehicles keep their points in the turret armor (gvehicl.cpp:1232-1260)
    // `cur = max = points` assigns through the unsigned char `maxArmor`, so both wrap to 8 bits.
    let turret = armor.len() - 1;
    let to_armor = |points: i64| {
        let max = points as u8;
        (f32::from(max), max)
    };
    if v.refit_points > 0 {
        armor[turret] = to_armor(v.refit_points);
    }
    if v.recover_points > 0 {
        armor[turret] = to_armor(v.recover_points);
    }
    if v.mine_layer != 0 {
        armor[turret] = to_armor(v.mine_layer);
    }

    Ok(VehicleBuild {
        loadout: Loadout {
            armor,
            body,
            inventory,
            ammo_totals: Vec::new(),
            counts: (v.num_weapons, v.num_other, v.num_ammo),
            critical,
            stats: MoverStats {
                battle_rating: v.battle_rating,
                ..MoverStats::default()
            },
            tail: Vec::new(),
            item_to_inv: Vec::new(),
        },
        tonnage: v.tonnage,
        max_move_speed: v.dynamics.speed,
        acceleration: v.dynamics.accel,
    })
}
