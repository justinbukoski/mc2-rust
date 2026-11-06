//! The mission fixture and dump helpers of the fire probe: the Rust twin of `oracle/combat_probe.inc` (`tests/combat_probe.rs`
//! interprets the scripts; the other fire tests build the same world here).

#![allow(dead_code)]

use std::path::PathBuf;

use mc2_formats::vfs::Vfs;
use mc2_sim::combat::{self, loadout_of, AttackSource, WeaponShotInfo};
use mc2_sim::digest::fmt_f32;
use mc2_sim::ids::{CommanderId, ObjectHandle, TeamId, WatchId};
use mc2_sim::mission::init::{prepare, prepare_spawn, LoadCtx, LoadSpec};
use mc2_sim::mission::loadout;
use mc2_sim::mission::plan::{self, MoverClass};
use mc2_sim::mission::spawn::SpawnSpec;
use mc2_sim::mover::{Control, MechState, MoverCore, VehicleState};
use mc2_sim::object::{flags, Common, Kind, Obj, ObjectClass, ObjectStatus};
use mc2_sim::rng::RngMode;
use mc2_sim::World;
use mc2_sim_data::objtype::{ObjectTypeClass, ObjectTypeData};
use mc2_sim_data::pilot::Skill;

pub fn manifest(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(sub)
}

pub fn rng_mode(name: &str) -> RngMode {
    match name {
        "msvc" => RngMode::LegacyMsvc,
        "glibc" => RngMode::LegacyGlibc,
        _ => RngMode::Pcg32,
    }
}

/// The world the oracle's probe runs in: the movers of a mission with their pilots, in `moverList` order.
pub struct Fixture {
    pub w: World,
    /// Mover handles in `moverList` order (mechs, then vehicles).
    pub movers: Vec<ObjectHandle>,
    /// Visual draws the load made before the probe started (the `v` tokens count from there).
    pub vis_base: u64,
}

pub fn build_fixture(
    vfs: &mut Vfs,
    mission: &str,
    rng: &str,
    seed: u32,
) -> Result<Fixture, String> {
    let spec = LoadSpec::quickstart(mission, seed, rng_mode(rng));
    let mut world = World::new(spec.rng_mode, spec.seed);
    let mut ctx = LoadCtx {
        world: &mut world,
        vfs,
        spec: &spec,
    };
    let prepared = prepare(&mut ctx)?;
    let lp = &prepared.plan;
    let mut types: Vec<u32> = lp.parts.iter().skip(1).map(|p| p.obj_number).collect();
    types.sort_unstable();
    types.dedup();
    prepare_spawn(&mut ctx, &types, &[])?;

    // the object store: only the movers (no terrain objects), the weapon effect table for the explosions
    let table = ctx.world.data.object_types.clone().ok_or("object types")?;
    ctx.world.objects.types = Some(table.clone());
    mc2_sim::object::load::load_weapon_effects(ctx.vfs, &mut ctx.world.objects)?;
    let rng_shared = ctx.world.rng.clone();
    ctx.world
        .objects
        .set_num_objects(&rng_shared, lp.num_mechs, lp.num_vehicles);

    let comps = ctx.world.data.components.clone().ok_or("components")?;
    let mut created: Vec<Option<ObjectHandle>> = vec![None; lp.parts.len()];
    for m in &lp.order {
        let w = &mut *ctx.world;
        let handle = match m.class {
            MoverClass::Mech => w.objects.mechs[m.slot],
            MoverClass::Vehicle => w.objects.vehicles[m.slot],
        };
        let part = &lp.parts[m.part];
        let sp = SpawnSpec::from_part(part);
        let ty = table
            .get(sp.obj_number)
            .ok_or_else(|| format!("object type {}", sp.obj_number))?;
        let mut core = MoverCore {
            team: TeamId(sp.team),
            commander: CommanderId(sp.commander as i8),
            pilot_slot: sp.pilot_slot as u16,
            squad_id: -1,
            group_id: -1,
            ..MoverCore::default()
        };
        let (class, kind, tonnage) = match (&ty.data, ty.object_type_class) {
            (ObjectTypeData::BattleMech(_), ObjectTypeClass::BattleMech) => {
                let csv = ty.base.appearance_name.clone().ok_or("appearance name")?;
                let chassis = w
                    .data
                    .chassis
                    .get(&csv.to_ascii_lowercase())
                    .cloned()
                    .ok_or_else(|| format!("chassis {csv}"))?;
                let build = loadout::build_mech(&chassis, sp.variant, &comps)?;
                core.max_move_speed = build.max_move_speed;
                let state = MechState {
                    m: core,
                    control: Control::default(),
                    variant_name: build.variant_name.clone(),
                    variant_id: build.variant_id,
                    num_jump_jets: build.num_jump_jets,
                    last_jump_time: -100.0,
                    center_torso_blow_time: -1.0,
                    engine_blow_time: -1.0,
                    loadout: build.loadout,
                    ..MechState::default()
                };
                (
                    ObjectClass::BattleMech,
                    Kind::Mech(Box::new(state)),
                    build.tonnage,
                )
            }
            (ObjectTypeData::GroundVehicle(_), ObjectTypeClass::Vehicle) => {
                let v = w
                    .data
                    .vehicle_objects
                    .get(&sp.obj_number)
                    .cloned()
                    .ok_or("vehicle packet")?;
                let build = loadout::build_vehicle(&v, &comps)?;
                core.max_move_speed = build.max_move_speed;
                let state = VehicleState {
                    m: core,
                    loadout: build.loadout,
                    ..VehicleState::default()
                };
                (
                    ObjectClass::GroundVehicle,
                    Kind::Vehicle(Box::new(state)),
                    build.tonnage,
                )
            }
            _ => return Err("not a mover type".into()),
        };
        let mut c = Common::new(class, handle);
        c.type_handle = sp.obj_number as i32;
        c.tonnage = tonnage;
        c.flags |= flags::TANGIBLE | flags::SENSOR | flags::OBSCURED;
        c.position = part.position;
        c.rotation = part.rotation;
        w.objects.list[handle.0 as usize] = Some(Obj { c, kind });
        // the mech's appearance (weapon nodes are the weapon `facing`)
        if class == ObjectClass::BattleMech {
            let name = ty.base.appearance_name.clone().ok_or("appearance name")?;
            let find = |sys: &mc2_sim::appearance::AppearanceSystem, n: &str| {
                sys.mech_types
                    .iter()
                    .position(|t| t.name.eq_ignore_ascii_case(n))
            };
            let idx = find(&w.appearance, &name)
                .or_else(|| find(&w.appearance, "MADCAT"))
                .ok_or("mech appearance type")?;
            let streams = w.rng.0.clone();
            let mut r = streams.borrow_mut();
            w.appearance.create_mech(handle, idx, &mut r.sim);
        } else {
            w.appearance.put(
                handle,
                mc2_sim::appearance::Appearance::Vehicle(Box::default()),
            );
        }
        let wid = w.objects.watch_id(handle);
        w.pilots.slots[sp.pilot_slot].mover = wid;
        if let Some(o) = w.objects.get_mut(handle) {
            o.c.flags |= flags::AWAKE;
            o.set_exists(sp.exists);
        }
        created[m.part] = Some(handle);
    }
    // part ids (populate step 8b)
    {
        let members = plan::group_members(&prepared.mission, &prepared.commanders, &|i| {
            created.get(i).is_some_and(|c| c.is_some())
        });
        for g in &members {
            if let Some(h) = created[g.part] {
                ctx.world
                    .objects
                    .set_part_id(h, g.commander as i32, g.group as i32, g.mate as i32);
            }
        }
    }
    ctx.world.teams.home = 0;
    // the combat numbers (the same call `spawn_into_slot` makes, after the pilot is attached and the team set)
    for m in &lp.order {
        let h = created[m.part].expect("created");
        combat::init_mover_stats(ctx.world, h)?;
    }
    // the status rules for parts that start destroyed (`createPartObject`, after the stats): a damage chain starts from that status
    for m in &lp.order {
        if lp.parts[m.part].destroyed {
            let h = created[m.part].expect("created");
            mc2_sim::mission::spawn::apply_destroyed_part(ctx.world, h, 0);
        }
    }
    let w = &mut *ctx.world;
    let movers: Vec<ObjectHandle> = w.objects.mechs[..lp.num_mechs]
        .iter()
        .chain(w.objects.vehicles[..lp.num_vehicles].iter())
        .copied()
        .collect();
    let vis_base = world.rng.0.borrow().visual.draws();
    Ok(Fixture {
        w: world,
        movers,
        vis_base,
    })
}

