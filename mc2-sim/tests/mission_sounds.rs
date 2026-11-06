// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Production-trigger tests for mission SFX against shipped mission data.
mod common;

use mc2_sim::combat::stats::body;
use mc2_sim::events::{sfx, SimEvent};
use mc2_sim::math::{Vec3, Vec3Ext};
use mc2_sim::object::structure;
use mc2_sim::object::Kind;
use mc2_sim::object::ObjectStore;
use mc2_sim_data::objtype::ObjectTypeData;

#[test]
fn shipped_explosion_sound_ids_are_emitted_from_the_effect_data() {
    let Some(mut fixture) = common::fixture("mc2_01", "pcg") else {
        return;
    };
    let types = fixture
        .world
        .objects
        .types
        .as_ref()
        .expect("loaded object types");
    let carried: Vec<(u32, u32)> = types
        .types
        .iter()
        .filter_map(|(&num, ty)| match &ty.data {
            ObjectTypeData::Explosion(effect) => Some((num, effect.sound_effect_id)),
            _ => None,
        })
        .collect();
    println!("shipped explosion object type / SoundEffectId pairs: {carried:?}");
    let (effect_id, expected) = fixture
        .world
        .objects
        .effect_objects
        .iter()
        .enumerate()
        .find_map(|(effect_id, &num)| {
            let ty = fixture.world.objects.type_by_num(num)?;
            match &ty.data {
                ObjectTypeData::Explosion(effect) => {
                    Some((effect_id as i32, effect.sound_effect_id as i32))
                }
                _ => None,
            }
        })
        .expect("a shipped weapon effect resolves to an ExplosionType");
    fixture.world.events.clear();
    assert!(ObjectStore::create_explosion(
        &mut fixture.world,
        effect_id,
        None,
        Vec3::from_xyz(10.0, 20.0, 3.0),
        0.0,
        0.0,
    )
    .is_some());
    assert!(fixture.world.events.iter().any(|event| matches!(
        event,
        SimEvent::Explosion { sound_id: Some(id), at, .. }
            if *id == expected && *at == Vec3::from_xyz(10.0, 20.0, 3.0)
    )));
}

#[test]
fn opening_a_shipped_gate_emits_the_positional_gate_sample() {
    let found = (1..=24).find_map(|n| {
        let mission = format!("mc2_{n:02}");
        let mut fixture = common::fixture(&mission, "pcg")?;
        let gate = *fixture.world.objects.gates.first()?;
        fixture.world.events.clear();
        Some((fixture.world, gate))
    });
    let Some((mut world, gate)) = found else {
        // Some test-data subsets omit all gate-bearing missions.
        return;
    };
    if let Some(Kind::Gate(state)) = world.objects.get_mut(gate).map(|o| &mut o.kind) {
        state.reason_to_open = true;
    }
    structure::open_gate(&mut world, gate);
    assert!(world.events.iter().any(|event| matches!(
        event,
        SimEvent::Sound { id, at: Some(_), allow_dupes: true, .. } if *id == sfx::GATE_OPEN
    )));
}

#[test]
fn destroyed_mech_arm_emits_the_critical_hit_sample() {
    let Some(mut fixture) = common::fixture("mc2_01", "pcg") else {
        return;
    };
    let mech = fixture
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            matches!(
                fixture.world.objects.get(h).map(|o| &o.kind),
                Some(Kind::Mech(_))
            )
        })
        .expect("mc2_01 has a mech");
    let internal = fixture
        .world
        .objects
        .get(mech)
        .and_then(mc2_sim::combat::loadout_of)
        .unwrap()
        .body[body::LARM]
        .cur_internal_structure;
    fixture.world.events.clear();
    mc2_sim::combat::damage::injure_body_location(
        &mut fixture.world,
        mech,
        body::LARM,
        internal + 1.0,
    );
    assert!(fixture.world.events.iter().any(|event| matches!(
        event,
        SimEvent::Sound { id, at: Some(_), allow_dupes: true, .. } if *id == sfx::CRITICAL_HIT_SFX
    )));
}

