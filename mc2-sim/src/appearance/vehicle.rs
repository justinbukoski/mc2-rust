// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `GVAppearanceType` / `GVAppearance` (`mclib/gvactor.{h,cpp}`): ground vehicle animation state, LOD/damage shapes, turret
//! twist, terrain pitch/roll, weapon node bookkeeping and node geometry. Presentation (textures, paint, sensor shapes,
//! shadows, dust/wake/activity/destruct effects, flashing) is dropped.

use mc2_assets::stuff::{quat_from_euler, quat_multiply, sin_cos};
use mc2_assets::tgl::decode_animation;
use mc2_assets::tgl_instance::{AnimSet, ShapeInstance, ShapeType};
use mc2_formats::vfs::Vfs;

use super::fx::FxCall;
use super::mech::{object_status, read_shape_file, weapon_type, NodeData, UpdateCtx, TGL_PATH};
use super::tables::{DEGREES_TO_RADS, MAX_LODS};
use super::AppearanceError;
use crate::events::{fx_id, FxAction, FxBase, FxEffect, FxPlacement, FxSlot};
use crate::math::Vec3;
use crate::rng::SimRng;

/// `MAX_GV_ANIMATIONS` (gvactor.h:38).
pub const MAX_GV_ANIMATIONS: usize = 10;
/// `MAX_SMOKE_NODES`, `MAX_WEAPON_NODES`, `MAX_FOOT_NODES` (gvactor.cpp:91-93).
pub const GV_SMOKE_NODES: i32 = 2;
/// See [`GV_SMOKE_NODES`].
pub const GV_WEAPON_NODES: i32 = 4;
/// See [`GV_SMOKE_NODES`].
pub const GV_FOOT_NODES: i32 = 2;
/// `RADS_TO_DEGREES` (mclib/dstd.h:34).
#[allow(clippy::excessive_precision)]
pub const RADS_TO_DEGREES: f32 = 57.295_779_513_2;

/// Terrain queries the appearance updates make (`land`, `GameMap`).
pub trait AppearanceTerrain {
    /// `land->getTerrainAngle(position, &normal)`'s normal when the cell under `position` is neither deep nor shallow water
    /// (`GameMap->getDeepWater/getShallowWater`), `None` on water (no pitch/roll). `GVAppearance::updateGeometry`,
    /// gvactor.cpp:2288-2300.
    fn terrain_angle_normal(&self, position: Vec3) -> Option<Vec3>;
    /// `Terrain::waterElevation` (spinning marker buildings stay above it, bdactor.cpp:1962).
    fn water_elevation(&self) -> f32 {
        0.0
    }
    /// `land->getTerrainElevation(position)`.
    fn elevation(&self, _position: Vec3) -> f32 {
        0.0
    }
}

/// Which type shape `gvShape` was created from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GvShapeKind {
    /// `gvShape[lod]`.
    Lod(usize),
    /// `gvDmgShape` (destroyed or disabled).
    Damage,
}

/// `GVAppearanceType`.
#[derive(Clone, Debug, Default)]
pub struct VehicleAppearanceType {
    /// `name`.
    pub name: String,
    /// `gvShape[MAX_LODS]`.
    pub shapes: [Option<ShapeType>; MAX_LODS],
    /// `lodDistance[MAX_LODS]`.
    pub lod_distance: [f32; MAX_LODS],
    /// `gvDmgShape`.
    pub damage: Option<ShapeType>,
    /// `destructEffect` (empty: none).
    pub destruct_effect: String,
    /// `gvAnimData[MAX_GV_ANIMATIONS]`.
    pub anims: Vec<Option<AnimSet>>,
    /// `gvAnimLoop`.
    pub anim_loop: [bool; MAX_GV_ANIMATIONS],
    /// `gvReverse`.
    pub anim_reverse: [bool; MAX_GV_ANIMATIONS],
    /// `gvRandom`.
    pub anim_random: [bool; MAX_GV_ANIMATIONS],
    /// `gvStartF`.
    pub anim_start: [i32; MAX_GV_ANIMATIONS],
    /// `rotationalNodeId` (turret node, `NONE` when there is none).
    pub rotational_node_id: String,
    /// `nodeData`: `Smoke_01..02`, `Weapon_01..04` (any weapon), `FootNode_01..02`.
    pub node_data: Vec<NodeData>,
}

