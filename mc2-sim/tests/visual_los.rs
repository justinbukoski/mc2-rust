// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Visual line of sight for sensor contacts (`GameObject::lineOfSight`, code/gameobj.cpp:1846-1960, driven by
//! `SensorSystem::calcContactStatus`, code/contact.cpp:260-340) on the shipped `mc2_01` mission.
//!
//! Every expected value below is read from the shipped data in the test itself (the raw `.fit` text, not the typed loaders under
//! test) or derived by plain geometry; none is taken from the code under test.

mod common;

use mc2_sim::ids::ObjectHandle;
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::rng::RngMode;
use mc2_sim::sim::FrameInput;
use mc2_sim::{Sim, World};
use std::rc::Rc;

fn load(mission: &str) -> Option<(Sim, mc2_formats::vfs::Vfs)> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let outcome = load_world(&mut vfs, &LoadSpec::quickstart(mission, 7, RngMode::Pcg32))
        .unwrap_or_else(|e| panic!("{mission}: {e}"));
    assert!(outcome.complete && outcome.missing_hooks.is_empty());
    Some((Sim::new(outcome), vfs))
}

fn step(sim: &mut Sim, frame: u64) {
    sim.step(FrameInput::fixed(
        30,
        frame,
        Rc::new(mc2_sim::visibility::AllVisible),
        Vec::new(),
    ))
    .expect("step");
}

fn mover(w: &World, part: i32) -> ObjectHandle {
    *w.objects
        .movers
        .iter()
        .find(|h| w.objects.get(**h).unwrap().c.part_id.0 == part)
        .unwrap_or_else(|| panic!("no mover with part id {part}"))
}

/// The value after `key =` in the raw text of the first `[block]` that contains it (whitespace and `//` comments stripped).
fn raw_value(text: &str, block: &str, key: &str) -> f32 {
    let mut in_block = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_block = line.trim_end_matches('\r') == format!("[{block}]");
            continue;
        }
        if !in_block {
            continue;
        }
        let words: Vec<&str> = line
            .split(|c: char| c.is_whitespace() || c == '=')
            .filter(|s| !s.is_empty())
            .collect();
        if let Some(i) = words.iter().position(|w| *w == key) {
            return words[i + 1].parse().unwrap();
        }
    }
    panic!("[{block}] {key} not found");
}

/// The 256 entries of `VisualRangeTable` read from the raw `gamesys.fit` text.
fn raw_visual_range_table(text: &str) -> Vec<i32> {
    let start = text.find("VisualRangeTable").expect("VisualRangeTable");
    let body = &text[start + "VisualRangeTable".len()..];
    let body = &body[body.find('=').unwrap() + 1..body.find('[').unwrap()];
    body.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect()
}

/// Float-intermediate tolerance (FIDELITY RULE): the C++ and Rust may differ in the last bits of a product.
fn close(a: f32, b: f32) {
    assert!((a - b).abs() <= 1e-3 * b.abs().max(1.0), "{a} vs {b}");
}

const WORLD_UNITS_PER_METER: f32 = 5.01; // terrain.cpp worldUnitsPerMeter

/// The altitude scale: `Terrain::userMin/userMax` and `MapData::waterDepth` from the mission `.fit`, and the 256-entry
/// `visualRangeTable` from `gamesys.fit` (gameobj.cpp:1860-1881).
#[test]
fn altitude_scale_and_range_table_are_the_shipped_values() {
    let Some((sim, mut vfs)) = load("mc2_01") else {
        return;
    };
    let mission = String::from_utf8_lossy(&vfs.read("data/missions/mc2_01.fit").unwrap().unwrap())
        .to_string();
    let user_min = raw_value(&mission, "Terrain", "UserMin");
    let user_max = raw_value(&mission, "Terrain", "UserMax");
    let water = raw_value(&mission, "Water", "Elevation");
    assert_eq!(
        (user_min, user_max, water),
        (0.0, 500.0, 350.0),
        "the file itself"
    );
    assert_eq!(
        mc2_sim::object::hooks::altitude_range(&sim.world),
        (user_min, user_max, water)
    );
    let gamesys = String::from_utf8_lossy(&vfs.read("data/missions/gamesys.fit").unwrap().unwrap())
        .to_string();
    let table = raw_visual_range_table(&gamesys);
    assert_eq!(table.len(), 256);
    assert_eq!(sim.world.data.gamesys.visual_range_table, table);
}

