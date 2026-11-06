// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Mech3DAppearanceType` / `Mech3DAppearance` (`mclib/mech3d.{h,cpp}`): the gesture state machine, jump timing, weapon
//! node bookkeeping and node geometry of a mech, without rendering, lighting, sound or gosFX.
//!
//! What is kept is everything the simulation can observe: the gesture/frame state (`setGestureGoal`, `update`), the gesture
//! velocity (`getVelocityOfGesture`), jump state and velocity, weapon node use/recycle counters, the skeleton matrices the
//! node getters read (computed by [`mc2_assets::tgl_instance`] exactly like `TG_MultiShape::TransformMultiShape`), the LOD
//! shape switch the camera triggers in `recalcBounds`, and the RNG draws the C++ makes (idle time and the destroyed-fall
//! `RollDice(50)` on the SIM stream, arm debris on the VISUAL stream). Dropped as presentation: textures, paint, fog,
//! lights, shadows, sensor contact shapes, foot poofs, dust/smoke/jump-jet/wake/shoulder effects,
//! flashing, the blown-off arm flight, selection boxes and screen positions. Footprints are kept as events
//! ([`FootState`]): the sink draws them as craters.

use mc2_assets::stuff::quat_from_euler;
use mc2_assets::tgl::{decode_animation, decode_shape};
use mc2_assets::tgl_instance::{AnimSet, ShapeInstance, ShapeType};
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;

use super::fx::FxCall;
use super::tables::{
    gesture, state, BASE_NODE_RECYCLE_TIME, DEGREES_TO_RADS, GESTURE_OFFSET_SIZE, IDLE_MAX,
    MAX_ANIMATION_FILES, MAX_LODS, MAX_TRANSITION_GESTURES, MECH_ANIMATION_NAMES, TRANSITION_ARRAY,
};
use super::{AppearanceError, AppearanceTerrain};
use crate::events::{fx_id, FxAction, FxBase, FxEffect, FxPlacement, FxSlot};
use crate::math::Vec3;
use crate::rng::SimRng;

/// `tglPath` (mclib/paths.cpp:42).
pub const TGL_PATH: &str = "data/tgl/";

/// `MECH3D_WEAPONTYPE_*` (objectappearance.h:26-32).
pub mod weapon_type {
    pub const NONE: i32 = -1;
    pub const ANY: i32 = 0;
    pub const MISSILE: i32 = 1;
    pub const BALLISTIC: i32 = 2;
    pub const NONENERGY: i32 = 3;
    pub const ENERGY: i32 = 4;
    pub const DIRECT: i32 = 5;
}

/// `OBJECT_STATUS_*` values the appearance compares against (code/dgameobj.h).
pub mod object_status {
    pub const NORMAL: i32 = 0;
    pub const DISABLED: i32 = 1;
    pub const DESTROYED: i32 = 2;
    pub const STARTING_UP: i32 = 3;
    pub const SHUTTING_DOWN: i32 = 4;
    pub const SHUTDOWN: i32 = 5;
}

/// `GestureData` (mech3d.h:219): one `[Gestures%d]` block of the mech `.ini`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GestureData {
    /// `StartVel`: speed at frame 0 (m/s).
    pub start_vel: f32,
    /// `EndVel`: speed at the last frame.
    pub end_vel: f32,
    /// `StartFrame` (`long`). For the jump gesture: takeoff frame `<< 16 |` landing frame.
    pub frame_start: i32,
    /// `Reverse`: play backwards (negates the animation's frame rate at load).
    pub reverse: bool,
    /// `RightFootDown0/1`, `LeftFootDown0/1` (99999 when missing): footprint frames, presentation only.
    pub foot_down: [i32; 4],
}

/// `NodeData` (objectappearance.h:37).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeData {
    /// `nodeId`.
    pub node_id: String,
    /// `weaponType` (`MECH3D_WEAPONTYPE_*`).
    pub weapon_type: i32,
    /// `isRArmNode` (rewritten by every instance's `init`, see [`MechAppearance::new`]).
    pub is_r_arm_node: bool,
    /// `isLArmNode`.
    pub is_l_arm_node: bool,
}

/// Which type shape an instance's `mechShape` was created from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    /// `mechType->mechShape[lod]`.
    Lod(usize),
    /// `mechForwardDmgShape` (destroyed while fallen forward).
    ForwardDamage,
    /// `mechBackwardDmgShape` (destroyed while fallen backward; loaded from the *FallForwardDam* file, mech3d.cpp:330).
    BackwardDamage,
}

/// `Mech3DAppearanceType`: shared by every mech with the same appearance name. Mutable at run time in two places, as in
/// the C++: the animation sets cache node ids on first use, and every instance's `init` rewrites the arm flags of the
/// weapon nodes.
#[derive(Clone, Debug, Default)]
pub struct MechAppearanceType {
    /// `name` (the appearance base file name, e.g. `werewolf`).
    pub name: String,
    /// `mechShape[MAX_LODS]` node tables (`None` where the ini has no such LOD).
    pub shapes: [Option<ShapeType>; MAX_LODS],
    /// `lodDistance[MAX_LODS]`.
    pub lod_distance: [f32; MAX_LODS],
    /// `mechForwardDmgShape` (`None` when the file is missing or has no nodes).
    pub forward_damage: Option<ShapeType>,
    /// `mechBackwardDmgShape`.
    pub backward_damage: Option<ShapeType>,
    /// `mechAnim[MAX_MECH_ANIMATIONS]`, indexed by gesture.
    pub anims: Vec<Option<AnimSet>>,
    /// `gestures[MaxGestures]`.
    pub gestures: [GestureData; gesture::MAX],
    /// `numSmokeNodes`.
    pub num_smoke_nodes: i32,
    /// `numWeaponNodes`.
    pub num_weapon_nodes: i32,
    /// `numJumpNodes`.
    pub num_jump_nodes: i32,
    /// `numFootNodes`.
    pub num_foot_nodes: i32,
    /// `nodeData`: smoke, weapon, jump jet, foot nodes in that order.
    pub node_data: Vec<NodeData>,
    /// `rightFootprintType` / `leftFootprintType` (mech3d.cpp:421): the crater shape ids of the `[FootPrint]` block.
    pub right_footprint_type: i32,
    /// See `right_footprint_type`.
    pub left_footprint_type: i32,
}

/// The footprint bookkeeping of `Mech3DAppearance` (`rightFootDone0/1`, `leftFootDone0/1`) and the footprints it has
/// raised since the owner last took them. Presentation only: not saved, not in the digest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FootState {
    /// `rightFootDone0`, `rightFootDone1`, `leftFootDone0`, `leftFootDone1`.
    pub done: [bool; 4],
    /// `craterManager->addCrater(type, footPosition, rotation)` calls: (type, foot node position, rotation).
    pub pending: Vec<(i32, Vec3, f32)>,
}

/// `MAX_DUST_POOFS` (mech3d.h:244).
pub const MAX_DUST_POOFS: u8 = 3;
/// `JUMP_PITCH` (mech3d.cpp:140).
const JUMP_PITCH: f32 = 90.0;

/// The effect bookkeeping of `Mech3DAppearance` (`currentRightPoof`, `jumpFXSetup`, `isWaking`, `isSmoking`, `isDusting`,
/// `fallDust`) and the effect calls it has made since the owner last took them.
#[derive(Clone, Debug, PartialEq)]
pub struct MechFxState {
    /// `currentRightPoof`, `currentLeftPoof`.
    pub next_poof: [u8; 2],
    /// `jumpFXSetup`.
    pub jump_fx_setup: bool,
    /// `isWaking`.
    pub waking: bool,
    /// The foot dust poofs are not drawn (`isWaking`, mech3d.cpp:2432).
    pub feet_hidden: bool,
    /// The wake is not drawn (a jumping mech, mech3d.cpp:2579).
    pub wake_hidden: bool,
    /// `isSmoking`: -1 none, 0 damage smoke, 1 critical smoke.
    pub smoking: i32,
    /// `isDusting`.
    pub dusting: bool,
    /// `fallDust`.
    pub fall_dust: bool,
    /// `useNonWeaponEffects` (the owner sets it before each update).
    pub non_weapon: bool,
    /// `movedThisFrame`.
    pub moved: bool,
    /// The blown-off arms in flight: 0 is the right arm, 1 the left (`dVel[]`, `dAcc[]`, `dTime[]`, `rightArmPos` / `leftArmPos`).
    pub arms: [ArmFlight; 2],
    /// The calls, oldest first.
    pub calls: Vec<FxCall>,
}

/// The flight of a blown-off arm (`dVel`, `dAcc`, `dTime`, `leftArmPos`): only its position matters to the effects that
/// follow it (`leftArmSmoke`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmFlight {
    /// The arm smoke is running (`leftArmSmoke != NULL`).
    pub smoke: bool,
    /// The shoulder pop is running (`leftShoulderBoom != NULL`).
    pub boom: bool,
    pub pos: Vec3,
    pub vel: Vec3,
    pub acc: Vec3,
    /// `dTime`: seconds the arm may lie on the ground before it stops.
    pub time: f32,
}

