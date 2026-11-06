// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Unit tests on synthetic `compbas.csv` text. Rows are copied from the shipped
//! file and their ids are rewritten to match their position.

use super::*;

const HEADER: &str = "Component Table,Type,Name,crit hits,Recycle,Heat,Weight,Damage,BR,RP,Range,\
Location Head,Location CT,Location LT,Location RT,Location LA,Location RA,Location LL,Location RL,\
Missile type,Fields,Special FX ID,Ammo Master ID,Logistics Icon 1,Logistics Icon 2,String Index,\
Mechlipidia String,Icon X ,Icon Y";

const UNDEFINED: &str =
    "undefined,undefined,0,na,na,0,na,0,0,0,0,0,0,0,0,0,0,0,na,0,0,0,0,0,32000,34000,,";
const FUSION: &str =
    "Engine,Fusion Engine,6,NA,NA,0,NA,0,0,0,No,6,No,No,No,No,No,No,na,0,0,0,0,0,32001,34001,,";
const LARGE_LASER: &str = "EnergyWeapon,Large Laser,2,5.00,8.00,4.00,4.00,276.40,3800,medium,No,Yes,Yes,Yes,Yes,Yes,No,No,0,0,15,0,MCL_MC_LargeLaser.tga,MCL_MC_LargeLaser,32140,34140,1,2";
const STREAK: &str = "MissileWeapon,Streak SRM2 Pack,1,3.30,2.00,2.00,2.00,103.76,2000,short,No,Yes,Yes,Yes,Yes,Yes,No,No,ST,1,8,225,,,32125,34125,1,1";
const SENSOR: &str = "Sensors,Standard Sensors,1,N/A,0.00,0.00,1.00,0.00,500,0,Yes,No,No,No,No,No,No,No,na,0,0,0,MCL_MC_Sensor.tga,MCL_MC_Sensor,32013,34013,1,1";
const HEAT_SINK: &str = "HeatSink,Heat Sink,1,na,na,1,4,0,2000,0,Yes,Yes,Yes,Yes,Yes,Yes,Yes,Yes,na,0,0,0,MCL_MC_HEATSINK.tga,MCL_MC_HEATSINK,32007,34006,1,1";
const AMMO: &str = "Ammo,Streak SRM2 Pack Ammo,1,28,na,0,6.00,0.00,500,0,No,No,No,No,No,No,No,No,na,0,0,0,0,0,32225,34225,,";
const ECM: &str = "ECM,Mk. I ECM Suite,1,350.00,0.00,0.00,0.50,0.00,500,0,Yes,No,No,No,No,No,No,No,na,0,0,0,0,0,32038,34038,,";

/// Builds CSV bytes from row bodies (without the id), with the header first.
fn compbas(rows: &[&str]) -> Vec<u8> {
    let mut text = format!("{HEADER}\r\n");
    for (id, body) in rows.iter().enumerate() {
        text.push_str(&format!("{id},{body}\r\n"));
    }
    text.into_bytes()
}

fn load(rows: &[&str]) -> MasterComponentList {
    MasterComponentList::from_compbas_bytes(&compbas(rows), rows.len(), 1.0)
        .expect("synthetic list loads")
}

#[test]
fn undefined_row_keeps_its_id_and_has_no_form() {
    let list = load(&[UNDEFINED]);
    let record = list.get(0).expect("row present");
    assert_eq!(record.master_id, 0);
    assert_eq!(record.form, None);
    assert_eq!(record.form_text, "undefined");
    assert!(!record.is_logistics_listed());
    assert_eq!(record.health, 1);
}

#[test]
fn unknown_forms_are_kept_as_unset_records() {
    let list = load(&[
        "Removed,Case,1,na,na,0.5,na,0,0,0,0,Yes,Yes,Yes,Yes,Yes,Yes,Yes,Yes,na,0,0,0,0,0,32031,34031",
        "Optics,Standard Optics,1,N/A,0.00,0.00,1.25,0.00,500,0,Yes,No,No,No,No,No,No,No,na,0,0,0,0,0,32016,34016,,",
    ]);
    assert_eq!(list.len(), 2);
    assert_eq!(list.get(0).unwrap().form, None);
    assert_eq!(list.get(1).unwrap().form, None);
    assert_eq!(list.get(1).unwrap().form_text, "Optics");
}