/// `visual range = table[altitude level] * (1 + 0.2 scout) * LOSFactor * 25 * worldUnitsPerMeter` for a mover.
#[test]
fn visual_range_follows_the_table_scout_skill_and_los_factor() {
    let Some((mut sim, mut vfs)) = load("mc2_01") else {
        return;
    };
    for n in 1..=3 {
        step(&mut sim, n);
    }
    let table = raw_visual_range_table(&String::from_utf8_lossy(
        &vfs.read("data/missions/gamesys.fit").unwrap().unwrap(),
    ));
    let me = mover(&sim.world, 512);
    let z = sim.world.objects.get(me).unwrap().c.position.z;
    // altitude above max(waterDepth, userMin) = 350, in 1/256ths of (userMax - 350)
    let level = (((z - 350.0) / ((500.0 - 350.0) * 0.003_906_25)) as i64).clamp(0, 255) as usize;
    let base = table[level] as f32 * 25.0 * WORLD_UNITS_PER_METER;
    let range = |w: &World, h| mc2_sim::object::hooks::visual_range(w, h).unwrap();
    // mech 512's profile sets no LOSFactor (default 1.0): raw packet text has none
    let ty = sim.world.objects.type_of(me).unwrap();
    assert!(ty
        .raw_blocks
        .iter()
        .all(|b| b.lines.iter().all(|l| !l.contains("LOSFactor"))));
    assert_eq!(range(&sim.world, me), base);

    // scouting specialty (warrior.h:1210: specialtySkills[12] == 1, SCOUT): +20%
    let slot = match &sim.world.objects.get(me).unwrap().kind {
        mc2_sim::object::Kind::Mech(m) => m.m.pilot_slot,
        k => panic!("part 512 is not a mech: {:?}", std::mem::discriminant(k)),
    };
    assert!(!sim.world.pilots.get(slot).unwrap().specialty[12]);
    sim.world.pilots.get_mut(slot).unwrap().specialty[12] = true;
    close(range(&sim.world, me), base + base * 0.2);

    // a mech type whose profile sets LOSFactor 1.2 (type 393, read from the raw packet text)
    let (num, raw) = {
        let types = sim.world.objects.types.as_ref().unwrap();
        let t = types.get(393).unwrap();
        let line = t
            .raw_blocks
            .iter()
            .find(|b| b.id == "MechProfile")
            .and_then(|b| b.lines.iter().find(|l| l.contains("LOSFactor")))
            .unwrap();
        (
            393,
            raw_value(
                &format!("[MechProfile]\n{line}\n"),
                "MechProfile",
                "LOSFactor",
            ),
        )
    };
    assert_eq!(raw, 1.2);
    sim.world.objects.get_mut(me).unwrap().c.type_handle = num;
    close(range(&sim.world, me), (base + base * 0.2) * 1.2);
}

