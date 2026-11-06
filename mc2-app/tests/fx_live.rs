// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
#![allow(clippy::chunks_exact_to_as_chunks)]

//! The effect sink on the live simulation (headless, never a window; skipped without `MC2_DATA` or a
//! headless GPU adapter). The latter is an execution-environment prerequisite: the assertions below still
//! run on GPU-capable package-test hosts, while CPU-only compile/gate hosts can validate the rest of the
//! workspace without claiming a render result.
//! `mc2_01` runs through `GameCore` with `LiveSim`; a player mech's weapon is fired at an enemy through `fire_weapon` directly
//! (attack orders reach firing only with the sim-attack lane; the attack-order -> effect -> render evidence is the P3 integration slice's); the
//! `FxEventSink` of `mc2_app::fx_sink` is pushed into `GameCore::sinks` here (production registration is the
//! P3 integration slice's one line).

use mc2_app::fx_sink::{FxHub, FxRole};
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::layers::{OrderContext, OrderHook};
use mc2_app::live_sim::LiveSim;
use mc2_app::sim::{FrameContext, SimEventSink};
use mc2_fx::system::game_to_fx;
use mc2_sim::combat::HOME_COMMANDER;
use mc2_sim::command::ScriptedOrder;
use mc2_sim::object::Kind;
use mc2_sim::{PartId, PlayerCommand, SimEvent, WeaponImpact};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

/// What the game produced that the effect sink is built for.
#[derive(Clone, Default)]
struct Seen(Rc<RefCell<Vec<SimEvent>>>);

impl SimEventSink<LiveSim> for Seen {
    fn on_frame(&mut self, _ctx: &FrameContext<'_, LiveSim>, events: &[SimEvent]) {
        self.0.borrow_mut().extend(
            events
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        SimEvent::WeaponEffect { .. }
                            | SimEvent::Explosion { .. }
                            | SimEvent::Crater { .. }
                            | SimEvent::Flash { .. }
                    )
                })
                .cloned(),
        );
    }
}

/// Hands the queued commands to the simulation on the next game frame.
struct Queue(Rc<RefCell<Vec<PlayerCommand>>>);

impl OrderHook<PlayerCommand> for Queue {
    fn emits_sim_commands(&self) -> bool {
        true
    }
    fn update(&mut self, _ctx: &OrderContext<'_>, out: &mut Vec<PlayerCommand>) {
        out.append(&mut self.0.borrow_mut());
    }
}

/// The enemy mover (any non-home commander) and its position.
fn an_enemy(w: &mc2_sim::World) -> (PartId, mc2_sim::math::Vec3) {
    w.objects
        .movers
        .iter()
        .filter_map(|&h| w.objects.get(h))
        .filter(|o| o.exists())
        .find_map(|o| {
            let commander = match &o.kind {
                Kind::Mech(m) => i32::from(m.m.commander.0),
                Kind::Vehicle(v) => i32::from(v.m.commander.0),
                _ => return None,
            };
            (commander != HOME_COMMANDER).then_some((o.c.part_id, o.c.position))
        })
        .expect("mc2_01 has an enemy mover")
}

type Commands = Rc<RefCell<Vec<PlayerCommand>>>;

