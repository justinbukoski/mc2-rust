// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The ABL native `getfixed` (`execGetFixed`, ablmc2.cpp:4487) over the shipped `mc2_07` repair bay (part 69846, an enemy-team bay
//! with refit points) and the mech of part 512, plus the shipped library function `GetRepaired` (orders.abx:2492) driving it, and
//! `orderdeployelementals`, a silent no-op in the shipped game (its C++ body is under `#ifdef USE_ELEMENTALS`).
mod common;

use mc2_abl::{Config, Env, InstId, MemorySource, RngHost};
use mc2_sim::ai::AiUnavailable;
use mc2_sim::combat::{loadout_of_mut, HOME_COMMANDER};
use mc2_sim::command::OrderCode;
use mc2_sim::ids::{CommanderId, ObjectHandle, PartId, WatchId};
use mc2_sim::math::Vec3;
use mc2_sim::mission::bindings::SimBindings;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::{flags, Kind};
use mc2_sim::{FrameInput, Sim, SimEvent};
use std::rc::Rc;

const BAY: i32 = 69846;
const MECH: i32 = 512;

fn load() -> Option<(Sim, MemorySource)> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let sources = mc2_sim::mission::abl_load::collect_sources(&vfs).expect("abl sources");
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_07", seed, mode))
        .expect("mc2_07 loads");
    assert!(outcome.complete, "{:?}", outcome.missing_hooks);
    Some((Sim::new(outcome), sources))
}

fn handle(sim: &Sim, part: i32) -> ObjectHandle {
    sim.world
        .objects
        .find_by_part_id(PartId(part))
        .unwrap_or_else(|| panic!("part {part}"))
}

fn wid(sim: &mut Sim, h: ObjectHandle) -> WatchId {
    sim.world.objects.watch_id(h)
}

fn set_bay_team(sim: &mut Sim, team: i8) {
    let bay = handle(sim, BAY);
    match &mut sim.world.objects.get_mut(bay).unwrap().kind {
        Kind::Building(b) => b.team_id = team,
        _ => panic!("the bay is a building"),
    }
}

fn set_refit_buddy(sim: &mut Sim, h: ObjectHandle, to: WatchId) {
    match &mut sim.world.objects.get_mut(h).unwrap().kind {
        Kind::Mech(m) => m.m.refit_buddy = to,
        Kind::Vehicle(v) => v.m.refit_buddy = to,
        Kind::Building(b) => b.refit_buddy = to,
        _ => panic!("no refit buddy"),
    }
}

fn link(sim: &mut Sim, h: ObjectHandle, to: ObjectHandle) {
    let w = wid(sim, to);
    set_refit_buddy(sim, h, w);
}

fn damage_armor(sim: &mut Sim, mech: ObjectHandle, to_fraction: f32) {
    let l = loadout_of_mut(sim.world.objects.get_mut(mech).unwrap()).unwrap();
    for a in l.armor.iter_mut() {
        a.0 = a.1 as f32 * to_fraction;
    }
}

/// Runs `body` as a module's code with the shipped libraries loaded, CurObject / CurWarrior the pilot of `me`.
fn run_script(sim: &mut Sim, sources: &MemorySource, me: ObjectHandle, code: &str) -> i32 {
    let mut src = sources.clone();
    src.insert(
        "t.abl",
        if code.contains("code\n") {
            format!("module t : integer;\n{code}\nendmodule.\n")
        } else {
            format!("module t : integer;\ncode\n{code}\nendmodule.\n")
        },
    );
    let mut env = Env::new(
        Config::default(),
        src,
        Box::new(RngHost::new(sim.world.rng.clone())),
    );
    for lib in mc2_sim::mission::abl_load::LIBRARIES {
        env.load_library(lib).expect("library");
    }
    let h = env.pre_process("t.abl").expect("compile");
    let inst: InstId = env.new_instance(h).expect("instance");
    let slot = mc2_sim::ai::warrior::slot_of(&sim.world, me).expect("pilot");
    // the fresh environment replaces the mission's: no brain of the loaded mission may run in it
    for p in sim.world.pilots.slots.iter_mut() {
        p.brain = None;
    }
    sim.world.mission.brain = None;
    sim.world.mission.brain_callback = None;
    {
        let mut b = SimBindings::pilot(&mut sim.world, slot);
        env.execute(inst, &mut b).expect("run");
    }
    let result = env.get_integer(inst);
    sim.world.abl = Some(env);
    result
}

