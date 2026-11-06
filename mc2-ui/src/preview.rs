// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The 3D model panels (`SimpleCamera`, `code/simplecamera.cpp`): a mech in the mech bay and the
//! mech display, and the briefing's objective model (`camera` in `missionbriefingscreen.cpp`).
//!
//! The C++ draws the model straight into the screen each frame. Here a screen asks for the model's
//! picture by its request ([`PreviewRequest`]): a texture the first time, a queued request until the
//! caller renders it. The caller drains the queue after a frame with [`ModelPreviews::resolve`],
//! which renders the model offscreen with `mc2_render::SceneRenderer` and stores the picture in the
//! texture store, where the next frame finds it.
//!
//! The camera is `GameCamera` driven the way `SimpleCamera` drives `Camera` ([`PreviewCamera`]):
//! `data/cameras/cameras.fit` gives the projection angle and the light, the object kind the altitude.
//! The scene's background is keyed out to transparent so the
//! screen shows through as the C++ does.

use std::sync::{Arc, Mutex};

use mc2_assets::map::TerrainVertex;
use mc2_assets::tga::RgbaImage;
use mc2_formats::ini::FitIni;
use mc2_render::appearance::{AppearanceState, ShapeChoice, ShapeSet};
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::gpu::Gpu;
use mc2_render::light::LightModel;
use mc2_render::math::{deg_to_rad, stuff_to_world, Mat4, Vec3};
use mc2_render::mission::{Mission, TerrainParams, WaterParams};
use mc2_render::objects::Appearance;
use mc2_render::scene::{Frame, ModelId, SceneRenderer, ShapeInstance, ShapeStyle};
use mc2_render::shape::{instance_node_matrices, ShapeModel};
use mc2_render::terrain::TerrainMap;
use mc2_render::terrain_tex::TerrainTextures;
use mc2_render::vertex_light::{self, VertexLightInputs};
use mc2_render::Result;

use crate::ctx::UiAssets;
use crate::draw::{Blend, DrawList};
use crate::textures::TexId;

/// `options.cfg` `BaseColor`: the paint of a mech the player has not chosen a colour for.
pub const DEFAULT_PAINT: u32 = 0x00fa_e525;

/// The two colours the scene is cleared to. A pixel the model covers fully renders identically
/// against both; a pixel it covers partly (the anti-aliased edge, a narrow gap) differs by the
/// share of the background showing through, which recovers the coverage exactly ([`matte`]).
const CLEAR_A: [u8; 3] = [0xff, 0x00, 0xff];
const CLEAR_B: [u8; 3] = [0x00, 0xff, 0x00];

/// Colour-key "alpha bleed": every keyed (alpha 0) texel takes the average colour of its opaque
/// 8-neighbours, growing outward pass by pass until all have one, alpha staying 0. Opaque texels
/// are untouched; filtering across the edge then blends model colours, never the key colour.
fn bleed_keyed(rgba: &mut [u8], width: usize, height: usize) {
    let mut done: Vec<bool> = rgba.chunks_exact(4).map(|px| px[3] != 0).collect();
    if !done.contains(&true) {
        return;
    }
    while done.contains(&false) {
        let mut filled = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                if done[i] {
                    continue;
                }
                let (mut sum, mut n) = ([0u32; 3], 0u32);
                for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        let j = ny * width + nx;
                        if done[j] {
                            for (c, s) in sum.iter_mut().enumerate() {
                                *s += rgba[j * 4 + c] as u32;
                            }
                            n += 1;
                        }
                    }
                }
                if let Some(avg) = sum
                    .map(|s| s.checked_div(n))
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                {
                    filled.push((i, avg));
                }
            }
        }
        for (i, c) in filled {
            for (k, v) in c.iter().enumerate() {
                rgba[i * 4 + k] = *v as u8;
            }
            done[i] = true;
        }
    }
}

/// Recovers straight alpha from the same scene rendered over [`CLEAR_A`] (`a`) and [`CLEAR_B`]
/// (`b`): the background share of a pixel is its difference between the two renders over the
/// difference of the clear colours. Fully covered pixels are identical in both and are kept
/// exactly (opaque purple paint included); partly covered ones are un-mixed from the clear colour,
/// so no magenta is left in them. Keyed texels then take their neighbours' colour ([`bleed_keyed`]),
/// so bilinear sampling across the edge cannot pull the key colour back in.
fn matte(a: &[u8], b: &[u8], width: usize, height: usize) -> Vec<u8> {
    let span: [f32; 3] = std::array::from_fn(|c| CLEAR_A[c] as f32 - CLEAR_B[c] as f32);
    let mut out = vec![0u8; a.len()];
    for ((o, pa), pb) in out
        .chunks_exact_mut(4)
        .zip(a.chunks_exact(4))
        .zip(b.chunks_exact(4))
    {
        let mut cover = 0.0f32;
        for c in 0..3 {
            cover += (pa[c] as f32 - pb[c] as f32) / span[c] / 3.0;
        }
        let cover = cover.clamp(0.0, 1.0);
        if cover <= 4.0 / 255.0 {
            o.copy_from_slice(&[pa[0], pa[1], pa[2], 255]);
        } else if cover >= 0.98 {
            o.copy_from_slice(&[0, 0, 0, 0]);
        } else {
            let alpha = 1.0 - cover;
            for c in 0..3 {
                let v = (pa[c] as f32 - cover * CLEAR_A[c] as f32) / alpha;
                o[c] = v.round().clamp(0.0, 255.0) as u8;
            }
            o[3] = (alpha * 255.0).round() as u8;
        }
    }
    bleed_keyed(&mut out, width, height);
    out
}

/// What a `SimpleCamera` shows; `setMech` and `setVehicle` share one framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    /// `setMech` / `setVehicle` (simplecamera.cpp:304, 377).
    Mech,
    /// `setComponent` (simplecamera.cpp:446): a lab component, lit by the fixed component light.
    Component,
    /// `setBuilding` (simplecamera.cpp:519): `setComponent` then 800 and `bIsComponent = 0`.
    Building,
}

impl PreviewKind {
    /// `SimpleCamera::AltitudeTight` for the kind.
    pub fn altitude_tight(self) -> f32 {
        match self {
            PreviewKind::Mech => 650.0,
            PreviewKind::Component => 580.0,
            PreviewKind::Building => 800.0,
        }
    }
}

/// `SimpleCamera::rotation` (simplecamera.cpp:40).
pub const PREVIEW_ROTATION: f32 = -45.0;

/// `SimpleCamera::SimpleCamera` (simplecamera.cpp:24-42): the `[Cameras]` block of
/// `data/cameras/cameras.fit` through `Camera::init` (projection angle, start rotation and
/// light), then `rotateLightRight(90)`.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewCamera {
    pub projection_angle: f32,
    /// `StartRotation`: `Camera::init` turns the camera by it (camera.cpp:362-366). The object's own
    /// turn is [`PREVIEW_ROTATION`] (simplecamera.cpp:40,356).
    pub start_rotation: f32,
    pub light: LightModel,
}