fn start() -> Option<(HeadlessRun<LiveSim>, FxHub, Seen, Commands)> {
    let data = data()?;
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(r) => r,
        Err(e)
            if e.contains("no suitable GPU adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1") =>
        {
            eprintln!("fx_live needs a GPU adapter ({e}); MC2_SKIP_GPU_TESTS=1, skipping");
            return None;
        }
        Err(e) => panic!("fx_live setup failed: {e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    let hub = FxHub::new(&mut run.core.scene, &mut run.core.game.vfs, 7, true)
        .expect("effect library and textures");
    let seen = Seen::default();
    let queue = Rc::new(RefCell::new(Vec::new()));
    run.core.sinks.push(Box::new(seen.clone()));
    // the P3 integration slice does exactly these two lines
    run.core.sinks.push(Box::new(hub.sink()));
    run.core.effects = Some(Box::new(hub.scene()));
    run.core.orders = Box::new(Queue(queue.clone()));
    Some((run, hub, seen, queue))
}

/// The row `id` (0-based effect id) of `effects.csv`, column `col`, parsed here from the shipped bytes.
fn csv_name(run: &mut HeadlessRun<LiveSim>, id: usize, col: usize) -> String {
    let bytes = run
        .core
        .game
        .vfs
        .read("data/objects/effects.csv")
        .unwrap()
        .expect("effects.csv");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    text.lines()
        .nth(id + 1)
        .and_then(|l| l.split(',').nth(col))
        .unwrap_or("NONE")
        .trim()
        .to_string()
}

fn render_rgba(run: &mut HeadlessRun<LiveSim>) -> Vec<u8> {
    let gpu = run.core.scene.gpu_arc().clone();
    let target = run.target.target();
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fx") });
    run.core.render(&mut enc, &target).unwrap();
    let rb = target.encode_readback(&gpu, &mut enc).unwrap();
    gpu.queue.submit(Some(enc.finish()));
    rb.finish(&gpu).unwrap()
}

#[test]
fn a_weapon_hit_on_a_live_mc2_01_enemy_spawns_the_data_named_explosion_near_it_and_draws_it() {
    let Some((mut run, hub, seen, queue)) = start() else {
        return;
    };
    for _ in 0..5 {
        run.step(None).unwrap();
    }
    let (part, pos) = an_enemy(&run.core.mission.as_ref().unwrap().driver.sim().world);
    // look at the target so the effect is on screen
    {
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.set_position(
            mc2_render::math::Vec3::new(pos.x, pos.y, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
    }
    for _ in 0..2 {
        run.step(None).unwrap();
    }
    assert!(hub.spawned().is_empty(), "no effect before the shot");
    // `--hit-script`: a weapon shot (`WeaponShotInfo`) on the enemy, the damage code then blows it up
    queue.borrow_mut().push(PlayerCommand::ScriptedHit {
        part,
        damage: 500.0,
    });
    let mut waited = 0;
    while hub.spawned().is_empty() {
        run.step(None).unwrap();
        waited += 1;
        assert!(
            waited < 60,
            "no explosion effect within 60 frames of the hit"
        );
    }
    for _ in 0..4 {
        run.step(None).unwrap();
    }

    // 1. which effect: every Explosion event the sim sent has the effects.csv name (column 1), and is running at its position
    let events = seen.0.borrow().clone();
    let explosions: Vec<(i32, Option<i32>, mc2_sim::math::Vec3, f32)> = events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Explosion {
                effect_id,
                sound_id,
                at,
                radius,
            } => Some((*effect_id, *sound_id, *at, *radius)),
            _ => None,
        })
        .collect();
    assert!(
        !explosions.is_empty(),
        "the hit produced no Explosion event"
    );
    assert!(
        explosions
            .iter()
            .any(|(_, sound_id, _, _)| sound_id.is_some()),
        "the shipped hit explosion carries a parsed SoundEffectId"
    );
    let spawned = hub.spawned();
    let mut expected_named = 0;
    for &(id, _, at, _) in &explosions {
        let want = csv_name(&mut run, id as usize, 1);
        if want == "NONE" {
            continue;
        }
        expected_named += 1;
        assert!(
            spawned.iter().any(|s| s.role == FxRole::Explosion
                && s.name.eq_ignore_ascii_case(&want)
                && (s.at.x - at.x).abs() < 1.0
                && (s.at.y - at.y).abs() < 1.0
                && (s.at.z - at.z).abs() < 1.0),
            "effect '{want}' (id {id}) not spawned at {at:?}: {spawned:?}"
        );
    }
    assert!(
        expected_named > 0,
        "all explosions were NONE; nothing to check"
    );

    // 2. live particles near the event: running effects with geometry within 300 units of the event (gosFX frame)
    let cam = run
        .core
        .mission
        .as_ref()
        .unwrap()
        .render_camera((1920.0, 1080.0), run.core.config.interpolate_camera);
    let list = hub.draw_list(&cam);
    assert!(list.triangle_count() > 0, "the explosion draws nothing");
    let (_, _, at, _) = explosions
        .iter()
        .copied()
        .find(|&(id, _, _, _)| csv_name(&mut run, id as usize, 1) != "NONE")
        .unwrap();
    let c = game_to_fx(at.x, at.y, at.z);
    let near = list
        .batches
        .iter()
        .flat_map(|b| &b.vertices)
        .filter(|v| {
            let d = [v.pos[0] - c.x, v.pos[1] - c.y, v.pos[2] - c.z];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < 300.0
        })
        .count();
    assert!(near > 0, "no effect geometry within 300 units of {at:?}");
    assert!(hub.active_count() > 0);

    // 3. the rendered frame differs from the frame without the sink's effects, around the event's screen position
    let with = render_rgba(&mut run);
    let effects = run.core.effects.take();
    let without = render_rgba(&mut run);
    run.core.effects = effects;
    let (screen, _) = cam.project_z(mc2_render::math::Vec3::new(at.x, at.y, at.z));
    let (w, h) = (1920usize, 1080usize);
    let tol = 400.0; // explosion sprites are a few hundred pixels wide at the default zoom
    let (mut inside, mut outside) = (0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            if with[i..i + 4] != without[i..i + 4] {
                let near = (x as f32 - screen[0]).abs() < tol && (y as f32 - screen[1]).abs() < tol;
                if near {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    eprintln!("screen {screen:?}: {inside} pixels differ inside the region, {outside} outside");
    assert!(inside > 200, "only {inside} pixels differ near {screen:?}");
    assert_eq!(outside, 0, "pixels differ away from the event");

    // anything the sim sent that has no effect here is reported, not dropped
    let un = hub.unavailable();
    assert!(!un.keys().any(|k| k.contains("building flash")), "{un:?}");
    // a crater event is a decal in the hub's pool, not an effect and not unavailable
    let craters = events
        .iter()
        .filter(|e| matches!(e, SimEvent::Crater { .. }))
        .count();
    assert_eq!(hub.craters().len(), craters.min(1000), "{un:?}");
    assert!(!un.keys().any(|k| k.contains("crater")), "{un:?}");
}

/// The home movers (the player's lance).
fn home_movers(w: &mc2_sim::World) -> Vec<PartId> {
    w.objects
        .movers
        .iter()
        .filter_map(|&h| w.objects.get(h))
        .filter(|o| o.exists())
        .filter_map(|o| {
            let commander = match &o.kind {
                Kind::Mech(m) => i32::from(m.m.commander.0),
                Kind::Vehicle(v) => i32::from(v.m.commander.0),
                _ => return None,
            };
            (commander == HOME_COMMANDER).then_some(o.c.part_id)
        })
        .collect()
}

#[test]
fn a_live_fired_weapon_spawns_its_data_named_muzzle_and_impact_effects_and_draws_them() {
    let Some((mut run, hub, seen, queue)) = start() else {
        return;
    };
    // the mission script's opening camera pan has to be over before the lance takes orders
    let mut waited = 0;
    while run.core.mission.as_ref().unwrap().rig.goal_pos_time > 0.0 && waited < 1800 {
        run.step(None).unwrap();
        waited += 1;
    }
    eprintln!("opening script over after {waited} frames");
    let (target, _) = an_enemy(&run.core.mission.as_ref().unwrap().driver.sim().world);
    let lance = home_movers(&run.core.mission.as_ref().unwrap().driver.sim().world);
    // walk the lance to a point near the enemy first (an attack order alone does not start a march on an unseen target), then attack
    let (_, ep) = an_enemy(&run.core.mission.as_ref().unwrap().driver.sim().world);
    for (i, &part) in lance.iter().enumerate() {
        queue
            .borrow_mut()
            .push(PlayerCommand::Scripted(ScriptedOrder::Move {
                part,
                x: ep.x + 250.0 + 60.0 * i as f32,
                y: ep.y - 250.0,
                run: true,
            }));
    }
    let mut marched = 0;
    loop {
        run.step(None).unwrap();
        marched += 1;
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let close = lance
            .iter()
            .filter_map(|p| w.objects.find_by_part_id(*p))
            .filter_map(|h| w.objects.get(h))
            .any(|o| {
                ((o.c.position.x - ep.x).powi(2) + (o.c.position.y - ep.y).powi(2)).sqrt() < 800.0
            });
        if close || marched >= 6000 {
            eprintln!("marched {marched} frames, close = {close}");
            break;
        }
    }
    // `Mover::fireWeapon` (the sim's public weapon entry): the first lance weapon whose fire gate lets it shoot the enemy
    let target_wid = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let th = w.objects.find_by_part_id(target).unwrap();
        w.objects.watch_id(th)
    };
    // look at the lance's first mech and the enemy so the bolt's effects are on screen
    {
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let a = w.objects.find_by_part_id(lance[0]).unwrap();
        let (ap, tp) = (w.objects.get(a).unwrap().c.position, an_enemy(w).1);
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.set_position(
            mc2_render::math::Vec3::new((ap.x + tp.x) / 2.0, (ap.y + tp.y) / 2.0, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
    }
    for _ in 0..2 {
        run.step(None).unwrap();
    }
    let mark = seen.0.borrow().len();
    let mut shooter = None;
    let mut fired = false;
    let mut codes = std::collections::BTreeSet::new();
    'tries: for _ in 0..40 {
        for &part in &lance {
            for weapon in 0..64 {
                let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
                let Some(h) = w.objects.find_by_part_id(part) else {
                    continue;
                };
                let t = w.objects.by_watch_id(target_wid);
                let r = mc2_sim::combat::fire::fire_weapon(w, h, weapon, t, -1, None, 0);
                if r.code == 0 {
                    shooter = Some(w.objects.watch_id(h));
                    fired = true;
                    break 'tries;
                }
                codes.insert(r.code);
            }
        }
        for _ in 0..15 {
            run.step(None).unwrap();
        }
    }
    assert!(
        fired,
        "no lance weapon could fire at the enemy; fireWeapon codes {codes:?}"
    );
    // run until a bolt launched and then landed (hit or miss), as the weapon code really does it
    let launch = |e: &SimEvent| {
        matches!(
            e,
            SimEvent::WeaponEffect {
                impact: WeaponImpact::Launch,
                ..
            }
        )
    };
    let landing = |e: &SimEvent| {
        matches!(
            e,
            SimEvent::WeaponEffect {
                impact: WeaponImpact::Hit | WeaponImpact::Miss,
                ..
            }
        )
    };
    let mut frames = 0;
    let shooter_id = shooter.expect("a weapon fired");
    let own =
        |e: &&SimEvent| matches!(e, SimEvent::WeaponEffect { owner, .. } if *owner == shooter_id);
    // the shooter's own bolt must launch and land (another mech's landing does not follow this shot)
    while !(seen.0.borrow()[mark..].iter().filter(own).any(launch)
        && seen.0.borrow()[mark..].iter().filter(own).any(landing))
    {
        run.step(None).unwrap();
        frames += 1;
        assert!(
            frames < 600,
            "no weapon fired and landed within {frames} frames"
        );
    }
    // Let the sink's pending effects start and emit their first particles before sampling them.
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    let shooter = shooter.unwrap();
    // only this shot's events: after the fire call, owned by the mech that fired
    let events: Vec<SimEvent> = seen.0.borrow()[mark..]
        .iter()
        .filter(|e| matches!(e, SimEvent::WeaponEffect { owner, .. } if *owner == shooter))
        .cloned()
        .collect();
    let spawned = hub.spawned();
    eprintln!(
        "{frames} frames; {} weapon events; spawned {spawned:?}",
        events.len()
    );

    // ONE weapon event is followed from the sink to the screen: the last (launch or landing) whose effect the data names
    let mut candidates: Vec<(
        FxRole,
        String,
        mc2_sim::math::Vec3,
        mc2_fx::system::FxHandle,
    )> = Vec::new();
    for e in &events {
        let SimEvent::WeaponEffect {
            effect_id,
            from,
            to,
            impact,
            ..
        } = e
        else {
            continue;
        };
        let id = *effect_id as usize;
        let (role, want, at) = match impact {
            WeaponImpact::Launch => (FxRole::MuzzleFlash, csv_name(&mut run, id, 2), *from),
            WeaponImpact::Hit => (FxRole::Hit, csv_name(&mut run, id, 3), *to),
            WeaponImpact::Miss => {
                let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
                let water = mc2_app::fx_sink::over_water_cell(&w.terrain, to);
                if water {
                    // WATER_MISS_FX 76 (not for machine guns, effect 11)
                    let name = if id == 11 {
                        "NONE".into()
                    } else {
                        csv_name(&mut run, 76, 1)
                    };
                    (FxRole::WaterMiss, name, *to)
                } else {
                    (FxRole::Miss, csv_name(&mut run, id, 5), *to)
                }
            }
        };
        if want == "NONE" || want.is_empty() {
            continue;
        }
        // a miss never starts the hit effect (weaponbolt.cpp:614 is the target branch only)
        if *impact == WeaponImpact::Miss {
            assert!(
                !spawned.iter().any(|s| s.role == FxRole::Hit
                    && (s.at.x - to.x).abs() < 1.0
                    && (s.at.y - to.y).abs() < 1.0),
                "a miss at {to:?} spawned a hit effect"
            );
        }
        assert!(
            spawned.iter().any(|s| s.role == role
                && s.name.eq_ignore_ascii_case(&want)
                && (s.at.x - at.x).abs() < 1.0
                && (s.at.y - at.y).abs() < 1.0),
            "{role:?} '{want}' (effect {id}) not spawned at {at:?}: {spawned:?}"
        );
        // the effect this event started (matched by role, name and position) and that is still burning when the frame is rendered
        if let Some(s) = spawned.iter().rev().find(|s| {
            s.role == role
                && s.name.eq_ignore_ascii_case(&want)
                && (s.at.x - at.x).abs() < 1.0
                && (s.at.y - at.y).abs() < 1.0
                && hub.is_alive(s.handle)
        }) {
            if !candidates.iter().any(|candidate| candidate.3 == s.handle) {
                candidates.push((role, want, at, s.handle));
            }
        }
    }
    assert!(
        !candidates.is_empty(),
        "no effect started by this shot is still alive: {events:?}; unavailable: {:?}",
        hub.unavailable()
    );
    let cam = run
        .core
        .mission
        .as_ref()
        .unwrap()
        .render_camera((1920.0, 1080.0), run.core.config.interpolate_camera);
    let mut drawable_candidates = Vec::new();
    for (role, name, at, handle) in candidates {
        assert!(hub.is_alive(handle), "{role:?} '{name}' handle is alive");
        let list = hub.draw_list_of(handle, &cam);
        let c = game_to_fx(at.x, at.y, at.z);
        let near = list
            .batches
            .iter()
            .flat_map(|b| &b.vertices)
            .filter(|v| {
                let d = [v.pos[0] - c.x, v.pos[1] - c.y, v.pos[2] - c.z];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < 300.0
            })
            .count();
        if near > 0 {
            drawable_candidates.push((role, name, at, handle));
        }
    }
    assert!(
        !drawable_candidates.is_empty(),
        "no live shot effect has geometry within 300 units of its event position: {events:?}"
    );
    // Measure each effect's isolated contribution and retain the strongest visible one.
    let (w, h) = (1920usize, 1080usize);
    let tol = 250.0;
    let mut best: Option<(usize, FxRole, String, mc2_sim::math::Vec3)> = None;
    for (role, name, at, handle) in drawable_candidates {
        let marks = cam
            .project_z(mc2_render::math::Vec3::new(at.x, at.y, at.z))
            .0;
        let with = render_rgba(&mut run);
        hub.kill(handle);
        let without = render_rgba(&mut run);
        assert!(!hub.is_alive(handle), "the selected effect was ended");
        let mut inside = 0usize;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                if with[i..i + 4] != without[i..i + 4]
                    && (x as f32 - marks[0]).abs() < tol
                    && (y as f32 - marks[1]).abs() < tol
                {
                    inside += 1;
                }
            }
        }
        eprintln!("{inside} pixels differ near {marks:?} for {role:?} '{name}'");
        if best.as_ref().is_none_or(|(count, ..)| inside > *count) {
            best = Some((inside, role, name, at));
        }
    }
    let (inside, role, name, at) = best.expect("at least one live effect candidate");
    assert!(
        inside > 0,
        "strongest effect {role:?} '{name}' at {at:?} changes no nearby pixels"
    );
}

/// Crater events through the production sink: the hub drops them on the live world's terrain, the scene pass draws them
/// (`CraterManager::render`) and a new mission (`mission_start`) clears them.
#[test]
fn crater_events_become_decals_the_scene_pass_draws() {
    let Some((mut run, hub, _seen, _queue)) = start() else {
        return;
    };
    for _ in 0..5 {
        run.step(None).unwrap();
    }
    let (_, pos) = an_enemy(&run.core.mission.as_ref().unwrap().driver.sim().world);
    {
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.set_position(
            mc2_render::math::Vec3::new(pos.x, pos.y, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
    }
    for _ in 0..2 {
        run.step(None).unwrap();
    }
    let before = render_rgba(&mut run);
    assert!(hub.craters().is_empty());

    let events = [
        SimEvent::Crater {
            kind: mc2_sim::crater::CRATER_1 + 3,
            at: pos,
            rotation: 30.0,
        },
        SimEvent::Crater {
            kind: 2,
            at: mc2_sim::math::Vec3 {
                x: pos.x + 60.0,
                y: pos.y,
                z: pos.z,
            },
            rotation: 0.0,
        },
    ];
    {
        let world = &run.core.mission.as_ref().unwrap().driver.sim().world;
        hub.handle_events(Some(world), &events);
    }
    let pool = hub.craters();
    assert_eq!(pool.len(), 2);
    let ground = {
        let world = &run.core.mission.as_ref().unwrap().driver.sim().world;
        world.terrain.elevation(&pos)
    };
    let c = pool.slot(0).unwrap();
    assert_eq!(c.shape_id, mc2_sim::crater::CRATER_1 + 3);
    // the corners sit on the terrain of the live world
    let mean = c.corners.iter().map(|p| p.z).sum::<f32>() / 4.0;
    assert!((mean - ground).abs() < 60.0, "{mean} vs {ground}");

    let mut batches = Vec::new();
    hub.decal_batches([0.3; 3], &mut batches);
    assert_eq!(batches.len(), 2, "a crater batch and a footprint batch");

    let after = render_rgba(&mut run);
    let changed = before
        .chunks_exact(4)
        .zip(after.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 100, "{changed} pixels differ with two decals");

    // the preference off: existing decals are no longer drawn (CraterManager::render) ...
    hub.set_use_non_weapon_effects(false);
    let mut off = Vec::new();
    hub.decal_batches([0.3; 3], &mut off);
    assert!(off.is_empty());
    let hidden = render_rgba(&mut run);
    assert_eq!(
        hidden, before,
        "decals vanish from the pixels when effects are off"
    );
    // ... and back on they return
    hub.set_use_non_weapon_effects(true);
    assert_eq!(render_rgba(&mut run), after);
    hub.set_use_non_weapon_effects(false);
    // addCrater returns at once
    {
        let world = &run.core.mission.as_ref().unwrap().driver.sim().world;
        hub.handle_events(Some(world), &events);
    }
    assert_eq!(hub.craters().len(), 2);

    // a new mission empties the pool
    hub.reset();
    assert!(hub.craters().is_empty());
}