/// `Mover::getLOSPosition` (mover.cpp:3459-3513): the mover's position raised by its lowest weapon node above the terrain; and the
/// target's `getAppearRadius`: 0 for movers (gameobj.h:473, `Mover` does not override it), the shape radius for buildings.
#[test]
fn mover_los_position_and_appearance_radius() {
    let Some((mut sim, _)) = load("mc2_01") else {
        return;
    };
    for n in 1..=3 {
        step(&mut sim, n);
    }
    // two mechs (512, 513) and two ground vehicles (704, 715): each one's eye is its position raised by its lowest weapon node
    for part in [512, 513, 704, 715] {
        let h = mover(&sim.world, part);
        let pos = sim.world.objects.get(h).unwrap().c.position;
        let los = mc2_sim::object::hooks::los_position(&mut sim.world, h);
        assert_eq!((los.x, los.y), (pos.x, pos.y));
        // the C++ marks the appearance in view while it queries the node (mover.cpp:3465-3466)
        let was_in_view = sim.world.appearance.in_view(h);
        sim.world.appearance.set_in_view(h, true);
        let node = sim.world.appearance.lowest_weapon_node(h);
        assert!(node >= 0, "part {part}: the appearance has weapon nodes");
        let np = sim.world.appearance.weapon_node_position(h, node);
        let height = np.z - sim.world.terrain.elevation(&np);
        sim.world.appearance.set_in_view(h, was_in_view);
        assert!(height > 0.0 && height < 60.0, "part {part}: {height}");
        close(los.z, pos.z + height);
        assert_eq!(mc2_sim::object::hooks::appear_radius(&sim.world, h), 0.0);
    }
    // a building target is seen with its shape radius
    let building = (1..sim.world.objects.list.len() as u32)
        .map(ObjectHandle)
        .find(|h| {
            sim.world.objects.get(*h).is_some_and(|o| {
                matches!(o.kind, mc2_sim::object::Kind::Building(_))
                    && sim.world.appearance.radius(*h) > 0.0
            })
        })
        .expect("a building with a shape radius");
    assert_eq!(
        mc2_sim::object::hooks::appear_radius(&sim.world, building),
        sim.world.appearance.radius(building)
    );
}

/// Mech 512 (player team 0) is put `offset` away from the enemy mech `TARGET` (part 709, team 1, at (-2965.3, 533.3) in the open
/// west part of mc2_01; a mech so that its own weapon-node height is the shipped appearance's) and the mission is stepped.
/// Returns the first frame at which the player team holds a status-5 (visual) contact on the target, and the final team status.
const TARGET: i32 = 709;
const FRAME_BUDGET: u64 = 20;

fn acquire(offset: (f32, f32)) -> (Sim, ObjectHandle, ObjectHandle, Option<u64>) {
    acquire_part(TARGET, offset)
}

/// [`acquire`] for any enemy part.
fn acquire_part(part: i32, offset: (f32, f32)) -> (Sim, ObjectHandle, ObjectHandle, Option<u64>) {
    let (mut sim, _) = load("mc2_01").expect("MC2_DATA");
    let me = mover(&sim.world, 512);
    let tgt = mover(&sim.world, part);
    let mut p = sim.world.objects.get(tgt).unwrap().c.position;
    p.x += offset.0;
    p.y += offset.1;
    p.z = sim.world.terrain.elevation(&p);
    mc2_sim::object::set_position(&mut sim.world, me, p, true);
    let mut first = None;
    for n in 1..=FRAME_BUDGET {
        step(&mut sim, n);
        let status = sim
            .world
            .sensors
            .team_contact_status
            .get(&tgt)
            .map(|s| s[0]);
        if status == Some(5) && first.is_none() {
            first = Some(n);
        }
    }
    (sim, me, tgt, first)
}

/// Terrain height above the straight eye-to-eye line, maximised over the segment: positive means terrain sits between the two
/// (plain geometry, independent of the ray marcher under test).
fn terrain_clearance_deficit(sim: &mut Sim, a: ObjectHandle, b: ObjectHandle) -> f32 {
    let pa = mc2_sim::object::hooks::los_position(&mut sim.world, a);
    let pb = mc2_sim::object::hooks::los_position(&mut sim.world, b);
    let mut worst = f32::MIN;
    for i in 1..200 {
        let t = i as f32 / 200.0;
        let mut p = pa;
        p.x += (pb.x - pa.x) * t;
        p.y += (pb.y - pa.y) * t;
        let line_z = pa.z + (pb.z - pa.z) * t;
        worst = worst.max(sim.world.terrain.elevation(&p) - line_z);
    }
    worst
}