/// The object turn about its vertical axis through `centre`: [`PREVIEW_ROTATION`] plus the spin,
/// independent of the camera's `StartRotation`.
fn object_turn(centre: Vec3, yaw: f32) -> Mat4 {
    Mat4::translation(centre)
        .mul(&Mat4::rotation_y(deg_to_rad(PREVIEW_ROTATION + yaw)))
        .mul(&Mat4::translation(-centre))
}

impl PreviewCamera {
    pub const FIT: &'static str = "data/cameras/cameras.fit";

    /// Point `cam` the way `SimpleCamera` does: projection angle and `StartRotation` from the fit
    /// (the camera turn), and the kind's `ZoomTight` altitude whatever the model's size.
    fn aim(&self, cam: &mut GameCamera, kind: PreviewKind) {
        cam.projection_angle = self.projection_angle;
        cam.set_rotation(self.start_rotation);
        cam.altitude = kind.altitude_tight();
    }

    /// `Camera::init(FitIniFile*)` and `rotateLightRight(90)`. Every field the C++ asserts on is
    /// required: a present but malformed file is an error, not a quiet default.
    pub fn from_fit(fit: &mut FitIni) -> Result<Self> {
        let bad = |what: &str, e: &dyn std::fmt::Display| {
            mc2_render::RenderError::Other(format!("{}: {what}: {e}", Self::FIT))
        };
        fit.seek_block("Cameras")
            .map_err(|e| bad("[Cameras] block", &e))?;
        let projection_angle = fit
            .read_f32("ProjectionAngle")
            .map_err(|e| bad("ProjectionAngle", &e))?;
        let start_rotation = fit
            .read_f32("StartRotation")
            .map_err(|e| bad("StartRotation", &e))?;
        for name in [
            "LightRed",
            "LightGreen",
            "LightBlue",
            "AmbientRed",
            "AmbientGreen",
            "AmbientBlue",
        ] {
            fit.read_u8(name).map_err(|e| bad(name, &e))?;
        }
        for name in ["LightDirYaw", "LightDirPitch"] {
            fit.read_f32(name).map_err(|e| bad(name, &e))?;
        }
        let mut light = LightModel::from_fit(fit);
        // `Camera::rotateLightRight`: `lightYaw -= amount`, wrapped below -360
        light.yaw -= 90.0;
        if light.yaw < -360.0 {
            light.yaw += 360.0;
        }
        Ok(PreviewCamera {
            projection_angle,
            start_rotation,
            light,
        })
    }

    /// The camera of the game data. A missing `cameras.fit` stops the C++ (`STOP("Need Camera
    /// File")`); only that genuine absence falls back to the old fixed framing, said once in the
    /// log. A file that is present but unreadable or malformed is an error.
    pub fn load(game: &mut GameData) -> Result<Self> {
        if !game.exists(Self::FIT) {
            eprintln!(
                "mc2-ui preview: {} is absent; using the fixed fallback camera",
                Self::FIT
            );
            return Ok(PreviewCamera {
                projection_angle: GUESSED_PROJECTION,
                start_rotation: 0.0,
                light: LightModel::default(),
            });
        }
        let mut fit = game.fit(Self::FIT)?;
        Self::from_fit(&mut fit)
    }

    /// The light for `kind` on a camera that is `activated` or not ([`CameraActivation`]): every
    /// `SimpleCamera::setMech`/`setVehicle`/`setComponent` ends in `activate()` (simplecamera.cpp:358, 428, 502),
    /// which runs `updateDaylight(true)` only once the camera is `ready && active` (mclib/camera.cpp:1775-1784):
    /// `Ready` comes from `cameras.fit` and `active` from its first `Camera::update` (camera.cpp:1654). A fresh
    /// camera therefore lights its first model with the fit light (white for the shipped fit) and every later
    /// one with the activation sunset light, 252/159/140 at the shipped pitch of 22.5 degrees
    /// ([`LightModel::activate_daylight`]; both measured on the instrumented C++, rc-vqa-vertexdump).
    /// `SimpleCamera::render` overrides the colours for a component (:118-126).
    pub fn light_for(&self, kind: PreviewKind, activated: bool) -> LightModel {
        let mut light = if activated {
            self.light.activate_daylight()
        } else {
            self.light.clone()
        };
        if kind == PreviewKind::Component {
            light.sun_color = [196.0 / 255.0, 196.0 / 255.0, 220.0 / 255.0];
            light.ambient = [196.0 / 255.0; 3];
        }
        light
    }
}

/// The projection angle used only when `cameras.fit` is absent.
const GUESSED_PROJECTION: f32 = 10.0;

/// One picture of a model: what to draw, its paint, its scale and the size it is drawn at.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewRequest {
    /// The shape's name, lower case: the `data/tgl/<shape>.tgl` stem (a chassis file's stem).
    pub shape: String,
    /// The mech paint scheme `(highlight1, highlight2, base)`, in the exact order
    /// `Mech3DAppearance::setPaintScheme` receives it. The Mechlopedia passes the
    /// player's highlight colour for both highlight masks and their base colour for
    /// the blue/base mask.
    pub paint: [u32; 3],
    /// `setScale`: the chassis's or the objective model's scale.
    pub scale: f32,
    /// The turn of the model about its vertical axis in degrees (`SimpleCamera`'s `rotation`, which
    /// the lab and the options tab advance every frame). Callers quantise it, so a spinning model
    /// costs a bounded number of pictures.
    pub yaw: f32,
    pub width: u32,
    pub height: u32,
    /// `shape` names a building appearance (`data/tgl/<name>enc.ini` or `<name>.ini`), as
    /// `SimpleCamera::setComponent` takes it, not a shape file: the appearance names the shape.
    pub building: bool,
    /// Which `SimpleCamera::set*` the request stands for: it picks the altitude and the light, and
    /// is independent of `building` (which only says how `shape` is loaded).
    pub kind: PreviewKind,
    /// The camera had been updated before this `set*` call, so `activate()` relit it with the activation
    /// daylight ([`PreviewCamera::light_for`], [`CameraActivation`]).
    pub activated: bool,
}

impl PreviewRequest {
    /// The name the picture is stored under.
    pub fn key(&self) -> String {
        format!(
            "preview:{}{}{}:{:06x}:{:06x}:{:06x}:{:08x}:{:08x}:{}x{}{}",
            if self.building { "bldg:" } else { "" },
            match self.kind {
                PreviewKind::Mech => "",
                PreviewKind::Component => "comp:",
                PreviewKind::Building => "kbld:",
            },
            self.shape.to_ascii_lowercase(),
            self.paint[0] & 0x00ff_ffff,
            self.paint[1] & 0x00ff_ffff,
            self.paint[2] & 0x00ff_ffff,
            self.scale.to_bits(),
            self.yaw.to_bits(),
            self.width,
            self.height,
            if self.activated { ":act" } else { "" }
        )
    }
}

