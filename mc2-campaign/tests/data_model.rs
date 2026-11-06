// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The static model against the shipped data and against the independent `mc2-sim-data`
//! loaders (which read the same files for the simulation, not for logistics).

mod common;

use mc2_campaign::variant::Ctx;
use mc2_sim_data::component::{ComponentStats, MasterComponentList};
use mc2_sim_data::mech::load_chassis;

#[test]
fn component_catalogue_matches_the_master_list() {
    let data = require_data!();
    let mut vfs = common::vfs().unwrap();
    let master = MasterComponentList::load(&mut vfs).unwrap();

    let listed: Vec<_> = master.logistics_listed().collect();
    assert_eq!(data.catalog.len(), listed.len(), "same set of listed rows");
    assert_eq!(data.catalog.len(), 39);
    for (c, m) in data.catalog.iter().zip(&listed) {
        assert_eq!(c.id, m.master_id);
        assert_eq!(Some(c.form), m.form, "component {}", c.id);
        assert_eq!(c.cost, m.resource_points, "cost of {}", c.id);
        assert_eq!(c.weight, m.tonnage, "weight of {}", c.id);
        assert_eq!(c.string_id, m.logistics.string_index);
        assert_eq!(c.help_string_id, m.logistics.help_string_index);
        assert_eq!(c.width, m.logistics.icon_x);
        assert_eq!(c.height, m.logistics.icon_y);
        assert_eq!(Some(c.icon_file.as_str()), m.logistics.icon.as_deref());
        match &m.stats {
            ComponentStats::Weapon(w) => {
                // The master reader truncates heat to a whole number, the logistics one keeps
                // the fraction.
                assert_eq!(c.heat.trunc(), w.heat, "heat of {}", c.id);
                assert_eq!(c.damage, w.damage, "damage of {}", c.id);
                assert_eq!(c.recycle_time, w.recycle_time, "recycle of {}", c.id);
            }
            ComponentStats::HeatSink { dissipation } => {
                assert_eq!(c.damage as i16, *dissipation, "dissipation of {}", c.id);
            }
            _ => {}
        }
    }
}

#[test]
fn ammo_counts_come_from_the_ammo_rows_the_weapons_point_at() {
    let data = require_data!();
    let mut vfs = common::vfs().unwrap();
    let bytes = vfs.read("data/objects/compbas.csv").unwrap().unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // data row n (0-based, after the header)
    let rows: Vec<Vec<&str>> = text
        .lines()
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.trim_end_matches('\r').split(',').collect())
        .collect();

    let mut checked = 0;
    for c in data.catalog.iter().filter(|c| c.is_weapon()) {
        let row = rows
            .iter()
            .find(|r| r[0].trim().parse::<i32>() == Ok(c.id))
            .unwrap();
        let pointer: usize = row[22].trim().parse().unwrap_or(0);
        if pointer == 0 {
            assert_eq!(c.ammo, 0, "component {}", c.id);
            continue;
        }
        checked += 1;
        // the row it points at is an ammo row, and its recycle column holds the rounds
        let ammo_row = &rows[pointer];
        assert_eq!(
            ammo_row[1].trim(),
            "Ammo",
            "component {} points at row {pointer}",
            c.id
        );
        let rounds: f64 = ammo_row[4].trim().parse().unwrap();
        assert_eq!(
            f64::from(c.ammo),
            rounds.trunc(),
            "rounds of component {}",
            c.id
        );
    }
    assert!(checked >= 15, "most weapons carry ammo, saw {checked}");
}