impl Default for MechFxState {
    fn default() -> MechFxState {
        MechFxState {
            next_poof: [0; 2],
            jump_fx_setup: false,
            waking: false,
            feet_hidden: false,
            wake_hidden: false,
            smoking: -1,
            dusting: false,
            fall_dust: false,
            non_weapon: true,
            moved: false,
            arms: [ArmFlight::default(), ArmFlight::default()],
            calls: Vec::new(),
        }
    }
}

/// Reads `data/tgl/<file>.tgl` (the binary copy `LoadTGMultiShapeFromASE` loads); `None` when absent.
pub fn read_shape_file(vfs: &mut Vfs, file: &str) -> Result<Option<ShapeType>, AppearanceError> {
    let path = format!("{TGL_PATH}{}.tgl", file.to_ascii_lowercase());
    match vfs.read(&path)? {
        Some(bytes) => Ok(Some(ShapeType::from_multishape(&decode_shape(&bytes)?))),
        None => Ok(None),
    }
}

impl MechAppearanceType {
    /// `Mech3DAppearanceType::init(fileName)` (mech3d.cpp:187), simulation-relevant data only (no shadow, arm, sensor
    /// shapes). INI reads keep the FIT semantics the C++ relies on: a missing `[Gestures%d]` block leaves the previous
    /// block selected, a missing key leaves the field at its previous value (zero, `MemSet(gestures, 0)` in the type's
    /// `init`, alariq's initialisation; re-derived: the original heap block was zero-filled in practice).
    pub fn load(vfs: &mut Vfs, name: &str) -> Result<MechAppearanceType, AppearanceError> {
        let ini_path = format!("{TGL_PATH}{}.ini", name.to_ascii_lowercase());
        let mut ini = vfs
            .read_fit(&ini_path)?
            .ok_or_else(|| AppearanceError(format!("mech appearance {ini_path} not found")))?;
        let mut t = MechAppearanceType {
            name: name.to_string(),
            ..Default::default()
        };
        for i in 0..gesture::MAX {
            let _ = ini.seek_block(&format!("Gestures{i}"));
            let g = &mut t.gestures[i];
            if let Ok(v) = ini.read_f32("StartVel") {
                g.start_vel = v;
            }
            if let Ok(v) = ini.read_f32("EndVel") {
                g.end_vel = v;
            }
            if let Ok(v) = ini.read_i64("StartFrame") {
                g.frame_start = v as i32;
            }
            if let Ok(v) = ini.read_bool("Reverse") {
                g.reverse = v;
            }
            for (k, key) in [
                "RightFootDown0",
                "RightFootDown1",
                "LeftFootDown0",
                "LeftFootDown1",
            ]
            .iter()
            .enumerate()
            {
                g.foot_down[k] = ini.read_i64(key).map_or(99999, |v| v as i32);
            }
        }
        ini.seek_block("TGLData")
            .map_err(|_| AppearanceError(format!("Mech {name} has no TGL Data")))?;
        if let Ok(file) = ini.read_string("FileName", 511) {
            t.shapes[0] = read_shape_file(vfs, &file)?;
        } else {
            for i in 0..MAX_LODS {
                if let Ok(file) = ini.read_string(&format!("FileName{i}"), 511) {
                    t.lod_distance[i] = ini.read_f32(&format!("Distance{i}")).map_err(|_| {
                        AppearanceError(format!("LOD {i} has no distance value in file {name}"))
                    })?;
                    t.shapes[i] = read_shape_file(vfs, &file)?;
                } else if i == 0 {
                    return Err(AppearanceError(format!("No base LOD for shape {name}")));
                }
            }
        }
        if t.shapes[0].is_none() {
            return Err(AppearanceError(format!(
                "mech {name}: base shape file missing"
            )));
        }
        // Both damage shapes read the FallForwardDam file (mech3d.cpp:327-331: backwardName is printed with the
        // forward suffix). A shape without nodes is dropped.
        let dmg = read_shape_file(vfs, &format!("{name}FallForwardDam"))?.filter(|s| !s.is_empty());
        t.forward_damage = dmg.clone();
        t.backward_damage = dmg;
        #[allow(clippy::needless_range_loop)]
        for i in 0..MAX_ANIMATION_FILES {
            let anim_name = format!("{name}{}", MECH_ANIMATION_NAMES[i]).to_ascii_lowercase();
            let ase = format!("{TGL_PATH}{anim_name}.ase");
            let agl = format!("{TGL_PATH}{anim_name}.agl");
            let set = if vfs.locate(&ase).is_some() || vfs.locate(&agl).is_some() {
                // LoadTGMultiShapeAnimationFromASE loads the binary copy; with only the .ase present the C++ would parse
                // ASCII (never the case in shipped data).
                let bytes = vfs.read(&agl)?.ok_or_else(|| {
                    AppearanceError(format!("{agl}: only an .ase file, which is not supported"))
                })?;
                let mut set = AnimSet::new(decode_animation(&bytes)?);
                if t.gestures[i].reverse {
                    set.reverse_frame_rate();
                }
                Some(set)
            } else {
                None
            };
            t.anims.push(set);
        }
        // [FootPrint] (mech3d.cpp:421): a missing block zeroes both types, but the keys are still read from whatever block
        // is selected then. `FootprintType` sets both feet; otherwise `RightFootprintType` and `LeftFootprintType`.
        if ini.seek_block("FootPrint").is_err() {
            t.left_footprint_type = 0;
            t.right_footprint_type = 0;
        }
        if let Ok(v) = ini.read_i64("FootprintType") {
            t.right_footprint_type = v as i32;
            t.left_footprint_type = t.right_footprint_type;
        } else {
            match ini.read_i64("RightFootprintType") {
                Ok(v) => t.right_footprint_type = v as i32,
                Err(_) => {
                    t.left_footprint_type = 0;
                    t.right_footprint_type = 0;
                }
            }
            t.left_footprint_type = ini
                .read_i64("LeftFootprintType")
                .map_or(t.right_footprint_type, |v| v as i32);
        }
        // [Nodes]: a missing block leaves the [FootPrint] block (or whatever was last found) selected.
        let _ = ini.seek_block("Nodes");
        t.num_smoke_nodes = ini.read_i64("NumSmoke").map_or(0, |v| v as i32);
        t.num_weapon_nodes = ini.read_i64("NumWeapon").map_or(0, |v| v as i32);
        t.num_jump_nodes = ini.read_i64("NumJumpjet").map_or(0, |v| v as i32);
        t.num_foot_nodes = ini.read_i64("NumFeet").map_or(0, |v| v as i32);
        let node = |ini: &mut FitIni,
                    block: String,
                    key: &str,
                    weapon: bool|
         -> Result<NodeData, AppearanceError> {
            ini.seek_block(&block)
                .map_err(|_| AppearanceError(format!("{name}: [{block}] missing")))?;
            let node_id = ini.read_string(key, 511)?;
            let weapon_type = if weapon {
                ini.read_i64("WeaponType")? as i32
            } else {
                weapon_type::NONE
            };
            Ok(NodeData {
                node_id,
                weapon_type,
                ..Default::default()
            })
        };
        for i in 0..t.num_smoke_nodes {
            t.node_data.push(node(
                &mut ini,
                format!("SmokeNode{i}"),
                "SmokeNodeName",
                false,
            )?);
        }
        for i in 0..t.num_weapon_nodes {
            t.node_data.push(node(
                &mut ini,
                format!("WeaponNode{i}"),
                "WeaponNodeName",
                true,
            )?);
        }
        for i in 0..t.num_jump_nodes {
            t.node_data.push(node(
                &mut ini,
                format!("JumpJetNode{i}"),
                "JumpNodeName",
                false,
            )?);
        }
        for i in 0..t.num_foot_nodes {
            t.node_data.push(node(
                &mut ini,
                format!("FootNode{i}"),
                "FootNodeName",
                false,
            )?);
        }
        Ok(t)
    }

    /// `getTotalNodes()`.
    pub fn total_nodes(&self) -> i32 {
        self.num_smoke_nodes + self.num_weapon_nodes + self.num_jump_nodes + self.num_foot_nodes
    }

    /// `getStartVel`.
    pub fn start_vel(&self, g: i32) -> f32 {
        if (0..gesture::MAX as i32).contains(&g) {
            self.gestures[g as usize].start_vel
        } else {
            0.0
        }
    }

    /// `getEndVel`.
    pub fn end_vel(&self, g: i32) -> f32 {
        if (0..gesture::MAX as i32).contains(&g) {
            self.gestures[g as usize].end_vel
        } else {
            0.0
        }
    }

    fn anim(&self, g: i32) -> Option<&AnimSet> {
        if (0..gesture::MAX as i32).contains(&g) {
            self.anims.get(g as usize).and_then(|a| a.as_ref())
        } else {
            None
        }
    }

    /// `getFrameRate` (mech3d.h:399): 0 without animation data.
    pub fn frame_rate(&self, g: i32) -> f32 {
        self.anim(g).map_or(0.0, |a| a.frame_rate())
    }

    /// `getNumFrames` (mech3d.h:407, a `long`): 0 without animation data.
    pub fn num_frames(&self, g: i32) -> i32 {
        self.anim(g).map_or(0, |a| a.num_frames())
    }

    /// `getJumpLandFrame`.
    pub fn jump_land_frame(&self) -> i32 {
        self.gestures[gesture::JUMP as usize].frame_start & 0x0000_ffff
    }