/// A mech in the open and in range becomes a visual contact within the frame budget; the same mech behind a ridge at the same
/// distance, and the same mech in the open but beyond visual range, do not. (Both still show as plain sensor contacts, status 1.)
#[test]
fn player_team_sees_the_enemy_in_the_open_but_not_behind_terrain_or_out_of_range() {
    if common::data_root().is_none() {
        return;
    }
    // 500 world units east of the target: clear ground (visual range of a mech here is 1127.25 wu)
    let (mut open, me, tgt, first) = acquire((500.0, 0.0));
    let frame = first.expect("visual contact in the open within the budget");
    assert!(frame <= 6, "acquired at frame {frame}");
    eprintln!("open: visual contact at frame {frame}");
    assert!(
        terrain_clearance_deficit(&mut open, me, tgt) < 0.0,
        "nothing between them"
    );
    assert_eq!(open.world.sensors.team_contact_status[&tgt][0], 5);
    assert!(open.world.sensors.team_contact_order[0].contains(&tgt));
    assert!(open
        .world
        .sensors
        .pool
        .iter()
        .any(|s| s.owner == me && s.contacts.contains(&(tgt, 5))));
    assert!(
        open.world.sensors.enemy_in_los,
        "the home team sees an enemy (contact.cpp:340)"
    );

    // 800 wu south (inside visual range): a ridge rises ~185 wu above the eye-to-eye line between them, so only the sensor sees it
    let (mut behind, me, tgt, first) = acquire((0.0, -800.0));
    assert_eq!(first, None, "no visual contact behind terrain");
    let deficit = terrain_clearance_deficit(&mut behind, me, tgt);
    assert!(deficit > 100.0, "a ridge is in the way: {deficit}");
    assert_eq!(behind.world.sensors.team_contact_status[&tgt][0], 1);

    // 1500 wu east in the open: beyond 1127.25 wu, only the sensor sees it
    let (mut far, me, tgt, first) = acquire((1500.0, 0.0));
    assert_eq!(first, None, "no visual contact out of range");
    assert!(mc2_sim::object::hooks::visual_range(&far.world, me).unwrap() < 1500.0);
    assert_eq!(far.world.sensors.team_contact_status[&tgt][0], 1);
    let _ = terrain_clearance_deficit(&mut far, me, tgt);
}

/// A ground vehicle is a visual contact too (its eye is its own weapon node, not a fixed offset).
#[test]
fn a_ground_vehicle_in_the_open_is_a_visual_contact() {
    if common::data_root().is_none() {
        return;
    }
    // vehicle part 704 (team 1, at (2880, 2112)), 500 wu west of it in the open
    let (sim, _, tgt, first) = acquire_part(704, (-500.0, 0.0));
    assert!(first.is_some(), "a vehicle in the open is seen too");
    assert_eq!(sim.world.sensors.team_contact_status[&tgt][0], 5);
}

/// Three kinds of mover, each LOS height checked against the shipped `.tgl` read here (`mc2_assets::tgl::decode_shape`, the raw node
/// table, not the appearance code under test): the eye of a mover is its position raised by its lowest weapon node, and a node's
/// rest height above the mover's origin is its `nodeCenter` y. Parts on mc2_01: 704 Centipede (tank, turret node), 715 infantry,
/// 512 Werewolf and 513 Bushwacker (mechs: lowest of the `weapon_*` nodes). Tolerance 0.1 world units: the Centipede's weapon node
/// hangs off its turret, which the appearance has already turned a fraction of a degree.
#[test]
fn representative_los_heights_follow_the_shipped_shape_nodes() {
    let Some((mut sim, mut vfs)) = load("mc2_01") else {
        return;
    };
    step(&mut sim, 1);
    let mut checked = Vec::new();
    for (part, expected_file) in [
        (704, "centipede"),
        (715, "infantry"),
        (512, "werewolf"),
        (513, "bushwacker"),
    ] {
        let h = mover(&sim.world, part);
        let name = sim
            .world
            .objects
            .type_of(h)
            .unwrap()
            .base
            .appearance_name
            .clone()
            .expect("an appearance name");
        let mut ini = vfs
            .read_fit(&format!("data/tgl/{}.ini", name.to_lowercase()))
            .unwrap()
            .expect("appearance ini");
        ini.seek_block("TGLData").unwrap();
        let file = ini
            .read_string("FileName", 511)
            .or_else(|_| ini.read_string("FileName0", 511))
            .unwrap();
        assert_eq!(file.to_lowercase(), expected_file);
        let bytes = vfs
            .read(&format!("data/tgl/{}.tgl", file.to_lowercase()))
            .unwrap()
            .expect("shape file");
        let shape = mc2_assets::tgl::decode_shape(&bytes).unwrap();
        let expected = shape
            .nodes
            .iter()
            .filter(|n| n.node_id.to_lowercase().starts_with("weapon_"))
            .map(|n| n.node_center[1])
            .fold(f32::MAX, f32::min);
        assert!(expected > 1.0 && expected < 60.0, "{part}: {expected}");
        let pos = sim.world.objects.get(h).unwrap().c.position;
        let los = mc2_sim::object::hooks::los_position(&mut sim.world, h);
        assert!(
            (los.z - pos.z - expected).abs() <= 0.1,
            "part {part} ({name}): eye height {} vs shipped node {expected}",
            los.z - pos.z
        );
        checked.push(expected);
    }
    assert_eq!(checked.len(), 4);
}