#[test]
fn chassis_agree_with_the_simulation_loader() {
    let data = require_data!();
    let mut vfs = common::vfs().unwrap();
    assert_eq!(data.chassis.len(), 32, "29 mechs and 3 helicopters");
    for c in &data.chassis {
        let sim = load_chassis(&mut vfs, c.stem()).unwrap();
        assert_eq!(c.max_weight, sim.tonnage, "{}", c.file_name);
        // Cells the logistics reader cannot read count as 0 (`CSVFile::readLong`).
        assert_eq!(c.base_cost, sim.base_cost.unwrap_or(0), "{}", c.file_name);
        assert_eq!(c.max_armor, sim.max_armor.unwrap_or(0), "{}", c.file_name);
        assert_eq!(
            c.base_armor,
            sim.total_armor.unwrap_or(0),
            "{}",
            c.file_name
        );
        assert_eq!(c.max_heat, sim.heat_index.unwrap_or(0), "{}", c.file_name);
        assert_eq!(c.house_id, sim.house_id.unwrap_or(0), "{}", c.file_name);
        assert_eq!(c.chassis_name_id, sim.descriptor_id.map_or(0, i64::from));
        assert_eq!(c.encyclopedia_id, sim.abbr_index.unwrap_or(0));
        assert_eq!(c.help_id, sim.help_id.unwrap_or(0));
        assert_eq!(c.component_area_width, sim.logistics_width.unwrap_or(0));
        assert_eq!(c.component_area_height, sim.logistics_height.unwrap_or(0));
        assert_eq!(c.can_have_adv_sensor, sim.allow.advanced_sensor);
        assert_eq!(c.can_have_optics, sim.allow.optics);
        assert_eq!(c.can_have_global_sensor, sim.allow.global_sensor);
        assert_eq!(c.can_have_ecm, sim.allow.ecm);
        assert_eq!(c.can_have_active_probe, sim.allow.active_probe);
        assert_eq!(c.can_have_jump_jets, sim.allow.jump_jets);
        assert_eq!(c.can_have_extended_sensor, sim.allow.extended_sensor);
        assert_eq!(
            c.icon_picture_index,
            sim.icon_picture_index.map_or(0, i64::from)
        );
        for (i, name) in c.icon_file_names.iter().enumerate() {
            if let Some(expected) = &sim.icon_files[i] {
                assert_eq!(name, expected, "icon {i} of {}", c.file_name);
            }
        }

        // the designer variants are the CSV's variants, in order, with their hidden flags
        let mine: Vec<_> = data
            .designer_variants
            .iter()
            .filter(|(ci, _)| *ci == c.index)
            .map(|(_, s)| s)
            .collect();
        assert_eq!(mine.len(), sim.variants.len(), "{}", c.file_name);
        for (spec, v) in mine.iter().zip(&sim.variants) {
            assert_eq!(spec.name, v.name);
            assert_eq!(spec.hidden, v.hidden);
            assert_eq!(spec.file_id as u32, v.index);
        }
    }
}

#[test]
fn every_designer_variant_component_is_placed() {
    let data = require_data!();
    let mut vfs = common::vfs().unwrap();
    let l = mc2_campaign::Logistics::new(data.clone()).unwrap();

    let mut total = 0;
    for (ci, spec) in &data.designer_variants {
        let chassis = &data.chassis[*ci];
        let sim = load_chassis(&mut vfs, chassis.stem()).unwrap();
        let sv = &sim.variants[spec.file_id as usize];
        // independent count: rows of the variant that name a component the catalogue has
        // (`LogisticsVariant::init` reads rows 26..96: 71 of the 72 inventory rows the simulation
        // loads; the last one is not part of a logistics variant)
        let expected = sv
            .items
            .iter()
            .take(71)
            .filter(|row| matches!(row.comp_bas, Some(id) if id != 255 && data.catalog.find(i32::from(id)).is_some()))
            .count();
        assert_eq!(
            spec.components.len(),
            expected,
            "{} {}",
            chassis.file_name,
            spec.name
        );
        // `LogisticsVariant::init` only adds components of 1-3 cells wide and 1-4 high; larger
        // ones (the Long Tom is 2x5) are skipped by its size loops.
        let too_big = spec
            .components
            .iter()
            .filter(|(index, _, _)| {
                let c = data.catalog.get(*index).unwrap();
                !(1..=3).contains(&c.width) || !(1..=4).contains(&c.height)
            })
            .count();

        let (_, built) = l
            .variants()
            .find(|(_, v)| v.designer && v.chassis == *ci && v.file_id == spec.file_id)
            .unwrap();
        // every component fits the lab grid in some free cell, so none is dropped
        if spec.name == "Renard's Atlas" {
            // The shipped loadout asks for 15 armor plates on a 6x5 grid that its four big
            // weapons leave 8 cells on; the C++ drops the plates that do not fit, so do we.
            assert_eq!(built.components.len(), 13, "5 weapons + 8 plates");
        } else {
            assert_eq!(
                built.components.len(),
                spec.components.len() - too_big,
                "{} / {}: a component was not placed",
                chassis.file_name,
                spec.name
            );
        }
        total += built.components.len();
    }
    assert!(total > 300, "{total} components in 39 variants");
}

