#![allow(clippy::chunks_exact_to_as_chunks)]

//! Appearance-attached effects on screen (headless, never a window; skipped without `MC2_DATA` or with `MC2_SKIP_GPU_TESTS=1` when
//! no GPU adapter is available). `mc2_01` runs through `GameCore` with `LiveSim` and the production fx registration (`FxHub` sink +
//! scene), with the player's non-weapon-effects preference on. A home mech is ordered to walk, then to jump; every frame the hub's
//! started effects are read. The first foot poof of the walk and the jump jet exhaust of the jump must start at the mech (the
//! foot node / the jump node), and the frames after each start are rendered into a strip (`<target tmp>/visual/appearance-fx-walk.png`,
//! `<target tmp>/visual/appearance-fx-jump.png`). The effects are then ended and the frame is rendered again: the pixels that differ lie
//! around the mech.

#[path = "common/mod.rs"]
mod common;

use mc2_app::fx_sink::{FxHub, FxRole};
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::layers::{OrderContext, OrderHook};
use mc2_app::live_sim::LiveSim;
use mc2_sim::command::ScriptedOrder;
use mc2_sim::object::Kind;
use mc2_sim::{FxSlot, ObjectHandle, PartId, PlayerCommand};
use std::cell::RefCell;
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

const W: usize = 1920;
const H: usize = 1080;
/// Side of the square cut out around the mech for the strip.
const CROP: usize = 220;

fn render_rgba(run: &mut HeadlessRun<LiveSim>) -> Vec<u8> {
    let gpu = run.core.scene.gpu_arc().clone();
    let target = run.target.target();
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("appearance-fx"),
        });
    run.core.render(&mut enc, &target).unwrap();
    let rb = target.encode_readback(&gpu, &mut enc).unwrap();
    gpu.queue.submit(Some(enc.finish()));
    rb.finish(&gpu).unwrap()
}

fn home_mech(w: &mc2_sim::World) -> (ObjectHandle, PartId) {
    let home = w.teams.home as i32;
    w.objects
        .movers
        .iter()
        .copied()
        .find_map(|h| {
            let o = w.objects.get(h)?;
            (o.exists() && o.team_id() == home && matches!(o.kind, Kind::Mech(_)))
                .then_some((h, o.c.part_id))
        })
        .expect("a home mech")
}

fn mech_pos(run: &HeadlessRun<LiveSim>, h: ObjectHandle) -> mc2_sim::math::Vec3 {
    run.core
        .mission
        .as_ref()
        .unwrap()
        .driver
        .sim()
        .world
        .objects
        .get(h)
        .unwrap()
        .c
        .position
}

/// Screen position of a world point.
fn project(run: &HeadlessRun<LiveSim>, p: mc2_sim::math::Vec3) -> [f32; 2] {
    let cam = run
        .core
        .mission
        .as_ref()
        .unwrap()
        .render_camera((W as f32, H as f32), run.core.config.interpolate_camera);
    let s = cam.project_z(mc2_render::math::Vec3::new(p.x, p.y, p.z)).0;
    [s[0], s[1]]
}

fn follow(run: &mut HeadlessRun<LiveSim>, p: mc2_sim::math::Vec3) {
    let rt = run.core.mission.as_mut().unwrap();
    let ground = rt.view.ground();
    rt.rig.set_position(
        mc2_render::math::Vec3::new(p.x, p.y, 0.0),
        false,
        &ground,
        1.0 / 30.0,
    );
}

/// The `CROP` x `CROP` square centred on `c`, clamped to the frame, as RGB.
fn crop(rgba: &[u8], c: [f32; 2]) -> Vec<u8> {
    let x0 = (c[0] as i64 - CROP as i64 / 2).clamp(0, (W - CROP) as i64) as usize;
    let y0 = (c[1] as i64 - CROP as i64 / 2).clamp(0, (H - CROP) as i64) as usize;
    let mut px = Vec::with_capacity(CROP * CROP * 3);
    for y in 0..CROP {
        for x in 0..CROP {
            let i = ((y0 + y) * W + x0 + x) * 4;
            px.extend_from_slice(&rgba[i..i + 3]);
        }
    }
    px
}

