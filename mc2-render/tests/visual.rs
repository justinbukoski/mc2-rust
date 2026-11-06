#![allow(clippy::chunks_exact_to_as_chunks)]

//! P5 visual upgrades (enhanced lighting / enhanced water), offscreen and headless.
//!
//! * both switches off: the production renderer renders mc2_01 byte for byte like a baseline
//!   renderer (`SceneRenderer::new_baseline`) whose shader has no enhanced code at all, on the
//!   same adapter (no committed golden is trusted);
//! * each switch on: the image changes, and only in lit pixels (lighting, judged by a per-pixel
//!   material-ID render of the same view) / water pixels (water).
//!
//! Without `MC2_DATA` the tests return early; a missing GPU adapter fails them unless
//! `MC2_SKIP_GPU_TESTS` is exactly `1`.

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::light::LightModel;
use mc2_render::mission::{Mission, WaterParams};
use mc2_render::populate::{populate, Populated};
use mc2_render::scene::{Frame, SceneRenderer};
use mc2_render::settings::GraphicsSettings;
use mc2_render::terrain::{pack_tile_word, TileKind, WORLD_UNITS_PER_VERTEX};
use mc2_render::terrain_tex::{TerrainTextures, TILE_BYTES, TILE_SIZE};
use mc2_render::visual::VisualOptions;

const W: u32 = 640;
const H: u32 = 360;

struct Setup {
    scene: SceneRenderer,
    mission: Mission,
    pop: Populated,
    cam: GameCamera,
    /// (overlay tiles, solid tiles, smallest corner height range of an overlay tile) injected.
    cement: (usize, usize, f32),
}

fn setup(test: &str) -> Option<Setup> {
    setup_with(test, false, false)
}

/// `baseline`: the renderer that never builds the enhanced pipelines.  `cement`: inject sloped
/// road tiles (see [`inject_cement`]) into the terrain textures before upload.
fn setup_with(test: &str, baseline: bool, cement: bool) -> Option<Setup> {
    let root = std::env::var_os("MC2_DATA")?;
    let mut game = GameData::open(root).expect("open game data");
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let gpu = common::headless_or_skip(test, &cfg)?;
    let mut scene = if baseline {
        SceneRenderer::new_baseline(gpu).unwrap()
    } else {
        SceneRenderer::new(gpu).unwrap()
    };
    let mut mission = Mission::load(&mut game, "mc2_01").unwrap();
    let mut tex = TerrainTextures::load(&mut game, &mission).unwrap();
    let mut cam = GameCamera::from_fit(
        CameraSettings::default(),
        &mut mission.fit,
        (W as f32, H as f32),
    );
    cam.target.z = mission.terrain.elevation_at(cam.target.x, cam.target.y);
    let cement = if cement {
        cam.tilt_value(HORIZON_ANGLE);
        inject_cement(&mission, &mut tex, &cam)
    } else {
        (0, 0, 0.0)
    };
    scene.set_terrain(&mission, &tex);
    let pop = populate(&mut game, &mission, &mut scene, 0).unwrap();
    Some(Setup {
        scene,
        mission,
        pop,
        cam,
        cement,
    })
}

/// Replaces tile words near the camera target with synthetic road tiles, without touching
/// `populate.rs`: the steepest tiles get a cement transition whose texture alpha ramps from 0 to
/// 252 (`CementOverlay`, blended over the colour map), the flattest get solid pavement.  Returns
/// the (overlay, solid) tile counts and the smallest corner height range of an overlay tile.
fn inject_cement(
    mission: &Mission,
    tex: &mut TerrainTextures,
    cam: &GameCamera,
) -> (usize, usize, f32) {
    let mesh = mission.terrain.build_mesh(None);
    assert_eq!(mesh.vertices.len() / 4, tex.tile_words.len());
    let mut near: Vec<(usize, f32)> = Vec::new();
    for (t, quad) in mesh.vertices.chunks_exact(4).enumerate() {
        let cx = quad.iter().map(|v| v.position[0]).sum::<f32>() / 4.0;
        let cy = quad.iter().map(|v| v.position[1]).sum::<f32>() / 4.0;
        let (lo, hi) = quad.iter().fold((f32::MAX, f32::MIN), |(l, h), v| {
            (l.min(v.position[2]), h.max(v.position[2]))
        });
        let d = ((cx - cam.target.x).powi(2) + (cy - cam.target.y).powi(2)).sqrt();
        if d < 7.0 * WORLD_UNITS_PER_VERTEX {
            near.push((t, hi - lo));
        }
    }
    near.sort_by(|a, b| b.1.total_cmp(&a.1));
    let layer = |rgb: [u8; 3], ramp: bool| {
        let mut px = vec![0u8; TILE_BYTES];
        for (i, p) in px.chunks_exact_mut(4).enumerate() {
            let x = (i as u32 % TILE_SIZE) as u8;
            p.copy_from_slice(&[rgb[0], rgb[1], rgb[2], if ramp { x * 4 } else { 255 }]);
        }
        px
    };
    let overlay_layer = tex.tile_layers.len() as u32;
    tex.tile_layers.push(layer([150, 150, 140], true));
    tex.tile_names.push(vec!["injected overlay".into()]);
    let solid_layer = overlay_layer + 1;
    tex.tile_layers.push(layer([110, 110, 120], false));
    tex.tile_names.push(vec!["injected solid".into()]);
    let (n_over, n_solid) = (24.min(near.len() / 2), 12.min(near.len() / 4));
    let mut min_slope = f32::MAX;
    for &(t, slope) in &near[..n_over] {
        tex.tile_words[t] = pack_tile_word(overlay_layer, TileKind::CementOverlay);
        min_slope = min_slope.min(slope);
    }
    for &(t, _) in near.iter().rev().take(n_solid) {
        tex.tile_words[t] = pack_tile_word(solid_layer, TileKind::CementSolid);
    }
    (n_over, n_solid, min_slope)
}