#[test]
fn designer_variants_do_not_overlap_and_stay_in_the_grid() {
    let data = require_data!();
    let l = mc2_campaign::Logistics::new(data.clone()).unwrap();
    for (_, v) in l.variants() {
        let chassis = &data.chassis[v.chassis];
        let ctx = Ctx {
            catalog: &data.catalog,
            chassis,
        };
        let mut occupied = std::collections::BTreeSet::new();
        for p in &v.components {
            let c = data.catalog.get(p.component).unwrap();
            if p.x < 0 {
                continue; // jump jets and head slot components are not on the grid
            }
            for dx in 0..i64::from(c.width) {
                for dy in 0..i64::from(c.height) {
                    let cell = (p.x + dx, p.y + dy);
                    assert!(occupied.insert(cell), "{}: overlap at {cell:?}", v.name);
                    assert!(
                        cell.0 < chassis.component_area_width
                            && cell.1 < chassis.component_area_height,
                        "{}: {cell:?} outside {}x{}",
                        v.name,
                        chassis.component_area_width,
                        chassis.component_area_height
                    );
                }
            }
        }
        let _ = ctx;
    }
}

#[test]
fn designer_variant_numbers_follow_the_c_formulas() {
    let data = require_data!();
    let l = mc2_campaign::Logistics::new(data.clone()).unwrap();
    for (_, v) in l.variants() {
        let chassis = &data.chassis[v.chassis];
        let ctx = l.ctx_for(v);

        // independent formulas from the component rows
        let comps: Vec<_> = v
            .components
            .iter()
            .map(|p| data.catalog.get(p.component).unwrap())
            .collect();
        let cost: i64 = chassis.base_cost + comps.iter().map(|c| i64::from(c.cost)).sum::<i64>();
        assert_eq!(i64::from(v.cost(&ctx)), cost, "{}", v.name);

        let mut weight = (f64::from(chassis.max_weight) * 0.2 + 7.0) as f32;
        for c in &comps {
            weight += c.weight;
        }
        assert_eq!(v.weight(&ctx), weight as i32, "{}", v.name);

        let bulk = comps
            .iter()
            .filter(|c| c.form == mc2_sim_data::component::ComponentForm::Bulk)
            .count() as i64;
        assert_eq!(
            i64::from(v.armor(&ctx)),
            chassis.base_armor + 32 * bulk,
            "{}",
            v.name
        );

        // the variant ID: build-up count in bits 16-23, house in 8-15, chassis index in 0-7
        assert_eq!(v.id & 0xffff, chassis.id, "{}", v.name);
        // a mech that ships can run its own weapons without overheating by more than a hair
        assert!(v.max_heat(&ctx) >= chassis.max_heat as i32);
    }
}

#[test]
fn variant_ids_count_up_per_chassis() {
    let data = require_data!();
    let l = mc2_campaign::Logistics::new(data.clone()).unwrap();
    for chassis in &data.chassis {
        let ids: Vec<u32> = l
            .variants()
            .filter(|(_, v)| v.chassis == chassis.index)
            .map(|(_, v)| v.id >> 16)
            .collect();
        let expected: Vec<u32> = (0..ids.len() as u32).collect();
        assert_eq!(ids, expected, "{}", chassis.file_name);
    }
    // Atlas has three variants: Prime, Claymore's (hidden), Renard's (hidden).
    let atlas = data.chassis_by_stem("atlas").unwrap();
    assert_eq!(l.chassis_variants(atlas).len(), 3);
}

#[test]
fn pilots_load_in_csv_order_with_skills_and_ranks() {
    let data = require_data!();
    let l = mc2_campaign::Logistics::new(data.clone()).unwrap();
    assert_eq!(l.pilots().len(), 52);
    // the first mission's purchase list (purchase01) names 15 of them
    assert_eq!(l.pilots().iter().filter(|p| p.available).count(), 15);
    assert_eq!(l.pilots()[0].file_name, "pmwbubba");
    for (i, p) in l.pilots().iter().enumerate() {
        assert_eq!(p.id as usize, i + 1, "ids follow the line numbers");
        assert!(!p.used && !p.dead);
        assert_eq!(
            p.rank,
            mc2_campaign::pilot::turn_average_into_rank((p.gunnery + p.piloting) / 2.0)
        );
        assert!(p.gunnery > 0.0 && p.piloting > 0.0, "{}", p.file_name);
    }
}