/// The `Camera::activate` state of one screen's `SimpleCamera` (a member of the screen, never deactivated): a
/// `set*` call relights the camera with the activation daylight only after `SimpleCamera::update` has run once
/// with a model (`if (pObject)`, simplecamera.cpp:219-221, which sets `active`, camera.cpp:1654), and the light
/// stays that way ([`PreviewCamera::light_for`]). The instrumented C++ showed the purchase screen's first mech
/// lit 255/255/255 and every mech selected after it 252/159/140 (rust/docs/evidence/rc-vqa-vertexdump.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CameraActivation {
    updated: bool,
}

impl CameraActivation {
    /// `setMech`/`setVehicle`/`setComponent`: `req` as this camera lights it.
    pub fn set(&self, req: PreviewRequest) -> PreviewRequest {
        PreviewRequest {
            activated: self.updated,
            ..req
        }
    }

    /// `SimpleCamera::update` with a model: later `set*` calls take the activation daylight.
    pub fn update(&mut self, has_model: bool) {
        self.updated |= has_model;
    }
}

/// The requests the screens make while they draw, until the caller renders them. The screens hold
/// [`UiAssets`] shared, so the queue is behind a lock.
#[derive(Default)]
pub struct PreviewQueue {
    pending: Mutex<Vec<PreviewRequest>>,
    /// The keys of requests that failed to render. A screen shows its fallback for these, rather
    /// than asking again.
    failed: Mutex<Vec<String>>,
}

impl PreviewQueue {
    fn request(&self, req: &PreviewRequest) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !pending.iter().any(|p| p.key() == req.key()) {
            pending.push(req.clone());
        }
    }

    /// Records that `req` failed to render (a missing or unreadable shape).
    fn fail(&self, req: &PreviewRequest) {
        let mut failed = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        if !failed.iter().any(|k| *k == req.key()) {
            failed.push(req.key());
        }
    }

    fn failed(&self, req: &PreviewRequest) -> bool {
        let failed = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        failed.iter().any(|k| *k == req.key())
    }

    /// Fails every queued request (the caller has no renderer): the screens show their fallback
    /// for these instead of waiting. Returns how many there were.
    pub fn fail_all(&self) -> usize {
        let queued = self.take();
        for request in &queued {
            self.fail(request);
        }
        queued.len()
    }

    /// Takes the queued requests, in the order they were made.
    pub fn take(&self) -> Vec<PreviewRequest> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// The picture of `req` if it is rendered; otherwise the request is queued and `None` returned. A
/// request that failed is not queued again: `None` stays, and [`preview_failed`] says why.
pub fn preview_texture(assets: &UiAssets, req: &PreviewRequest) -> Option<TexId> {
    match assets.textures.find(&req.key()) {
        Some(tex) => Some(tex),
        None => {
            if !preview_failed(assets, req) {
                assets.previews.request(req);
            }
            None
        }
    }
}

/// Whether `req` failed to render. The screen then shows its fallback (the briefing's "no model"
/// static); the failure is logged by [`ModelPreviews::resolve`].
pub fn preview_failed(assets: &UiAssets, req: &PreviewRequest) -> bool {
    assets.previews.failed(req)
}

/// The shape a chassis file names, as `SimpleCamera::setMech` takes it: the file's stem, with the
/// directory dropped (`data/objects/alpha.csv` is `alpha`).
pub fn shape_name(file: &str) -> String {
    let stem = file.split('.').next().unwrap_or(file);
    stem.rsplit(['/', '\\'])
        .next()
        .unwrap_or(stem)
        .to_ascii_lowercase()
}

/// The whole-pixel size of a rectangle `(left, top, right, bottom)`, at least one pixel a side.
pub fn rect_size((left, top, right, bottom): (f32, f32, f32, f32)) -> (u32, u32) {
    let w = (right - left).round().max(1.0) as u32;
    let h = (bottom - top).round().max(1.0) as u32;
    (w, h)
}

/// Draws the picture over `(x0, y0)`-`(x1, y1)`, as `SimpleCamera::render` draws the model.
///
/// C++ draws directly with `gos_Alpha_OneZero`; this stored offscreen picture instead uses alpha
/// blending so its keyed-out background leaves the screen panel visible.
pub fn draw_preview(dl: &mut DrawList, tex: TexId, rect: (f32, f32, f32, f32)) {
    dl.rect_quad(
        Some(tex),
        Blend::Alpha,
        rect,
        (0.0, 0.0, 1.0, 1.0),
        0xffff_ffff,
    );
}

/// `SimpleCamera::rotationIncrement`: degrees per second a model spins by (`simplecamera.cpp:382`,
/// `:498`).
pub const SPIN_DEGREES_PER_SECOND: f32 = 90.0;

/// The turn between two stored pictures of a spinning model. The C++ draws every frame; here each
/// picture is rendered offscreen and kept, so the turn is quantised to bound the pictures a model
/// costs (36 at 10 degrees). A picture that is not rendered yet is replaced by the last one drawn.
pub const SPIN_STEP_DEGREES: f32 = 10.0;

/// A model that turns about its vertical axis (`SimpleCamera::update`: `rotation = frameLength *
/// rotationIncrement + rotation`), drawn into a panel from the pictures [`ModelPreviews`] renders.
#[derive(Clone, Debug, Default)]
pub struct Spinner {
    /// The turn in degrees, 0..360.
    yaw: f32,
    /// The picture last drawn, kept while the next is rendered.
    last: Option<TexId>,
    /// Picture turns asked for and not yet adopted, oldest first.
    pending: Vec<f32>,
}

impl Spinner {
    /// `SimpleCamera::update`: one frame of turning.
    pub fn update(&mut self, dt: f32) {
        self.yaw = (self.yaw + dt * SPIN_DEGREES_PER_SECOND).rem_euclid(360.0);
    }

    /// The turn in degrees.
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// The turn of the picture asked for: [`Spinner::yaw`] rounded down to a step.
    pub fn picture_yaw(&self) -> f32 {
        (self.yaw / SPIN_STEP_DEGREES).floor() * SPIN_STEP_DEGREES
    }

    /// The request for the picture of `req`'s model at the current turn.
    pub fn request(&self, req: &PreviewRequest) -> PreviewRequest {
        PreviewRequest {
            yaw: self.picture_yaw(),
            ..req.clone()
        }
    }

    /// A new model starts from the start angle (`SimpleCamera::setMech`'s `rotation`) and forgets
    /// the picture last drawn.
    pub fn reset(&mut self) {
        self.yaw = 0.0;
        self.last = None;
        self.pending.clear();
    }