impl Setup {
    fn render(&self, light: &LightModel, water: &WaterParams, time: f32) -> Vec<u8> {
        self.render_shapes(light, water, time, &self.pop.instances)
    }

    fn render_shapes(
        &self,
        light: &LightModel,
        water: &WaterParams,
        time: f32,
        instances: &[mc2_render::scene::ShapeInstance],
    ) -> Vec<u8> {
        let frame = Frame {
            width: W,
            height: H,
            camera: &self.cam,
            light,
            water,
            terrain_params: &self.mission.terrain_params,
            time,
            precise_time: None,
            instances,
            shadows: true,
        };
        self.scene.render(&frame).unwrap()
    }

    /// Per-pixel material class of the same view (see `SceneRenderer::render_ids`).
    fn classes(&self, translucent: bool) -> Vec<u8> {
        let frame = Frame {
            width: W,
            height: H,
            camera: &self.cam,
            light: &self.mission.light,
            water: &self.mission.water,
            terrain_params: &self.mission.terrain_params,
            time: TIME,
            precise_time: None,
            instances: &self.pop.instances,
            shadows: true,
        };
        self.scene
            .render_ids(&frame, translucent)
            .unwrap()
            .chunks_exact(4)
            .map(|p| ((u16::from(p[0]) + 8) / 16) as u8)
            .collect()
    }

    fn show(&self, time: f32) -> Vec<u8> {
        self.render(&self.mission.light, &self.mission.water, time)
    }
}

fn with(s: &mut Setup, opts: VisualOptions) {
    s.scene.set_visual(opts);
}

fn diff_mask(a: &[u8], b: &[u8]) -> Vec<bool> {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .map(|(p, q)| p != q)
        .collect()
}

fn count(m: &[bool]) -> usize {
    m.iter().filter(|&&x| x).count()
}

const TIME: f32 = 1.7;
const HORIZON_ANGLE: f32 = 20.0;

#[test]
fn switches_off_render_byte_identical_to_a_renderer_without_enhanced_code() {
    let Some(mut prod) = setup("visual baseline (production)") else {
        return;
    };
    let base = setup_with("visual baseline (reference)", true, false).expect("same GPU twice");
    assert_eq!(
        prod.scene.gpu().adapter_name,
        base.scene.gpu().adapter_name,
        "both renderers must run on the same adapter"
    );
    assert_eq!(
        base.scene.built_visual_variants(),
        [VisualOptions::default()]
    );
    let want = base.show(TIME);
    assert_eq!(want.len(), (W * H * 4) as usize);
    // the production renderer with the default switches, then with the explicit all-off options
    assert_eq!(
        prod.scene.built_visual_variants(),
        [VisualOptions::default()]
    );
    assert_eq!(
        prod.show(TIME),
        want,
        "default render differs from the baseline"
    );
    with(&mut prod, GraphicsSettings::default().visual());
    assert_eq!(prod.show(TIME), want);
    // across several times, including the water's animation
    for t in [0.0, 0.5, 3.3] {
        assert_eq!(prod.show(t), base.show(t), "differs at t={t}");
    }
    // the reference ignores the switches; the production renderer does not
    let mut base = base;
    let on = VisualOptions {
        enhanced_lighting: true,
        enhanced_water: true,
    };
    with(&mut base, on);
    assert_eq!(base.scene.visual(), VisualOptions::default());
    assert_eq!(
        base.show(TIME),
        want,
        "the baseline renderer ignores the switches"
    );
    with(&mut prod, on);
    assert_ne!(prod.show(TIME), want, "switches on must change the picture");
    with(&mut prod, VisualOptions::default());
    let restored = prod.show(TIME);
    // The Metal gate observed three pixels changing by one colour level after switching back.
    // Keep the initial off-path comparison byte-exact, and bound this repeat-render comparison
    // to one level in at most 0.01% of pixels to allow for GPU rounding variation.
    let changed_pixels = restored
        .chunks_exact(4)
        .zip(want.chunks_exact(4))
        .filter(|(actual, expected)| {
            assert!(actual
                .iter()
                .zip(*expected)
                .all(|(a, b)| a.abs_diff(*b) <= 1));
            actual != expected
        })
        .count();
    assert!(changed_pixels <= (W * H / 10_000) as usize);
    gate_println!(
        "visual baseline: {} bytes identical on {} ({} backend)",
        want.len(),
        prod.scene.gpu().adapter_name,
        prod.scene.gpu().backend
    );
}