    /// `getJumpTakeoffFrame`.
    pub fn jump_takeoff_frame(&self) -> i32 {
        self.gestures[gesture::JUMP as usize].frame_start >> 16
    }

    /// The node table a [`ShapeKind`] refers to.
    pub fn shape(&self, kind: ShapeKind) -> &ShapeType {
        static EMPTY: ShapeType = ShapeType { nodes: Vec::new() };
        match kind {
            ShapeKind::Lod(i) => self.shapes.get(i).and_then(|s| s.as_ref()),
            ShapeKind::ForwardDamage => self.forward_damage.as_ref(),
            ShapeKind::BackwardDamage => self.backward_damage.as_ref(),
        }
        .unwrap_or(&EMPTY)
    }

    /// `Mech3DAppearanceType::setAnimation(shape, gesture)` (mech3d.cpp:655).
    fn set_animation(&mut self, shape: &mut ShapeInstance, kind: ShapeKind, g: i32) {
        let ty = self.shape(kind).clone();
        match self.anims.get_mut(g as usize).and_then(|a| a.as_mut()) {
            Some(set) => shape.set_animation_state(&ty, set, g as usize),
            None => shape.clear_animation(),
        }
    }
}

/// Context of an appearance update: the clock values and the simulation RNG stream.
pub struct UpdateCtx<'a> {
    /// `turn`.
    pub turn: i64,
    /// `frameLength`.
    pub frame_length: f32,
    /// The SIM stream (the destroyed-fall `RollDice(50)`, mech3d.cpp:3959).
    pub sim_rng: &'a mut SimRng,
    /// Stream for a vehicle `setGesture` random start frame (gvactor.cpp:1383, VISUAL per `visual_rng_sites.txt`);
    /// `None` uses `sim_rng`.
    pub gesture_rng: Option<&'a mut SimRng>,
    /// Terrain for the vehicle pitch/roll (`land`/`GameMap`); `None` behaves like `land == NULL` (no pitch/roll).
    pub terrain: Option<&'a dyn super::vehicle::AppearanceTerrain>,
}

/// `Mech3DAppearance` logic state.
#[derive(Clone, Debug)]
pub struct MechAppearance {
    /// Index of the [`MechAppearanceType`] in [`super::AppearanceSystem::mech_types`].
    pub type_index: usize,
    /// `mechShape` (node matrices the getters read) and the type shape it was created from.
    pub shape: ShapeInstance,
    /// See [`ShapeKind`].
    pub shape_kind: ShapeKind,
    /// `inView`.
    pub in_view: bool,
    /// `position`.
    pub position: Vec3,
    /// `rotation` (degrees, -180..180).
    pub rotation: f32,
    /// `torsoRotation`.
    pub torso_rotation: f32,
    /// `leftArmRotation` / `rightArmRotation` (stored, unused by geometry).
    pub arm_rotation: (f32, f32),
    /// `status` (`OBJECT_STATUS_*`, from `setObjStatus`).
    pub status: i32,
    /// `status` as the C++ appearance holds it, which only `BattleMech::update` hands over: every frame while the mech is not destroyed
    /// (mech.cpp:6282-6283) and, once destroyed, from the frame after `mechRemoved` (mech.cpp:6229-6233). `status` above gets DESTROYED at
    /// once for the fall-damage shape (D-52); the destroyed/disabled fall roll, a SIM draw (mech3d.cpp:3952-3967), reads this one.
    pub cpp_status: i32,
    /// `currentGestureId`.
    pub current_gesture: i32,
    /// `currentFrame`.
    pub current_frame: f32,
    /// The gesture whose animation is bound to `shape` (`setAnimation`, `mech3d.cpp:655`), `None` before any binding.
    /// Bound in `recalcBounds` and `updateGeometry` only, and not while destroyed (`mech3d.cpp:2244`, `3137-3140`),
    /// so it stays at the gesture the mech was destroyed in while `current_gesture` moves on.  With `shape.frame_num`
    /// this is the pose the render draws (see `mc2_render::appearance::AppearanceState`).
    pub bound_gesture: Option<usize>,
    /// `currentStateGoal` (-1 = none).
    pub current_state_goal: i32,
    /// `oldStateGoal`.
    pub old_state_goal: i32,
    /// `transitionState`.
    pub transition_state: i32,
    /// `hitGestureId` (gesture to return to after a hit gesture, -1 = none).
    pub hit_gesture_id: i32,
    /// `atTransitionToNextGesture`.
    pub at_transition: bool,
    /// `inReverse`.
    pub in_reverse: bool,
    /// `inJump`.
    pub in_jump: bool,
    /// `jumpSetup`.
    pub jump_setup: bool,
    /// `jumpAirborne`.
    pub jump_airborne: bool,
    /// `oncePerFrame`: a gesture goal was accepted since the last `update`.
    pub once_per_frame: bool,
    /// `forceStop` (`setBrake`).
    pub force_stop: bool,
    /// `fallen`.
    pub fallen: bool,
    /// `inCombatMode`.
    pub in_combat_mode: bool,
    /// `idleTime` (starts at `RandomNumber(idleMAX)` from the SIM stream).
    pub idle_time: f32,
    /// `mechFrameRate`.
    pub mech_frame_rate: f32,
    /// `velocity`: initialised to 0 and never assigned (so an airborne mech reports speed 0).
    pub velocity: f32,
    /// `jumpVelocity`.
    pub jump_velocity: Vec3,
    /// `jumpDestination`.
    pub jump_destination: Vec3,
    /// `baseRootNodeHeight` (-99999.9 = unset).
    pub base_root_node_height: f32,
    /// `currentLOD`.
    pub current_lod: usize,
    /// `nodeUsed[numWeaponNodes]`.
    pub node_used: Vec<i32>,
    /// `nodeRecycle[numWeaponNodes]`.
    pub node_recycle: Vec<f32>,
    /// `leftArmOff`.
    pub left_arm_off: bool,
    /// `rightArmOff`.
    pub right_arm_off: bool,
    /// `isHelicopter` (`setMoverParameters`).
    pub is_helicopter: bool,
    /// `isSmoking`, the level `startSmoking` last set (-1 none): saved, so a loaded mech smokes again (mech3d.cpp:5008).
    pub is_smoking: i32,
    /// Cached node indices, looked up on first use in whatever shape is current then (the C++ never refreshes them after
    /// an LOD switch): `rotationalNodeIndex`, `hitLeftNodeIndex`, `hitRightNodeIndex`, `rootNodeIndex`,
    /// `leftArmNodeIndex`, `rightArmNodeIndex`.
    pub rotational_node_index: i32,
    /// See `rotational_node_index`.
    pub hit_left_node_index: i32,
    /// See `rotational_node_index`.
    pub hit_right_node_index: i32,
    /// See `rotational_node_index`.
    pub root_node_index: i32,
    /// See `rotational_node_index`.
    pub left_arm_node_index: i32,
    /// See `rotational_node_index`.
    pub right_arm_node_index: i32,
    /// Footprint state, see [`FootState`].
    pub foot: FootState,
    /// Attached-effect state, see [`MechFxState`].
    pub fx: MechFxState,
}

impl MechAppearance {
    /// `Mech3DAppearance::init(type, obj)` (mech3d.cpp:996). Draws `idleTime = RandomNumber(idleMAX)` from `sim_rng`
    /// (mech3d.cpp:1023, a SIM draw: it gates the idle gesture) and rewrites the type's weapon-node
    /// arm flags. The flags test `nodeData->nodeId` (node 0, a smoke node, for every weapon node: mech3d.cpp:1207-1216),
    /// so all weapon nodes get the same flags.
    pub fn new(
        ty: &mut MechAppearanceType,
        type_index: usize,
        sim_rng: &mut SimRng,
    ) -> MechAppearance {
        let idle_time = sim_rng.random_number(IDLE_MAX as i32) as f32;
        let shape = ShapeInstance::create_from(ty.shape(ShapeKind::Lod(0)));
        let nw = ty.num_weapon_nodes.max(0) as usize;
        let first = ty
            .node_data
            .first()
            .map(|n| n.node_id.clone())
            .unwrap_or_default();
        let base = ty.shape(ShapeKind::Lod(0)).clone();
        let r = base.is_child_of(&first, "joint_ruarm");
        let l = base.is_child_of(&first, "joint_luarm");
        for i in 0..nw {
            if let Some(n) = ty.node_data.get_mut(i + ty.num_smoke_nodes.max(0) as usize) {
                n.is_r_arm_node = r;
                n.is_l_arm_node = l;
            }
        }
        MechAppearance {
            type_index,
            shape,
            shape_kind: ShapeKind::Lod(0),
            in_view: false,
            position: Vec3::default(),
            rotation: 0.0,
            torso_rotation: 0.0,
            arm_rotation: (0.0, 0.0),
            status: 0,
            cpp_status: 0,
            current_gesture: gesture::STAND,
            current_frame: 0.0,
            bound_gesture: None,
            current_state_goal: -1,
            old_state_goal: state::STANDING,
            transition_state: 0,
            hit_gesture_id: -1,
            at_transition: false,
            in_reverse: false,
            in_jump: false,
            jump_setup: false,
            jump_airborne: false,
            once_per_frame: false,
            force_stop: false,
            fallen: false,
            in_combat_mode: false,
            idle_time,
            mech_frame_rate: 30.0,
            velocity: 0.0,
            jump_velocity: Vec3::default(),
            jump_destination: Vec3::default(),
            base_root_node_height: -99999.9,
            current_lod: 0,
            node_used: vec![0; nw],
            node_recycle: vec![0.0; nw],
            left_arm_off: false,
            right_arm_off: false,
            is_helicopter: false,
            is_smoking: -1,
            rotational_node_index: -1,
            hit_left_node_index: -1,
            hit_right_node_index: -1,
            root_node_index: -1,
            left_arm_node_index: -1,
            right_arm_node_index: -1,
            foot: FootState::default(),
            fx: MechFxState::default(),
        }
    }

