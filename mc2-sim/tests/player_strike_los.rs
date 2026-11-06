// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! A player LARGE strike through `Sim::step` on the shipped `mc2_01` mission: the point the home team can see stays a LARGE strike
//! and sounds the `MAPBUTTONS_GUI` click; a point the team cannot see becomes a SMALL (random-offset) strike and sounds nothing
//! (`IfaceCallStrike`, artlry.cpp:185, 199-218: `bRandom = Team::home->teamLineOfSight(strikeLoc, 0.0f) ? 0 : 1`, the click only for
//! a LARGE strike the team sees, `strikeID == ARTILLERY_LARGE && bRandom` remaps to SMALL). The strike type that reaches
//! `CallArtillery` is observable as the strike object's type (`strikeObjectId[]`, objmgr.cpp:2748: `BIG_ARTLRY` 249 for LARGE,
//! `SMALL_ARTLRY` 248 for SMALL, dobjnum.h:221-222).
//!
//! EVIDENCE RULE: the world is the production `load_world` of `mc2_01`; nothing is installed. The team's eyes are the real loaded
//! movers. The blocked point is derived from the world's own shipped data, not from the code under test: the distance to the farthest
//! home mover and the LOS range of every home mover are read here from the loaded world's state and type data, and the point is put
//! on the ray away from the farthest mover far enough that no home mover can reach it — `Team::teamLineOfSight` (team.cpp:762,
//! ported in `object::hooks::team_line_of_sight`) fails its distance gate for every roster member.

mod common;

use mc2_sim::combat::artillery::STRIKE_OBJECT_IDS;
use mc2_sim::combat::artillery_math::{ARTILLERY_LARGE, ARTILLERY_SMALL};
use mc2_sim::ids::ObjectHandle;
use mc2_sim::math::Vec3;
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::rng::RngMode;
use mc2_sim::sim::FrameInput;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{PlayerCommand, Sim, World};
use std::rc::Rc;

fn load(mission: &str) -> Option<Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let outcome = load_world(&mut vfs, &LoadSpec::quickstart(mission, 7, RngMode::Pcg32))
        .unwrap_or_else(|e| panic!("{mission}: {e}"));
    assert!(
        outcome.complete && outcome.missing_hooks.is_empty(),
        "{mission}: {:?}",
        outcome.missing_hooks
    );
    Some(Sim::new(outcome))
}

/// The home team's live movers, in roster order (the roster `Team::teamLineOfSight` walks, team.cpp:762-790).
fn home_movers(w: &World) -> Vec<ObjectHandle> {
    let mut out = Vec::new();
    for wid in &w.teams.teams[w.teams.home].as_ref().unwrap().roster {
        if let Some(h) = w.objects.by_watch_id(*wid) {
            let live = w.objects.get(h).is_some_and(|o| {
                o.exists()
                    && matches!(
                        &o.kind,
                        mc2_sim::object::Kind::Mech(_) | mc2_sim::object::Kind::Vehicle(_)
                    )
                    && !o.c.is_destroyed()
                    && !o.c.is_disabled()
            });
            if live {
                out.push(h);
            }
        }
    }
    out
}