    /// Draws the model into `rect`: its picture at the current turn, else the last one drawn. The
    /// picture is asked for when it is not stored yet.
    pub fn draw(
        &mut self,
        dl: &mut DrawList,
        assets: &UiAssets,
        req: &PreviewRequest,
        rect: (f32, f32, f32, f32),
    ) {
        let now = self.request(req);
        if self.pending.last() != Some(&now.yaw) {
            self.pending.push(now.yaw);
            let excess = self.pending.len().saturating_sub(64);
            self.pending.drain(..excess);
        }
        // Adopt the newest picture that has been rendered, even if its turn has been passed
        // (resolution happens after drawing); older pending turns are dropped.
        let mut ready = None;
        for (i, yaw) in self.pending.iter().enumerate().rev() {
            let r = PreviewRequest {
                yaw: *yaw,
                ..req.clone()
            };
            let tex = if i + 1 == self.pending.len() {
                preview_texture(assets, &r)
            } else {
                assets.textures.find(&r.key())
            };
            if let Some(tex) = tex {
                ready = Some((i, tex));
                break;
            }
        }
        if let Some((i, tex)) = ready {
            self.last = Some(tex);
            self.pending.drain(..=i);
        }
        if let Some(tex) = self.last {
            draw_preview(dl, tex, rect);
        }
    }
}

/// The scene draws a terrain before any shape, and a model preview has none. This stand-in is one
/// tile, 100,000 units under the origin: a tile that far off covers less than a pixel of the view,
/// so it never shows.
fn stand_in_mission() -> Mission {
    let flat = TerrainVertex {
        normal: [0.0, 0.0, 1.0],
        elevation: -100_000.0,
        texture_data: 0,
        local_rgb_light: 0xffff_ffff,
        terrain_type: 0,
        selected: 0,
        water: 0,
        shadow: 0,
        highlighted: 0,
    };
    Mission {
        name: "preview".into(),
        fit: FitIni::from_bytes(b"FITini\r\n".to_vec()).expect("an empty fit"),
        light: LightModel::default(),
        water: WaterParams::default(),
        terrain_params: TerrainParams::default(),
        weather: None,
        terrain: TerrainMap::new(2, vec![flat; 4]),
        objects: Vec::new(),
        parts: Vec::new(),
        color_map_name: None,
    }
}

fn stand_in_textures() -> TerrainTextures {
    let one = RgbaImage {
        width: 1,
        height: 1,
        pixels: vec![0xff; 4],
    };
    TerrainTextures {
        color_map: one.clone(),
        color_map_name: String::new(),
        detail: one.clone(),
        detail_name: String::new(),
        water: one,
        water_name: String::new(),
        water_detail: Vec::new(),
        water_detail_names: Vec::new(),
        tile_layers: Vec::new(),
        tile_names: Vec::new(),
        tile_words: vec![0xffff],
        missing_tiles: Vec::new(),
    }
}

/// `SimpleCamera::setComponent` (simplecamera.cpp:438-520): the appearance `<name>enc` when its
/// `.ini` exists, else `<name>`; its base shape is the model drawn.
pub fn building_shape(game: &mut GameData, name: &str) -> Result<String> {
    let enc = format!("{name}enc");
    let appearance = match Appearance::load(game, &enc)? {
        Some(a) => a,
        None => Appearance::load(game, name)?.ok_or_else(|| {
            mc2_render::RenderError::Other(format!("building appearance {name}: no data/tgl ini"))
        })?,
    };
    Ok(appearance.shape().to_string())
}

/// `MC2_DUMP_VERTEX_LIGHT` (debug, off by default): writes the preview model's per-vertex lit colour, as
/// `fs_shape` computes it, to `<dir>/<shape>-<w>x<h>-yaw<yaw>.txt` for comparison with the instrumented C++
/// (`mc2_render::vertex_light`). A failure to write is logged and does not stop the render.
fn dump_vertex_light(
    dir: &std::path::Path,
    req: &PreviewRequest,
    shape: &str,
    model: &ShapeModel,
    inst: &ShapeInstance,
    light: &LightModel,
    style: &ShapeStyle,
) {
    match write_vertex_light(dir, req, shape, model, inst, light, style) {
        Ok(path) => log::info!("vertex light dump: {}", path.display()),
        Err(e) => log::warn!("vertex light dump of {shape} failed: {e}"),
    }
}

fn write_vertex_light(
    dir: &std::path::Path,
    req: &PreviewRequest,
    shape: &str,
    model: &ShapeModel,
    inst: &ShapeInstance,
    light: &LightModel,
    style: &ShapeStyle,
) -> std::io::Result<std::path::PathBuf> {
    let inputs = VertexLightInputs {
        shape_dir: light.shape_light_direction(),
        infinite: inst.infinite_light,
        ambient: inst.ambient_override.unwrap_or(light.ambient),
        highlight: inst.highlight,
        night_factor: light.night_factor,
        is_night: light.is_night,
        glow: style.glow,
    };
    let mats = instance_node_matrices(&inst.pose, inst.position, inst.yaw_degrees);
    let path = dir.join(format!(
        "{shape}-{}x{}-yaw{:.0}.txt",
        req.width, req.height, req.yaw
    ));
    let mut w = std::io::BufWriter::new(std::fs::File::create(&path)?);
    vertex_light::write_dump(&mut w, model, &mats, &inputs)?;
    std::io::Write::flush(&mut w)?;
    Ok(path)
}

/// The preview's model pose ([`ModelPreviews::render`]): the bind pose scaled by `req.scale` (1 when the chassis
/// has none) and turned by `req.yaw` about the vertical axis through the model's centre. Returns the centre too.
fn preview_pose(model: &ShapeModel, req: &PreviewRequest) -> (Vec3, Vec<Mat4>) {
    let (lo, hi) = model.bounds;
    // a chassis with no scale of its own (`buildings.csv` gives 0 for mechs) is drawn at its size
    let scale = if req.scale > 0.0 { req.scale } else { 1.0 };
    let centre = Vec3::new(
        (lo.x + hi.x) * 0.5 * scale,
        (lo.y + hi.y) * 0.5 * scale,
        (lo.z + hi.z) * 0.5 * scale,
    );
    // the model turns about the vertical axis through its centre (Stuff's y is up)
    let turn = object_turn(centre, req.yaw);
    let pose = model
        .bind_pose()
        .iter()
        .map(|m| turn.mul(&Mat4::scale(Vec3::new(scale, scale, scale)).mul(m)))
        .collect();
    (centre, pose)
}