pub fn f(v: f32) -> String {
    fmt_f32(v)
}

pub fn find(fx: &Fixture, part: i64) -> ObjectHandle {
    fx.w.objects
        .find_by_part_id(mc2_sim::ids::PartId(part as i32))
        .unwrap_or_else(|| panic!("no mover with part id {part}"))
}

pub fn pilot_of(fx: &mut Fixture, h: ObjectHandle) -> &mut mc2_sim::ai::Pilot {
    let slot = combat::fx::pilot_slot(&fx.w, h).expect("pilot");
    fx.w.pilots.get_mut(slot).unwrap()
}

pub fn rng_tail(w: &World) -> String {
    let r = w.rng.0.borrow();
    format!("r{}:{:x}", r.sim.draws(), r.sim.state_hash())
}

pub fn dump_state(
    fx: &mut Fixture,
    tag: &str,
    h: ObjectHandle,
    a: i64,
    b: i64,
    out: &mut Vec<String>,
) {
    let (wounds, pstatus) = {
        let p = pilot_of(fx, h);
        (p.wounds, p.status)
    };
    let slot = combat::fx::pilot_slot(&fx.w, h).unwrap();
    let sp = fx.w.mission.pilot_loads[usize::from(slot)].skill_points;
    let o = fx.w.objects.get(h).unwrap();
    let l = loadout_of(o).unwrap();
    let mut s = format!("{tag} {} {a} {b} st{}", o.c.part_id.0, o.c.status as i32);
    s += " A";
    for ar in &l.armor {
        s += &format!(" {}/{}", f(ar.0), ar.1);
    }
    s += " S";
    for bd in &l.body {
        s += &format!(" {}:{}", f(bd.cur_internal_structure), bd.damage_state);
    }
    s += " C";
    let mech = matches!(o.kind, Kind::Mech(_));
    if mech {
        for bd in &l.body {
            s += " ";
            for c in &bd.critical_spaces {
                s += if c.1 { "1" } else { "0" };
            }
        }
    }
    s += " I";
    for it in l.inventory.iter().take(l.num_items()) {
        s += &format!(" {}:{}:{}", it.health, i32::from(it.disabled), it.amount);
    }
    s += &format!(" cv {} {} th {}", o.c.cur_cv, o.c.max_cv, o.c.threat_rating);
    s += &format!(
        " we {} fr {} {} {} {}",
        f(l.stats.weapon_effectiveness),
        f(l.stats.min_range),
        f(l.stats.max_range),
        f(l.stats.optimal_range),
        l.stats.num_functional_weapons
    );
    let engine_blow = match &o.kind {
        Kind::Mech(m) => m.engine_blow_time,
        _ => -1.0,
    };
    s += &format!(
        " dr {} pc {} nh {} eb {}",
        f(l.stats.damage_rate_tally),
        f(l.stats.pilot_check_damage_tally),
        l.stats.num_weapon_hits_handled,
        f(engine_blow)
    );
    if let Kind::Mech(m) = &o.kind {
        s += &format!(
            " ls {} ts {} ct {} dt {} da {} hf{} hb{} fp{} la{} ra{} pch{}",
            m.leg_status.0,
            l.stats.torso_status,
            f(m.center_torso_blow_time),
            f(l.stats.damage_this_frame),
            f(l.stats.damage_after_disabled),
            i32::from(m.hit_from_front),
            i32::from(m.hit_from_behind),
            i32::from(m.failed_piloting_check),
            i32::from(m.left_arm_blown),
            i32::from(m.right_arm_blown),
            i32::from(l.stats.played_critical_hit)
        );
        s += &format!(" cs {}", i32::from(m.sent_crippled_msg));
    }
    if !mech {
        s += &format!(
            " ml{} ms{}",
            l.stats.mine_layer,
            i32::from(l.stats.mine_sweeper)
        );
    }
    s += &format!(
        " w {} ps {} sp {} {} {} {}",
        f(wounds),
        pstatus as i32,
        f(sp[0]),
        f(sp[1]),
        f(sp[2]),
        rng_tail(&fx.w)
    );
    out.push(s);
}