#[test]
fn engine_row_has_no_stats_and_one_critical_location() {
    let list = load(&[FUSION]);
    let record = list.get(0).unwrap();
    assert_eq!(record.form, Some(ComponentForm::Engine));
    assert_eq!(record.stats, ComponentStats::None);
    assert_eq!(record.size, 6);
    assert_eq!(
        record.critical_spaces,
        [-1, 6, -1, -1, -1, -1, -1, -1],
        "No is -1, a count is kept"
    );
    assert_eq!(record.critical_space_at(8), -1);
    assert_eq!(record.logistics.icon, None, "icon 0 is not listed");
    assert!(!record.is_logistics_listed());
}

#[test]
fn energy_weapon_reads_damage_heat_and_range() {
    let list = load(&[UNDEFINED, FUSION, LARGE_LASER]);
    let record = list.get(2).unwrap();
    assert_eq!(record.form, Some(ComponentForm::WeaponEnergy));
    let weapon = record.weapon().expect("weapon stats");
    assert_eq!(weapon.damage, 4.0);
    assert_eq!(weapon.recycle_time, 5.0);
    assert_eq!(weapon.damage_per_10_seconds(), 8.0);
    assert_eq!(weapon.heat, 8.0);
    assert_eq!(weapon.ammo_amount, 1);
    assert_eq!(weapon.ammo_type, WeaponAmmoType::Unlimited);
    assert_eq!(weapon.range, Some(WeaponRange::Medium));
    assert_eq!(weapon.flags, 0);
    assert_eq!(weapon.special_effect, 15);
    assert_eq!(weapon.ammo_master_id, 0);
    assert_eq!(record.resource_points, 3800);
    assert_eq!(record.battle_rating, 276.4);
    assert_eq!(record.tonnage, 4.0);
    assert!(record.is_logistics_listed());
    assert_eq!(
        record.logistics.icon.as_deref(),
        Some("MCL_MC_LargeLaser.tga")
    );
    assert_eq!(record.logistics.string_index, 32140);
    assert_eq!(record.logistics.help_string_index, 34140);
    assert_eq!((record.logistics.icon_x, record.logistics.icon_y), (1, 2));
}

#[test]
fn streak_missile_sets_only_the_streak_flag() {
    let list = load(&[UNDEFINED, FUSION, LARGE_LASER, STREAK]);
    let weapon = list.get(3).unwrap().weapon().unwrap().clone();
    assert_eq!(weapon.ammo_type, WeaponAmmoType::St);
    assert_eq!(weapon.range, Some(WeaponRange::Short));
    assert_eq!(weapon.flags, WEAPON_FLAG_STREAK);
    assert_eq!(weapon.declared_flags, 1);
    assert_eq!(weapon.special_effect, 8);
    assert_eq!(weapon.ammo_master_id, 225);
    assert_eq!(list.get(3).unwrap().logistics.icon, None, "empty icon");
}

#[test]
fn declared_fields_column_is_read_but_not_applied() {
    // Fields column 8 (artillery) is not copied into the flags.
    let row = "MissileWeapon,Thunderbolt,10,6.00,8.00,20.00,13.00,1356.99,16400,medium,No,Yes,Yes,Yes,Yes,Yes,No,No,LRM,8,27,228,MCL_x.tga,MCL_x,32161,34161,2,4";
    let list = load(&[row]);
    let weapon = list.get(0).unwrap().weapon().unwrap();
    assert_eq!(weapon.declared_flags, 8);
    assert_eq!(weapon.flags, 0);
    assert_eq!(weapon.ammo_type, WeaponAmmoType::Lrm);
}

#[test]
fn sensor_range_is_damage_times_base_sensor_range() {
    let list = MasterComponentList::from_compbas_bytes(&compbas(&[SENSOR]), 1, 2.5).unwrap();
    let record = list.get(0).unwrap();
    assert_eq!(record.stats, ComponentStats::Sensor { range: 2.5 });
}

#[test]
fn heat_sink_dissipation_comes_from_damage() {
    let list = load(&[HEAT_SINK]);
    assert_eq!(
        list.get(0).unwrap().stats,
        ComponentStats::HeatSink { dissipation: 4 }
    );
}

#[test]
fn ammo_row_reads_ammo_per_ton_from_recycle_column() {
    let list = load(&[AMMO]);
    assert_eq!(
        list.get(0).unwrap().stats,
        ComponentStats::Ammo {
            ammo_per_ton: 28,
            explosive_damage: 6.0
        }
    );
}

#[test]
fn ecm_row_reads_effect_and_range() {
    let row = "ECM,Mk. I ECM Suite,1,350.00,0.00,0.00,0.50,0.70,500,0,Yes,No,No,No,No,No,No,No,na,0,0,0,0,0,32038,34038,,";
    let list = load(&[row]);
    assert_eq!(
        list.get(0).unwrap().stats,
        ComponentStats::Ecm {
            effect: 0.5,
            range: 350.0
        }
    );
}