/// The one instance a preview draws: `model_id` in `pose`, lit by `light`'s sun at intensity 1 (no terrain).
fn preview_instance(model_id: ModelId, pose: Vec<Mat4>, light: &LightModel) -> ShapeInstance {
    // A preview is deliberately a single bind-pose LOD-0 shape: it does not run a mission
    // appearance update.  Keep the R1 instance state internally consistent so the shared
    // renderer API remains usable after the LOD/damage additions.
    let mut shapes = ShapeSet::default();
    shapes.lods[0] = Some(model_id);
    ShapeInstance {
        model: model_id,
        shapes,
        state: AppearanceState::default(),
        choice: ShapeChoice::default(),
        pose,
        pose_key: None,
        turret: None,
        position: Vec3::ZERO,
        yaw_degrees: 0.0,
        infinite_light: light.ramp(1.0),
        ambient_override: None,
        casts_shadow: false,
        in_view: true,
        root_height_difference: 0.0,
        highlight: 0,
    }
}

/// The CPU half of the `MC2_DUMP_VERTEX_LIGHT` dump, without a GPU: loads `req`'s shape from `game` and writes the
/// lit colour of every vertex in the pose and light [`ModelPreviews::render`] uses, to `dir`. Returns the file.
pub fn dump_preview_vertex_light(
    game: &mut GameData,
    req: &PreviewRequest,
    dir: &std::path::Path,
) -> Result<std::path::PathBuf> {
    let camera = PreviewCamera::load(game)?;
    let shape = if req.building {
        building_shape(game, &req.shape)?
    } else {
        req.shape.clone()
    };
    let model = ShapeModel::load(game, &shape)?;
    let (_, pose) = preview_pose(&model, req);
    let light = camera.light_for(req.kind, req.activated);
    let inst = preview_instance(ModelId(0), pose, &light);
    write_vertex_light(
        dir,
        req,
        &shape,
        &model,
        &inst,
        &light,
        &ShapeStyle::default(),
    )
    .map_err(|e| mc2_render::RenderError::Other(format!("vertex light dump: {e}")))
}

/// Renders [`PreviewRequest`]s offscreen.
pub struct ModelPreviews {
    scene: SceneRenderer,
    game: GameData,
    settings: CameraSettings,
    camera: PreviewCamera,
    /// `Environment.screenWidth` of the screen being drawn: `SimpleCamera` projects through the whole
    /// screen and the model panel is a window onto it ([`ModelPreviews::render`]).
    screen_width: f32,
}

impl ModelPreviews {
    /// `game` supplies the shapes; `cameras` come from `system.cfg`'s `[CameraSettings]`.
    pub fn new(gpu: impl Into<Arc<Gpu>>, mut game: GameData) -> Result<Self> {
        let settings = CameraSettings::from_system_cfg(&mut game.fit("system.cfg")?);
        let camera = PreviewCamera::load(&mut game)?;
        let mut scene = SceneRenderer::new(gpu)?;
        scene.set_terrain(&stand_in_mission(), &stand_in_textures());
        Ok(ModelPreviews {
            scene,
            game,
            settings,
            camera,
            screen_width: 800.0,
        })
    }

    /// Renders `req` and stores its picture in `assets`. A request whose shape is missing is an
    /// error; the picture is then not stored.
    pub fn render_into(&mut self, assets: &mut UiAssets, req: &PreviewRequest) -> Result<()> {
        self.screen_width = assets.screen.0.max(1) as f32;
        let rgba = self.render(req)?;
        assets
            .textures
            .insert(&req.key(), req.width, req.height, rgba);
        Ok(())
    }

    /// Renders every queued request that is not stored yet. A request that fails is logged and
    /// recorded as failed ([`preview_failed`]), so the screen shows its fallback rather than a
    /// blank panel, and the request is not queued again.
    pub fn resolve(&mut self, assets: &mut UiAssets) {
        for req in assets.previews.take() {
            if assets.textures.find(&req.key()).is_some() {
                continue;
            }
            if let Err(e) = self.render_into(assets, &req) {
                log::warn!("preview of {} failed: {e}", req.shape);
                assets.previews.fail(&req);
            }
        }
    }