fn get_fixed(sim: &mut Sim, src: &MemorySource, fixee: i32, bay: i32) -> i32 {
    let me = handle(sim, MECH);
    run_script(
        sim,
        src,
        me,
        &format!("return(getFixed({fixee}, {bay}, 0));"),
    )
}

fn tac_order_code(sim: &Sim, h: ObjectHandle) -> OrderCode {
    let slot = mc2_sim::ai::warrior::slot_of(&sim.world, h).unwrap();
    sim.world.pilots.get(slot).unwrap().tac_orders[0].code
}

#[test]
fn getfixed_returns_the_cpp_codes_in_the_cpp_order() {
    let Some((mut sim, src)) = load() else {
        return;
    };
    let mech = handle(&sim, MECH);
    let bay = handle(&sim, BAY);
    let home = sim.world.teams.home as i8;
    assert!(sim.world.objects.get(bay).unwrap().c.flag(flags::MECHBAY));
    assert!(sim.world.objects.get(bay).unwrap().c.flag(flags::CANREFIT));
    // a second mover to stand in as "something else"
    let other = sim
        .world
        .objects
        .list
        .iter()
        .flatten()
        .find(|o| matches!(o.kind, Kind::Mech(_)) && o.c.part_id.0 != MECH)
        .map(|o| o.c.part_id.0)
        .expect("another mech");
    let other = handle(&sim, other);

    // 8: the bay is not a repair bay (a mech; a missing object)
    assert_eq!(get_fixed(&mut sim, &src, MECH, MECH), 8);
    assert_eq!(get_fixed(&mut sim, &src, MECH, 987_654), 8);
    // 7: the fixee is not a mech or a vehicle (the bay itself; a missing object)
    assert_eq!(get_fixed(&mut sim, &src, BAY, BAY), 7);
    assert_eq!(get_fixed(&mut sim, &src, 987_654, BAY), 7);
    // 9: the shipped bay is the enemy's; this check comes before the buddy and bay-kind checks
    assert_ne!(
        sim.world.objects.get(bay).unwrap().team_id(),
        i32::from(home)
    );
    link(&mut sim, bay, other);
    assert_eq!(get_fixed(&mut sim, &src, MECH, BAY), 9);
    set_refit_buddy(&mut sim, bay, WatchId::NONE);
    set_bay_team(&mut sim, home);
    // an undamaged mech: 6, and no order is issued
    assert_eq!(get_fixed(&mut sim, &src, MECH, BAY), 6);
    assert_eq!(tac_order_code(&sim, mech), OrderCode::None);
    // 4 / 3: the bay is already repairing something else / this fixee
    link(&mut sim, bay, other);
    assert_eq!(get_fixed(&mut sim, &src, MECH, BAY), 4);
    link(&mut sim, bay, mech);
    assert_eq!(get_fixed(&mut sim, &src, MECH, BAY), 3);
    set_refit_buddy(&mut sim, bay, WatchId::NONE);
    // 2: a bay that is not a mech bay
    sim.world
        .objects
        .get_mut(bay)
        .unwrap()
        .c
        .set_flag(flags::MECHBAY, false);
    assert_eq!(get_fixed(&mut sim, &src, MECH, BAY), 2);
    sim.world
        .objects
        .get_mut(bay)
        .unwrap()
        .c
        .set_flag(flags::MECHBAY, true);
    // 5: the fixee is already being repaired by something else
    damage_armor(&mut sim, mech, 0.2);
    link(&mut sim, mech, other);
    assert_eq!(get_fixed(&mut sim, &src, MECH, BAY), 5);
    set_refit_buddy(&mut sim, mech, WatchId::NONE);
    // 0: a damaged mech is sent to its own side's free bay: the pilot has the GetFixed order
    assert_eq!(get_fixed(&mut sim, &src, MECH, BAY), 0);
    assert_eq!(tac_order_code(&sim, mech), OrderCode::GetFixed);
    let slot = mc2_sim::ai::warrior::slot_of(&sim.world, mech).unwrap();
    let bay_wid = wid(&mut sim, bay);
    let order = &sim.world.pilots.get(slot).unwrap().tac_orders[0];
    assert_eq!(order.target, bay_wid);
    assert_eq!(order.selection_index, -1);
    // 1: the bay is out of refit points
    let level = mc2_sim::object::structure::damage_level(&sim.world, bay);
    sim.world
        .objects
        .get_mut(bay)
        .unwrap()
        .structure_mut()
        .unwrap()
        .damage = level;
    assert_eq!(get_fixed(&mut sim, &src, MECH, BAY), 1);
    // -1: the order cannot be given, directly on the native: the mech's pilot slot is gone
    sim.world
        .objects
        .get_mut(bay)
        .unwrap()
        .structure_mut()
        .unwrap()
        .damage = 0.0;
    match &mut sim.world.objects.get_mut(mech).unwrap().kind {
        Kind::Mech(m) => m.m.pilot_slot = 0,
        _ => unreachable!(),
    }
    let mut b = SimBindings::mission(&mut sim.world);
    assert_eq!(mc2_sim::ai::natives::get_fixed(&mut b, MECH, BAY, 0), -1);
}