// ---------------------------------------------------------------------------------------------------------------------
// the fire directives: `time`, `frame`, `recycle`, `effects`, `fire`, `boltstep` (oracle/combat_probe.inc)

use mc2_sim::math::Vec3;

fn parts_of(fx: &Fixture, wid: mc2_sim::ids::WatchId) -> i32 {
    fx.w.objects
        .by_watch_id(wid)
        .and_then(|h| fx.w.objects.get(h))
        .map_or(0, |o| o.c.part_id.0)
}

/// Bolts that are in flight: `exists && !hitTarget`.
fn live_bolts(fx: &Fixture) -> usize {
    fx.w.objects
        .weapons
        .iter()
        .filter(|&&h| {
            fx.w.objects.get(h).is_some_and(|o| {
                o.exists() && matches!(&o.kind, Kind::WeaponBolt(b) if !b.hit_target)
            })
        })
        .count()
}

/// `time <seconds>`.
pub fn time(fx: &mut Fixture, t: &[&str], out: &mut Vec<String>) {
    let s: f32 = t[1].parse().unwrap();
    fx.w.clock.scenario_time = s;
    out.push(format!("# time {}", f(s)));
}

/// `frame <fps>`: `frameLength = 1.0f / fps`.
pub fn frame(fx: &mut Fixture, t: &[&str], out: &mut Vec<String>) {
    let fps: f32 = t[1].parse().unwrap();
    fx.w.clock.frame_length = 1.0f32 / fps;
    out.push(format!("# frame {}", f(fx.w.clock.frame_length)));
}

/// `recycle <part> <node> <seconds>`.
pub fn recycle(fx: &mut Fixture, t: &[&str]) {
    let h = find(fx, t[1].parse().unwrap());
    fx.w.appearance
        .set_weapon_node_recycle(h, t[2].parse().unwrap(), t[3].parse().unwrap());
}

/// `effects`: `EF <i> <objNum>` per row and `EN <numEffects>`.
pub fn effects(fx: &Fixture, out: &mut Vec<String>) {
    for i in 0..fx.w.objects.effect_objects.len() {
        out.push(format!(
            "EF {i} {}",
            fx.w.objects.effect_object_number(i as i32)
        ));
    }
    out.push(format!("EN {}", fx.w.objects.effect_objects.len()));
}

