// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Indirect turret fire uses the team line of sight (`Turret::fireWeapon`, turret.cpp:1386-1389):
//! `LOS = lineOfSight(target); if (!LOS && indirect) LOS = getTeam()->teamLineOfSight(target->getPosition(), target->getAppearRadius())`.

mod common;

use mc2_sim::combat::turret;
use mc2_sim::combat::turret_math::LosOutcome;
use mc2_sim::math::Vec3;
use mc2_sim::object::{hooks, Kind};
use mc2_sim_data::objtype::ObjectTypeData;
use std::rc::Rc;

/// The recycle clock of weapon 0: `ready` zeroes it and a made shot (`Turret::fireWeapon`) starts the recycle.
fn recycle_started(w: &mc2_sim::World, h: mc2_sim::ids::ObjectHandle) -> bool {
    matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Turret(ts)) if ts.ready_time[0] > 0.0)
}

/// A turret's indirect weapon fires at a target it cannot see when a team member sees it, and does not when no team member does;
/// a direct weapon never asks the team.
#[test]
fn indirect_turret_fires_on_what_its_team_sees() {
    let Some(mut fx) = common::fixture("mc2_05", "msvc") else {
        return;
    };
    let w = &mut fx.world;
    let (turret_h, weapon) = w
        .objects
        .turrets
        .iter()
        .copied()
        .find_map(|h| {
            if !w.objects.get(h)?.exists() {
                return None;
            }
            match &w.objects.type_of(h)?.data {
                ObjectTypeData::Turret(t) if t.weapon_master_id[0] != -1 => {
                    Some((h, t.weapon_master_id[0]))
                }
                _ => None,
            }
        })
        .expect("mc2_05 has a turret with a weapon");
    let movers: Vec<_> = w
        .objects
        .movers
        .iter()
        .copied()
        .filter(|&h| matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(_))))
        .collect();
    let (target, spotter) = (movers[0], movers[1]);
    let team = w.objects.get(turret_h).unwrap().team_id();
    let team_idx = usize::try_from(team).expect("turret team");
    // The spotter belongs to the turret's team and stands beside the target, inside the weapon's range but out of the turret's own sight.
    if let Some(Kind::Mech(m)) = w.objects.get_mut(spotter).map(|o| &mut o.kind) {
        m.m.team = mc2_sim::ids::TeamId(team as i8);
    }
    let tpos = w.objects.get(turret_h).unwrap().c.position;
    // The nearest spot along +x, beyond 45 m (the weapon's minimum range), that the turret itself does not see.
    let radius = hooks::appear_radius(w, turret_h);
    let mut found = false;
    for step in 45..400 {
        let far = Vec3 {
            x: tpos.x + 10.0 * step as f32,
            ..tpos
        };
        for (h, dx) in [(target, 0.0), (spotter, 20.0)] {
            w.objects.get_mut(h).unwrap().c.position = Vec3 {
                x: far.x + dx,
                ..far
            };
        }
        if !hooks::line_of_sight(w, turret_h, target, radius) {
            found = true;
            break;
        }
    }
    assert!(found, "a spot the turret does not see");
    assert!(
        !hooks::line_of_sight(w, turret_h, target, hooks::appear_radius(w, turret_h)),
        "the turret itself cannot see the target"
    );
    let spotter_watch = w.objects.watch_id_peek(spotter);

    // Make the turret's weapon indirect (fixture input).
    let mut gs = (**w.data.gamesys.tables.as_ref().expect("tables")).clone();
    gs.mover_general.indirect_fire_weapons[0] = weapon;
    w.data.gamesys.tables = Some(Rc::new(gs));
    let ready = |w: &mut mc2_sim::World| {
        if let Some(Kind::Turret(ts)) = w.objects.get_mut(turret_h).map(|o| &mut o.kind) {
            ts.ready_time = [0.0; 4];
        }
    };

    // No team member sees the target: no shot.
    w.teams.teams[team_idx].as_mut().unwrap().roster.clear();
    ready(w);
    assert_eq!(
        turret::fire(w, turret_h, target, 0),
        LosOutcome::Blocked,
        "nobody on the team sees the target"
    );
    assert!(!recycle_started(w, turret_h), "no shot was made");

    // A team member sees it: the indirect weapon fires.
    w.teams.teams[team_idx]
        .as_mut()
        .unwrap()
        .roster
        .push(spotter_watch);
    assert!(hooks::team_line_of_sight(
        w,
        team,
        w.objects.get(target).unwrap().c.position,
        hooks::appear_radius(w, target)
    ));
    ready(w);
    assert_eq!(turret::fire(w, turret_h, target, 0), LosOutcome::Clear);
    assert!(recycle_started(w, turret_h), "the indirect shot was made");

    // A direct weapon does not fall back on the team.
    let mut gs = (**w.data.gamesys.tables.as_ref().unwrap()).clone();
    gs.mover_general.indirect_fire_weapons[0] = -1;
    w.data.gamesys.tables = Some(Rc::new(gs));
    ready(w);
    assert_eq!(turret::fire(w, turret_h, target, 0), LosOutcome::Blocked);
    assert!(!recycle_started(w, turret_h));
}