impl VehicleAppearanceType {
    /// `GVAppearanceType::init(fileName)` (gvactor.cpp:103).
    pub fn load(vfs: &mut Vfs, name: &str) -> Result<VehicleAppearanceType, AppearanceError> {
        let ini_path = format!("{TGL_PATH}{}.ini", name.to_ascii_lowercase());
        let mut ini = vfs.read_fit(&ini_path)?.ok_or_else(|| {
            AppearanceError(format!(
                "Could not find vehicle appearance INI file {ini_path}"
            ))
        })?;
        let mut t = VehicleAppearanceType {
            name: name.to_string(),
            ..Default::default()
        };
        ini.seek_block("TGLData").map_err(|_| {
            AppearanceError(format!(
                "{name}: Could not find block in vehicle appearance INI file"
            ))
        })?;
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
                "vehicle {name}: base shape file missing"
            )));
        }
        if ini.seek_block("TGLDamage").is_ok() {
            let file = ini
                .read_string("FileName", 511)
                .map_err(|_| AppearanceError(format!("{name}: [TGLDamage] has no FileName")))?;
            t.damage = read_shape_file(vfs, &file)?.filter(|s| !s.is_empty());
        }
        if ini.seek_block("TGLDestructEffect").is_ok() {
            t.destruct_effect = ini.read_string("FileName", 59)?;
        }
        for i in 0..MAX_GV_ANIMATIONS {
            let mut set = None;
            if ini.seek_block(&format!("Animation:{i}")).is_ok() {
                // gosASSERTs only (debug builds): a missing key keeps the initial value.
                let anim_name = ini.read_string("AnimationName", 511).unwrap_or_default();
                if let Ok(v) = ini.read_bool("LoopAnimation") {
                    t.anim_loop[i] = v;
                }
                if let Ok(v) = ini.read_bool("Reverse") {
                    t.anim_reverse[i] = v;
                }
                if let Ok(v) = ini.read_bool("Random") {
                    t.anim_random[i] = v;
                }
                t.anim_start[i] = ini.read_i64("StartFrame").map_or(0, |v| v as i32);
                let lower = anim_name.to_ascii_lowercase();
                let ase = format!("{TGL_PATH}{lower}.ase");
                let agl = format!("{TGL_PATH}{lower}.agl");
                if vfs.locate(&ase).is_some() || vfs.locate(&agl).is_some() {
                    let bytes = vfs.read(&agl)?.ok_or_else(|| {
                        AppearanceError(format!("{agl}: only an .ase file, which is not supported"))
                    })?;
                    set = Some(AnimSet::new(decode_animation(&bytes)?));
                }
            }
            t.anims.push(set);
        }
        t.rotational_node_id = if ini.seek_block("AnimationNode").is_ok() {
            ini.read_string("AnimationNodeId", 24)?
        } else {
            "NONE".to_string()
        };
        let node = |id: String, wt: i32| NodeData {
            node_id: id,
            weapon_type: wt,
            ..Default::default()
        };
        for i in 1..=GV_SMOKE_NODES {
            t.node_data
                .push(node(format!("Smoke_{i:02}"), weapon_type::NONE));
        }
        for i in 1..=GV_WEAPON_NODES {
            t.node_data
                .push(node(format!("Weapon_{i:02}"), weapon_type::ANY));
        }
        for i in 1..=GV_FOOT_NODES {
            t.node_data
                .push(node(format!("FootNode_{i:02}"), weapon_type::NONE));
        }
        Ok(t)
    }

    fn anim(&self, g: i32) -> Option<&AnimSet> {
        if (0..MAX_GV_ANIMATIONS as i32).contains(&g) {
            self.anims[g as usize].as_ref()
        } else {
            None
        }
    }

    /// `getNumFrames` (0 without data).
    pub fn num_frames(&self, g: i32) -> i32 {
        self.anim(g).map_or(0, |a| a.num_frames())
    }

    /// `getFrameRate` (0 without data).
    pub fn frame_rate(&self, g: i32) -> f32 {
        self.anim(g).map_or(0.0, |a| a.frame_rate())
    }

    fn flag(&self, g: i32, table: &[bool; MAX_GV_ANIMATIONS]) -> bool {
        self.anim(g).is_some() && table[g as usize]
    }

    /// `getTotalNodes()`.
    pub fn total_nodes(&self) -> i32 {
        GV_SMOKE_NODES + GV_WEAPON_NODES + GV_FOOT_NODES
    }

    /// The node table of a shape kind.
    pub fn shape(&self, kind: GvShapeKind) -> &ShapeType {
        static EMPTY: ShapeType = ShapeType { nodes: Vec::new() };
        match kind {
            GvShapeKind::Lod(i) => self.shapes.get(i).and_then(|s| s.as_ref()),
            GvShapeKind::Damage => self.damage.as_ref(),
        }
        .unwrap_or(&EMPTY)
    }

    /// `GVAppearanceType::setAnimation(shape, n)` (gvactor.cpp:407).
    fn set_animation(&mut self, shape: &mut ShapeInstance, kind: GvShapeKind, g: i32) {
        let ty = self.shape(kind).clone();
        match self.anims.get_mut(g as usize).and_then(|a| a.as_mut()) {
            Some(set) => shape.set_animation_state(&ty, set, g as usize),
            None => shape.clear_animation(),
        }
    }
}

