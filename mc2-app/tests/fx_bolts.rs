#![allow(clippy::chunks_exact_to_as_chunks)]

//! Weapon bolts on screen (headless, never a window; skipped without `MC2_DATA` or with
//! `MC2_SKIP_GPU_TESTS=1` when no GPU adapter is available).
//! `mc2_01` runs through `GameCore` with `LiveSim` and the production fx registration (`FxHub` sink + scene). The lance gets an
//! attack order on an enemy; every frame the sim's own bolts (`BoltState`) are read, and on a frame with a laser/beam bolt in flight
//! and one with a projectile in flight the frame is rendered twice: as it is, and with the bolt effects ended. The pixels that
//! differ must lie along the bolt's projected segment (launch point -> target, computed here from the sim's positions and the camera).

#[path = "common/mod.rs"]
mod common;

use mc2_app::fx_sink::FxHub;
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::layers::{OrderContext, OrderHook};
use mc2_app::live_sim::LiveSim;
use mc2_sim::combat::weapon_bolt::{bolt_params, bolt_state};
use mc2_sim::combat::HOME_COMMANDER;
use mc2_sim::command::ScriptedOrder;
use mc2_sim::object::Kind;
use mc2_sim::{ObjectHandle, PartId, PlayerCommand};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

type Commands = Rc<RefCell<Vec<PlayerCommand>>>;

struct Queue(Commands);

impl OrderHook<PlayerCommand> for Queue {
    fn emits_sim_commands(&self) -> bool {
        true
    }
    fn update(&mut self, _ctx: &OrderContext<'_>, out: &mut Vec<PlayerCommand>) {
        out.append(&mut self.0.borrow_mut());
    }
}

fn movers(w: &mc2_sim::World, home: bool) -> Vec<(PartId, mc2_sim::math::Vec3)> {
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
            ((commander == HOME_COMMANDER) == home).then_some((o.c.part_id, o.c.position))
        })
        .collect()
}

/// One bolt the sim is flying: handle, beam or projectile, current `laserPosition`, target.
struct Flying {
    handle: ObjectHandle,
    beam: bool,
    target: mc2_sim::math::Vec3,
}

fn flying(w: &mc2_sim::World) -> Vec<Flying> {
    let mut out = Vec::new();
    for &h in &w.objects.weapons {
        if !w.objects.get(h).is_some_and(|o| o.exists()) {
            continue;
        }
        let (Some(b), Some(p)) = (bolt_state(w, h), bolt_params(w, h)) else {
            continue;
        };
        if b.just_created || !b.has_target_position || b.hit_target {
            continue;
        }
        out.push(Flying {
            handle: h,
            beam: p.is_beam,
            target: b.target_position,
        });
    }
    out
}

fn render_rgba(run: &mut HeadlessRun<LiveSim>) -> Vec<u8> {
    let gpu = run.core.scene.gpu_arc().clone();
    let target = run.target.target();
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bolts"),
        });
    run.core.render(&mut enc, &target).unwrap();
    let rb = target.encode_readback(&gpu, &mut enc).unwrap();
    gpu.queue.submit(Some(enc.finish()));
    rb.finish(&gpu).unwrap()
}

fn dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    };
    ((p[0] - a[0] - t * dx).powi(2) + (p[1] - a[1] - t * dy).powi(2)).sqrt()
}

fn data_png_path() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bolts-mc2_01.png")
}

fn save_png(path: &std::path::Path, rgba: &[u8]) {
    // half size keeps the committed file small
    let (w, h) = (960usize, 540usize);
    let mut px = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * 2) * 1920 + x * 2) * 4;
            px.extend_from_slice(&rgba[i..i + 3]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&px).unwrap();
}

/// What the render of one in-flight bolt shows: differing pixels along its segment, and in all.
struct Evidence {
    frame: usize,
    /// Screen length of the beam.
    len: f32,
    /// Differing pixels within 12 px of the middle 40% of the beam's span.
    interior: usize,
    total: usize,
}