/// `fire <attacker> <target|0> <weaponIndex> <aim> [<x> <y> <z>]`: `FR` and one `BO` per new bolt.
pub fn fire(fx: &mut Fixture, t: &[&str], out: &mut Vec<String>) {
    let n = |i: usize| -> i64 { t[i].parse().unwrap() };
    let (att, tgt, widx, aim) = (n(1), n(2), n(3) as i32, n(4) as i32);
    let point = (t.len() >= 8).then(|| Vec3 {
        x: t[5].parse().unwrap(),
        y: t[6].parse().unwrap(),
        z: t[7].parse().unwrap(),
    });
    let ah = find(fx, att);
    let th = (tgt != 0).then(|| find(fx, tgt));
    let before: Vec<bool> =
        fx.w.objects
            .weapons
            .iter()
            .map(|&h| fx.w.objects.get(h).unwrap().exists())
            .collect();
    let r = combat::fire::fire_weapon(&mut fx.w, ah, widx, th, aim, point, 0);
    let (facing, ready, shots) = {
        let o = fx.w.objects.get(ah).unwrap();
        let l = loadout_of(o).unwrap();
        let comps = fx.w.data.components.as_ref().unwrap();
        (
            i32::from(l.inventory[widx as usize].facing),
            l.inventory[widx as usize].ready_time,
            l.weapon_shots(comps, i64::from(widx)),
        )
    };
    let nr = if matches!(fx.w.objects.get(ah).unwrap().kind, Kind::Mech(_)) {
        fx.w.appearance.weapon_node_recycle(ah, facing)
    } else {
        -1.0
    };
    let (draws, state, vis) = {
        let rng = fx.w.rng.0.borrow();
        (
            rng.sim.draws(),
            rng.sim.state_hash(),
            rng.visual.draws() - fx.vis_base,
        )
    };
    out.push(format!(
        "FR {att} {tgt} {widx} {aim} ret{} dmg{} r{draws} s{state:x} v{vis} amt{shots} rt{} nr{} nb{}",
        r.code,
        f(r.dmg_done),
        f(ready),
        f(nr),
        live_bolts(fx)
    ));
    for (i, &h) in fx.w.objects.weapons.clone().iter().enumerate() {
        let o = fx.w.objects.get(h).unwrap();
        if !o.exists() || before[i] {
            continue;
        }
        let Kind::WeaponBolt(b) = &o.kind else {
            continue;
        };
        let shot = b.weapon_shot.unwrap_or(combat::WeaponShotInfo {
            attacker: mc2_sim::ids::WatchId::NONE,
            master_id: 0,
            damage: 0.0,
            hit_location: 0,
            entry_angle: 0.0,
        });
        let tp = if b.has_target_position {
            format!(
                "{},{},{}",
                f(b.target_position.x),
                f(b.target_position.y),
                f(b.target_position.z)
            )
        } else {
            "-".to_string()
        };
        out.push(format!(
            "BO {} {} hs{} ths{} m{} d{} l{} a{} tp{tp} eff{} tw{}",
            parts_of(fx, b.owner),
            parts_of(fx, b.target),
            b.hot_spots.0,
            b.hot_spots.1,
            shot.master_id,
            f(shot.damage),
            shot.hit_location,
            f(shot.entry_angle),
            b.effect_id,
            i32::from(!b.target.is_none())
        ));
    }
}