/// `GVAppearance` logic state.
#[derive(Clone, Debug)]
pub struct VehicleAppearance {
    /// Index into [`super::AppearanceSystem::vehicle_types`].
    pub type_index: usize,
    /// `gvShape`.
    pub shape: ShapeInstance,
    /// What `gvShape` was created from.
    pub shape_kind: GvShapeKind,
    /// `inView`.
    pub in_view: bool,
    /// `position`.
    pub position: Vec3,
    /// `rotation` (normalised to -180..180 by `updateGeometry`).
    pub rotation: f32,
    /// `turretRotation`.
    pub turret_rotation: f32,
    /// `pitch` (`setMoverParameters` lArmRot).
    pub pitch: f32,
    /// `roll` (`setMoverParameters` rArmRot).
    pub roll: f32,
    /// `isInfantry` (`setMoverParameters` isAirborne).
    pub is_infantry: bool,
    /// `status`.
    pub status: i32,
    /// `gvAnimationState` (-1 = none).
    pub current_gesture: i32,
    /// `currentFrame`.
    pub current_frame: f32,
    /// `gvFrameRate`.
    pub frame_rate: f32,
    /// `isReversed`.
    pub is_reversed: bool,
    /// `isLooping`.
    pub is_looping: bool,
    /// `setFirstFrame`.
    pub set_first_frame: bool,
    /// `canTransition`.
    pub can_transition: bool,
    /// `currentLOD`.
    pub current_lod: usize,
    /// `nodeUsed[4]`.
    pub node_used: Vec<i32>,
    /// `nodeRecycle[4]`.
    pub node_recycle: Vec<f32>,
    /// `rotationalNodeIndex` (cached on first use, kept across shape changes).
    pub rotational_node_index: i32,
    /// `hitNodeId`.
    pub hit_node_id: i32,
    /// `weaponNodeId[4]`: shape index per weapon node, -1 unknown, -2 missing (marked by a query while in view).
    pub weapon_node_id: [i32; 4],
    /// Attached-effect state, see [`VehicleFxState`].
    pub fx: VehicleFxState,
}

/// The effect bookkeeping of `GVAppearance` (`isWaking`, `dustCloudStart`, `isActivitying`, `movedThisFrame`) and the effect calls it
/// made since the owner took them. Presentation only: not saved, not in the digest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleFxState {
    /// `isWaking`.
    pub waking: bool,
    /// `dustCloudStart`.
    pub dust_started: bool,
    /// The dust cloud is not drawn while the wake is (gvactor.cpp:2131-2168).
    pub dust_hidden: bool,
    /// `isActivitying`, and the `effects.csv` row `startActivity` was given.
    pub activity: Option<i32>,
    /// `movedThisFrame`.
    pub moved: bool,
    /// `useNonWeaponEffects` (the owner sets it before each update).
    pub non_weapon: bool,
    /// The calls, oldest first.
    pub calls: Vec<FxCall>,
}

/// A placeholder without a type (`type_index` out of range): the [`super::AppearanceSystem`] methods treat it like an
/// appearance-less object, except `inView` and the gesture id. Real vehicles use [`VehicleAppearance::new`].
impl Default for VehicleAppearance {
    fn default() -> Self {
        VehicleAppearance {
            type_index: usize::MAX,
            ..VehicleAppearance::new(&VehicleAppearanceType::default(), usize::MAX)
        }
    }
}

impl VehicleAppearance {
    /// `GVAppearance::init(type, obj)` (gvactor.cpp:675).
    pub fn new(ty: &VehicleAppearanceType, type_index: usize) -> VehicleAppearance {
        VehicleAppearance {
            type_index,
            shape: ShapeInstance::create_from(ty.shape(GvShapeKind::Lod(0))),
            shape_kind: GvShapeKind::Lod(0),
            in_view: false,
            position: Vec3::default(),
            rotation: 0.0,
            turret_rotation: 0.0,
            pitch: 0.0,
            roll: 0.0,
            is_infantry: false,
            status: 0,
            current_gesture: -1,
            current_frame: 0.0,
            frame_rate: 0.0,
            is_reversed: false,
            is_looping: false,
            set_first_frame: false,
            can_transition: true,
            current_lod: 0,
            node_used: vec![0; GV_WEAPON_NODES as usize],
            node_recycle: vec![0.0; GV_WEAPON_NODES as usize],
            rotational_node_index: -1,
            hit_node_id: -1,
            weapon_node_id: [-1; 4],
            fx: VehicleFxState::default(),
        }
    }