    // ---------------------------------------------------------------------------------------------- gesture control

    /// `setGestureGoal(stateGoal)` (mech3d.h:741). Returns -1 when the goal was accepted, 0 when ignored: one accepted
    /// goal per frame (`oncePerFrame`, cleared by `update`), none while another goal is in progress or a hit gesture
    /// plays, jumping only after `setJumpParameters`.
    pub fn set_gesture_goal(&mut self, state_goal: i32) -> i32 {
        if self.once_per_frame
            || self.old_state_goal == state_goal
            || self.current_state_goal == state_goal
            || !(state::PARKED..=state::LIMPING_RIGHT).contains(&state_goal)
            || self.current_state_goal != -1
            || is_hit_gesture(self.current_gesture)
            || (state_goal == state::JUMPING && !self.jump_setup)
        {
            return 0;
        }
        self.transition_state = 0;
        self.current_state_goal = state_goal;
        self.at_transition = true;
        self.once_per_frame = true;
        -1
    }

    /// `setBrake(brake)`.
    pub fn set_brake(&mut self, brake: bool) {
        self.force_stop = brake;
    }

    /// `checkStop()`.
    pub fn check_stop(&self) -> bool {
        self.force_stop
    }

    /// `setCombatMode(combatMode)`.
    pub fn set_combat_mode(&mut self, combat: bool) {
        self.in_combat_mode = combat;
    }

    /// `getVelocityOfGesture(gestureId)` (mech3d.h:803): `StartVel`, or interpolated towards `EndVel` by
    /// `currentFrame / numFrames` (of the *current* frame, whichever gesture is asked about).
    pub fn velocity_of_gesture(&self, ty: &MechAppearanceType, g: i32) -> f32 {
        let result = ty.start_vel(g);
        let mut other = ty.end_vel(g);
        if other == result {
            return result;
        }
        let mut vel = other - result;
        other = ty.num_frames(g) as f32;
        if other > crate::time::STUFF_SMALL {
            vel *= self.current_frame / other;
            vel += result;
        }
        vel
    }

    /// `getVelocityMagnitude()` (mech3d.h:828).
    pub fn velocity_magnitude(&self, ty: &MechAppearanceType) -> f32 {
        if self.jump_airborne {
            self.velocity
        } else {
            self.velocity_of_gesture(ty, self.current_gesture)
        }
    }

    /// `setJumpParameters(end)` (mech3d.cpp:3662).
    pub fn set_jump_parameters(&mut self, end: Vec3) -> bool {
        if !self.in_jump {
            self.jump_setup = true;
            self.jump_destination = end;
            return true;
        }
        false
    }

    /// `hitFront/hitBack/hitLeft/hitRight` (mech3d.cpp:1479-1570), `which` = 16..19 (the hit gesture). Only from stand,
    /// walk, reverse, run or limp.
    pub fn hit(&mut self, ty: &MechAppearanceType, which: i32) {
        if matches!(
            self.current_gesture,
            gesture::STAND
                | gesture::WALK
                | gesture::REVERSE
                | gesture::RUN
                | gesture::LIMP_LEFT
                | gesture::LIMP_RIGHT
        ) {
            self.hit_gesture_id = self.current_gesture;
            self.current_gesture = which;
            let first_frame = ty.gestures[which as usize].frame_start;
            self.at_transition = false;
            self.in_reverse = false;
            self.current_frame = first_frame as f32;
        }
    }

    /// `setObjectParameters(pos, rot, ...)` (mech3d.cpp:1847): the rotation freezes while falling/fallen/getting up.
    pub fn set_object_parameters(&mut self, pos: Vec3, rot: f32) {
        self.fx.moved = self.rotation != rot || self.position != pos;
        if !rotation_locked(self.current_gesture) {
            self.rotation = rot;
            if self.rotation > 180.0 {
                self.rotation -= 360.0;
            }
            if self.rotation < -180.0 {
                self.rotation += 360.0;
            }
        }
        self.position = pos;
    }

    /// `setMoverParameters(turretRot, lArmRot, rArmRot, isAirborne)` (mech3d.cpp:1876).
    pub fn set_mover_parameters(
        &mut self,
        turret_rot: f32,
        l_arm: f32,
        r_arm: f32,
        airborne: bool,
    ) {
        self.is_helicopter = airborne;
        if !rotation_locked(self.current_gesture) {
            self.torso_rotation = turret_rot;
            self.arm_rotation = (l_arm, r_arm);
            if self.torso_rotation > 180.0 {
                self.torso_rotation -= 360.0;
            }
            if self.torso_rotation < -180.0 {
                self.torso_rotation += 360.0;
            }
        }
    }

    /// Whether the gesture animation and frame are still bound to the shape. The C++ withholds the destroyed status from the
    /// appearance until the mech is removed (`mech.cpp:6282-6283`), so a dying mech keeps animating its fall; here the status
    /// is set at once (D-52), so the fall binds until the terminal damage shape is latched, which then holds its pose.
    fn binds_animation(&self) -> bool {
        self.status != object_status::DESTROYED || matches!(self.shape_kind, ShapeKind::Lod(_))
    }

    /// Latches a destroyed mech's fallen appearance once it reaches its terminal fall gesture.
    ///
    /// `Mech3DAppearance::setObjStatus` only did this when the status transition happened after
    /// a mech was already prone (`mech3d.cpp:2871-2900`). A normal killed mech transitions to a
    /// fallen gesture later, leaving its intact shape on screen. A destroyed mech visibly falling
    /// without a destroyed shape is an obvious shipped-behaviour bug, so the Rust port fixes it
    /// when the fall completes (see `rust/docs/DIFFERENCES.md`).
    fn latch_destroyed_fall_shape(&mut self, ty: &MechAppearanceType) {
        if self.status != object_status::DESTROYED {
            return;
        }
        let kind = match self.current_gesture {
            gesture::FALLEN_FORWARD if ty.forward_damage.is_some() => {
                Some(ShapeKind::ForwardDamage)
            }
            gesture::FALLEN_BACKWARD if ty.backward_damage.is_some() => {
                Some(ShapeKind::BackwardDamage)
            }
            _ => None,
        };
        if let Some(kind) = kind.filter(|&kind| self.shape_kind != kind) {
            self.shape = ShapeInstance::create_from(ty.shape(kind));
            self.shape_kind = kind;
            self.current_lod = 0;
        }
    }

    /// `setObjStatus(oStatus)` (mech3d.cpp:2871): destroyed while fallen swaps in the damage shape.
    pub fn set_obj_status(&mut self, ty: &MechAppearanceType, o_status: i32) {
        if self.status != o_status && o_status == object_status::DESTROYED {
            self.status = o_status;
            self.latch_destroyed_fall_shape(ty);
            self.current_lod = 0;
            return;
        }
        self.status = o_status;
    }

    /// `blowLeftArm` / `blowRightArm` (mech3d.cpp:4350, 4476). The shoulder pop and the arm smoke start at the arm node, six
    /// VISUAL draws give the arm its velocity and spin (listed sites mech3d.cpp:4450-4456 / 4576-4582) and the nodes below the
    /// shoulder stop being drawn.
    pub fn blow_arm(&mut self, ty: &MechAppearanceType, left: bool, visual_rng: &mut SimRng) {
        if (left && self.left_arm_off) || (!left && self.right_arm_off) {
            return;
        }
        let joint = if left { "joint_luarm" } else { "joint_ruarm" };
        let shape_ty = ty.shape(self.shape_kind);
        let idx = if left {
            &mut self.left_arm_node_index
        } else {
            &mut self.right_arm_node_index
        };
        if *idx == -1 {
            *idx = shape_ty.node_name_id(joint);
        }
        let node = *idx;
        let node_pos = self.node_id_position(ty, node);
        let place = self.arm_place(node_pos);
        let a = usize::from(left);
        self.fx.arms[a].pos = node_pos;
        self.fx.arms[a].boom = true;
        self.fx.arms[a].smoke = true;
        for (slot, id) in [
            (FxSlot::ShoulderBoom(left), fx_id::SHOULDER_POP_ID),
            (FxSlot::ArmSmoke(left), fx_id::ARM_FLYING_ID),
        ] {
            self.fx.calls.push(FxCall {
                slot,
                effect: FxEffect::Id(id),
                action: FxAction::Start {
                    place,
                    looping: false,
                },
            });
        }
        if left {
            self.left_arm_off = true;
        } else {
            self.right_arm_off = true;
        }
        let xlat_base = (12.0f32 + 50.0 / 2.0) as i32;
        let up_base = (25.0f32 as f64 + 100.0) as i32;
        let rot_base = (25.0f32 as f64 + 100.0) as i32;
        let x2 = (xlat_base as f64 * 2.0) as i32;
        let r2 = (rot_base as f64 * 2.0) as i32;
        let mut draws = [0i32; 6];
        for (d, range) in draws.iter_mut().zip([x2, x2, up_base, r2, r2, r2]) {
            *d = visual_rng.random_number(range);
        }
        let vel = Vec3 {
            x: (draws[0] - xlat_base) as f32,
            y: (draws[1] - xlat_base) as f32,
            z: (draws[2] + up_base) as f32,
        };
        let arm = &mut self.fx.arms[a];
        arm.vel = vel;
        arm.acc = Vec3 {
            x: -(vel.x * 0.1),
            y: -(vel.y * 0.1),
            z: -(vel.z * 0.5),
        };
        arm.time = 4.0;
        self.shape.stop_using(shape_ty, joint);
    }