/// `boltstep <frames>`: `BF` per frame that has a bolt to report, then a `D` line per mover a bolt hit.
pub fn boltstep(fx: &mut Fixture, frames: i64, out: &mut Vec<String>) {
    let mut hit_parts = std::collections::BTreeSet::new();
    for step in 0..frames {
        fx.w.clock.turn += 1;
        fx.w.clock.scenario_time += fx.w.clock.frame_length;
        let pool = fx.w.objects.weapons.clone();
        let hit_before: Vec<bool> = pool
            .iter()
            .map(|&h| matches!(&fx.w.objects.get(h).unwrap().kind, Kind::WeaponBolt(b) if b.hit_target))
            .collect();
        for &h in &pool {
            if fx.w.objects.get(h).unwrap().exists() && !combat::update_bolt(&mut fx.w, h) {
                fx.w.objects.get_mut(h).unwrap().set_exists(false);
            }
        }
        let mut line = String::new();
        for (i, &h) in pool.iter().enumerate() {
            let o = fx.w.objects.get(h).unwrap();
            let Kind::WeaponBolt(b) = &o.kind else {
                continue;
            };
            if !o.exists() && !(b.hit_target && !hit_before[i]) {
                continue;
            }
            if b.hit_target && hit_before[i] {
                continue;
            }
            if b.hit_target {
                let p = parts_of(fx, b.target);
                if p != 0 {
                    hit_parts.insert(p);
                }
            }
            line += &format!(
                " [i{i} hs{} ht{} lp{},{},{} dt{} gh{} hl{} tl{}]",
                b.hot_spots.0,
                i32::from(b.hit_target),
                f(b.laser_position.x),
                f(b.laser_position.y),
                f(b.laser_position.z),
                f(b.distance_to_target),
                f(b.goal_height),
                f(b.hit_left),
                f(b.time_left)
            );
        }
        if !line.is_empty() {
            let (draws, vis) = {
                let r = fx.w.rng.0.borrow();
                (r.sim.draws(), r.visual.draws() - fx.vis_base)
            };
            out.push(format!(
                "BF {step} nb{}{line} r{draws} v{vis}",
                live_bolts(fx)
            ));
        }
    }
    for p in hit_parts {
        let h = find(fx, i64::from(p));
        dump_state(fx, "D", h, 0, 0, out);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the interpreter of the combat probe scripts (the Rust twin of oracle/combat_probe.inc) and the comparison helpers

pub fn data_root() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) {
        Some(p) => Some(PathBuf::from(p)),
        None if std::env::var_os("MC2_ALLOW_SKIP").is_some() => {
            eprintln!("MC2_DATA not set and MC2_ALLOW_SKIP set; skipping the combat probe test");
            None
        }
        None => panic!("MC2_DATA is not set: the combat probe goldens would not be compared (set MC2_ALLOW_SKIP=1 to skip)"),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the interpreter: the Rust twin of combat_probe.inc

pub fn dump_stats(fx: &mut Fixture, h: ObjectHandle, out: &mut Vec<String>) {
    let (gun, pil) = {
        let p = pilot_of(fx, h);
        (
            p.skills[Skill::Gunnery.index()],
            p.skills[Skill::Piloting.index()],
        )
    };
    let o = fx.w.objects.get(h).unwrap();
    let part = o.c.part_id.0;
    let l = loadout_of(o).unwrap();
    let variant = match &o.kind {
        Kind::Mech(m) => i64::from(m.variant_id),
        _ => -1,
    };
    out.push(format!(
        "S {} {} {} {} {} {} {} {} {} {}",
        part,
        o.c.class as i32,
        o.c.type_handle,
        variant,
        gun,
        pil,
        l.num_other(),
        l.num_weapons(),
        l.num_ammos(),
        l.ammo_totals.len()
    ));
    let mut s = format!("SA {part}");
    for t in &l.ammo_totals {
        s += &format!(" {}:{}:{}", t.0, t.1, t.2);
    }
    out.push(s);
    let mut s = format!("SI {part}");
    for (i, it) in l.inventory.iter().enumerate().take(l.num_items()) {
        s += &format!(
            " {}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            i,
            it.master_id,
            it.health,
            i32::from(it.disabled),
            it.facing,
            it.start_amount,
            it.amount,
            it.ammo_index,
            f(it.ready_time),
            it.body_location,
            it.effectiveness
        );
    }
    out.push(s);
    out.push(format!(
        "SR {} {} {} {} {} {} {}",
        part,
        f(l.stats.min_range),
        f(l.stats.max_range),
        f(l.stats.optimal_range),
        l.stats.num_functional_weapons,
        f(l.stats.last_weapon_effectiveness_calc),
        f(l.stats.last_optimal_range_calc)
    ));
    match &o.kind {
        Kind::Mech(m) => {
            out.push(format!(
                "SC {} {} {} {} {} {} {} {}",
                part,
                o.c.max_cv,
                o.c.cur_cv,
                o.c.threat_rating,
                f(l.stats.weapon_effectiveness),
                f(l.stats.max_weapon_effectiveness),
                f(l.stats.max_weapon_damage),
                l.stats.chassis_class
            ));
            let c = |i: usize| l.critical[i];
            out.push(format!(
                "SX {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
                part,
                c(0),
                c(1),
                c(2),
                c(3),
                c(4),
                c(5),
                c(6),
                l.stats.gyro,
                l.stats.actuator[0],
                l.stats.actuator[1],
                l.stats.actuator[2],
                l.stats.actuator[3],
                l.stats.anti_missile.len(),
                m.leg_status.0,
                l.stats.torso_status,
                l.stats.num_armor_components
            ));
        }
        _ => out.push(format!(
            "SC {} {} {} {} {} {} 0 0",
            part,
            o.c.max_cv,
            o.c.cur_cv,
            o.c.threat_rating,
            f(l.stats.weapon_effectiveness),
            f(l.stats.max_weapon_effectiveness)
        )),
    }
    let mut s = format!("SB {part}");
    for b in &l.body {
        s += &format!(
            " {}:{}:{}",
            b.total_spaces,
            i32::from(b.case),
            b.damage_state
        );
        if matches!(o.kind, Kind::Mech(_)) {
            s += ":";
            let ids: Vec<String> = b.critical_spaces.iter().map(|c| c.0.to_string()).collect();
            s += &ids.join(".");
        }
    }
    out.push(s);
}

pub fn run_script(fx: &mut Fixture, script: &str, header: String) -> Vec<String> {
    let mut out = vec![header];
    for line in script.lines() {
        let line = line.trim_end();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let t: Vec<&str> = line.split_whitespace().collect();
        let n = |i: usize| -> i64 { t[i].parse().unwrap() };
        let x = |i: usize| -> f32 { t[i].parse().unwrap() };
        match t[0] {
            "difficulty" => {
                fx.w.mission.difficulty = n(1) as i32;
                out.push(format!("# difficulty {}", n(1)));
            }
            "unlimited" => fx.w.mission.unlimited_ammo = n(1) != 0,
            "seed" => {
                fx.w.rng.0.borrow_mut().sim.seed(n(1) as u32);
                out.push(format!("# seed {}", n(1)));
            }
            "place" => {
                let h = find(fx, n(1));
                let o = fx.w.objects.get_mut(h).unwrap();
                o.c.position.x = x(2);
                o.c.position.y = x(3);
                o.c.rotation = x(4);
            }
            "status" => {
                let h = find(fx, n(1));
                fx.w.objects.get_mut(h).unwrap().c.status = match n(2) {
                    0 => ObjectStatus::Normal,
                    1 => ObjectStatus::Disabled,
                    2 => ObjectStatus::Destroyed,
                    3 => ObjectStatus::StartingUp,
                    4 => ObjectStatus::ShuttingDown,
                    _ => ObjectStatus::ShutDown,
                };
            }
            "fallen" => {
                let h = find(fx, n(1));
                if let Kind::Mech(m) = &mut fx.w.objects.get_mut(h).unwrap().kind {
                    m.fallen = n(2) != 0;
                }
            }
            "skill" => {
                let h = find(fx, n(1));
                let p = pilot_of(fx, h);
                p.skills[Skill::Piloting.index()] = n(2) as i32;
                p.skills[Skill::Sensors.index()] = n(3) as i32;
                p.skills[Skill::Gunnery.index()] = n(4) as i32;
            }
            "spec" => {
                let h = find(fx, n(1));
                pilot_of(fx, h).specialty[n(2) as usize] = n(3) != 0;
            }
            "stats" => {
                if t[1] == "all" {
                    for h in fx.movers.clone() {
                        dump_stats(fx, h, &mut out);
                    }
                } else {
                    let h = find(fx, n(1));
                    dump_stats(fx, h, &mut out);
                }
            }
            "statshash" => {
                let list = if t[1] == "all" {
                    fx.movers.clone()
                } else {
                    vec![find(fx, n(1))]
                };
                for h in list {
                    let mut lines = Vec::new();
                    dump_stats(fx, h, &mut lines);
                    let mut hash = 1_469_598_103_934_665_603u64;
                    for l in &lines {
                        for b in l.bytes().chain(std::iter::once(b'\n')) {
                            hash ^= u64::from(b);
                            hash = hash.wrapping_mul(1_099_511_628_211);
                        }
                    }
                    let part = fx.w.objects.get(h).unwrap().c.part_id.0;
                    out.push(format!("SH {part} {hash:x}"));
                }
            }
            "shot" => {
                let (att, master, dmg, loc, ang) = (n(1), n(2) as i32, x(3), n(4) as i32, x(5));
                let wid = if att != 0 {
                    let h = find(fx, att);
                    fx.w.objects.watch_id(h)
                } else {
                    WatchId::NONE
                };
                let mut s = WeaponShotInfo::init(&fx.w, wid, master, dmg, loc, ang);
                let a = s.damage;
                s.set_damage(dmg / 3.0f32);
                let b = s.damage;
                let mut tt = WeaponShotInfo::init(&fx.w, WatchId::NONE, 0, 0.0, 0, 0.0);
                tt.assign(&fx.w, s);
                out.push(format!(
                    "W {} {} {} {} {} | init {} | setDamage {} | assign {}",
                    att,
                    master,
                    f(dmg),
                    loc,
                    f(ang),
                    f(a),
                    f(b),
                    f(tt.damage)
                ));
            }
            "chance" => {
                let (att, tgt, widx, aim) = (n(1), n(2), n(3) as i32, n(4) as i32);
                let ah = find(fx, att);
                let th = (tgt != 0).then(|| find(fx, tgt));
                let r = combat::calc_attack_chance(&fx.w, ah, th, aim, widx, None);
                out.push(format!(
                    "A {} {} {} {} {} {}",
                    att,
                    tgt,
                    widx,
                    aim,
                    f(r.chance),
                    r.range.unwrap_or(-1)
                ));
            }
            "chancept" => {
                let (att, widx, aim) = (n(1), n(2) as i32, n(3) as i32);
                let fv = |i: usize| -> f32 { t[i].parse().unwrap() };
                let pt = mc2_sim::math::Vec3 {
                    x: fv(4),
                    y: fv(5),
                    z: fv(6),
                };
                let r = combat::calc_attack_chance(&fx.w, find(fx, att), None, aim, widx, Some(pt));
                out.push(format!(
                    "A {} pt {} {} {} {}",
                    att,
                    widx,
                    aim,
                    f(r.chance),
                    r.range.unwrap_or(-1)
                ));
            }
            "hitloc" => {
                let (tgt, att, src, ty, count) = (n(1), n(2), n(3), n(4) as i32, n(5));
                let th = find(fx, tgt);
                let ah = if att != 0 {
                    find(fx, att)
                } else {
                    ObjectHandle::NONE
                };
                for _ in 0..count {
                    let loc = combat::calc_hit_location(
                        &mut fx.w,
                        th,
                        ah,
                        -1,
                        AttackSource::from_code(src as i32).expect("source"),
                        ty,
                    );
                    out.push(format!(
                        "H {} {} {} {} {} {}",
                        tgt,
                        att,
                        src,
                        ty,
                        loc,
                        rng_tail(&fx.w)
                    ));
                }
            }
            "dmg" => {
                let (tgt, att, master, dmg, loc, ang) =
                    (n(1), n(2), n(3) as i32, x(4), n(5) as i32, x(6));
                let th = find(fx, tgt);
                let wid = if att != 0 {
                    let h = find(fx, att);
                    fx.w.objects.watch_id(h)
                } else {
                    WatchId::NONE
                };
                let mut s = WeaponShotInfo::init(&fx.w, wid, master, dmg, loc, ang);
                let before = s.damage;
                combat::damage::handle_weapon_hit_mut(&mut fx.w, th, &mut s);
                dump_state(
                    fx,
                    "D",
                    th,
                    (before * 4.0f32) as i64,
                    (s.damage * 4.0f32) as i64,
                    &mut out,
                );
            }
            "crit" => {
                let (tgt, loc, count) = (n(1), n(2) as usize, n(3));
                let th = find(fx, tgt);
                for i in 0..count {
                    combat::damage::calc_critical_hit(&mut fx.w, th, loc);
                    dump_state(fx, "K", th, loc as i64, i, &mut out);
                }
            }
            "reset" => {
                let h = find(fx, n(1));
                let comps: Vec<i32> = t[2..].iter().map(|s| s.parse().unwrap()).collect();
                combat::reset_components(&mut fx.w, h, &comps).expect("reset");
                dump_stats(fx, h, &mut out);
                dump_state(fx, "R", h, comps.len() as i64, 0, &mut out);
            }
            "time" => time(fx, &t, &mut out),
            "frame" => frame(fx, &t, &mut out),
            "recycle" => recycle(fx, &t),
            "effects" => effects(fx, &mut out),
            "fire" => fire(fx, &t, &mut out),
            "boltstep" => boltstep(fx, n(1), &mut out),
            other => panic!("unknown directive {other}"),
        }
    }
    out
}

pub fn first_difference(want: &str, got: &[String]) -> Option<String> {
    let mut gi = got.iter();
    for (i, w) in want.lines().enumerate() {
        match gi.next() {
            None => {
                return Some(format!(
                    "line {}: golden has more lines; first missing: {w}",
                    i + 1
                ))
            }
            Some(g) if g != w => {
                return Some(format!("line {}:\n  oracle: {w}\n  rust:   {g}", i + 1));
            }
            _ => {}
        }
    }
    gi.next().map(|g| format!("rust has more lines: {g}"))
}

pub fn check(mission: &str, rng: &str) {
    check_files(
        mission,
        rng,
        &format!("tests/golden/combat_probe_{mission}_{rng}.txt"),
        &format!("oracle/scenarios/combat_probe_{mission}.txt"),
    );
}

pub fn check_files(mission: &str, rng: &str, golden_path: &str, script_path: &str) {
    let Some(root) = data_root() else { return };
    let golden = std::fs::read_to_string(manifest(golden_path)).expect("golden");
    let script = std::fs::read_to_string(manifest(script_path)).expect("script");
    let mut vfs = Vfs::from_system_cfg(&root).expect("mount archives");
    // The scenario is declared here (the oracle's default seed, the mission's own difficulty from the load spec), not read from the golden.
    let seed: u32 = 1;
    let mut fx = build_fixture(&mut vfs, mission, rng, seed).expect("fixture");
    let rng_index = match rng {
        "msvc" => 0,
        "glibc" => 1,
        _ => 2,
    };
    let header = format!(
        "CH {mission} {seed} {rng_index} {}",
        fx.w.mission.difficulty
    );
    let got = run_script(&mut fx, &script, header);
    if let Some(d) = first_difference(&golden, &got) {
        panic!("{mission} {rng}: {d}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// the fire probe (lane C slice 2): `oracle/scenarios/fire_probe_mc2_01.txt` against `tests/golden/fire_probe_mc2_01_<rng>.txt`.
//
// Every test replays a PREFIX of the script (the full script, or the part before a marker line) from the same state as the oracle's
// run, so the prefix's output is a prefix of the golden and every token, including the RNG draw counts and states, is comparable.
// What differs between the tests is which lines and which tokens of FR (`FR att tgt w aim ret dmg r s v amt rt nr nb`) they check.

/// Token positions of an `FR` line (after the five fixed fields).
pub const FR_RET: usize = 5;
pub const FR_DMG: usize = 6;
pub const FR_R: usize = 7;
pub const FR_S: usize = 8;
pub const FR_V: usize = 9;
pub const FR_AMT: usize = 10;
pub const FR_RT: usize = 11;
pub const FR_NR: usize = 12;
pub const FR_NB: usize = 13;

/// Which lines of the golden a fire test compares and, for `FR`, which tokens.
pub struct FireCheck {
    /// Tags compared in full (`BO`, `BF`, `D`, `EF`, `EN`, `#`, `CH`).
    pub full_tags: &'static [&'static str],
    /// `FR` token positions compared (the first five fields are always compared). `None`: do not compare FR lines.
    pub fr_tokens: Option<&'static [usize]>,
    /// Compare every other line (stats dumps, state lines) in full too.
    pub all_other_lines: bool,
}

pub const ALL_TOKENS: &[usize] = &[
    FR_RET, FR_DMG, FR_R, FR_S, FR_V, FR_AMT, FR_RT, FR_NR, FR_NB,
];

/// Stop the script before the first line starting with `marker` (all of it for `None`).
pub fn script_prefix(script: &str, marker: Option<&str>) -> String {
    let mut out = String::new();
    for line in script.lines() {
        if marker.is_some_and(|m| line.starts_with(m)) {
            break;
        }
        out += line;
        out.push('\n');
    }
    out
}

/// Replay a prefix of the fire probe script (the whole script for `None`) from the oracle's start state in the RNG mode `rng`; returns
/// the world and what Rust printed, with the golden's lines. `None` without `MC2_DATA` under `MC2_ALLOW_SKIP`.
pub fn fire_replay(rng: &str, marker: Option<&str>) -> Option<(Fixture, Vec<String>, Vec<String>)> {
    let root = data_root()?;
    let script = std::fs::read_to_string(manifest("oracle/scenarios/fire_probe_mc2_01.txt"))
        .expect("script");
    let script = script_prefix(&script, marker);
    let golden = std::fs::read_to_string(manifest(&format!(
        "tests/golden/fire_probe_mc2_01_{rng}.txt"
    )))
    .expect("golden");
    let mut vfs = Vfs::from_system_cfg(&root).expect("mount archives");
    // The scenario is declared here (the oracle's seed), not read from the golden.
    let seed = 1u32;
    let mut fx = build_fixture(&mut vfs, "mc2_01", rng, seed).expect("fixture");
    let rng_index = match rng {
        "msvc" => 0,
        "glibc" => 1,
        _ => 2,
    };
    let header = format!("CH mc2_01 {seed} {rng_index} {}", fx.w.mission.difficulty);
    let got = run_script(&mut fx, &script, header);
    Some((fx, got, golden.lines().map(str::to_string).collect()))
}

pub fn fire_check(rng: &str, marker: Option<&str>, check: &FireCheck) {
    let Some((_fx, got, want)) = fire_replay(rng, marker) else {
        return;
    };
    assert!(
        got.len() <= want.len(),
        "{rng}: rust printed more lines ({}) than the golden has ({}); first extra: {}",
        got.len(),
        want.len(),
        got[want.len()]
    );
    // a prefix of the script can stop in the middle of a bolt-step run only at a marker, so its output is a prefix of the golden
    for (i, g) in got.iter().enumerate() {
        let w = want[i].as_str();
        let tag = w.split_whitespace().next().unwrap_or("");
        let compared = |a: &str, b: &str| -> bool {
            if tag == "FR" {
                let Some(tokens) = check.fr_tokens else {
                    return true;
                };
                let (a, b): (Vec<&str>, Vec<&str>) = (
                    a.split_whitespace().collect(),
                    b.split_whitespace().collect(),
                );
                a.len() == b.len()
                    && a[..5] == b[..5]
                    && tokens.iter().all(|&k| a.get(k) == b.get(k))
            } else if check.full_tags.contains(&tag)
                || tag.starts_with('#')
                || check.all_other_lines
            {
                a == b
            } else {
                true
            }
        };
        assert!(
            compared(w, g),
            "{rng}: line {} differs\n  oracle: {w}\n  rust:   {g}",
            i + 1
        );
    }
    // every golden line of a compared kind inside the prefix must have been produced
    if marker.is_none() {
        assert_eq!(got.len(), want.len(), "{rng}: line count");
    }
}

pub const EVERYTHING: FireCheck = FireCheck {
    full_tags: &["CH", "EF", "EN", "BO", "BF", "D"],
    fr_tokens: Some(ALL_TOKENS),
    all_other_lines: true,
};
pub const FIRE_AND_BOLT_LINES: FireCheck = FireCheck {
    full_tags: &["CH", "BO"],
    fr_tokens: Some(ALL_TOKENS),
    all_other_lines: false,
};

// ---------------------------------------------------------------------------------------------------------------------
// helpers of the small fire tests

/// The mc2_01 mover fixture (msvc RNG, the oracle's seed) or `None` when `MC2_DATA` is not set under `MC2_ALLOW_SKIP`.
pub fn mc2_01() -> Option<Fixture> {
    let root = data_root()?;
    let mut vfs = Vfs::from_system_cfg(&root).expect("mount archives");
    Some(build_fixture(&mut vfs, "mc2_01", "msvc", 1).expect("fixture"))
}

/// Effect ids of the weapon effects table whose object type is a weapon bolt.
pub fn bolt_effect_ids(fx: &Fixture) -> Vec<i32> {
    (0..fx.w.objects.effect_objects.len() as i32)
        .filter(|&e| {
            fx.w.objects
                .type_by_num(fx.w.objects.effect_object_number(e))
                .is_some_and(|t| matches!(t.data, ObjectTypeData::WeaponBolt(_)))
        })
        .collect()
}

/// `(sim draws, visual draws)` of the world's two streams.
pub fn draws(fx: &Fixture) -> (u64, u64) {
    let r = fx.w.rng.0.borrow();
    (r.sim.draws(), r.visual.draws())
}

/// Sum of the armor values of a mover.
pub fn total_armor(fx: &Fixture, h: ObjectHandle) -> f32 {
    loadout_of(fx.w.objects.get(h).unwrap())
        .unwrap()
        .armor
        .iter()
        .map(|a| a.0)
        .sum()
}

/// The `BF` lines of a replay, each with the bolts it lists: `(step, [(pool index, "ht" flag, the bracket text)])`.
pub type BfLine = (i64, Vec<(usize, bool, String)>);

pub fn bf_bolts(lines: &[String]) -> Vec<BfLine> {
    lines
        .iter()
        .filter(|l| l.starts_with("BF "))
        .map(|l| {
            let step: i64 = l.split_whitespace().nth(1).unwrap().parse().unwrap();
            let bolts = l
                .split('[')
                .skip(1)
                .map(|b| {
                    let b = b.split(']').next().unwrap();
                    let i: usize = b.split_whitespace().next().unwrap()[1..].parse().unwrap();
                    (i, b.contains(" ht1 "), b.to_string())
                })
                .collect();
            (step, bolts)
        })
        .collect()
}

/// For each bolt (pool index) of `BF` lines, the step index of its first `ht1`, in first-seen order. A bolt's steps restart at 0 with
/// each `boltstep` directive, so the bolt index plus the position of the `BF` line within its run identify it uniquely only per run:
/// the pairs are `(pool index, step of its first ht1)`.
pub fn first_hit_steps(lines: &[String]) -> Vec<(usize, i64)> {
    let mut out: Vec<(usize, i64)> = Vec::new();
    for (step, bolts) in bf_bolts(lines) {
        for (i, hit, _) in bolts {
            if hit && !out.iter().any(|&(j, _)| j == i) {
                out.push((i, step));
            }
        }
    }
    out
}