    /// `setObjStatus(oStatus)` (gvactor.cpp:1023): destroyed/disabled swaps in the damage shape, normal the base shape.
    pub fn set_obj_status(&mut self, ty: &VehicleAppearanceType, o: i32) {
        if self.status != o {
            if o == object_status::DESTROYED || o == object_status::DISABLED {
                if ty.damage.is_some() {
                    self.shape = ShapeInstance::create_from(ty.shape(GvShapeKind::Damage));
                    self.shape_kind = GvShapeKind::Damage;
                }
                self.current_lod = 0;
            }
            if o == object_status::NORMAL {
                self.shape = ShapeInstance::create_from(ty.shape(GvShapeKind::Lod(0)));
                self.shape_kind = GvShapeKind::Lod(0);
                self.current_lod = 0;
            }
        }
        self.status = o;
    }

    /// `setGesture(gestureId)` (gvactor.cpp:1352). A random-start animation draws `RandomNumber(numFrames - 1)` from
    /// `rng` (`oracle/visual_rng_sites.txt` assigns gvactor.cpp:1383 its stream).
    pub fn set_gesture(&mut self, ty: &mut VehicleAppearanceType, g: u32, rng: &mut SimRng) {
        if g as usize >= MAX_GV_ANIMATIONS
            || self.status == object_status::DESTROYED
            || self.status == object_status::DISABLED
        {
            return;
        }
        let gi = g as i32;
        ty.set_animation(&mut self.shape, self.shape_kind, gi);
        self.current_gesture = gi;
        self.current_frame = 0.0;
        if ty.anim_start[g as usize] != 0 {
            self.current_frame = ty.anim_start[g as usize] as f32;
        }
        self.is_reversed = false;
        if ty.flag(gi, &ty.anim_reverse) {
            self.current_frame = (ty.num_frames(gi) - 1) as f32;
            self.is_reversed = true;
        }
        if ty.flag(gi, &ty.anim_random) {
            self.current_frame = rng.random_number(ty.num_frames(gi) - 1) as f32;
        }
        self.is_looping = ty.flag(gi, &ty.anim_loop);
        self.frame_rate = ty.frame_rate(gi);
        self.set_first_frame = true;
        self.can_transition = false;
    }

    /// `setObjectParameters(pos, rot, ...)` (gvactor.cpp:1405): the turret follows the body until `setMoverParameters`.
    pub fn set_object_parameters(&mut self, pos: Vec3, rot: f32) {
        self.fx.moved = self.rotation != rot || self.position != pos;
        self.rotation = rot;
        self.turret_rotation = rot;
        self.position = pos;
    }

    /// `setMoverParameters(turretRot, lArmRot, rArmRot, isAirborne)` (gvactor.cpp:1425).
    pub fn set_mover_parameters(&mut self, turret_rot: f32, pitch: f32, roll: f32, infantry: bool) {
        self.turret_rotation = turret_rot;
        self.pitch = pitch;
        self.roll = roll;
        self.is_infantry = infantry;
    }

    /// `recalcBounds()` (gvactor.cpp:1543) with the camera results as inputs.
    pub fn recalc_bounds(
        &mut self,
        ty: &mut VehicleAppearanceType,
        in_view: bool,
        lod: usize,
    ) -> bool {
        self.in_view = in_view;
        if in_view
            && self.status != object_status::DESTROYED
            && self.status != object_status::DISABLED
            && lod != self.current_lod
            && lod < MAX_LODS
            && ty.shapes[lod].is_some()
        {
            self.current_lod = lod;
            let kind = GvShapeKind::Lod(lod);
            self.shape = ShapeInstance::create_from(ty.shape(kind));
            self.shape_kind = kind;
            if self.current_gesture != -1 {
                ty.set_animation(&mut self.shape, kind, self.current_gesture);
            }
        }
        self.in_view
    }