/// One step with a single `CallStrike` command; returns the strike object the step created, if any, and the sound events.
fn strike_step(
    sim: &mut Sim,
    strike: i32,
    at: Vec3,
) -> (Option<ObjectHandle>, Vec<i32>, Vec<Option<Vec3>>) {
    let out = sim
        .step(FrameInput::fixed(
            30,
            1,
            Rc::new(AllVisible),
            vec![PlayerCommand::CallStrike { strike, at }],
        ))
        .expect("step");
    let click_at = out
        .events
        .iter()
        .filter_map(|e| match e {
            mc2_sim::SimEvent::Sound { at, .. } => Some(*at),
            _ => None,
        })
        .collect();
    let sounds = out
        .events
        .iter()
        .filter_map(|e| match e {
            mc2_sim::SimEvent::Sound { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    let created = sim.world.objects.artillery.iter().copied().find(|h| {
        sim.world.objects.get(*h).is_some_and(|o| {
            o.exists() && matches!(&o.kind, mc2_sim::object::Kind::Artillery(a) if !a.boom)
        })
    });
    (created, sounds, click_at)
}

/// A point the home team can see: on top of one of its own movers (the distance gate passes and the ray starts at the eye).
#[test]
fn a_seen_large_strike_stays_large_and_clicks() {
    let Some(mut sim) = load("mc2_01") else {
        return;
    };
    let eyes = home_movers(&sim.world);
    assert!(!eyes.is_empty(), "mc2_01 has home movers");
    let at = sim.world.objects.get(eyes[0]).unwrap().c.position;
    let (created, sounds, click_at) = strike_step(&mut sim, ARTILLERY_LARGE, at);
    let h = created.expect("a seen LARGE strike creates its strike object");
    let num = sim.world.objects.get(h).unwrap().c.type_handle;
    assert_eq!(
        num, STRIKE_OBJECT_IDS[ARTILLERY_LARGE as usize],
        "a LARGE strike the team sees stays LARGE (BIG_ARTLRY)"
    );
    assert!(
        sounds.contains(&mc2_sim::ai::MAPBUTTONS_GUI),
        "artlry.cpp:199-205: the click sounds for a LARGE strike the team sees"
    );
    assert!(
        click_at.iter().all(Option::is_none),
        "artlry.cpp:199-205: the click is non-positional (centered, distance independent)"
    );
}

/// A point no home mover can see: beyond every home mover's LOS range on the ray away from the farthest one. `teamLineOfSight`
/// fails for the whole roster, so the C++ remaps LARGE to SMALL (artlry.cpp:212-214) and plays nothing (the click gate at 199-205
/// tests the team sees the point).
#[test]
fn a_blind_large_strike_becomes_small_and_is_silent() {
    let Some(mut sim) = load("mc2_01") else {
        return;
    };
    let eyes = home_movers(&sim.world);
    assert!(!eyes.is_empty(), "mc2_01 has home movers");
    // The farthest home mover and the widest LOS range in the team, both read from the loaded world.
    let origin = eyes[0];
    let (mut far, mut far_d) = (origin, 0.0f32);
    let mut max_range = 0.0f32;
    for &e in &eyes {
        let p = sim.world.objects.get(e).unwrap().c.position;
        let d = {
            let o = sim.world.objects.get(origin).unwrap().c.position;
            let (dx, dy, dz) = (p.x - o.x, p.y - o.y, p.z - o.z);
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        if d > far_d {
            far = e;
            far_d = d;
        }
        if let Some(r) = mc2_sim::object::hooks::visual_range(&sim.world, e) {
            max_range = max_range.max(r);
        }
    }
    assert!(max_range > 0.0, "home movers have a visual range");
    let op = sim.world.objects.get(origin).unwrap().c.position;
    let fp = sim.world.objects.get(far).unwrap().c.position;
    let (dx, dy) = (fp.x - op.x, fp.y - op.y);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    // Push out along the ray until the point is farther from EVERY home mover than the widest LOS range in the team.
    let mut at = Vec3 {
        x: fp.x + dx / len * (max_range + far_d + 100.0),
        y: fp.y + dy / len * (max_range + far_d + 100.0),
        z: 0.0,
    };
    at.z = sim.world.terrain.elevation(&at);
    for &e in &eyes {
        let p = sim.world.objects.get(e).unwrap().c.position;
        let (ex, ey, ez) = (at.x - p.x, at.y - p.y, at.z - p.z);
        let d = (ex * ex + ey * ey + ez * ez).sqrt();
        let r = mc2_sim::object::hooks::visual_range(&sim.world, e).unwrap_or(0.0);
        assert!(
            d > r,
            "the chosen point must be out of range of every home mover ({d} vs {r})"
        );
    }
    let (created, sounds, _) = strike_step(&mut sim, ARTILLERY_LARGE, at);
    let h = created.expect("a blind LARGE strike still creates a strike object (as SMALL)");
    let num = sim.world.objects.get(h).unwrap().c.type_handle;
    assert_eq!(
        num, STRIKE_OBJECT_IDS[ARTILLERY_SMALL as usize],
        "artlry.cpp:212-214: a LARGE strike the team cannot see becomes SMALL (SMALL_ARTLRY)"
    );
    assert!(
        !sounds.contains(&mc2_sim::ai::MAPBUTTONS_GUI),
        "artlry.cpp:199-205: no click when the team cannot see the point"
    );
}

/// A shipped in-map point inside an observer's visual range but hidden by the terrain ray.
#[test]
fn terrain_obstructed_large_strike_becomes_small_and_is_silent() {
    let Some(mut sim) = load("mc2_01") else {
        return;
    };
    let eyes = home_movers(&sim.world);
    let origin = sim.world.objects.get(eyes[0]).unwrap().c.position;
    let team = sim.world.teams.home as i32;
    let mut target = None;
    'search: for row in -24..=24 {
        for col in -24..=24 {
            let mut at = Vec3 {
                x: origin.x + col as f32 * 128.0,
                y: origin.y + row as f32 * 128.0,
                z: 0.0,
            };
            if !sim.world.terrain.is_game_select_position(&at) {
                continue;
            }
            at.z = sim.world.terrain.elevation(&at);
            let mut obstructed_in_range = false;
            for &eye in &eyes {
                let p = sim.world.objects.get(eye).unwrap().c.position;
                let d = mc2_sim::math::approximate_length(&Vec3 {
                    x: at.x - p.x,
                    y: at.y - p.y,
                    z: at.z - p.z,
                });
                if d <= mc2_sim::object::hooks::visual_range(&sim.world, eye).unwrap_or(0.0) {
                    let from = mc2_sim::object::hooks::los_position(&mut sim.world, eye);
                    if !mc2_sim::ai::sensors::line_of_sight_ray(
                        &sim.world, from, at, team, 0.0, 0.0,
                    ) {
                        obstructed_in_range = true;
                    }
                }
            }
            if obstructed_in_range
                && !mc2_sim::object::hooks::team_line_of_sight(&mut sim.world, team, at, 0.0)
            {
                target = Some(at);
                break 'search;
            }
        }
    }
    let at = target.expect("shipped terrain has an in-range obstructed point");
    let (created, sounds, _) = strike_step(&mut sim, ARTILLERY_LARGE, at);
    let strike = created.expect("blind strike is created");
    assert_eq!(
        sim.world.objects.get(strike).unwrap().c.type_handle,
        STRIKE_OBJECT_IDS[ARTILLERY_SMALL as usize]
    );
    assert!(!sounds.contains(&mc2_sim::ai::MAPBUTTONS_GUI));
}