#[test]
fn empty_cell_shifts_later_cells_as_strtok_does() {
    // The empty Fields cell (index 20) is skipped, so Special FX reads the cell
    // after it and Ammo Master reads the logistics icon (atoi gives 0).
    let row = "EnergyWeapon,Shifted,1,5.00,8.00,4.00,4.00,276.40,3800,medium,No,Yes,Yes,Yes,Yes,Yes,No,No,0,,15,0,MCL_a.tga,MCL_a,32140,34140,1,2";
    let list = load(&[row]);
    let weapon = list.get(0).unwrap().weapon().unwrap();
    assert_eq!(weapon.declared_flags, 15);
    assert_eq!(weapon.special_effect, 0);
    assert_eq!(weapon.ammo_master_id, 0);
    assert_eq!(
        list.get(0).unwrap().logistics.icon.as_deref(),
        Some("MCL_a.tga"),
        "logistics reader is positional and is not shifted"
    );
}

#[test]
fn range_zero_is_no_range_and_bad_text_is_an_error() {
    let list = load(&[FUSION]);
    assert!(list.get(0).unwrap().weapon().is_none());

    let bad = "EnergyWeapon,Bad,1,5.00,8.00,4.00,4.00,276.40,3800,huge,No,Yes,Yes,Yes,Yes,Yes,No,No,0,0,15,0,MCL_a.tga,MCL_a,32140,34140,1,2";
    let err = MasterComponentList::from_compbas_bytes(&compbas(&[bad]), 1, 1.0).unwrap_err();
    assert!(matches!(err, SimDataError::OutOfRange { .. }), "{err}");
}

#[test]
fn range_text_is_case_insensitive() {
    let row = "EnergyWeapon,Caps,1,5.00,8.00,4.00,4.00,276.40,3800,LONG,No,Yes,Yes,Yes,Yes,Yes,No,No,0,0,15,0,MCL_a.tga,MCL_a,32140,34140,1,2";
    let list = load(&[row]);
    assert_eq!(
        list.get(0).unwrap().weapon().unwrap().range,
        Some(WeaponRange::Long)
    );
}

#[test]
fn critical_cells_accept_yes_no_and_counts() {
    let row = "EnergyWeapon,Crits,1,5.00,8.00,4.00,4.00,276.40,3800,short,Yes,No,3,No,Yes,No,No,No,0,0,15,0,MCL_a.tga,MCL_a,32140,34140,1,2";
    let list = load(&[row]);
    assert_eq!(
        list.get(0).unwrap().critical_spaces,
        [0, -1, 3, -1, 0, -1, -1, -1]
    );
}

#[test]
fn id_must_match_row_position() {
    let text = format!("{HEADER}\r\n1,{UNDEFINED}\r\n");
    let err = MasterComponentList::from_compbas_bytes(text.as_bytes(), 1, 1.0).unwrap_err();
    assert!(matches!(err, SimDataError::Schema { .. }), "{err}");
}

#[test]
fn short_file_is_an_error() {
    let text = format!("{HEADER}\r\n0,{UNDEFINED}\r\n");
    let err = MasterComponentList::from_compbas_bytes(text.as_bytes(), 2, 1.0).unwrap_err();
    assert!(matches!(err, SimDataError::Schema { .. }), "{err}");
}

#[test]
fn logistics_listed_skips_rows_without_icon() {
    let list = load(&[UNDEFINED, FUSION, LARGE_LASER, STREAK, SENSOR]);
    let listed: Vec<i32> = list.logistics_listed().map(|c| c.master_id).collect();
    assert_eq!(listed, vec![2, 4]);
}

#[test]
fn special_ids_are_minus_one_after_load() {
    let list = load(&[UNDEFINED]);
    let special = list.special();
    assert_eq!(special, SpecialComponentIds::AFTER_LOAD);
    assert_eq!(special.arm_actuator, -1);
    assert_eq!(special.clan_anti_missile, -1);
}

#[test]
fn ecm_and_ammo_forms_map_to_their_stats() {
    let list = load(&[AMMO, ECM]);
    assert!(matches!(
        list.get(0).unwrap().form,
        Some(ComponentForm::Ammo)
    ));
    assert!(matches!(
        list.get(1).unwrap().stats,
        ComponentStats::Ecm { .. }
    ));
}
