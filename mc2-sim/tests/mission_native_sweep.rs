//! The first three shipped campaign missions run through the headless sim and do not report the
//! object/mission native ids this slice binds as unavailable.

use mc2_formats::vfs::Vfs;
use mc2_sim::ai::AiUnavailable;
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::rng::RngMode;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, FrameUnavailable, Sim};
use std::path::Path;
use std::rc::Rc;

const OBJECT_MISSION_IDS_EXPECTED_AVAILABLE: &[u16] = &[
    52, 55, 56, 59, 60, 62, 76, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 92, 93, 98, 99, 100,
    101, 104, 105, 114, 119, 120, 121, 122, 123, 124, 125, 126, 131, 134, 140, 207, 212, 213, 228,
];

fn run_mission_native(world: &mut mc2_sim::World, vars: &str, code: &str) -> i32 {
    let mut source = mc2_abl::MemorySource::new();
    source.insert(
        "native-test.abl",
        format!("module native_test : integer;\nvar\n{vars}\ncode\n{code}\nendmodule.\n"),
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        source,
        Box::new(mc2_abl::RngHost::new(world.rng.clone())),
    );
    let handle = env
        .pre_process("native-test.abl")
        .expect("compile native test");
    let instance = env.new_instance(handle).expect("native test instance");
    {
        let mut bindings = mc2_sim::mission::bindings::SimBindings::mission(world);
        env.execute(instance, &mut bindings)
            .expect("execute native test");
    }
    env.get_integer(instance)
}

#[test]
fn infantry_brain_attaches_during_mission_script() {
    let Some(root) = std::env::var_os("MC2_DATA").filter(|p| !p.is_empty()) else {
        return;
    };
    let mut vfs = Vfs::from_system_cfg(Path::new(&root)).unwrap();
    let loaded = load_world(
        &mut vfs,
        &LoadSpec::quickstart("mc2_01", 1, RngMode::LegacyMsvc),
    )
    .unwrap();
    let mut world = Sim::new(loaded).world;
    let mut source = mc2_sim::mission::abl_load::collect_sources(&vfs).unwrap();
    source.insert("data/missions/infantrytest.abl", "module infantrytest : integer; var real[3] p; code p[0] = 500.0; p[1] = 500.0; createInfantry(0, 0, p, 900); return(0); endmodule.");
    mc2_sim::mission::abl_load::init_env(&mut world, source).unwrap();
    mc2_sim::mission::abl_load::load_mission_script(&mut world, "infantrytest").unwrap();
    let count = world.objects.movers.len();
    mc2_sim::mission::bindings::run_mission_brain(&mut world);
    assert_eq!(world.objects.movers.len(), count + 1);
    let h = *world.objects.movers.last().unwrap();
    let wid = world.objects.watch_id(h);
    let pilot = world.pilots.slots.iter().find(|p| p.mover == wid).unwrap();
    assert!(pilot.brain.is_some());
    assert_eq!(pilot.memory.first(), Some(&900));
    let slot = world
        .pilots
        .slots
        .iter()
        .position(|p| p.mover == wid)
        .unwrap() as u16;
    mc2_sim::ai::brain::run_brain(&mut world, slot);
    assert!(!world
        .ai_unavailable_frame
        .iter()
        .any(|(_, what)| matches!(what, AiUnavailable::AblRuntimeError)));
}