    /// The placement the arm effects use: `Effect_Against_Motion` with the mech's yaw at `at` (mech3d.cpp:4400-4419).
    fn arm_place(&self, at: Vec3) -> FxPlacement {
        FxPlacement {
            at,
            pitch: 0.0,
            yaw: self.rotation,
            base: FxBase::AgainstMotion,
        }
    }

    /// The part of `update` (mech3d.cpp:4180-4260, 3484-3550) that moves the blown-off arms and keeps their effects on them: the
    /// shoulder pop follows the (stopped) shoulder node, the arm smoke follows the arm through its flight.
    fn update_arms(
        &mut self,
        ty: &MechAppearanceType,
        frame_length: f32,
        terrain: Option<&dyn AppearanceTerrain>,
    ) {
        for left in [false, true] {
            let a = usize::from(left);
            if self.fx.arms[a].boom {
                let node = if left {
                    self.left_arm_node_index
                } else {
                    self.right_arm_node_index
                };
                let pos = self.node_id_position(ty, node);
                self.fx.calls.push(FxCall {
                    slot: FxSlot::ShoulderBoom(left),
                    effect: FxEffect::Id(fx_id::SHOULDER_POP_ID),
                    action: FxAction::Move {
                        place: self.arm_place(pos),
                        looping: None,
                    },
                });
            }
            let off = if left {
                self.left_arm_off
            } else {
                self.right_arm_off
            };
            if !off {
                continue;
            }
            let arm = &mut self.fx.arms[a];
            let speed = length(&arm.vel);
            if speed != 0.0 {
                arm.vel = Vec3 {
                    x: arm.vel.x + arm.acc.x * frame_length,
                    y: arm.vel.y + arm.acc.y * frame_length,
                    z: arm.vel.z + arm.acc.z * frame_length,
                };
                if length(&arm.vel) < 1e-4 {
                    arm.vel = Vec3::default();
                }
                arm.pos = Vec3 {
                    x: arm.pos.x + arm.vel.x * frame_length,
                    y: arm.pos.y + arm.vel.y * frame_length,
                    z: arm.pos.z + arm.vel.z * frame_length,
                };
                let elev = terrain.map_or(0.0, |t| t.elevation(arm.pos));
                if arm.pos.z < elev {
                    arm.pos.z = elev;
                    arm.time -= frame_length;
                    if arm.time < 0.0 {
                        arm.vel = Vec3::default();
                    }
                }
            }
            if arm.smoke {
                let pos = arm.pos;
                let place = self.arm_place(pos);
                self.fx.calls.push(FxCall {
                    slot: FxSlot::ArmSmoke(left),
                    effect: FxEffect::Id(fx_id::ARM_FLYING_ID),
                    action: FxAction::Move {
                        place,
                        looping: None,
                    },
                });
            }
        }
    }

    /// The `Start` / `Execute` of a foot poof (mech3d.cpp:3193-3207): round robin over `MAX_DUST_POOFS` effects, at the foot node.
    fn foot_poof(&mut self, right: bool, pos: Vec3) {
        let cursor = &mut self.fx.next_poof[usize::from(!right)];
        let i = *cursor;
        *cursor = if i + 1 >= MAX_DUST_POOFS { 0 } else { i + 1 };
        self.fx.calls.push(FxCall {
            slot: if right {
                FxSlot::RightFootPoof(i)
            } else {
                FxSlot::LeftFootPoof(i)
            },
            effect: FxEffect::Id(fx_id::FX_POOF_ID),
            action: FxAction::Start {
                place: FxPlacement::plain(pos),
                looping: false,
            },
        });
    }

    /// The jump jet exhaust (mech3d.cpp:3380-3410, 3449-3479): started once when the jump begins, follows the jump node while the
    /// mech is in its jump, wound down when it lands.
    fn update_jump_jet(&mut self, ty: &MechAppearanceType) {
        let jumping = self.current_gesture == gesture::JUMP && self.in_jump;
        let place = |s: &mut MechAppearance| {
            let node = ty.num_smoke_nodes + ty.num_weapon_nodes;
            FxPlacement {
                at: s.node_position(ty, node),
                pitch: JUMP_PITCH,
                yaw: s.rotation,
                base: FxBase::AgainstMotion,
            }
        };
        if !self.fx.jump_fx_setup && jumping {
            let place = place(self);
            self.fx.calls.push(FxCall {
                slot: FxSlot::JumpJet,
                effect: FxEffect::Id(fx_id::FX_JUMP_ID),
                action: FxAction::Start {
                    place,
                    looping: true,
                },
            });
            self.fx.jump_fx_setup = true;
        } else if jumping {
            let place = place(self);
            self.fx.calls.push(FxCall {
                slot: FxSlot::JumpJet,
                effect: FxEffect::Id(fx_id::FX_JUMP_ID),
                action: FxAction::Move {
                    place,
                    looping: None,
                },
            });
        }
        if !self.in_jump && self.fx.jump_fx_setup {
            self.fx.calls.push(FxCall {
                slot: FxSlot::JumpJet,
                effect: FxEffect::Id(fx_id::FX_JUMP_ID),
                action: FxAction::Stop,
            });
            self.fx.jump_fx_setup = false;
        }
    }

    /// `startSmoking(smokeLvl)` (mech3d.cpp:4602): -1 puts the smoke out, 0 is the damage smoke, above 0 the critical smoke.
    /// Gated by `useNonWeaponEffects` like the C++.
    pub fn start_smoking(&mut self, ty: &MechAppearanceType, level: i32) {
        if !self.fx.non_weapon {
            return;
        }
        if level != -1 {
            if self.fx.smoking == -1 || self.fx.smoking != level {
                let at = self.node_position(ty, 0);
                let (slot, id) = if level > 0 {
                    (FxSlot::CriticalSmoke, fx_id::CRITICAL_SMOKE_ID)
                } else {
                    (FxSlot::Smoke, fx_id::MECH_SMOKE_ID)
                };
                self.fx.calls.push(FxCall {
                    slot,
                    effect: FxEffect::Id(id),
                    action: FxAction::Start {
                        place: FxPlacement::plain(at),
                        looping: true,
                    },
                });
            }
        } else {
            for (slot, id) in [
                (FxSlot::Smoke, fx_id::MECH_SMOKE_ID),
                (FxSlot::CriticalSmoke, fx_id::CRITICAL_SMOKE_ID),
            ] {
                self.fx.calls.push(FxCall {
                    slot,
                    effect: FxEffect::Id(id),
                    action: FxAction::Kill,
                });
            }
        }
        self.fx.smoking = level;
        self.is_smoking = level;
    }

    /// `startWaterWake()` (mech3d.cpp:4691): no wake for a helicopter or a jumping mech; started once, at the water level.
    pub fn start_water_wake(&mut self, water_elevation: f32) {
        if self.is_helicopter || self.current_gesture == gesture::JUMP || !self.fx.non_weapon {
            return;
        }
        if !self.fx.waking {
            let place = self.wake_place(water_elevation);
            self.fx.calls.push(FxCall {
                slot: FxSlot::WaterWake,
                effect: FxEffect::Id(fx_id::MECH_WATER_WAKE),
                action: FxAction::Start {
                    place,
                    looping: true,
                },
            });
            self.fx.waking = true;
        }
    }

    /// `stopWaterWake()` (mech3d.cpp:4755).
    pub fn stop_water_wake(&mut self) {
        if self.fx.waking {
            self.fx.calls.push(FxCall {
                slot: FxSlot::WaterWake,
                effect: FxEffect::Id(fx_id::MECH_WATER_WAKE),
                action: FxAction::Kill,
            });
        }
        self.fx.waking = false;
    }

    /// The wake lies flat at the water level: `Euler(90, rotation)` behind `Effect_Against_Motion` (`Into_Motion` in reverse).
    fn wake_place(&self, water_elevation: f32) -> FxPlacement {
        FxPlacement {
            at: Vec3 {
                x: self.position.x,
                y: self.position.y,
                z: water_elevation,
            },
            pitch: 90.0,
            yaw: self.rotation,
            base: if self.in_reverse {
                FxBase::IntoMotion
            } else {
                FxBase::AgainstMotion
            },
        }
    }