    /// `GVAppearance::update(animate)` (gvactor.cpp:2598).
    pub fn update(
        &mut self,
        ty: &mut VehicleAppearanceType,
        animate: bool,
        ctx: &mut UpdateCtx,
    ) -> i32 {
        for r in self.node_recycle.iter_mut() {
            if *r > 0.0 {
                *r -= ctx.frame_length;
                if *r < 0.0 {
                    *r = 0.0;
                }
            }
        }
        if animate && self.frame_rate != 0.0 {
            let frame_inc = self.frame_rate * ctx.frame_length;
            if frame_inc != 0.0 {
                if !self.set_first_frame {
                    if self.is_reversed {
                        self.current_frame -= frame_inc;
                    } else {
                        self.current_frame += frame_inc;
                    }
                } else {
                    self.set_first_frame = false;
                }
                let n = ty.num_frames(self.current_gesture) as f32;
                if self.current_frame >= n {
                    if self.is_looping {
                        self.current_frame -= n;
                    } else {
                        self.current_frame = n - 1.0;
                    }
                    self.can_transition = true;
                }
                if self.current_frame < 0.0 {
                    if self.is_looping {
                        self.current_frame += ty.num_frames(self.current_gesture) as f32;
                    } else {
                        self.current_frame = 0.0;
                    }
                    self.can_transition = true;
                }
            }
            self.shape.frame_num = self.current_frame;
        }
        if ctx.turn < 3 || self.in_view {
            self.update_geometry(ty, ctx);
        }
        1
    }

    /// `startWaterWake()` / `stopWaterWake()` (gvactor.cpp:2755-2817): the wake lies flat at the water level, once.
    pub fn set_wake(&mut self, wading: bool, water_elevation: f32) {
        if wading {
            if self.fx.non_weapon && !self.fx.waking {
                let place = self.wake_place(water_elevation);
                self.fx.calls.push(FxCall {
                    slot: FxSlot::WaterWake,
                    effect: FxEffect::Id(fx_id::VEHICLE_WATER_WAKE),
                    action: FxAction::Start {
                        place,
                        looping: true,
                    },
                });
                self.fx.waking = true;
            }
        } else {
            if self.fx.waking {
                self.fx.calls.push(FxCall {
                    slot: FxSlot::WaterWake,
                    effect: FxEffect::Id(fx_id::VEHICLE_WATER_WAKE),
                    action: FxAction::Kill,
                });
            }
            self.fx.waking = false;
        }
    }

    fn wake_place(&self, water_elevation: f32) -> FxPlacement {
        FxPlacement {
            at: Vec3 {
                x: self.position.x,
                y: self.position.y,
                z: water_elevation,
            },
            pitch: 90.0,
            yaw: self.rotation,
            base: FxBase::AgainstMotion,
        }
    }

    /// `startActivity(effectId, loop)` (gvactor.cpp:2821): the minelayer and repair truck effect at `activity_node`.
    pub fn start_activity(&mut self, ty: &VehicleAppearanceType, effect_id: i32, looping: bool) {
        if !self.fx.non_weapon || self.fx.activity.is_some() {
            return;
        }
        let at = self.node_name_position(ty, "activity_node");
        self.fx.calls.push(FxCall {
            slot: FxSlot::Activity,
            effect: FxEffect::Id(effect_id),
            action: FxAction::Start {
                place: FxPlacement::plain(at),
                looping,
            },
        });
        self.fx.activity = Some(effect_id);
    }

    /// `stopActivity()` (gvactor.cpp:2885).
    pub fn stop_activity(&mut self) {
        if let Some(id) = self.fx.activity.take() {
            self.fx.calls.push(FxCall {
                slot: FxSlot::Activity,
                effect: FxEffect::Id(id),
                action: FxAction::Kill,
            });
        }
    }

    /// The `Update GOSFX` part of `updateGeometry` (gvactor.cpp:2424-2545): the wake or, off the water, the dust cloud at
    /// `dust_body` (looping while the vehicle moves, never for infantry), and the activity effect at `activity_node`.
    fn update_attached(&mut self, ty: &VehicleAppearanceType, water_elevation: f32) {
        if !self.fx.non_weapon {
            return;
        }
        if self.fx.waking != self.fx.dust_hidden {
            self.fx.dust_hidden = self.fx.waking;
            self.fx.calls.push(FxCall {
                slot: FxSlot::DustCloud,
                effect: FxEffect::Id(fx_id::VEHICLE_DUST_CLOUD),
                action: FxAction::Draw(!self.fx.waking),
            });
        }
        if self.fx.waking {
            let place = self.wake_place(water_elevation);
            self.fx.calls.push(FxCall {
                slot: FxSlot::WaterWake,
                effect: FxEffect::Id(fx_id::VEHICLE_WATER_WAKE),
                action: FxAction::Move {
                    place,
                    looping: None,
                },
            });
        } else {
            let moving = self.fx.moved && !self.is_infantry;
            let place = FxPlacement {
                at: self.node_name_position(ty, "dust_body"),
                pitch: 0.0,
                yaw: self.rotation,
                base: FxBase::AgainstMotion,
            };
            let action = if !self.fx.dust_started {
                self.fx.dust_started = true;
                FxAction::Start {
                    place,
                    looping: moving,
                }
            } else {
                FxAction::Move {
                    place,
                    looping: Some(moving),
                }
            };
            self.fx.calls.push(FxCall {
                slot: FxSlot::DustCloud,
                effect: FxEffect::Id(fx_id::VEHICLE_DUST_CLOUD),
                action,
            });
        }
        if let Some(id) = self.fx.activity {
            let place = FxPlacement::plain(self.node_name_position(ty, "activity_node"));
            self.fx.calls.push(FxCall {
                slot: FxSlot::Activity,
                effect: FxEffect::Id(id),
                action: FxAction::Move {
                    place,
                    looping: None,
                },
            });
        }
    }