#[test]
fn power_samples_use_shipped_ids_when_movement_consumes_the_transition() {
    let Some(mut fixture) = common::fixture("mc2_01", "pcg") else {
        return;
    };
    let mech = fixture.world.objects.movers.iter().copied().find(|&h| {
        matches!(fixture.world.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(m)) if m.m.move_level != 2)
    }).expect("mc2_01 has a ground mech");
    let vehicle = fixture.world.objects.movers.iter().copied().find(|&h| {
        matches!(fixture.world.objects.get(h).map(|o| &o.kind), Some(Kind::Vehicle(v)) if v.m.move_level != 2)
    }).expect("mc2_01 has a ground vehicle");

    for (h, down_id, up_id, is_mech) in [(mech, 60, 61, true), (vehicle, 108, 109, false)] {
        fixture.world.events.clear();
        mc2_sim::ai::mover_ops::shut_down(&mut fixture.world, h);
        fixture.world.events.clear();
        if is_mech {
            mc2_sim::mover::update_mech(&mut fixture.world, h).unwrap();
        } else {
            mc2_sim::mover::update_vehicle(&mut fixture.world, h).unwrap();
        }
        assert!(
            fixture.world.events.iter().any(|event| matches!(
                event,
                SimEvent::Sound { id, at: Some(_), allow_dupes: false, .. } if *id == down_id
            )),
            "shutdown sample {down_id}"
        );

        fixture.world.events.clear();
        mc2_sim::ai::mover_ops::start_up(&mut fixture.world, h);
        fixture.world.events.clear();
        if is_mech {
            mc2_sim::mover::update_mech(&mut fixture.world, h).unwrap();
        } else {
            mc2_sim::mover::update_vehicle(&mut fixture.world, h).unwrap();
        }
        assert!(
            fixture.world.events.iter().any(|event| matches!(
                event,
                SimEvent::Sound { id, at: Some(_), allow_dupes: false, .. } if *id == up_id
            )),
            "startup sample {up_id}"
        );
    }

    // A declared fixture input: use the shipped mech and set its move type to
    // AIR so the production transition selects the helicopter samples.
    if let Some(Kind::Mech(m)) = fixture.world.objects.get_mut(mech).map(|o| &mut o.kind) {
        m.m.move_level = 2;
    }
    fixture.world.clock.scenario_time = 9.0;
    for (down, id) in [(true, 106), (false, 107)] {
        fixture.world.events.clear();
        if down {
            mc2_sim::ai::mover_ops::shut_down(&mut fixture.world, mech);
        } else {
            mc2_sim::ai::mover_ops::start_up(&mut fixture.world, mech);
        }
        fixture.world.events.clear();
        mc2_sim::mover::update_mech(&mut fixture.world, mech).unwrap();
        assert!(fixture.world.events.iter().any(|event| matches!(
            event,
            SimEvent::Sound { id: sound_id, at: Some(_), allow_dupes: false, .. } if *sound_id == id
        )), "helicopter power sample {id}");
    }
}

#[test]
fn engine_disabled_helicopter_transitions_to_destroyed_and_sounds_once() {
    let Some(mut fixture) = common::fixture("mc2_16", "pcg") else {
        return;
    };
    // mc2_16 hostile 707: a shipped air mover (`MoveType` AIR, `moveLevel` 2) through the production disable lifecycle.
    // The DESTROYED transition reads the type's move type (mech.cpp:6155), so a ground type with a forced move level
    // would stay disabled.
    let heli = fixture.mover_handles[&707];
    assert!(matches!(
        fixture.world.objects.type_of(heli).map(|t| &t.data),
        Some(ObjectTypeData::BattleMech(t)) if t.move_type == 1
    ));
    mc2_sim::combat::damage::disable(&mut fixture.world, heli, 2);
    fixture.world.events.clear();
    mc2_sim::mover::update_mech(&mut fixture.world, heli).unwrap();
    assert!(matches!(
        fixture.world.objects.get(heli).map(|o| o.c.status),
        Some(mc2_sim::object::ObjectStatus::Destroyed)
    ));
    assert_eq!(fixture.world.events.iter().filter(|event| matches!(
        event,
        SimEvent::Sound { id, at: Some(_), allow_dupes: false, .. } if *id == sfx::HELICOPTER_DEATH
    )).count(), 1);
    fixture.world.events.clear();
    mc2_sim::mover::update_mech(&mut fixture.world, heli).unwrap();
    assert!(!fixture.world.events.iter().any(|event| matches!(
        event,
        SimEvent::Sound { id, .. } if *id == sfx::HELICOPTER_DEATH
    )));
}