    /// The picture of `req` as tightly packed RGBA8 with the background keyed out.
    pub fn render(&mut self, req: &PreviewRequest) -> Result<Vec<u8>> {
        // a building appearance has no paint scheme: `BldgAppearance` keeps the empty
        // `Appearance::resetPaintScheme` (appear.h:390-392), so `setComponent`'s call changes nothing
        let style = ShapeStyle {
            paint: (!req.building).then_some((req.paint[0], req.paint[1], req.paint[2])),
            ..ShapeStyle::default()
        };
        let shape = if req.building {
            building_shape(&mut self.game, &req.shape)?
        } else {
            req.shape.clone()
        };
        let model_id = self.scene.load_shape(&mut self.game, &shape, style)?;
        let model = self.scene.model(model_id);
        let (centre, pose) = preview_pose(model, req);

        let mut cam = GameCamera::new(self.settings.clone(), (req.width as f32, req.height as f32));
        // the model is framed well inside the view, with the haze and the clip planes out of the way
        cam.settings.min_haze_distance = 1.0e9;
        cam.settings.max_clip_distance = 2.0e9;
        cam.settings.near_plane = -1.0;
        // `target` is in world coordinates; the model is in the Stuff frame
        cam.target = stuff_to_world().transform_point(centre);
        let kind = req.kind;
        // `SimpleCamera::update` sets the viewport to the whole screen (`screenResolution = viewMul`)
        // and only shifts the model into the panel, so the lens spans the screen, not the panel: the
        // panel sees the central `width / screen_width` of that field
        let half_tan = (deg_to_rad(cam.settings.fov_degrees) * 0.5).tan() * req.width as f32
            / self.screen_width;
        cam.settings.fov_degrees = (half_tan.atan() * 2.0).to_degrees();
        self.camera.aim(&mut cam, kind);

        let base_light = self.camera.light_for(kind, req.activated);
        let light_for = |clear: [u8; 3]| LightModel {
            fog_color: clear.map(|c| c as f32 / 255.0),
            // `SimpleCamera::render` sets `useFog = 0`
            fog_start: -1.0,
            fog_full: -2.0,
            ..base_light.clone()
        };
        let light = light_for(CLEAR_A);
        let water = WaterParams::default();
        let terrain = TerrainParams::default();
        let instances = [preview_instance(model_id, pose, &light)];
        if let Some(dir) = vertex_light::dump_dir() {
            dump_vertex_light(dir, req, &shape, model, &instances[0], &light, &style);
        }
        let mut frame = Frame {
            width: req.width,
            height: req.height,
            camera: &cam,
            light: &light,
            water: &water,
            terrain_params: &terrain,
            time: 0.0,
            precise_time: None,
            instances: &instances,
            shadows: false,
        };
        let over_a = self.scene.render(&frame)?;
        let light_b = light_for(CLEAR_B);
        frame.light = &light_b;
        let over_b = self.scene.render(&frame)?;
        Ok(matte(
            &over_a,
            &over_b,
            req.width as usize,
            req.height as usize,
        ))
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    /// Both red and blue stand above green: a blend with the magenta clear colour.
    fn is_magenta_tinted(px: &[u8]) -> bool {
        px[0] > px[1].saturating_add(8) && px[2] > px[1].saturating_add(8)
    }

    /// Composites `px` (straight RGBA) over `bg` the way the clear does.
    fn over(px: &[[u8; 4]], bg: [u8; 3]) -> Vec<u8> {
        px.iter()
            .flat_map(|p| {
                let a = p[3] as f32 / 255.0;
                let c: [u8; 3] = std::array::from_fn(|i| {
                    (p[i] as f32 * a + bg[i] as f32 * (1.0 - a)).round() as u8
                });
                [c[0], c[1], c[2], 255]
            })
            .collect()
    }

    fn matted(px: &[[u8; 4]]) -> Vec<u8> {
        matte(&over(px, CLEAR_A), &over(px, CLEAR_B), px.len(), 1)
    }

    /// Opaque purple paint is kept exactly wherever it sits, even beside the background.
    #[test]
    fn opaque_purple_is_preserved_next_to_the_background() {
        let purple = [139u8, 29, 140, 255];
        let mut row = vec![[0, 0, 0, 0]];
        row.extend(std::iter::repeat_n(purple, 6));
        let out = matted(&row);
        assert_eq!(out[3], 0);
        for p in out.chunks_exact(4).skip(1) {
            assert_eq!(p, &purple);
        }
    }

    /// A half-covered edge texel loses the key colour: it is un-mixed, and a bilinear sample
    /// between it and the keyed texel stays untinted.
    #[test]
    fn partial_edge_is_unmixed_and_filters_without_magenta() {
        let out = matted(&[[0, 0, 0, 0], [0, 230, 230, 128], [0, 230, 230, 255]]);
        assert_eq!(out[3], 0);
        assert!((out[7] as i32 - 128).abs() <= 2);
        assert!(
            (out[5] as i32 - 230).abs() <= 3 && out[4] <= 3,
            "{:?}",
            &out[4..8]
        );
        for step in 0..=8u32 {
            let mix: Vec<u8> = (0..3)
                .map(|c| ((out[c] as u32 * (8 - step) + out[4 + c] as u32 * step) / 8) as u8)
                .collect();
            assert!(!is_magenta_tinted(&mix), "{mix:?}");
        }
    }

    /// An isolated keyed texel inside an opaque region, and a narrow gap of near-key blends with no
    /// keyed texel beside them: after matting and a bilinear sample at every position no magenta
    /// survives, and every opaque texel is unchanged.
    #[test]
    fn isolated_keyed_texel_and_gap_blends_leave_no_magenta_after_scaling() {
        let body = [0u8, 230, 230, 255];
        let row = [
            body,
            body,
            [0, 0, 0, 0],
            [60, 90, 70, 40],
            [40, 50, 45, 90],
            body,
        ];
        let out = matted(&row);
        assert_eq!(&out[0..4], &body);
        assert_eq!(&out[20..24], &body);
        assert_eq!(out[11], 0);
        for i in 0..row.len() - 1 {
            for step in 0..=8u32 {
                let mix: Vec<u8> = (0..3)
                    .map(|c| {
                        ((out[i * 4 + c] as u32 * (8 - step) + out[i * 4 + 4 + c] as u32 * step)
                            / 8) as u8
                    })
                    .collect();
                assert!(!is_magenta_tinted(&mix), "texel {i} step {step}: {mix:?}");
            }
        }
    }

    #[test]
    fn bleed_gives_every_keyed_texel_a_neighbour_colour() {
        let mut px = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 230, 230, 255];
        bleed_keyed(&mut px, 3, 1);
        assert_eq!(&px[0..4], &[0, 230, 230, 0]);
        assert_eq!(&px[4..8], &[0, 230, 230, 0]);
    }
}

#[cfg(test)]
mod spinner_tests {
    use super::*;

    fn req() -> PreviewRequest {
        PreviewRequest {
            shape: "x".into(),
            paint: [0; 3],
            scale: 0.0,
            yaw: 0.0,
            width: 2,
            height: 2,
            building: false,
            kind: PreviewKind::Mech,
            activated: false,
        }
    }

    /// Production resolves the queued requests after the frame is drawn.
    fn resolve(assets: &mut UiAssets) {
        for r in assets.previews.take() {
            assets.textures.insert(&r.key(), 2, 2, vec![255; 16]);
        }
    }

    #[test]
    fn a_fresh_panel_shows_the_first_picture_at_eight_fps_and_keeps_turning() {
        let mut assets = UiAssets::default();
        let mut dl = DrawList::new(64, 64);
        let mut s = Spinner::default();
        let r = req();
        s.draw(&mut dl, &assets, &r, (0.0, 0.0, 2.0, 2.0));
        assert!(s.last.is_none());
        resolve(&mut assets);
        let mut shown = Vec::new();
        for _ in 0..12 {
            s.update(0.125);
            s.draw(&mut dl, &assets, &r, (0.0, 0.0, 2.0, 2.0));
            resolve(&mut assets);
            shown.push(s.last);
        }
        assert!(
            shown[0].is_some(),
            "the picture is adopted on the next frame"
        );
        assert!(
            shown.windows(2).any(|w| w[0] != w[1]),
            "and it keeps turning"
        );
    }

    #[test]
    fn a_repainted_request_is_a_different_cache_key() {
        let a = req();
        let mut b = a.clone();
        b.paint = [1, 2, 3];
        assert_ne!(a.key(), b.key());
    }