    /// `updateGeometry()` (gvactor.cpp:2272), node part: terrain pitch/roll, turret twist, hierarchy transform.
    pub fn update_geometry(&mut self, ty: &VehicleAppearanceType, ctx: &mut UpdateCtx) {
        if self.rotation > 180.0 {
            self.rotation -= 360.0;
        }
        if self.rotation < -180.0 {
            self.rotation += 360.0;
        }
        if self.turret_rotation > 180.0 {
            self.turret_rotation -= 360.0;
        }
        if self.turret_rotation < -180.0 {
            self.turret_rotation += 360.0;
        }
        let mut pitch_angle = self.pitch;
        let mut roll_angle = self.roll;
        if self.status == object_status::NORMAL || self.status == object_status::SHUTDOWN {
            if let Some(normal) = ctx
                .terrain
                .and_then(|t| t.terrain_angle_normal(self.position))
            {
                let mut world_k = normal;
                rotate(&mut world_k, self.rotation);
                let mut pitch_k = world_k;
                pitch_k.x = 0.0;
                normalize(&mut pitch_k);
                let mut roll_k = world_k;
                roll_k.y = 0.0;
                normalize(&mut roll_k);
                // up = (0, 0, 1); `up * v` is the dot product x*v.x + y*v.y + z*v.z.
                pitch_angle = 0.0 * pitch_k.x + 0.0 * pitch_k.y + 1.0 * pitch_k.z;
                pitch_angle = mc2_libm::acosf(pitch_angle) * RADS_TO_DEGREES;
                if pitch_k.y < 0.0 {
                    pitch_angle = -pitch_angle;
                }
                roll_angle = 0.0 * roll_k.x + 0.0 * roll_k.y + 1.0 * roll_k.z;
                roll_angle = mc2_libm::acosf(roll_angle) * RADS_TO_DEGREES;
                if roll_k.x < 0.0 {
                    roll_angle = -roll_angle;
                }
            }
        }
        let yaw = self.rotation * DEGREES_TO_RADS;
        pitch_angle *= DEGREES_TO_RADS;
        roll_angle *= DEGREES_TO_RADS;
        let rot = quat_from_euler(0.0, yaw, 0.0);
        let pitch_roll = quat_from_euler(pitch_angle, 0.0, roll_angle);
        let total_rotation = quat_multiply(&pitch_roll, &rot);
        let xlat = [-self.position.x, self.position.z, self.position.y];
        let turret_rot = quat_from_euler(0.0, self.turret_rotation * DEGREES_TO_RADS, 0.0);
        let shape_ty = ty.shape(self.shape_kind);
        if self.rotational_node_index == -1 {
            self.rotational_node_index =
                self.shape
                    .set_node_rotation_by_name(shape_ty, &ty.rotational_node_id, &turret_rot);
        }
        self.shape
            .set_node_rotation(self.rotational_node_index, &turret_rot);
        self.shape
            .transform(shape_ty, &ty.anims, xlat, &total_rotation, ctx.turn);
        let water = ctx.terrain.map_or(0.0, |t| t.water_elevation());
        self.update_attached(ty, water);
    }

    fn touch_turret(&mut self, ty: &VehicleAppearanceType) {
        let turret_rot = quat_from_euler(0.0, self.turret_rotation * DEGREES_TO_RADS, 0.0);
        if self.rotational_node_index == -1 {
            self.rotational_node_index = self.shape.set_node_rotation_by_name(
                ty.shape(self.shape_kind),
                &ty.rotational_node_id,
                &turret_rot,
            );
        }
        self.shape
            .set_node_rotation(self.rotational_node_index, &turret_rot);
    }

    fn v(p: [f32; 3]) -> Vec3 {
        Vec3 {
            x: p[0],
            y: p[1],
            z: p[2],
        }
    }

    fn or_position(&self, p: [f32; 3]) -> Vec3 {
        if p == [0.0, 0.0, 0.0] {
            self.position
        } else {
            Self::v(p)
        }
    }