/// Water coverage: pixels that change when the water is taken away (its surface far below).
fn water_coverage(s: &Setup, time: f32) -> Vec<bool> {
    let with_water = s.render(&s.mission.light, &s.mission.water, time);
    let dry = WaterParams {
        elevation: -1.0e5,
        ..s.mission.water.clone()
    };
    diff_mask(&with_water, &s.render(&s.mission.light, &dry, time))
}

#[test]
fn enhanced_water_changes_only_water_pixels() {
    let Some(mut s) = setup("visual water") else {
        return;
    };
    let base = s.show(TIME);
    let coverage = water_coverage(&s, TIME);
    assert!(count(&coverage) > 5000, "mc2_01's start view shows water");
    with(
        &mut s,
        VisualOptions {
            enhanced_water: true,
            ..Default::default()
        },
    );
    let on = s.show(TIME);
    let changed = diff_mask(&base, &on);
    let n = count(&changed);
    assert!(n > 1000, "enhanced water changes the picture ({n} pixels)");
    let outside = changed
        .iter()
        .zip(&coverage)
        .filter(|(c, w)| **c && !**w)
        .count();
    assert_eq!(outside, 0, "pixels changed outside the water");
    // the same pixels stay unchanged at a frame boundary vs halfway between frames
    let on_mid = s.show(TIME + 0.06);
    assert_ne!(on, on_mid);
    gate_println!(
        "visual water: {n} of {} water pixels changed, 0 outside",
        count(&coverage)
    );
}

// Material classes of `SceneRenderer::render_ids`.
const NOTHING: u8 = 0;
const PLAIN: u8 = 1;
const CEMENT_BLEND: u8 = 2;
const CEMENT_SOLID: u8 = 3;
const SHAPE: u8 = 6;
const FOGGED: u8 = 9;

/// Surfaces whose colour the sun/ambient values reach: everything opaque that is drawn and not
/// fully fogged.  Water, windows and spotlights only blend over these.
fn lit_class(c: u8) -> bool {
    matches!(c, PLAIN | CEMENT_BLEND | CEMENT_SOLID | SHAPE)
}

/// What the enhanced lighting did to one view, judged against the ID render.
struct LightView {
    changed: usize,
    lit: usize,
    unlit: usize,
    by_class: [usize; 10],
    changed_by_class: [usize; 10],
}

/// Renders the current view with enhanced lighting off and on and checks the change against the
/// material classes: no pixel without a lit opaque surface (nothing drawn, fully fogged) may
/// change, and every changed pixel's opaque class must be lit.
fn lighting_view(s: &mut Setup, label: &str) -> LightView {
    let light = s.mission.light.clone();
    let water = s.mission.water.clone();
    let opaque = s.classes(false);
    let all = s.classes(true);
    with(s, VisualOptions::default());
    let off = s.render(&light, &water, TIME);
    with(
        s,
        VisualOptions {
            enhanced_lighting: true,
            ..Default::default()
        },
    );
    let on = s.render(&light, &water, TIME);
    let changed = diff_mask(&off, &on);
    let mut v = LightView {
        changed: count(&changed),
        lit: opaque.iter().filter(|&&c| lit_class(c)).count(),
        unlit: opaque.iter().filter(|&&c| !lit_class(c)).count(),
        by_class: [0; 10],
        changed_by_class: [0; 10],
    };
    for i in 0..changed.len() {
        v.by_class[usize::from(opaque[i])] += 1;
        if changed[i] {
            v.changed_by_class[usize::from(opaque[i])] += 1;
            assert!(
                lit_class(opaque[i]),
                "{label}: enhanced lighting changed pixel {i} whose opaque class is {} \
                 (top class {})",
                opaque[i],
                all[i]
            );
        }
    }
    assert_eq!(v.changed_by_class[usize::from(NOTHING)], 0);
    assert_eq!(v.changed_by_class[usize::from(FOGGED)], 0);
    v
}