    /// The reference rig's prefs (`BaseColor = 16442661`, `Highlightcolor = 460862`) reach the
    /// three masks in the C++ order `resetPaintScheme(h1, h2, base)` (simplecamera.cpp:355) and come
    /// come out dark blue and yellow, as on Windows: the prefs are `0x00RRGGBB` swatch colours
    /// (`optionsarea.cpp:962-963`) and the Windows `setPaintScheme` uses them unswapped
    /// (no `BGR`, `mech3d.cpp:1738-1741`; `docs/evidence/rc-paint-windows.md`).
    #[test]
    fn the_players_prefs_paint_the_masks_in_the_cpp_order() {
        use mc2_render::shape::{apply_paint_scheme, PaintRoutine};
        let mut assets = UiAssets::default();
        assets.set_player_paint(16_442_661, 460_862);
        assert_eq!(assets.player_paint, [460_862, 460_862, 16_442_661]);
        let [red, green, blue] = assets.player_paint;
        // one pure-red, one pure-green and one pure-blue texel at full value
        let mut px = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255];
        apply_paint_scheme(&mut px, red, green, blue, PaintRoutine::Mech);
        let k = 1.0 - (1.0 - 255.0f32 / 256.0).powi(2);
        let want = |rgb: [u8; 3]| rgb.map(|c| (c as f32 * k) as u8);
        assert_eq!(px[0..3], want([0x07, 0x08, 0x3e]), "highlight 1");
        assert_eq!(px[4..7], want([0x07, 0x08, 0x3e]), "highlight 2");
        assert_eq!(px[8..11], want([0xfa, 0xe5, 0x25]), "base");
        assert_eq!(px[8..11], [249, 228, 36], "base is yellow");
        assert_eq!(px[0..3], [6, 7, 61], "highlight is dark blue");
    }

    /// The independent anchor of `docs/evidence/rc-paint-windows.md`: the options screen draws a
    /// prefs colour as its swatch (`rects[36].setColor(prefs.baseColor)`, `optionsarea.cpp:992`,
    /// a `gos_VERTEX` `argb`) and paints its Bushwacker with the same value
    /// (`optionsarea.cpp:949-950`).  On Windows both show the same colour, so a full-strength mask
    /// texel takes the swatch's colour (one truncation step under it).
    #[test]
    fn a_painted_mask_shows_the_colour_its_swatch_draws() {
        use mc2_render::shape::{apply_paint_scheme, PaintRoutine};
        for c in [0x00fa_e525u32, 0x0007_083e, 0xffff_7e00, 0xff12_3456] {
            let swatch = crate::render::argb_to_rgba(c);
            let mut px = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255];
            apply_paint_scheme(&mut px, c, c, c, PaintRoutine::Mech);
            for texel in px.chunks_exact(4) {
                for (g, w) in texel[..3].iter().zip(&swatch[..3]) {
                    assert!(
                        *w as i32 - *g as i32 <= 1 && g <= w,
                        "{c:08x}: {texel:?} vs {swatch:?}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod camera_tests {
    use super::*;

    const FIT: &str = "FITini\r\n[Cameras]\r\nf ProjectionAngle = 33.5\r\nf StartRotation = 180.0\r\nuc LightRed = 200\r\nuc LightGreen = 100\r\nuc LightBlue = 50\r\nuc AmbientRed = 10\r\nuc AmbientGreen = 20\r\nuc AmbientBlue = 30\r\nf LightDirYaw = 30.0\r\nf LightDirPitch = 40.0\r\nFITend\r\n";

    #[test]
    fn altitude_is_chosen_by_object_kind() {
        // simplecamera.cpp:304,377 (mech, vehicle), :446 (component), :519 (building)
        assert_eq!(PreviewKind::Mech.altitude_tight(), 650.0);
        assert_eq!(PreviewKind::Component.altitude_tight(), 580.0);
        assert_eq!(PreviewKind::Building.altitude_tight(), 800.0);
        assert_eq!(PREVIEW_ROTATION, -45.0);
    }

    #[test]
    fn camera_reads_the_fit_and_turns_the_light_right_by_ninety() {
        let mut fit = FitIni::from_bytes(FIT.as_bytes().to_vec()).unwrap();
        let cam = PreviewCamera::from_fit(&mut fit).unwrap();
        assert_eq!(cam.projection_angle, 33.5);
        assert_eq!(cam.start_rotation, 180.0);
        assert_eq!(cam.light.yaw, -60.0);
        assert_eq!(cam.light.pitch, 40.0);
        assert_eq!(
            cam.light.sun_color,
            [200.0 / 255.0, 100.0 / 255.0, 50.0 / 255.0]
        );
        // 40 degrees is above the 35 of the sunset blend: an activated camera keeps the day light
        assert_eq!(
            cam.light_for(PreviewKind::Mech, true).sun_color,
            cam.light.sun_color
        );
        let comp = cam.light_for(PreviewKind::Component, false);
        assert_eq!(
            comp.sun_color,
            [196.0 / 255.0, 196.0 / 255.0, 220.0 / 255.0]
        );
        assert_eq!(comp.ambient, [196.0 / 255.0; 3]);
        assert_eq!(cam.light_for(PreviewKind::Building, false), cam.light);
        assert_eq!(
            cam.light_for(PreviewKind::Component, true).sun_color,
            comp.sun_color
        );
    }

    /// The light a Mech preview feeds the shapes, for the shipped `cameras.fit` numbers
    /// (LightRGB 255, AmbientRGB 31, LightDirYaw -61.25, LightDirPitch 22.5), against the C++:
    /// `Mech3DAppearance::update` (mech3d.cpp:3025-3038, `setLightColor(0, ...)` at 3037) with `land == NULL` in logistics
    /// (`lightIntensity = 1.0`, `land` only exists between `Mission::init` and mission.cpp:3171)
    /// sets light 0 to `Camera::getLightRed/Green/Blue(1.0)` = `min(light + ambient, 255)` and light 1
    /// (ambient) is `ambient` (camera.cpp:1644-1647); `SimpleCamera::render`'s white light 1
    /// (simplecamera.cpp:148-149) comes after the vertices are lit, so no vertex sees it. A camera's first
    /// model is lit by the fit light; once the camera has been updated, `setMech`'s `activate()` runs
    /// `updateDaylight(true)` (camera.cpp:1775-1784): the sunset blend at 22.5 degrees, 252/159/140 and 31. Both
    /// are what the instrumented C++ dumped (rust/docs/evidence/rc-vqa-vertexdump.md: purchase screen 255/255/255
    /// for its first mech, 252/159/140 for the next ones).
    #[test]
    fn mech_preview_light_is_the_cpp_light_with_no_terrain() {
        let fit = "FITini\r\n[Cameras]\r\nf ProjectionAngle = 35\r\nf StartRotation = 180\r\n\
                   uc LightRed = 255\r\nuc LightGreen = 255\r\nuc LightBlue = 255\r\n\
                   uc AmbientRed = 31\r\nuc AmbientGreen = 31\r\nuc AmbientBlue = 31\r\n\
                   f LightDirYaw = -61.25\r\nf LightDirPitch = 22.5\r\nFITend\r\n";
        let mut fit = FitIni::from_bytes(fit.as_bytes().to_vec()).unwrap();
        let cam = PreviewCamera::from_fit(&mut fit).unwrap();
        // a fresh camera: the fit light; intensity 1.0 (no terrain): 255 + 31 saturates at 255
        let first = cam.light_for(PreviewKind::Mech, false);
        assert_eq!(first.sun_color, [1.0; 3]);
        assert_eq!(first.ambient, [31.0 / 255.0; 3]);
        assert_eq!(first.ramp(1.0), [1.0; 3]);
        // an activated camera: the sunset light; light 0 = min(light + ambient, 255) = 0xffbeab
        let l = cam.light_for(PreviewKind::Mech, true);
        assert_eq!(l.sun_color, [252.0 / 255.0, 159.0 / 255.0, 140.0 / 255.0]);
        assert_eq!(l.ambient, [31.0 / 255.0; 3]);
        assert_eq!(
            l.ramp(1.0).map(|v| (v * 255.0).round() as u32),
            [0xff, 0xbe, 0xab]
        );
        // `rotateLightRight(90)`: yaw -61.25 - 90; the shapes are lit by the TG light matrix
        // `EulerAngles(pitch, yaw + 135)` (camera.cpp:2893), not by `lightDirection` (:1517-1521)
        assert_eq!(l.yaw, -151.25);
        let d = l.shape_light_direction();
        let (p, a) = (22.5f32.to_radians(), (-151.25f32 + 45.0).to_radians());
        for (g, w) in [
            (d.x, p.cos() * a.cos()),
            (d.y, p.cos() * a.sin()),
            (d.z, p.sin()),
        ] {
            assert!((g - w).abs() < 1e-6, "{d:?}");
        }
        // a building keeps the fit light; a component takes the fixed 196/196/220 and 196 ambient
        assert_eq!(cam.light_for(PreviewKind::Building, true), l);
        assert_eq!(cam.light_for(PreviewKind::Building, false), first);
        let c = cam.light_for(PreviewKind::Component, true);
        assert_eq!(c.ramp(1.0), [1.0; 3]);
        assert_eq!(c.ambient, [196.0 / 255.0; 3]);
    }

    #[test]
    fn camera_and_object_rotation_are_applied_separately() {
        let mut fit = FitIni::from_bytes(FIT.as_bytes().to_vec()).unwrap();
        let pc = PreviewCamera::from_fit(&mut fit).unwrap();
        let mut cam = GameCamera::new(CameraSettings::default(), (640.0, 480.0));
        pc.aim(&mut cam, PreviewKind::Component);
        assert_eq!(cam.projection_angle, 33.5);
        assert_eq!(cam.altitude, 580.0);
        let r0 = cam.rotation;
        pc.aim(&mut cam, PreviewKind::Building);
        assert_eq!(cam.altitude, 800.0);
        assert_eq!(cam.rotation, r0);
        // the object turn depends on the spin only, never on the camera's StartRotation
        let c = Vec3::new(1.0, 2.0, 3.0);
        let a = object_turn(c, 0.0);
        let b = Mat4::translation(c)
            .mul(&Mat4::rotation_y(deg_to_rad(-45.0)))
            .mul(&Mat4::translation(-c));
        let p = Vec3::new(5.0, 1.0, -2.0);
        let (pa, pb) = (a.transform_point(p), b.transform_point(p));
        assert!((pa.x - pb.x).abs() < 1e-5 && (pa.z - pb.z).abs() < 1e-5);
        let spun = object_turn(c, 90.0).transform_point(p);
        assert!((spun.x - pa.x).abs() > 1e-3 || (spun.z - pa.z).abs() > 1e-3);
    }

    #[test]
    fn shipped_cameras_fit_values() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED {}: MC2_DATA is unset", module_path!());
            return;
        };
        let mut game = GameData::open(root).expect("open game data");
        let mut fit = game
            .fit(PreviewCamera::FIT)
            .expect("cameras.fit ships in camera.fst");
        let cam = PreviewCamera::from_fit(&mut fit).unwrap();
        eprintln!("shipped cameras.fit: {cam:?}");
        assert_eq!(cam.projection_angle, 35.0);
        assert_eq!(cam.start_rotation, 180.0); // camera.fst: StartRotation
        assert_eq!(cam.light.yaw, -151.25); // LightDirYaw -61.25, minus the 90 of rotateLightRight
        assert_eq!(cam.light.pitch, 22.5);
        assert_eq!(cam.light.sun_color, [1.0; 3]);
        // an activated camera's sunset light (camera.cpp:1775-1784), as the instrumented C++ dumped it
        assert_eq!(
            cam.light_for(PreviewKind::Mech, true).sun_color,
            [252.0 / 255.0, 159.0 / 255.0, 140.0 / 255.0]
        );
        assert_eq!(cam.light.ambient, [31.0 / 255.0; 3]);
    }
}

#[cfg(test)]
mod camera_data_tests {
    use super::*;

    fn parse(text: &str) -> Result<PreviewCamera> {
        let mut fit = FitIni::from_bytes(text.as_bytes().to_vec()).unwrap();
        PreviewCamera::from_fit(&mut fit)
    }

    #[test]
    fn present_but_invalid_camera_data_is_an_error() {
        // no [Cameras] block
        assert!(parse("FITini\r\n[Other]\r\nf X = 1\r\nFITend\r\n").is_err());
        // no ProjectionAngle
        let e = parse("FITini\r\n[Cameras]\r\nf StartRotation = 0\r\nFITend\r\n").unwrap_err();
        assert!(e.to_string().contains("ProjectionAngle"), "{e}");
        // no StartRotation
        let e = parse("FITini\r\n[Cameras]\r\nf ProjectionAngle = 35\r\nFITend\r\n").unwrap_err();
        assert!(e.to_string().contains("StartRotation"), "{e}");
        // no light
        let e = parse(
            "FITini\r\n[Cameras]\r\nf ProjectionAngle = 35\r\nf StartRotation = 0\r\nFITend\r\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("LightRed"), "{e}");
    }

    #[test]
    fn requests_of_every_kind_have_distinct_keys() {
        let mut r = PreviewRequest {
            shape: "x".into(),
            paint: [0; 3],
            scale: 1.0,
            yaw: 0.0,
            width: 2,
            height: 2,
            building: false,
            kind: PreviewKind::Mech,
            activated: false,
        };
        let mech = r.key();
        r.kind = PreviewKind::Component;
        let comp = r.key();
        r.kind = PreviewKind::Building;
        assert_ne!(mech, comp);
        assert_ne!(comp, r.key());
        // the same model on a camera that has been activated is lit differently, so it is another picture
        let first = r.key();
        r.activated = true;
        assert_ne!(first, r.key());
    }

    /// A screen's camera lights its first model with the fit light and, once it has been updated with a model,
    /// every later one with the activation daylight; an update with no model does not activate it
    /// (`SimpleCamera::update` does nothing without `pObject`, simplecamera.cpp:219-221).
    #[test]
    fn a_camera_is_activated_by_its_first_update_with_a_model() {
        let req = PreviewRequest {
            shape: "werewolf".into(),
            paint: [0; 3],
            scale: 0.0,
            yaw: 0.0,
            width: 125,
            height: 147,
            building: false,
            kind: PreviewKind::Mech,
            activated: true,
        };
        let mut cam = CameraActivation::default();
        assert!(!cam.set(req.clone()).activated);
        cam.update(false);
        assert!(!cam.set(req.clone()).activated);
        cam.update(true);
        assert!(cam.set(req.clone()).activated);
        cam.update(false);
        assert!(cam.set(req).activated);
    }
}