#[test]
fn live_laser_and_projectile_bolts_are_drawn_along_their_flight() {
    let Some(data) = std::env::var_os("MC2_DATA").map(PathBuf::from) else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(run) => run,
        Err(e) if common::skip_gpu_test_without_adapter(&e) => {
            return;
        }
        Err(e) => panic!("fx_bolts headless setup failed: {e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    let hub = FxHub::new(&mut run.core.scene, &mut run.core.game.vfs, 7, true)
        .expect("effect library and textures");
    let queue: Commands = Rc::new(RefCell::new(Vec::new()));
    {
        let rt = run.core.mission.as_ref().unwrap();
        hub.collect_bolt_types(&rt.driver.sim().world);
    }
    hub.upload_pending_textures(&mut run.core.game.vfs, &mut run.core.scene);
    run.core.sinks.push(Box::new(hub.sink()));
    run.core.effects = Some(Box::new(hub.scene()));
    run.core.orders = Box::new(Queue(queue.clone()));

    // the opening camera pan has to be over before the lance takes orders
    let mut waited = 0;
    while run.core.mission.as_ref().unwrap().rig.goal_pos_time > 0.0 && waited < 1800 {
        run.step(None).unwrap();
        waited += 1;
    }
    let (lance, enemy) = {
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        (movers(w, true), movers(w, false)[0])
    };
    let (target, ep) = enemy;
    // march close, then attack (an attack order alone does not start a march on an unseen target)
    for (i, (part, _)) in lance.iter().enumerate() {
        queue
            .borrow_mut()
            .push(PlayerCommand::Scripted(ScriptedOrder::Move {
                part: *part,
                x: ep.x + 250.0 + 60.0 * i as f32,
                y: ep.y - 250.0,
                run: true,
            }));
    }
    for marched in 0.. {
        run.step(None).unwrap();
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let close = movers(w, true)
            .iter()
            .any(|(_, p)| ((p.x - ep.x).powi(2) + (p.y - ep.y).powi(2)).sqrt() < 800.0);
        if close {
            break;
        }
        assert!(marched < 6000, "the lance never got near the enemy");
    }
    for (part, _) in &lance {
        queue
            .borrow_mut()
            .push(PlayerCommand::Scripted(ScriptedOrder::Attack {
                part: *part,
                target,
            }));
    }

    let mut launch: HashMap<ObjectHandle, (usize, mc2_sim::math::Vec3)> = HashMap::new();
    let mut beam_ev: Vec<Evidence> = Vec::new();
    let mut shot_ev: Option<(usize, usize)> = None;
    let mut kinds_seen = (0usize, 0usize);
    for frame in 0..3000 {
        // keep the fight on screen: between the lance and the enemy
        {
            let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
            let mid = movers(w, true)
                .first()
                .map(|(_, p)| mc2_sim::math::Vec3 {
                    x: (p.x + ep.x) / 2.0,
                    y: (p.y + ep.y) / 2.0,
                    z: 0.0,
                })
                .unwrap_or(ep);
            let rt = run.core.mission.as_mut().unwrap();
            let ground = rt.view.ground();
            rt.rig.set_position(
                mc2_render::math::Vec3::new(mid.x, mid.y, 0.0),
                false,
                &ground,
                1.0 / 30.0,
            );
        }
        run.step(None).unwrap();
        let bolts = {
            let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
            let f = flying(w);
            for b in &f {
                launch.entry(b.handle).or_insert_with(|| {
                    // the first frame a bolt is seen it is still at its muzzle
                    (frame, bolt_state(w, b.handle).unwrap().laser_position)
                });
            }
            f
        };
        for b in bolts {
            if b.beam {
                kinds_seen.0 += 1;
            } else {
                kinds_seen.1 += 1;
            }
            // the effect has flown a few frames (its particles exist from its second update)
            if frame < launch[&b.handle].0 + 4 {
                continue;
            }
            // A projectile is sampled once after its trail has had time to
            // appear. If the first one is outside the camera or invisible in
            // this capture, keep looking for an on-screen projectile rather
            // than treating its empty frame as the evidence for the test.
            if !b.beam && frame != launch[&b.handle].0 + 4 {
                continue;
            }
            if b.beam && (beam_ev.len() >= 2 || beam_ev.iter().any(|e| e.frame == frame)) {
                continue;
            }
            if !b.beam && shot_ev.is_some() {
                continue;
            }
            // a projectile whose effects were ended by an earlier comparison is not evidence of the trail
            if !b.beam && hub.bolt_effects().is_empty() {
                continue;
            }
            let (from, head, time_left) = {
                let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
                let o = w.objects.get(b.handle).unwrap();
                (
                    o.c.position,
                    bolt_state(w, b.handle).unwrap().laser_position,
                    bolt_state(w, b.handle).unwrap().time_left,
                )
            };
            let cam = run
                .core
                .mission
                .as_ref()
                .unwrap()
                .render_camera((1920.0, 1080.0), run.core.config.interpolate_camera);
            let project = |p: mc2_sim::math::Vec3| {
                let s = cam.project_z(mc2_render::math::Vec3::new(p.x, p.y, p.z)).0;
                [s[0], s[1]]
            };
            if !b.beam {
                let h = project(head);
                // The battle can be behind the camera for a few frames while its rig settles. Pixel evidence is meaningful only
                // once the projectile head itself lies in the render target; otherwise this would record a zero-pixel off-screen
                // sample and prevent a later visible projectile from supplying the evidence.
                if !(0.0..1920.0).contains(&h[0]) || !(0.0..1080.0).contains(&h[1]) {
                    continue;
                }
            }
            // everything on, then the bolt effects ended and the bolt faces off, then the faces alone
            let full = render_rgba(&mut run);
            hub.kill_bolt_effects();
            hub.set_draw_bolt_faces(false);
            let nothing = render_rgba(&mut run);
            hub.set_draw_bolt_faces(true);
            let faces = render_rgba(&mut run);
            hub.set_draw_bolt_faces(true);
            let differ = |x: &[u8], y: &[u8], on: &dyn Fn(f32, f32) -> bool| {
                let mut n = 0usize;
                for py in 0..1080usize {
                    for px in 0..1920usize {
                        let i = (py * 1920 + px) * 4;
                        if x[i..i + 4] != y[i..i + 4] && on(px as f32, py as f32) {
                            n += 1;
                        }
                    }
                }
                n
            };
            if b.beam {
                // the beam is its faces: muzzle to target, so pixels differ in the INTERIOR of the span, away from both ends
                // (the target-side gosFX effect lives at the ends)
                let (a, t) = (project(from), project(b.target));
                let len = ((t[0] - a[0]).powi(2) + (t[1] - a[1]).powi(2)).sqrt();
                let (p0, p1) = (
                    [a[0] + 0.3 * (t[0] - a[0]), a[1] + 0.3 * (t[1] - a[1])],
                    [a[0] + 0.7 * (t[0] - a[0]), a[1] + 0.7 * (t[1] - a[1])],
                );
                let interior = differ(&faces, &nothing, &|x, y| {
                    dist_to_segment([x, y], p0, p1) < 12.0
                });
                let total = differ(&faces, &nothing, &|_, _| true);
                eprintln!(
                    "frame {frame}: beam {from:?} -> {:?}, screen {a:?} -> {t:?} ({len:.0} px), time_left {time_left}: {interior} interior pixels, {total} in all",
                    b.target
                );
                if beam_ev.is_empty()
                    && (std::env::var_os("MC2_WRITE_EVIDENCE").is_some()
                        || !data_png_path().exists())
                {
                    save_png(&data_png_path(), &full);
                }
                beam_ev.push(Evidence {
                    frame,
                    len,
                    interior,
                    total,
                });
            } else {
                // a projectile is its trail effect (the full frame) and, without one, its diamond at the head
                let (a, h) = (project(from), project(head));
                let trail = differ(&full, &nothing, &|x, y| {
                    dist_to_segment([x, y], a, h) < 60.0
                });
                let diamond = differ(&faces, &nothing, &|x, y| {
                    dist_to_segment([x, y], h, h) < 40.0
                });
                eprintln!(
                    "frame {frame}: projectile {from:?} -> {head:?}, screen {a:?} -> {h:?}: {trail} trail pixels, {diamond} diamond pixels at the head"
                );
                if trail >= 30 && diamond >= 5 {
                    shot_ev = Some((trail, diamond));
                }
            }
        }
        if beam_ev.len() >= 2 && shot_ev.is_some() {
            break;
        }
    }
    eprintln!(
        "bolt-frames seen: beams {} projectiles {}",
        kinds_seen.0, kinds_seen.1
    );
    assert!(!beam_ev.is_empty(), "no beam (laser) bolt was in flight");
    let (trail, diamond) = shot_ev.expect("no projectile (missile/ballistic) bolt was in flight");
    for e in &beam_ev {
        assert!(
            e.len > 30.0 && e.interior >= 40,
            "frame {}: the beam's interior shows only {} pixels ({} in all, {:.0} px long)",
            e.frame,
            e.interior,
            e.total,
            e.len
        );
    }
    assert!(
        beam_ev.len() >= 2,
        "the beam must be seen at two frames of its flight"
    );
    assert!(
        trail >= 30,
        "the projectile's trail shows only {trail} pixels along its flight"
    );
    assert!(
        diamond >= 5,
        "a projectile without its trail shows only {diamond} diamond pixels"
    );
    assert!(
        !hub.unavailable().keys().any(|k| k.contains("trail")),
        "{:?}",
        hub.unavailable()
    );
}