    /// The per-frame follow-up of the effects `startSmoking`, `startWaterWake` and `playEjection` started (mech3d.cpp:3552-3650).
    fn update_attached(&mut self, ty: &MechAppearanceType, water_elevation: f32, moved: bool) {
        if self.fx.smoking != -1 {
            let at = self.node_position(ty, 0);
            let (slot, id) = if self.fx.smoking > 0 {
                (FxSlot::CriticalSmoke, fx_id::CRITICAL_SMOKE_ID)
            } else {
                (FxSlot::Smoke, fx_id::MECH_SMOKE_ID)
            };
            self.fx.calls.push(FxCall {
                slot,
                effect: FxEffect::Id(id),
                action: FxAction::Move {
                    place: FxPlacement::plain(at),
                    looping: None,
                },
            });
        }
        if self.fx.waking {
            let place = self.wake_place(water_elevation);
            self.fx.calls.push(FxCall {
                slot: FxSlot::WaterWake,
                effect: FxEffect::Id(fx_id::MECH_WATER_WAKE),
                action: FxAction::Move {
                    place,
                    looping: Some(moved),
                },
            });
        }
        // The draw conditions: no foot dust while wading, no wake while jumping (keep calcing it, though).
        let hide_feet = self.fx.waking;
        if hide_feet != self.fx.feet_hidden {
            self.fx.feet_hidden = hide_feet;
            for i in 0..3u8 {
                for slot in [FxSlot::RightFootPoof(i), FxSlot::LeftFootPoof(i)] {
                    self.fx.calls.push(FxCall {
                        slot,
                        effect: FxEffect::Id(fx_id::FX_POOF_ID),
                        action: FxAction::Draw(!hide_feet),
                    });
                }
            }
        }
        let hide_wake = self.fx.waking && self.current_gesture == gesture::JUMP;
        if hide_wake != self.fx.wake_hidden {
            self.fx.wake_hidden = hide_wake;
            self.fx.calls.push(FxCall {
                slot: FxSlot::WaterWake,
                effect: FxEffect::Id(fx_id::MECH_WATER_WAKE),
                action: FxAction::Draw(!hide_wake),
            });
        }
    }

    /// `playEjection()` (mech3d.cpp:4764): the dust cloud of a helicopter shutting down or powering up and of a mech that fell.
    pub fn play_ejection(&mut self) {
        if !self.fx.non_weapon {
            return;
        }
        self.fx.calls.push(FxCall {
            slot: FxSlot::FallDust,
            effect: FxEffect::Id(fx_id::HELICOPTER_DUST_CLOUD),
            action: FxAction::Start {
                place: FxPlacement::plain(self.position),
                looping: false,
            },
        });
        self.fx.dusting = true;
    }

    /// `setGesture(gestureId)` (mech3d.h:720): helicopters forced to park at start-up. Runs a full `update` in view.
    pub fn set_gesture(&mut self, ty: &mut MechAppearanceType, g: u32, ctx: &mut UpdateCtx) {
        self.current_gesture = gesture::STAND;
        self.in_view = true;
        self.update(ty, true, ctx);
        self.current_gesture = g as i32;
        self.old_state_goal = 0;
        self.current_state_goal = -1;
        self.transition_state = 0;
        self.current_frame = (ty.num_frames(self.current_gesture) - 1) as f32;
    }

    /// `recalcBounds()` (mech3d.cpp:2047) with the camera replaced by its results: `in_view` is the final `inView`, `lod`
    /// the LOD the camera distance selects. When in view and not destroyed the shape is recreated for a new LOD (fresh
    /// matrices; the cached node indices are kept) and the current gesture's animation is bound.
    pub fn recalc_bounds(
        &mut self,
        ty: &mut MechAppearanceType,
        in_view: bool,
        lod: usize,
    ) -> bool {
        self.in_view = in_view;
        if in_view && self.binds_animation() {
            if lod != self.current_lod
                && lod < MAX_LODS
                && ty.shapes[lod].as_ref().is_some_and(|s| !s.is_empty())
            {
                self.current_lod = lod;
                let kind = ShapeKind::Lod(lod);
                self.shape = ShapeInstance::create_from(ty.shape(kind));
                self.shape_kind = kind;
                let shape_ty = ty.shape(kind).clone();
                if self.right_arm_off {
                    self.shape.stop_using(&shape_ty, "joint_ruarm");
                }
                if self.left_arm_off {
                    self.shape.stop_using(&shape_ty, "joint_luarm");
                }
            }
            ty.set_animation(&mut self.shape, self.shape_kind, self.current_gesture);
            self.bound_gesture = usize::try_from(self.current_gesture).ok();
        }
        self.in_view
    }

    // ---------------------------------------------------------------------------------------------- update

    /// `Mech3DAppearance::update(animate)` (mech3d.cpp:3790), simulation part. Returns 1 (`TRUE`).
    pub fn update(
        &mut self,
        ty: &mut MechAppearanceType,
        animate: bool,
        ctx: &mut UpdateCtx,
    ) -> i32 {
        let frame_length = ctx.frame_length;
        // Recycle the weapon nodes.
        for r in self.node_recycle.iter_mut() {
            if *r > 0.0 {
                *r -= frame_length;
                if *r < 0.0 {
                    *r = 0.0;
                }
            }
        }

        self.once_per_frame = false;
        let mut set_first_frame = false;

        if is_hit_gesture(self.current_gesture) && self.at_transition {
            self.current_gesture = self.hit_gesture_id;
            self.hit_gesture_id = -1;
            set_first_frame = true;
            let mut first_frame = ty
                .gestures
                .get(self.current_gesture as usize)
                .map_or(0, |g| g.frame_start);
            if first_frame < 0 {
                first_frame = ty.num_frames(self.current_gesture) - 1;
            }
            self.in_reverse = (ty.frame_rate(self.current_gesture) as f64) < 0.0;
            self.at_transition = false;
            self.current_frame = first_frame as f32;
        }

        // Idle after idleMAX seconds standing out of combat.
        if !self.in_combat_mode && self.current_gesture == gesture::STAND {
            self.idle_time += frame_length;
            if self.idle_time > IDLE_MAX {
                self.current_gesture = gesture::IDLE;
            }
        } else {
            self.idle_time = 0.0;
        }

        if self.current_gesture == gesture::IDLE && self.at_transition {
            self.current_gesture = gesture::STAND;
            self.current_frame = 0.0;
        }

        // Stop NOW (brake).
        if self.check_stop()
            && !self.jump_setup
            && self.velocity_magnitude(ty) != 0.0
            && self.current_gesture != gesture::JUMP
        {
            self.current_gesture = gesture::STAND;
            self.old_state_goal = state::STANDING;
            self.current_state_goal = -1;
            self.transition_state = 0;
            self.current_frame = 0.0;
        }

        // Walk the transition chain towards currentStateGoal.
        if self.current_state_goal != -1 {
            let array_idx = (self.old_state_goal as i64 * GESTURE_OFFSET_SIZE as i64)
                + (self.current_state_goal as i64 * MAX_TRANSITION_GESTURES as i64)
                + self.transition_state as i64;
            // The index stays inside the table for every reachable (oldStateGoal, goal, step) (10 x 10 x 10).
            let new_gesture_id = TRANSITION_ARRAY
                .get(array_idx as usize)
                .map_or(-1, |&g| g as i32);
            if self.at_transition {
                self.transition_state += 1;
                let mut first_frame = self.current_frame as i32;
                if new_gesture_id == -1 {
                    self.old_state_goal = self.current_state_goal;
                    if self.old_state_goal == state::JUMPING {
                        self.old_state_goal = state::STANDING;
                    }
                    self.current_state_goal = -1;
                    self.transition_state = 0;
                    self.at_transition = false;
                    if (ty.frame_rate(self.current_gesture) as f64) < 0.0 {
                        self.in_reverse = true;
                    }
                } else {
                    self.current_gesture = new_gesture_id;
                    set_first_frame = true;
                    first_frame = ty.gestures[self.current_gesture as usize].frame_start;
                    if self.current_gesture == gesture::JUMP {
                        first_frame = 0;
                    }
                    if first_frame < 0 {
                        first_frame = ty.num_frames(self.current_gesture) - 1;
                    }
                    self.in_reverse = (ty.frame_rate(self.current_gesture) as f64) < 0.0;
                    self.at_transition = false;
                }
                self.current_frame = first_frame as f32;
            }
        }

        // The C++ appearance's own status (`cpp_status`), not the D-52 one: a destroyed mech rolls only from the frame after it was removed.
        if self.cpp_status == object_status::DESTROYED || self.cpp_status == object_status::DISABLED
        {
            // One SIM draw every update while the goal is below 7 (also -1, i.e. after the fall completed).
            if self.current_state_goal < state::FALLEN_FORWARD {
                if ctx.sim_rng.roll_dice(50) {
                    self.set_gesture_goal(state::FALLEN_FORWARD);
                } else {
                    self.set_gesture_goal(state::FALLEN_BACKWARD);
                }
            }
        }

        self.mech_frame_rate = ty.frame_rate(self.current_gesture);
        if (self.mech_frame_rate as f64) < 0.0 {
            self.mech_frame_rate = -self.mech_frame_rate;
        }

        // Jump set-up: straight-line velocity to the destination over the airborne frames at 30 fps.
        if self.current_gesture == gesture::JUMP && !self.in_jump {
            self.in_jump = true;
            self.jump_setup = false;
            let takeoff = ty.jump_takeoff_frame() as f32;
            let landing = ty.jump_land_frame() as f32;
            let mut v = Vec3 {
                x: self.jump_destination.x - self.position.x,
                y: self.jump_destination.y - self.position.y,
                z: self.jump_destination.z - self.position.z,
            };
            let vel_mag = length(&v) / ((landing - takeoff) / 30.0);
            // A zero-length jump would divide by zero in the C++ (NaN velocity); fix: jump in place.
            let raw_len = length(&v);
            let len = if raw_len > 1e-6 { 1.0 / raw_len } else { 0.0 };
            v = Vec3 {
                x: v.x * len,
                y: v.y * len,
                z: v.z * len,
            };
            self.jump_velocity = Vec3 {
                x: v.x * vel_mag,
                y: v.y * vel_mag,
                z: v.z * vel_mag,
            };
        }

        if self.current_gesture == gesture::JUMP && self.in_jump {
            if self.current_frame >= ty.jump_takeoff_frame() as f32 {
                self.jump_airborne = true;
            }
            if self.current_frame >= ty.jump_land_frame() as f32 && self.jump_airborne {
                self.jump_airborne = false;
                self.jump_velocity = Vec3::default();
                // `jumpJetEffect->Stop()`: done drawing it (mech3d.cpp:4017)
                self.fx.calls.push(FxCall {
                    slot: FxSlot::JumpJet,
                    effect: FxEffect::Id(fx_id::FX_JUMP_ID),
                    action: FxAction::Stop,
                });
            }
        }

        self.fallen = false;
        if self.current_gesture == gesture::FALLEN_FORWARD
            || self.current_gesture == gesture::FALLEN_BACKWARD
        {
            self.fallen = true;
            if self.current_state_goal != -1 {
                self.at_transition = true;
            }
            // The helicopter dust cloud for a fall; once, until the mech gets up (mech3d.cpp:4027-4039).
            if !self.fx.fall_dust {
                self.play_ejection();
                self.fx.fall_dust = true;
            }
        } else if self.current_gesture == gesture::PARK {
            self.fx.fall_dust = false;
            if self.current_state_goal != -1 {
                self.at_transition = true;
            }
        } else if animate {
            self.fx.fall_dust = false;
            let frame_inc = self.mech_frame_rate * frame_length;
            self.at_transition = false;
            if frame_inc != 0.0 {
                if !set_first_frame {
                    if self.in_reverse {
                        self.current_frame -= frame_inc;
                    } else {
                        self.current_frame += frame_inc;
                    }
                }
                let n = ty.num_frames(self.current_gesture) as f32;
                if self.current_frame >= n {
                    if self.current_state_goal == -1
                        && self.hit_gesture_id == -1
                        && self.current_gesture != gesture::IDLE
                    {
                        self.current_frame -= n;
                    } else {
                        self.at_transition = true;
                        self.current_frame = n - 1.0;
                        if self.current_gesture == gesture::JUMP {
                            self.in_jump = false;
                            self.jump_setup = false;
                            self.old_state_goal = state::STANDING;
                        }
                    }
                }
                if self.current_frame < 0.0 {
                    if self.current_state_goal == -1 && self.hit_gesture_id == -1 {
                        self.current_frame += ty.num_frames(self.current_gesture) as f32;
                    } else {
                        self.at_transition = true;
                        self.current_frame = 0.0;
                    }
                }
            } else if self.current_state_goal != -1 {
                // A single-frame gesture: always ready to transition.
                self.at_transition = true;
            }
        }

        if self.current_frame < 0.0 {
            self.current_frame = 0.0;
        }
        if self.current_frame > 0.0
            && self.current_frame >= ty.num_frames(self.current_gesture) as f32
        {
            self.current_frame = 0.0;
        }

        self.latch_destroyed_fall_shape(ty);

        if ctx.turn < 3 || self.in_view || self.current_gesture == gesture::JUMP {
            self.update_geometry(ty, ctx.turn);
        }

        if self.base_root_node_height == -99999.9 {
            if self.root_node_index == -1 {
                self.root_node_index = ty.shape(self.shape_kind).node_name_id("joint_root");
            }
            let old = self.in_view;
            self.in_view = true;
            self.base_root_node_height =
                self.node_id_position(ty, self.root_node_index).z - self.position.z;
            self.in_view = old;
        }
        let water = ctx.terrain.map_or(0.0, |t| t.water_elevation());
        self.update_arms(ty, frame_length, ctx.terrain);
        self.update_attached(ty, water, self.fx.moved);
        1
    }