    /// `getWeaponNodePosition(nodeId)` (gvactor.cpp:439): resolves the node lazily and marks a missing one -2.
    pub fn weapon_node_position(&mut self, ty: &VehicleAppearanceType, node_id: i32) -> Vec3 {
        if !(GV_SMOKE_NODES..GV_SMOKE_NODES + GV_WEAPON_NODES).contains(&node_id) || !self.in_view {
            return self.position;
        }
        self.touch_turret(ty);
        let k = (node_id - GV_SMOKE_NODES) as usize;
        let shape_ty = ty.shape(self.shape_kind);
        if self.weapon_node_id[k] == -1 {
            self.weapon_node_id[k] = shape_ty.node_name_id(&ty.node_data[node_id as usize].node_id);
        }
        if self.weapon_node_id[k] == -1 {
            self.weapon_node_id[k] = -2;
        }
        let mut result = self.position;
        if self.weapon_node_id[k] >= 0 {
            result = Self::v(self.shape.node_position(self.weapon_node_id[k]));
        }
        if result.x == 0.0 && result.y == 0.0 && result.z == 0.0 {
            result = self.position;
        }
        result
    }

    /// `getSmokeNodePosition(nodeId)` (gvactor.cpp:488).
    pub fn smoke_node_position(&mut self, ty: &VehicleAppearanceType, node_id: i32) -> Vec3 {
        if !(0..GV_SMOKE_NODES).contains(&node_id) || !self.in_view {
            return self.position;
        }
        self.touch_turret(ty);
        Self::v(self.shape.node_position_by_name(
            ty.shape(self.shape_kind),
            &ty.node_data[node_id as usize].node_id,
        ))
    }

    /// `getDustNodePosition(nodeId)` (gvactor.cpp:521).
    pub fn dust_node_position(&mut self, ty: &VehicleAppearanceType, node_id: i32) -> Vec3 {
        if !(0..GV_FOOT_NODES).contains(&node_id) || !self.in_view {
            return self.position;
        }
        self.touch_turret(ty);
        let name = &ty.node_data[(node_id + GV_WEAPON_NODES + GV_SMOKE_NODES) as usize].node_id;
        Self::v(
            self.shape
                .node_position_by_name(ty.shape(self.shape_kind), name),
        )
    }

    /// `getLowestWeaponNode()` (gvactor.cpp:554).
    pub fn lowest_weapon_node(&mut self, ty: &VehicleAppearanceType) -> i32 {
        let mut best_node = -1;
        let mut lowest_pos_z = 9_999_999_999_999.0f32;
        for i in 0..GV_WEAPON_NODES {
            let p = self.weapon_node_position(ty, i + GV_SMOKE_NODES);
            if (p.z - self.position.z) < lowest_pos_z && p.z != self.position.z {
                lowest_pos_z = p.z - self.position.z;
                best_node = i + GV_SMOKE_NODES;
            }
        }
        if lowest_pos_z == 0.0 || best_node < 0 || best_node >= ty.total_nodes() {
            return -1;
        }
        best_node
    }

    /// `getWeaponNode(weaponType)` (gvactor.cpp:579): least used compatible node; unlike the mech version it does not mark
    /// the node used.
    pub fn weapon_node(&self, ty: &VehicleAppearanceType, wtype: i32) -> i32 {
        use self::weapon_type as wt;
        let mut least_used = 999_999_999;
        let mut best_node = -1;
        for i in 0..GV_WEAPON_NODES {
            if self.weapon_node_id[i as usize] == -2 {
                continue;
            }
            let nt = ty.node_data[(GV_SMOKE_NODES + i) as usize].weapon_type;
            let ok = match wtype {
                wt::MISSILE | wt::BALLISTIC => [wtype, wt::ANY, wt::NONENERGY].contains(&nt),
                wt::ENERGY => [wtype, wt::ANY].contains(&nt),
                wt::ANY => true,
                _ => false,
            };
            if ok && self.node_used[i as usize] < least_used {
                least_used = self.node_used[i as usize];
                best_node = i + GV_SMOKE_NODES;
            }
        }
        if best_node < 0 || best_node >= ty.total_nodes() {
            return -1;
        }
        best_node
    }

    /// `setWeaponNodeUsed(node)` (gvactor.cpp:428).
    pub fn set_weapon_node_used(&mut self, node: i32) {
        let n = node - GV_SMOKE_NODES;
        if (0..GV_WEAPON_NODES).contains(&n) {
            self.node_used[n as usize] += 1;
            self.node_recycle[n as usize] = super::tables::BASE_NODE_RECYCLE_TIME;
        }
    }