/// A ground-vehicle type whose data names no appearance gets the `APC` (gvehicl.cpp:963-969): the spawn loads that appearance
/// type, binds it, and the vehicle has a lowest weapon node, so it has an eye height. The shipped types all name an appearance, so
/// one type's `AppearanceName` is removed from a second read of the shipped object table.
#[test]
fn a_vehicle_type_without_an_appearance_name_spawns_with_the_apc() {
    use mc2_sim::math::Vec3;
    use mc2_sim::mission::init::{prepare_spawn, LoadCtx, MoverInitData};
    use mc2_sim::mission::spawn::add_mover;
    use mc2_sim_data::objtype::{load_object_table, ObjectTypeData};
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let spec = LoadSpec::quickstart("mc2_01", 7, RngMode::Pcg32);
    let mut world = load_world(&mut vfs, &spec).unwrap().world;
    let mut table =
        load_object_table(vfs.root().join("data/objects/object2.pak")).expect("object2.pak");
    let (number, named) = table
        .types
        .iter()
        .find_map(|(n, t)| {
            (matches!(t.data, ObjectTypeData::GroundVehicle(_))
                && t.base
                    .appearance_name
                    .as_deref()
                    .is_some_and(|a| !a.eq_ignore_ascii_case("APC")))
            .then(|| (*n, t.base.appearance_name.clone().unwrap()))
        })
        .expect("a ground vehicle type");
    table.types.get_mut(&number).unwrap().base.appearance_name = None;
    world.data.object_types = Some(Rc::new(table));
    {
        let mut ctx = LoadCtx {
            world: &mut world,
            vfs: &mut vfs,
            spec: &spec,
        };
        prepare_spawn(&mut ctx, &[number], &["pmw00080"]).unwrap();
    }
    assert!(world
        .appearance
        .vehicle_types
        .iter()
        .any(|t| t.name.eq_ignore_ascii_case("APC")));
    // placed on the ground, as the mission does (the node height is measured above the terrain under the node)
    let mut at = Vec3 {
        x: 2400.0,
        y: -1900.0,
        z: 0.0,
    };
    at.z = world.terrain.elevation(&at);
    let h = add_mover(
        &mut world,
        &MoverInitData {
            pilot_file: "pmw00080".into(),
            brain: "pbrain".into(),
            object_number: number as i32,
            roster_index: 255,
            control_type: 2,
            position: at,
            team: 1,
            commander: 1,
            gesture_id: 2,
            active: true,
            exists: true,
            ..Default::default()
        },
    )
    .unwrap();
    let bound = match world.appearance.get(h) {
        Some(mc2_sim::appearance::Appearance::Vehicle(v)) => v.type_index,
        other => panic!("no vehicle appearance bound ({named} had none to load): {other:?}"),
    };
    assert!(world.appearance.vehicle_types[bound]
        .name
        .eq_ignore_ascii_case("APC"));
    // what `Mover::update` does with the position on a later turn (geometry is computed once per turn) before the sensors read the eye
    world.clock.turn += 1;
    mc2_sim::object::hooks::set_object_parameters(&mut world, h);
    world.appearance.set_in_view(h, true);
    mc2_sim::appearance::update(&mut world, h, true);
    world.appearance.set_in_view(h, false);
    let pos = world.objects.get(h).unwrap().c.position;
    let los = mc2_sim::object::hooks::los_position(&mut world, h);
    let node = match &world.objects.get(h).unwrap().kind {
        mc2_sim::object::Kind::Vehicle(v) => v.m.cached_los_weapon_node,
        _ => panic!("not a vehicle"),
    };
    let (node_id, height) = node.expect("the APC has a lowest weapon node");
    assert!(node_id >= 0 && height > 1.0 && height < 60.0, "{node:?}");
    assert!((los.z - pos.z - height).abs() < 1e-3);
}