fn differing_near(a: &[u8], b: &[u8], c: [f32; 2], radius: f32) -> usize {
    let mut n = 0;
    for y in 0..H {
        for x in 0..W {
            let (dx, dy) = (x as f32 - c[0], y as f32 - c[1]);
            if dx * dx + dy * dy <= radius * radius {
                let i = (y * W + x) * 4;
                if a[i..i + 4] != b[i..i + 4] {
                    n += 1;
                }
            }
        }
    }
    n
}

fn write_strip(name: &str, frames: &[Vec<u8>]) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("visual");
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (CROP * frames.len(), CROP);
    let mut px = vec![0u8; w * h * 3];
    for (k, f) in frames.iter().enumerate() {
        for y in 0..CROP {
            let dst = (y * w + k * CROP) * 3;
            px[dst..dst + CROP * 3].copy_from_slice(&f[y * CROP * 3..(y + 1) * CROP * 3]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(dir.join(name)).unwrap());
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&px).unwrap();
}

fn is_poof(s: FxSlot) -> bool {
    matches!(s, FxSlot::RightFootPoof(_) | FxSlot::LeftFootPoof(_))
}

fn count_started(hub: &FxHub, pred: impl Fn(FxSlot) -> bool) -> usize {
    hub.spawned()
        .iter()
        .filter(|s| matches!(s.role, FxRole::Appearance(slot) if pred(slot)))
        .count()
}

