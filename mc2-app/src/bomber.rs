// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The airstrike bomber: the "Shilone" `GVAppearance` an `Artillery` strike owns (`code/artlry.cpp:1580-1596`) and the bomb run it
//! plays from `timeToImpact <= 2.5` (`artlry.cpp:938-1041`, drawn by `Artillery::render`, `artlry.cpp:1407-1411`).
//!
//! The strike keeps the bomber at the strike position with gesture 0 (`setGesture(0)`); the bomb-run animation moves the mesh nodes
//! from the aircraft's approach to the drop, and the jet contrail is re-placed at the animated `activity_node` every frame.
//! Everything comes from the shipped data: the appearance `.ini`, its `.tgl` mesh and its gesture-0 `.agl`.

use mc2_render::appearance::{AppearanceKind, AppearanceState, ShapeChoice, ShapeSet};
use mc2_render::game::GameData;
use mc2_render::math::Vec3;
use mc2_render::scene::{ModelId, SceneRenderer, ShapeInstance, ShapeStyle};
use mc2_render::shape::{instance_node_matrices, AnimationClip};
use mc2_sim::appearance::VehicleAppearanceType;
use std::collections::HashMap;

/// `appearanceTypeList->getAppearance(GV_TYPE << 24, "Shilone")` (`artlry.cpp:1580`).
pub const BOMBER_APPEARANCE: &str = "Shilone";
/// The node the right contrail follows (`bomber->getNodeNamePosition("activity_node")`, `artlry.cpp:953`).
pub const ACTIVITY_NODE: &str = "activity_node";

/// How long a bomber without an animation rate stays up, in seconds.
const STATIC_RUN_SECONDS: f32 = 5.0;

/// The loaded bomber: its uploaded mesh and gesture-0 animation, and where the contrail node is on each frame.
#[derive(Clone, Debug)]
pub struct BomberRig {
    /// The uploaded mesh.
    pub model: ModelId,
    clip: AnimationClip,
    num_frames: i32,
    frame_rate: f32,
    start_frame: f32,
    looping: bool,
    reversed: bool,
    /// The `activity_node` position relative to the strike position, in game world coordinates, for each animation frame
    /// (`None` when the mesh has no such node).
    activity: Option<Vec<Vec3>>,
}

impl BomberRig {
    /// Loads the Shilone appearance: type `.ini`, mesh and gesture-0 animation (`GVAppearanceType::init`, `gvactor.cpp:103`).
    pub fn load(game: &mut GameData, scene: &mut SceneRenderer) -> Result<Self, String> {
        let ty = VehicleAppearanceType::load(&mut game.vfs, BOMBER_APPEARANCE)
            .map_err(|e| e.to_string())?;
        let ini_path = format!("data/tgl/{}.ini", BOMBER_APPEARANCE.to_ascii_lowercase());
        let mut ini = game
            .vfs
            .read_fit(&ini_path)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{ini_path} not found"))?;
        ini.seek_block("TGLData")
            .map_err(|_| format!("{ini_path}: no TGLData block"))?;
        let file = ini
            .read_string("FileName", 511)
            .map_err(|_| format!("{ini_path}: TGLData has no FileName"))?;
        let set = ty
            .anims
            .first()
            .and_then(Option::as_ref)
            .ok_or("the bomber has no gesture 0 animation")?;
        let model = scene
            .load_shape_owned(game, &file, ShapeStyle::default(), BOMBER_APPEARANCE)
            .map_err(|e| e.to_string())?;
        let clip = AnimationClip::from_agl(scene.model(model), &set.entries);
        scene.set_gesture_clips(model, vec![Some(clip.clone())]);
        let num_frames = ty.num_frames(0);
        let activity = scene.model(model).node_index(ACTIVITY_NODE).map(|node| {
            (0..num_frames.max(1))
                .map(|f| {
                    let pose = scene
                        .model(model)
                        .pose(Some(&clip), f as f32, &HashMap::new());
                    instance_node_matrices(&pose, Vec3::ZERO, 0.0)[node].transform_point(Vec3::ZERO)
                })
                .collect()
        });
        Ok(Self {
            model,
            clip,
            num_frames,
            frame_rate: ty.frame_rate(0),
            start_frame: ty.anim_start[0] as f32,
            looping: ty.anim_loop[0],
            reversed: ty.anim_reverse[0],
            activity,
        })
    }

    /// The animation frame `steps` frames after the bomb run started: `setGesture(0)` puts the animation on its start frame, the
    /// first `update` keeps it (`setFirstFrame`), and every later one advances it by `frameRate * frameLength`
    /// (`GVAppearance::update`, `gvactor.cpp`; the same rule as `mc2_sim`'s `VehicleAppearance::update`).
    pub fn frame_after(&self, steps: i64, frame_length: f32) -> f32 {
        let n = self.num_frames as f32;
        let mut frame = if self.reversed {
            n - 1.0
        } else {
            self.start_frame
        };
        if self.frame_rate == 0.0 || n <= 0.0 {
            return frame;
        }
        let inc = self.frame_rate * frame_length;
        for _ in 0..steps.max(0) {
            frame += if self.reversed { -inc } else { inc };
            if frame >= n {
                frame = if self.looping { frame - n } else { n - 1.0 };
            }
            if frame < 0.0 {
                frame = if self.looping { frame + n } else { 0.0 };
            }
        }
        frame
    }

    /// Whether a run that started `steps` frames ago is over. The C++ deletes the bomber and its contrails once the hit effect has
    /// finished (`artlry.cpp:875-923`), long after the sim object stops existing here, so the presentation ends with the bomb-run
    /// animation (`numFrames / frameRate`; a bomber with no animation rate gets `STATIC_RUN_SECONDS`).
    pub fn run_finished(&self, steps: i64, frame_length: f32) -> bool {
        let seconds = if self.frame_rate == 0.0 || self.num_frames <= 0 {
            STATIC_RUN_SECONDS
        } else {
            self.num_frames as f32 / self.frame_rate.abs()
        };
        steps as f32 * frame_length >= seconds
    }

    /// Where the contrail is on `frame`, as an offset from the strike position (game world coordinates); zero when the mesh has
    /// no `activity_node`.
    pub fn activity_offset(&self, frame: f32) -> Vec3 {
        let Some(track) = &self.activity else {
            return Vec3::ZERO;
        };
        let i = (frame.round_ties_even().max(0.0) as usize).min(track.len().saturating_sub(1));
        track.get(i).copied().unwrap_or(Vec3::ZERO)
    }

    /// The bomber's instance at the strike position `at`, posed on `frame`.
    pub fn instance(
        &self,
        scene: &SceneRenderer,
        at: Vec3,
        frame: f32,
        infinite_light: [f32; 3],
    ) -> ShapeInstance {
        let state = AppearanceState {
            gesture: Some(0),
            frame,
            ..Default::default()
        };
        let mut lods = [None; mc2_render::appearance::MAX_LODS];
        lods[0] = Some(self.model);
        let pose = scene.model(self.model).pose(
            Some(&self.clip),
            frame.round_ties_even().max(0.0),
            &HashMap::new(),
        );
        ShapeInstance {
            model: self.model,
            shapes: ShapeSet {
                kind: AppearanceKind::Vehicle,
                lods,
                ..Default::default()
            },
            state,
            choice: ShapeChoice::default(),
            pose,
            pose_key: Some((self.model, state)),
            turret: None,
            position: at,
            yaw_degrees: 0.0,
            infinite_light,
            ambient_override: None,
            casts_shadow: true,
            in_view: true,
            root_height_difference: 0.0,
            highlight: 0,
        }
    }
}