    /// `updateGeometry()` (mech3d.cpp:2971), the node-transform part: bind the gesture animation and frame (not when
    /// destroyed), apply the torso twist to `joint_torso`, transform the hierarchy at the mech's position and yaw.
    pub fn update_geometry(&mut self, ty: &mut MechAppearanceType, turn: i64) {
        let q_rotation = quat_from_euler(0.0, self.rotation * DEGREES_TO_RADS, 0.0);
        if self.binds_animation() {
            ty.set_animation(&mut self.shape, self.shape_kind, self.current_gesture);
            self.bound_gesture = usize::try_from(self.current_gesture).ok();
            self.shape.frame_num = self.current_frame;
        }
        let torso_rot = quat_from_euler(0.0, self.torso_rotation * DEGREES_TO_RADS, 0.0);
        let shape_ty = ty.shape(self.shape_kind);
        if self.rotational_node_index == -1 {
            self.rotational_node_index =
                self.shape
                    .set_node_rotation_by_name(shape_ty, "joint_torso", &torso_rot);
        }
        self.shape
            .set_node_rotation(self.rotational_node_index, &torso_rot);
        let xlat = [-self.position.x, self.position.z, self.position.y];
        self.shape
            .transform(shape_ty, &ty.anims, xlat, &q_rotation, turn);
        self.update_footprints(ty);
        self.update_jump_jet(ty);
    }

    /// The foot poofs and footprints of `updateGeometry` (mech3d.cpp:3175), after the hierarchy has been transformed: when the
    /// animation frame is within `FOOTPRINT_SLOP` of a foot-down frame of the gesture and that foot has not printed yet, it
    /// prints at the foot node. A foot may print again once the frame has left its window. The C++ only does this for a mech
    /// that is in view (`visible`, a fully detected one) and not wading (`isWaking`); the poof is presentation and not kept.
    fn update_footprints(&mut self, ty: &MechAppearanceType) {
        if !self.in_view {
            return;
        }
        let total = ty.total_nodes();
        let Some(g) = usize::try_from(self.current_gesture)
            .ok()
            .and_then(|i| ty.gestures.get(i))
        else {
            return;
        };
        let (r_pos, l_pos) = (
            self.node_position(ty, total - 2),
            self.node_position(ty, total - 1),
        );
        let f = self.current_frame;
        let slop = crate::crater::FOOTPRINT_SLOP as f32;
        // (done index, foot-down frame, foot position, crater type): right 0, right 1, left 0, left 1
        let feet = [
            (0, g.foot_down[0], r_pos, ty.right_footprint_type),
            (1, g.foot_down[1], r_pos, ty.right_footprint_type),
            (2, g.foot_down[2], l_pos, ty.left_footprint_type),
            (3, g.foot_down[3], l_pos, ty.left_footprint_type),
        ];
        for (i, down, pos, kind) in feet {
            let (lo, hi) = (down as f32 - slop, down as f32 + slop);
            if !self.foot.done[i] && f >= lo && f <= hi {
                self.foot.done[i] = true;
                // the dust poof, not for the idle gesture; then the print, not while wading (mech3d.cpp:3193-3216)
                if self.fx.non_weapon && self.current_gesture != gesture::IDLE {
                    self.foot_poof(i < 2, pos);
                }
                if !self.fx.waking {
                    self.foot.pending.push((kind, pos, self.rotation));
                }
            } else if self.foot.done[i] && (f < lo || f > hi) {
                self.foot.done[i] = false;
            }
        }
    }

    // ---------------------------------------------------------------------------------------------- nodes

    /// The torso twist every node getter re-applies before reading (mech3d.cpp:726-731). It changes the stored base
    /// rotation only, never the matrices, and it is the value `updateGeometry` sets anyway; kept for the index caching.
    fn touch_torso(&mut self, ty: &MechAppearanceType) {
        let torso_rot = quat_from_euler(0.0, self.torso_rotation * DEGREES_TO_RADS, 0.0);
        let shape_ty = ty.shape(self.shape_kind);
        if self.rotational_node_index == -1 {
            self.rotational_node_index =
                self.shape
                    .set_node_rotation_by_name(shape_ty, "joint_torso", &torso_rot);
        }
        self.shape
            .set_node_rotation(self.rotational_node_index, &torso_rot);
    }

    fn to_vec(p: [f32; 3]) -> Vec3 {
        Vec3 {
            x: p[0],
            y: p[1],
            z: p[2],
        }
    }

    fn zero_to_position(&self, p: [f32; 3]) -> Vec3 {
        if p[0] == 0.0 && p[1] == 0.0 && p[2] == 0.0 {
            self.position
        } else {
            Self::to_vec(p)
        }
    }