#[test]
fn a_walking_mech_kicks_up_dust_and_a_jumping_mech_shows_its_exhaust() {
    let Some(data) = std::env::var_os("MC2_DATA").map(PathBuf::from) else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (W as u32, H as u32);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(run) => run,
        Err(e) if common::skip_gpu_test_without_adapter(&e) => return,
        Err(e) => panic!("fx_appearance headless setup failed: {e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    run.core.non_weapon_effects = true;
    let hub = FxHub::new(&mut run.core.scene, &mut run.core.game.vfs, 7, true)
        .expect("effect library and textures");
    let queue: Commands = Rc::new(RefCell::new(Vec::new()));
    hub.collect_bolt_types(&run.core.mission.as_ref().unwrap().driver.sim().world);
    hub.upload_pending_textures(&mut run.core.game.vfs, &mut run.core.scene);
    run.core.sinks.push(Box::new(hub.sink()));
    run.core.effects = Some(Box::new(hub.scene()));
    run.core.orders = Box::new(Queue(queue.clone()));
    let mut waited = 0;
    while run.core.mission.as_ref().unwrap().rig.goal_pos_time > 0.0 && waited < 1800 {
        run.step(None).unwrap();
        waited += 1;
    }
    let (mech, part) = home_mech(&run.core.mission.as_ref().unwrap().driver.sim().world);
    let start = mech_pos(&run, mech);

    // ---- walking: the first foot poof, then the frames after it
    queue
        .borrow_mut()
        .push(PlayerCommand::Scripted(ScriptedOrder::Move {
            part,
            x: start.x + 400.0,
            y: start.y,
            run: false,
        }));
    let mut strip: Vec<Vec<u8>> = Vec::new();
    let mut started_at: Option<usize> = None;
    let mut second_poof: Option<usize> = None;
    let mut diff = 0;
    for frame in 0..1500 {
        let at = mech_pos(&run, mech);
        follow(&mut run, at);
        run.step(None).unwrap();
        let poofs = count_started(&hub, is_poof);
        if started_at.is_none() && poofs > 0 {
            started_at = Some(frame);
            let first = hub
                .spawned()
                .into_iter()
                .find(|s| matches!(s.role, FxRole::Appearance(slot) if is_poof(slot)))
                .unwrap();
            // the poof starts at the foot node: next to the mech, at about ground level
            let p = mech_pos(&run, mech);
            let d = ((first.at.x - p.x).powi(2) + (first.at.y - p.y).powi(2)).sqrt();
            assert!(d < 40.0, "the poof starts {d} units from the mech");
            assert!(
                (first.at.z - p.z).abs() < 25.0,
                "at the foot: z {} vs {}",
                first.at.z,
                p.z
            );
            assert!(!first.name.is_empty());
        }
        if let Some(f0) = started_at {
            let k = frame - f0;
            if k % 3 == 2 && strip.len() < 8 {
                let rgba = render_rgba(&mut run);
                let c = project(&run, mech_pos(&run, mech));
                strip.push(crop(&rgba, [c[0], c[1] - 40.0]));
            }
            if second_poof.is_none() && poofs > 1 && strip.len() >= 8 {
                second_poof = Some(frame);
            }
            if let Some(f1) = second_poof {
                if frame == f1 + 2 {
                    // the next poof is running: ending the effects must change the pixels around the feet
                    let with = render_rgba(&mut run);
                    hub.kill_attached_effects();
                    let without = render_rgba(&mut run);
                    let c = project(&run, mech_pos(&run, mech));
                    diff = differing_near(&with, &without, c, 150.0);
                    break;
                }
            }
        }
    }
    assert!(started_at.is_some(), "no foot poof in the walk");
    assert!(strip.len() >= 6, "{} strip frames", strip.len());
    assert!(
        diff >= 30,
        "ending the dust changes only {diff} pixels around the mech"
    );
    write_strip("appearance-fx-walk.png", &strip);

    // ---- jumping: the exhaust starts at the jump node and follows the mech; the first jump fills the strip, the second
    // jump is the one whose exhaust is ended for the pixel comparison
    for _ in 0..120 {
        run.step(None).unwrap(); // let the walk end
    }
    queue
        .borrow_mut()
        .push(PlayerCommand::Scripted(ScriptedOrder::Stop { part }));
    for _ in 0..60 {
        run.step(None).unwrap();
    }
    let mut strip: Vec<Vec<u8>> = Vec::new();
    let mut diff = 0;
    for jump in 0..2 {
        let before = count_started(&hub, |s| s == FxSlot::JumpJet);
        let p = mech_pos(&run, mech);
        queue
            .borrow_mut()
            .push(PlayerCommand::Scripted(ScriptedOrder::Jump {
                part,
                x: p.x + 90.0,
                y: p.y,
            }));
        let mut f0: Option<usize> = None;
        for frame in 0..900 {
            let at = mech_pos(&run, mech);
            follow(&mut run, at);
            run.step(None).unwrap();
            if f0.is_none() && count_started(&hub, |s| s == FxSlot::JumpJet) > before {
                f0 = Some(frame);
                let jet = hub
                    .spawned()
                    .into_iter()
                    .rev()
                    .find(|s| matches!(s.role, FxRole::Appearance(FxSlot::JumpJet)))
                    .unwrap();
                let q = mech_pos(&run, mech);
                let d = ((jet.at.x - q.x).powi(2) + (jet.at.y - q.y).powi(2)).sqrt();
                assert!(d < 40.0, "the jet starts {d} units from the mech");
            }
            let Some(f) = f0 else { continue };
            let k = frame - f;
            if jump == 0 && k % 3 == 2 && strip.len() < 8 {
                let rgba = render_rgba(&mut run);
                let c = project(&run, mech_pos(&run, mech));
                strip.push(crop(&rgba, [c[0], c[1] + 20.0]));
            }
            if jump == 1 && k == 12 {
                // the exhaust is running: ending it must change the pixels around the mech
                let with = render_rgba(&mut run);
                hub.kill_attached_effects();
                let without = render_rgba(&mut run);
                let c = project(&run, mech_pos(&run, mech));
                diff = differing_near(&with, &without, c, 150.0);
                break;
            }
            if jump == 0 && strip.len() >= 8 {
                break;
            }
        }
        assert!(f0.is_some(), "no jump jet exhaust started for jump {jump}");
        for _ in 0..240 {
            run.step(None).unwrap(); // land
        }
    }
    assert!(
        diff >= 30,
        "ending the exhaust changes only {diff} pixels around the mech"
    );
    write_strip("appearance-fx-jump.png", &strip);
}