/// The shipped `GetRepaired` (orders.abx:2492, called from `MoveTo` and `Move`): a badly damaged mech next to a free bay of its own
/// side calls `GetFixed(Me, bay, 0)`, takes the 0, and the mech drives into the bay and is linked to it.
#[test]
fn get_repaired_from_orders_abx_sends_a_damaged_mech_to_the_bay() {
    let Some((mut sim, src)) = load() else {
        return;
    };
    let mech = handle(&sim, MECH);
    let bay = handle(&sim, BAY);
    let home = sim.world.teams.home as i8;
    set_bay_team(&mut sim, home);
    match &mut sim.world.objects.get_mut(mech).unwrap().kind {
        Kind::Mech(m) => m.m.commander = CommanderId(HOME_COMMANDER as i8),
        _ => unreachable!(),
    }
    damage_armor(&mut sim, mech, 0.3);
    let bp = sim.world.objects.get(bay).unwrap().c.position;
    let flat = Vec3 {
        x: bp.x - 30.0,
        y: bp.y,
        z: 0.0,
    };
    let at = Vec3 {
        z: sim.world.terrain.elevation(&flat),
        ..flat
    };
    let o = sim.world.objects.get_mut(mech).unwrap();
    o.c.position = at;
    o.c.set_awake(true);
    let got = run_script(
        &mut sim,
        &src,
        mech,
        &format!(
            "initOrders;\n setIntegerMemory(I_PERMIT_REPAIR, YES);\n TotalMechBays = 1;\n MechBays[0] = {BAY};\n if (GetRepaired) then return(1); else return(7); endif;"
        ),
    );
    assert_eq!(got, 1, "GetRepaired took the bay");
    assert_eq!(tac_order_code(&sim, mech), OrderCode::GetFixed);
    for f in 1..=(30u64 * 10) {
        sim.step(FrameInput::fixed(
            30,
            f,
            Rc::new(mc2_sim::visibility::AllVisible),
            Vec::new(),
        ))
        .expect("step");
    }
    // the shipped bay sits in a walled compound: the mech drives the planned path (toward its entrance) to the bay's position
    let moved = distance_from(&sim, mech, at);
    assert!(moved > 100.0, "the mech drove off along its path ({moved})");
    assert_eq!(tac_order_code(&sim, mech), OrderCode::GetFixed);
    let slot = mc2_sim::ai::warrior::slot_of(&sim.world, mech).unwrap();
    let goal = sim
        .world
        .pilots
        .get(slot)
        .unwrap()
        .cur_order
        .move_params
        .way_path
        .points[0];
    assert_eq!((goal.x, goal.y), (bp.x, bp.y), "the move goal is the bay");
}

fn distance_from(sim: &Sim, h: ObjectHandle, from: Vec3) -> f32 {
    let p = sim.world.objects.get(h).unwrap().c.position;
    (p.x - from.x).hypot(p.y - from.y)
}

/// `orderdeployelementals`: no shipped `USE_ELEMENTALS`, so the C++ pops its argument and nothing else; no unavailable report.
#[test]
fn order_deploy_elementals_is_a_silent_no_op() {
    let Some((mut sim, src)) = load() else {
        return;
    };
    let mech = handle(&sim, MECH);
    let id = mc2_abl::natives::NATIVES
        .iter()
        .position(|n| n.name == "orderdeployelementals")
        .expect("native") as u16;
    let got = run_script(
        &mut sim,
        &src,
        mech,
        "orderDeployElementals(0);\n return(41);",
    );
    assert_eq!(got, 41, "the script went on past the call");
    assert!(
        sim.world.ai_native_calls_frame.iter().any(|c| c.1 == id),
        "the call was made"
    );
    assert!(!sim
        .world
        .ai_unavailable_frame
        .contains(&(MECH, AiUnavailable::Native { id })));
    assert!(!sim.world.events.iter().any(|e| matches!(
        e,
        SimEvent::AiUnavailable {
            what: AiUnavailable::Native { id: i },
            ..
        } if *i == id
    )));
}