    /// `getWeaponNodeRecycle(node)` (gvactor.cpp:664).
    pub fn weapon_node_recycle(&self, node: i32) -> f32 {
        let n = node - GV_SMOKE_NODES;
        if (0..GV_WEAPON_NODES).contains(&n) {
            self.node_recycle[n as usize]
        } else {
            9999.0
        }
    }

    /// `getNodeNamePosition(name)` (gvactor.cpp:1931).
    pub fn node_name_position(&mut self, ty: &VehicleAppearanceType, name: &str) -> Vec3 {
        if !self.in_view {
            return self.position;
        }
        self.touch_turret(ty);
        let p = self
            .shape
            .node_position_by_name(ty.shape(self.shape_kind), name);
        self.or_position(p)
    }

    /// `getNodeIdPosition(id)` (gvactor.cpp:1967).
    pub fn node_id_position(&mut self, ty: &VehicleAppearanceType, id: i32) -> Vec3 {
        if !self.in_view {
            return self.position;
        }
        self.touch_turret(ty);
        let p = self.shape.node_position(id);
        self.or_position(p)
    }

    /// `getHitNode()` (gvactor.cpp:1395).
    pub fn hit_node(&mut self, ty: &VehicleAppearanceType) -> Vec3 {
        if self.hit_node_id == -1 {
            self.hit_node_id = ty.shape(self.shape_kind).node_name_id("hitnode");
        }
        self.node_id_position(ty, self.hit_node_id)
    }

    /// `playDestruction()` (gvactor.cpp:1888) with the effect started: the type's `destructEffect` at the vehicle, plain.
    pub fn play_destruction_fx(&mut self, ty: &VehicleAppearanceType) -> bool {
        if ty.destruct_effect.is_empty() {
            return false;
        }
        self.fx.calls.push(FxCall {
            slot: FxSlot::Destruct,
            effect: FxEffect::Named(ty.destruct_effect.clone()),
            action: FxAction::Start {
                place: FxPlacement::plain(self.position),
                looping: false,
            },
        });
        true
    }

    /// `playDestruction()` (gvactor.cpp:1888): true when the type names a destruct effect that the effect library has
    /// (`effect_exists`); the effect itself is presentation.
    pub fn play_destruction(
        &self,
        ty: &VehicleAppearanceType,
        effect_exists: &dyn Fn(&str) -> bool,
    ) -> bool {
        !ty.destruct_effect.is_empty() && effect_exists(&ty.destruct_effect)
    }
}

/// `Rotate(Vector3D&, angle)` (mclib/mathfunc.cpp:34): `sin`/`cos` of a float, merged like [`sin_cos`].
pub fn rotate(v: &mut Vec3, angle: f32) {
    let tx = v.x;
    let (sine, cosine) = sin_cos(angle * DEGREES_TO_RADS);
    v.x *= cosine;
    v.x += v.y * sine;
    v.y *= cosine;
    v.y -= tx * sine;
}

/// `Vector3D::Normalize(v)` on itself.
fn normalize(v: &mut Vec3) {
    let len = 1.0 / (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    v.x *= len;
    v.y *= len;
    v.z *= len;
}

// ------------------------------------------------------------------------------------------------ save/load

use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};

// Static (not saved): `VehicleAppearanceType` (shapes, damage shape, lod distances, animation entries and flags, node
// ids). Its `AnimSet::shape_ids` cache is saved by the owner, `AppearanceSystem` (mod.rs).

impl Snapshot for GvShapeKind {
    fn save(&self, w: &mut Writer) {
        match *self {
            GvShapeKind::Lod(i) => {
                w.u8(0);
                i.save(w);
            }
            GvShapeKind::Damage => w.u8(1),
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        match r.u8()? {
            0 => Ok(GvShapeKind::Lod(usize::load(r)?)),
            1 => Ok(GvShapeKind::Damage),
            tag => Err(SaveError(format!("save: bad vehicle shape tag {tag}"))),
        }
    }
}

// Every field is dynamic: no field is static or derived (`weapon_node_id` keeps the -1/-2 marks a query sets).
crate::snapshot_struct!(VehicleAppearance {
    type_index,
    shape,
    shape_kind,
    in_view,
    position,
    rotation,
    turret_rotation,
    pitch,
    roll,
    is_infantry,
    status,
    current_gesture,
    current_frame,
    frame_rate,
    is_reversed,
    is_looping,
    set_first_frame,
    can_transition,
    current_lod,
    node_used,
    node_recycle,
    rotational_node_index,
    hit_node_id,
    weapon_node_id,
    fx => VehicleFxState::default(),
});