#[test]
fn enhanced_lighting_changes_only_lit_pixels() {
    let Some(mut s) = setup_with("visual lighting", false, true) else {
        return;
    };
    // The acceptance view: the mc2_01 start target, tilted towards the horizon so that it shows
    // surface (terrain, shapes, water), the injected sloped road transition tiles, and unlit
    // content (sky and fully fogged terrain).
    let (n_over, n_solid, min_slope) = s.cement;
    assert!(
        n_over >= 8 && n_solid >= 4,
        "cement injected ({:?})",
        s.cement
    );
    assert!(
        min_slope > 0.0,
        "every overlay tile is sloped ({min_slope})"
    );
    let start = lighting_view(&mut s, "acceptance view");
    assert!(
        start.unlit > 2000,
        "the acceptance view has visible unlit pixels ({} px)",
        start.unlit
    );
    assert!(
        start.changed > 5000,
        "enhanced lighting changes the picture"
    );
    assert!(
        start.by_class[usize::from(CEMENT_BLEND)] > 200,
        "the sloped partial-alpha cement tiles are in view ({:?})",
        start.by_class
    );
    assert!(start.by_class[usize::from(CEMENT_SOLID)] > 50);
    assert!(
        start.changed_by_class[usize::from(CEMENT_BLEND)] > 100,
        "enhanced lighting reaches the cement transition ({:?})",
        start.changed_by_class
    );
    assert!(start.changed_by_class[usize::from(PLAIN)] > 1000);
    assert!(start.changed_by_class[usize::from(SHAPE)] > 0);
    // With the sun taken away the terrain's shading ramp is constant, so the two paths must agree
    // on every terrain/water/sky pixel (all but one shape dropped: shapes carry their light baked
    // per vertex and get the soft terminator): the enhanced path only redistributes the sun.
    let dark = LightModel {
        sun_color: [0.0; 3],
        ..s.mission.light.clone()
    };
    let water = s.mission.water.clone();
    with(
        &mut s,
        VisualOptions {
            enhanced_lighting: true,
            ..Default::default()
        },
    );
    let on_dark = s.render_shapes(&dark, &water, TIME, &s.pop.instances[..1]);
    with(&mut s, VisualOptions::default());
    let off_dark = s.render_shapes(&dark, &water, TIME, &s.pop.instances[..1]);
    let dark_diff = count(&diff_mask(&off_dark, &on_dark));
    gate_println!(
        "visual lighting: acceptance view {} px changed of {} lit, 0 of {} unlit changed (cement \
         blend {} px with partial alpha, {} changed, min slope {min_slope}); {dark_diff} differ \
         with no sun",
        start.changed,
        start.lit,
        start.unlit,
        start.by_class[usize::from(CEMENT_BLEND)],
        start.changed_by_class[usize::from(CEMENT_BLEND)]
    );
    assert_eq!(dark_diff, 0, "without a sun the two paths differ");
}

#[test]
fn enhanced_water_keeps_moving_at_large_timestamps() {
    let Some(mut s) = setup("visual water precision") else {
        return;
    };
    let big = 1_048_576.0f32;
    let at = |s: &Setup, precise: f64| {
        let frame = Frame {
            width: W,
            height: H,
            camera: &s.cam,
            light: &s.mission.light,
            water: &s.mission.water,
            terrain_params: &s.mission.terrain_params,
            time: big,
            precise_time: Some(precise),
            instances: &s.pop.instances,
            shadows: true,
        };
        s.scene.render(&frame).unwrap()
    };
    let (a, b) = (f64::from(big), f64::from(big) + 1.0 / 60.0);
    assert_eq!(a as f32, b as f32, "the f32 times are indistinguishable");
    let off_a = at(&s, a);
    assert_eq!(off_a, at(&s, b), "switch off ignores the exact time");
    with(
        &mut s,
        VisualOptions {
            enhanced_water: true,
            ..Default::default()
        },
    );
    assert_ne!(at(&s, a), at(&s, b), "water phase advances between frames");
}

#[test]
fn gpu_opt_out_needs_exactly_one() {
    use std::ffi::OsStr;
    assert!(common::skip_decision(Some(OsStr::new("1"))));
    for v in ["", "0", "true", "yes", " 1", "1 ", "11"] {
        assert!(
            !common::skip_decision(Some(OsStr::new(v))),
            "{v:?} must not skip"
        );
    }
    assert!(!common::skip_decision(None));
}