/// An offscreen mech past turn 6 in a transition gesture holds a cached weapon height of zero (a not-in-view appearance answers
/// every node query with its own position). `Mover::getLOSPosition` repairs that with the appearance temporarily in view
/// (mover.cpp:3498-3502); the goal scoring of an attack order depends on this, so it must not be left unavailable.
#[test]
fn offscreen_mech_with_zero_cached_weapon_height_repairs_it_in_view() {
    let Some((mut sim, _)) = load("mc2_01") else {
        return;
    };
    for n in 1..=3 {
        step(&mut sim, n);
    }
    let h = mover(&sim.world, 512);
    sim.world.clock.turn = 10;
    mc2_sim::appearance::set_gesture(&mut sim.world, h, 3);
    assert_eq!(sim.world.appearance.current_gesture_id(h), 3);
    sim.world.appearance.set_in_view(h, true);
    let node = sim.world.appearance.lowest_weapon_node(h);
    assert!(node >= 0);
    let np = sim.world.appearance.weapon_node_position(h, node);
    let expected = np.z - sim.world.terrain.elevation(&np);
    assert!(expected > 0.0);
    sim.world.appearance.set_in_view(h, false);
    match &mut sim.world.objects.get_mut(h).unwrap().kind {
        mc2_sim::object::Kind::Mech(m) => m.m.cached_los_weapon_node = Some((node, 0.0)),
        _ => panic!("mech expected"),
    }
    let pos = sim.world.objects.get(h).unwrap().c.position;
    let los = mc2_sim::object::hooks::los_position(&mut sim.world, h);
    close(los.z, pos.z + expected);
    assert!(!sim.world.appearance.in_view(h), "visibility is restored");
    match &sim.world.objects.get(h).unwrap().kind {
        mc2_sim::object::Kind::Mech(m) => {
            let (n, height) = m.m.cached_los_weapon_node.unwrap();
            assert_eq!(n, node);
            close(height, expected);
        }
        _ => unreachable!(),
    }
}

/// `Mover::getLOSPosition` re-reads `lowestWeaponNodeZ` on every call while `turn < 6` (mover.cpp:3475), but only the callers the C++ has ask:
/// the sensors' sight tests, the target scans, a shot. The oracle's mc2_01 mech 512 is first asked at turn 40 (no call at turns 1-7 on any mover;
/// trace of the instrumented oracle, `evidence/rc-mc2-01-objectives.md`), while mc2_17's vehicle 738 is asked by the sensors in turns 1-5 and
/// holds 54.5593567 (`Z 2 738 1 54.56`). Priming every mover (an earlier stand-in) gave 512 node 4 at 23.0 instead of node 1 at 29.63 later.
#[test]
fn only_the_movers_the_sensors_ask_have_their_los_height_by_turn_6() {
    let Some((mut sim, _)) = load("mc2_01") else {
        return;
    };
    for n in 1..=7 {
        step(&mut sim, n);
    }
    let cached = |sim: &Sim, part: i32| match &sim
        .world
        .objects
        .get(mover(&sim.world, part))
        .unwrap()
        .kind
    {
        mc2_sim::object::Kind::Mech(m) => m.m.cached_los_weapon_node,
        mc2_sim::object::Kind::Vehicle(v) => v.m.cached_los_weapon_node,
        _ => panic!("part {part} is not a mover"),
    };
    for part in [512, 513, 514] {
        assert_eq!(
            cached(&sim, part),
            None,
            "part {part}: nothing asks the player mechs for their eye in turns 1-7"
        );
    }
    let Some((mut sim, _)) = load("mc2_17") else {
        return;
    };
    for n in 1..=7 {
        step(&mut sim, n);
    }
    let (node, height) = cached(&sim, 738).expect("the sensors ask 738 in turns 1-5");
    assert_eq!(node, 1);
    close(height, 54.559_357);
}