#[test]
fn first_three_campaign_missions_do_not_report_object_native_ids_unavailable() {
    let Some(root) = std::env::var_os("MC2_DATA").filter(|p| !p.is_empty()) else {
        eprintln!("MC2_DATA not set; skipping campaign native sweep");
        return;
    };
    for mission in ["mc2_01", "mc2_02", "mc2_03"] {
        let mut vfs = Vfs::from_system_cfg(Path::new(&root)).expect("system.cfg");
        let loaded = load_world(
            &mut vfs,
            &LoadSpec::quickstart(mission, 1, RngMode::LegacyMsvc),
        )
        .unwrap_or_else(|e| panic!("{mission} loads: {e}"));
        assert!(loaded.complete, "{mission}: {:?}", loaded.missing_hooks);
        let mut sim = Sim::new(loaded);
        if mission == "mc2_01" {
            let group_target = sim.world.objects.movers.iter().find_map(|&h| {
                let object = sim.world.objects.get(h)?;
                let squad = match &object.kind {
                    mc2_sim::object::Kind::Mech(m) => m.m.squad_id,
                    mc2_sim::object::Kind::Vehicle(v) => v.m.squad_id,
                    _ => return None,
                };
                ((1..=511).contains(&squad)).then_some((squad, object.c.part_id.0))
            });
            let (group, target) = group_target.expect("mc2_01 has a mover group");
            let visible = mc2_sim::mission::natives_object::object_visible(
                &mut mc2_sim::mission::bindings::SimBindings::mission(&mut sim.world),
                group,
                target,
            );
            assert_eq!(
                visible, 1,
                "a group containing the target sees its own member"
            );
        }
        for frame in 1..=180 {
            let input = FrameInput::fixed(30, frame, Rc::new(AllVisible), Vec::new());
            let output = sim
                .step(input)
                .unwrap_or_else(|e| panic!("{mission} frame {frame}: {e}"));
            let unavailable: Vec<_> = output
                .unavailable
                .into_iter()
                .filter(|u| {
                    matches!(u, FrameUnavailable::Ai { what: AiUnavailable::Native { id }, .. }
                    if OBJECT_MISSION_IDS_EXPECTED_AVAILABLE.contains(id))
                })
                .collect();
            assert!(
                unavailable.is_empty(),
                "{mission} frame {frame}: {unavailable:?}"
            );
        }
        if mission == "mc2_01" {
            let h = sim.world.objects.movers[0];
            let object = sim.world.objects.get(h).unwrap();
            let part_id = object.c.part_id.0;
            let old_team = object.team_id();
            let new_team = if old_team == 0 { 1 } else { 0 };
            mc2_sim::mission::natives_object::change_sides(
                &mut mc2_sim::mission::bindings::SimBindings::mission(&mut sim.world),
                part_id,
                new_team,
            );
            assert_eq!(sim.world.objects.get(h).unwrap().team_id(), new_team);
            mc2_sim::mission::natives_object::change_sides(
                &mut mc2_sim::mission::bindings::SimBindings::mission(&mut sim.world),
                part_id,
                old_team,
            );

            let damage_target = sim
                .world
                .objects
                .list
                .iter()
                .flatten()
                .find_map(|o| {
                    (matches!(&o.kind, mc2_sim::object::Kind::Building(_))
                        && mc2_sim::object::structure::damage_level(&sim.world, o.c.handle) > 0.0)
                        .then_some(o.c.part_id.0)
                })
                .expect("mc2_01 contains a destructible building");
            let before = {
                let bindings = mc2_sim::mission::bindings::SimBindings::mission(&mut sim.world);
                mc2_sim::mission::natives_object::object_damage_points(&bindings, damage_target)
            };
            let call = format!("return(setObjectDamage({damage_target}, 100));");
            assert_eq!(run_mission_native(&mut sim.world, "", &call), 1);
            let after = {
                let bindings = mc2_sim::mission::bindings::SimBindings::mission(&mut sim.world);
                mc2_sim::mission::natives_object::object_damage_points(&bindings, damage_target)
            };
            assert!(
                after > before,
                "setObjectDamage changed no shipped structure damage"
            );

            let movers_before = sim.world.objects.movers.len();
            assert_eq!(
                run_mission_native(
                    &mut sim.world,
                    "real[3] position;",
                    "position[0] = 500.0; position[1] = 500.0; position[2] = 0.0; createInfantry(0, 0, position, 900); return(0);",
                ),
                0
            );
            assert_eq!(sim.world.objects.movers.len(), movers_before + 1);
            let infantry = *sim.world.objects.movers.last().unwrap();
            let infantry_watch_id = sim.world.objects.watch_id(infantry);
            assert_eq!(sim.world.objects.get(infantry).unwrap().team_id(), 0);
            let infantry_pilot = sim
                .world
                .pilots
                .slots
                .iter()
                .find(|pilot| pilot.mover == infantry_watch_id)
                .expect("createInfantry assigned a pilot");
            assert_eq!(infantry_pilot.memory.first(), Some(&900));
            assert!(infantry_pilot.brain.is_some());
        }
    }
}

