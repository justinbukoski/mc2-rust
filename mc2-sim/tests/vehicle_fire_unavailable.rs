// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! A grouped shooter's successful shot alerts its pilot (group.cpp:1056-1064).

mod common;

use mc2_sim::ai::frame_unavailable;
use mc2_sim::object::Kind;

/// A grouped shooter's successful shot sends `PILOT_ALARM_MATE_FIRED_WEAPON` to its pilot (group.cpp:1056-1064).
#[test]
fn grouped_mech_shot_triggers_mate_fired_alarm_for_shooter() {
    use common::fire_dump::{find, mc2_01, recycle};
    use mc2_sim::ai::warrior::alarm;
    use mc2_sim::combat::fire::fire_weapon as full_fire;
    let Some(mut fx) = mc2_01() else { return };
    fx.w.mission.unlimited_ammo = true;
    fx.w.clock.scenario_time = 1000.0;
    fx.w.clock.frame_length = 1.0 / 30.0;
    let (att, tgt) = (find(&fx, 709), find(&fx, 706));
    // The compact fire fixture populates the mech roster but not the rebuilt global mover list.
    if !fx.w.objects.movers.contains(&att) {
        fx.w.objects.movers.push(att);
    }
    for (h, x, y, rot) in [(att, 0.0, 0.0, 90.0), (tgt, 100.0, 0.0, 270.0)] {
        let o = fx.w.objects.get_mut(h).unwrap();
        o.c.position.x = x;
        o.c.position.y = y;
        o.c.rotation = rot;
    }
    for node in ["1", "4", "5"] {
        recycle(&mut fx, &["recycle", "709", node, "0"]);
    }
    for group in [-1i8, 3] {
        fx.w.combat_unavailable_frame.clear();
        mc2_sim::combat::loadout_of_mut(fx.w.objects.get_mut(att).unwrap())
            .unwrap()
            .inventory[14]
            .ready_time = 0.0;
        if let Some(Kind::Mech(m)) = fx.w.objects.get_mut(att).map(|o| &mut o.kind) {
            m.m.group_id = group;
        }
        let r = full_fire(&mut fx.w, att, 14, Some(tgt), -1, None, 0);
        assert_eq!(r.code, 0, "the shot must proceed (group {group})");
        assert!(frame_unavailable(&fx.w).is_empty(), "group {group}");
        if group > -1 {
            let wid = fx.w.objects.watch_id(att).0 as i32;
            let shooter_slot = mc2_sim::combat::fx::pilot_slot(&fx.w, att).unwrap();
            assert_eq!(
                fx.w.pilots.get(shooter_slot).unwrap().alarm_history[alarm::MATE_FIRED_WEAPON]
                    .last(),
                Some(&wid)
            );
        }
        fx.w.clock.scenario_time += 1000.0;
        for node in ["1", "4", "5"] {
            recycle(&mut fx, &["recycle", "709", node, "0"]);
        }
    }
}