    /// `getWeaponNodePosition(nodeId)` (mech3d.cpp:706): the object position for non-weapon nodes, when not in view, or
    /// when the node is missing (reads as the origin).
    pub fn weapon_node_position(&mut self, ty: &MechAppearanceType, node_id: i32) -> Vec3 {
        if node_id < ty.num_smoke_nodes
            || node_id >= ty.num_smoke_nodes + ty.num_weapon_nodes
            || !self.in_view
        {
            return self.position;
        }
        self.touch_torso(ty);
        let name = &ty.node_data[node_id as usize].node_id;
        let p = self
            .shape
            .node_position_by_name(ty.shape(self.shape_kind), name);
        self.zero_to_position(p)
    }

    /// `getNodeNamePosition(nodeName)` (mech3d.cpp:746).
    pub fn node_name_position(&mut self, ty: &MechAppearanceType, name: &str) -> Vec3 {
        if !self.in_view {
            return self.position;
        }
        self.touch_torso(ty);
        let p = self
            .shape
            .node_position_by_name(ty.shape(self.shape_kind), name);
        self.zero_to_position(p)
    }

    /// `getNodeIdPosition(nodeId)` (mech3d.cpp:782): by shape index.
    pub fn node_id_position(&mut self, ty: &MechAppearanceType, node_id: i32) -> Vec3 {
        if !self.in_view {
            return self.position;
        }
        self.touch_torso(ty);
        self.zero_to_position(self.shape.node_position(node_id))
    }

    /// `getNodePosition(nodeId)` (mech3d.cpp:818): by node table index; a missing node reads as the origin (no fallback).
    pub fn node_position(&mut self, ty: &MechAppearanceType, node_id: i32) -> Vec3 {
        if node_id < 0 || node_id >= ty.total_nodes() || !self.in_view {
            return self.position;
        }
        self.touch_torso(ty);
        let name = &ty.node_data[node_id as usize].node_id;
        Self::to_vec(
            self.shape
                .node_position_by_name(ty.shape(self.shape_kind), name),
        )
    }

    /// `getHitNodeLeft()` / `getHitNodeRight()` (mech3d.cpp:5065, 5102).
    pub fn hit_node(&mut self, ty: &MechAppearanceType, left: bool) -> Vec3 {
        let shape_ty = ty.shape(self.shape_kind);
        let idx = if left {
            &mut self.hit_left_node_index
        } else {
            &mut self.hit_right_node_index
        };
        if *idx == -1 {
            *idx = shape_ty.node_name_id(if left { "hit_left" } else { "hit_right" });
        }
        let idx = *idx;
        if !self.in_view {
            return self.position;
        }
        self.touch_torso(ty);
        self.zero_to_position(self.shape.node_position(idx))
    }

    /// `getLowestWeaponNode()` (mech3d.cpp:851).
    pub fn lowest_weapon_node(&mut self, ty: &MechAppearanceType) -> i32 {
        let mut best_node = -1;
        let mut lowest_pos_z = 9_999_999_999_999.0f32;
        for i in 0..ty.num_weapon_nodes {
            let p = self.node_position(ty, i + ty.num_smoke_nodes);
            if p.z < lowest_pos_z {
                lowest_pos_z = p.z;
                best_node = i + ty.num_smoke_nodes;
            }
        }
        if lowest_pos_z == 0.0 || best_node < 0 || best_node >= ty.total_nodes() {
            return -1;
        }
        best_node
    }

    /// `getWeaponNode(weaponType)` (mech3d.cpp:876): the least used compatible node (load time only in the C++), which
    /// is then marked used. -1 when none.
    pub fn weapon_node(&mut self, ty: &MechAppearanceType, wtype: i32) -> i32 {
        use self::weapon_type as wt;
        let mut least_used = 999_999_999;
        let mut best_node = -1;
        let ns = ty.num_smoke_nodes;
        for i in 0..ty.num_weapon_nodes {
            let n = &ty.node_data[(ns + i) as usize];
            let compatible = match wtype {
                wt::MISSILE => [wtype, wt::ANY, wt::NONENERGY].contains(&n.weapon_type),
                wt::BALLISTIC => {
                    [wtype, wt::ANY, wt::DIRECT, wt::NONENERGY].contains(&n.weapon_type)
                }
                wt::ENERGY => [wtype, wt::DIRECT, wt::ANY].contains(&n.weapon_type),
                wt::ANY => true,
                // STOP(("Sent down a bad weapon type")) in the C++.
                _ => false,
            };
            if !compatible
                || (self.right_arm_off && n.is_r_arm_node)
                || (self.left_arm_off && n.is_l_arm_node)
            {
                continue;
            }
            if self.node_used[i as usize] < least_used {
                least_used = self.node_used[i as usize];
                best_node = i + ns;
            }
        }
        if best_node < 0 || best_node >= ty.total_nodes() {
            return -1;
        }
        self.set_weapon_node_used(ty, best_node);
        best_node
    }

    /// `setWeaponNodeUsed(node)` (mech3d.cpp:686).
    pub fn set_weapon_node_used(&mut self, ty: &MechAppearanceType, node: i32) {
        let n = node - ty.num_smoke_nodes;
        if n >= 0 && n < ty.num_weapon_nodes {
            self.node_used[n as usize] += 1;
            self.node_recycle[n as usize] = BASE_NODE_RECYCLE_TIME;
        }
    }

    /// `setWeaponNodeRecycle(node, time)` (mech3d.cpp:698).
    pub fn set_weapon_node_recycle(&mut self, ty: &MechAppearanceType, node: i32, time: f32) {
        let n = node - ty.num_smoke_nodes;
        if n >= 0 && n < ty.num_weapon_nodes {
            self.node_recycle[n as usize] = time;
        }
    }

    /// `getWeaponNodeRecycle(node)` (mech3d.cpp:985): 9999 for non-weapon nodes.
    pub fn weapon_node_recycle(&self, ty: &MechAppearanceType, node: i32) -> f32 {
        let n = node - ty.num_smoke_nodes;
        if n >= 0 && n < ty.num_weapon_nodes {
            self.node_recycle[n as usize]
        } else {
            9999.0
        }
    }

    /// `resetWeaponNodes()` (mech3d.cpp:675).
    pub fn reset_weapon_nodes(&mut self) {
        for (u, r) in self.node_used.iter_mut().zip(self.node_recycle.iter_mut()) {
            *u = 0;
            *r = BASE_NODE_RECYCLE_TIME;
        }
    }
}

/// Hit gestures 16..19.
fn is_hit_gesture(g: i32) -> bool {
    matches!(
        g,
        gesture::HIT_FRONT | gesture::HIT_BACK | gesture::HIT_LEFT | gesture::HIT_RIGHT
    )
}

/// Gestures 14, 15, 21-24 freeze the appearance's rotation and torso twist.
fn rotation_locked(g: i32) -> bool {
    matches!(
        g,
        gesture::FALLEN_FORWARD
            | gesture::FALLEN_BACKWARD
            | gesture::ROLLOVER
            | gesture::GET_UP
            | gesture::FALL_BACKWARD
            | gesture::FALL_FORWARD
    )
}

/// `Vector3D::GetLength()`: `Sqrt(x*x + y*y + z*z)`.
fn length(v: &Vec3) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

// ------------------------------------------------------------------------------------------------ save/load

use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};

// Static (not saved, not even as a check): `MechAppearanceType` (shapes, damage shapes, lod distances, gestures,
// animation entries, node ids, weapon types). Its two run-time caches are saved by the owner, `AppearanceSystem`
// (mod.rs): the `AnimSet::shape_ids` node-index cache and the arm flags of `node_data`.

impl Snapshot for ShapeKind {
    fn save(&self, w: &mut Writer) {
        match *self {
            ShapeKind::Lod(i) => {
                w.u8(0);
                i.save(w);
            }
            ShapeKind::ForwardDamage => w.u8(1),
            ShapeKind::BackwardDamage => w.u8(2),
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        match r.u8()? {
            0 => Ok(ShapeKind::Lod(usize::load(r)?)),
            1 => Ok(ShapeKind::ForwardDamage),
            2 => Ok(ShapeKind::BackwardDamage),
            tag => Err(SaveError(format!("save: bad mech shape tag {tag}"))),
        }
    }
}

// Every field is dynamic: no field is static or derived. The node matrices (`shape`), the cached node indices and the
// geometry the getters read are saved as they are, not recomputed.
crate::snapshot_struct!(MechAppearance {
    type_index,
    shape,
    shape_kind,
    in_view,
    position,
    rotation,
    torso_rotation,
    arm_rotation,
    status,
    cpp_status,
    current_gesture,
    current_frame,
    bound_gesture,
    current_state_goal,
    old_state_goal,
    transition_state,
    hit_gesture_id,
    at_transition,
    in_reverse,
    in_jump,
    jump_setup,
    jump_airborne,
    once_per_frame,
    force_stop,
    fallen,
    in_combat_mode,
    idle_time,
    mech_frame_rate,
    velocity,
    jump_velocity,
    jump_destination,
    base_root_node_height,
    current_lod,
    node_used,
    node_recycle,
    left_arm_off,
    right_arm_off,
    is_helicopter,
    is_smoking,
    rotational_node_index,
    hit_left_node_index,
    hit_right_node_index,
    root_node_index,
    left_arm_node_index,
    right_arm_node_index,
    foot => FootState::default(),
    fx => MechFxState::default(),
});