/// A script-created infantry squad in a real mission, with an enemy in range and NO injected orders,
/// paths, targets or weapon outputs, moves and attacks through the production frame loop.
#[test]
fn script_created_infantry_moves_and_attacks_unaided() {
    let Some(root) = std::env::var_os("MC2_DATA").filter(|p| !p.is_empty()) else {
        return;
    };
    let mut vfs = Vfs::from_system_cfg(Path::new(&root)).unwrap();
    let loaded = load_world(
        &mut vfs,
        &LoadSpec::quickstart("mc2_01", 1, RngMode::LegacyMsvc),
    )
    .unwrap();
    let mut sim = Sim::new(loaded);
    let (epos, eteam) = sim
        .world
        .objects
        .movers
        .iter()
        .find_map(|&h| {
            let o = sim.world.objects.get(h)?;
            (o.team_id() != 0).then(|| (o.c.position, o.team_id()))
        })
        .expect("mission has an enemy mover");
    let my_team = if eteam == 0 { 1 } else { 0 };
    let script = sim.world.mission.script_name.clone();
    // Build a new environment, give every loaded pilot its shipped brain back (brains compile against
    // the real mission script), then make a script that spawns the infantry the mission script.
    let mut source = mc2_sim::mission::abl_load::collect_sources(&vfs).unwrap();
    source.insert(
        "data/missions/infantrytest.abl",
        format!(
            "module infantrytest : integer; var real[3] p; static integer done; code if (done == 0) then done = 1; p[0] = {}; p[1] = {}; createInfantry({my_team}, {my_team}, p, 900); endif; return(0); endmodule.",
            epos.x + 100.0,
            epos.y + 100.0
        ),
    );
    mc2_sim::mission::abl_load::init_env(&mut sim.world, source).unwrap();
    mc2_sim::mission::abl_load::load_mission_script(&mut sim.world, &script).unwrap();
    let rebrain: Vec<(usize, String)> = sim
        .world
        .pilots
        .slots
        .iter()
        .enumerate()
        .filter(|(_, p)| p.brain.is_some())
        .map(|(i, _)| (i, sim.world.mission.pilot_loads[i].brain_name.clone()))
        .collect();
    for (slot, name) in rebrain {
        mc2_sim::mission::abl_load::make_pilot_brain(&mut sim.world, slot, &name).unwrap();
    }
    mc2_sim::mission::abl_load::load_mission_script(&mut sim.world, "infantrytest").unwrap();
    let count = sim.world.objects.movers.len();
    mc2_sim::mission::bindings::run_mission_brain(&mut sim.world);
    assert_eq!(sim.world.objects.movers.len(), count + 1);
    let inf = *sim.world.objects.movers.last().unwrap();
    let start = sim.world.objects.get(inf).unwrap().c.position;
    let mut fired = false;
    for frame in 1..=900 {
        let input = FrameInput::fixed(30, frame, Rc::new(AllVisible), Vec::new());
        let out = sim.step(input).expect("frame");
        fired |= out.events.iter().any(|e| {
            matches!(e, mc2_sim::SimEvent::WeaponEffect { owner, .. } if sim.world.objects.by_watch_id(*owner) == Some(inf))
        });
    }
    let end = sim.world.objects.get(inf).map(|o| o.c.position);
    let moved = end.is_some_and(|e| {
        let (dx, dy) = (e.x - start.x, e.y - start.y);
        dx * dx + dy * dy > 1.0
    });
    eprintln!(
        "movers {} start {start:?} end {end:?}",
        sim.world.objects.movers.len()
    );
    assert!(moved, "infantry moved: {start:?} -> {end:?}");
    assert!(fired, "infantry fired a weapon");
}
