// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `BldgAppearanceType` / `BldgAppearance` and `TreeAppearanceType` / `TreeAppearance` (`mclib/bdactor.{h,cpp}`):
//! buildings, turrets, gates and terrain objects. Simulation-relevant are the animation state machine (turret pop-up and
//! gate open/close timing through `getInTransition`), the LOD/damage shape bookkeeping, weapon/hit node positions, and the
//! footprint marking that writes the move map (`markMoveMap`, `calcCellsCovered`) and LOS heights (`markLOS`).
//! Dropped: textures, lights, fog, shadows, flashing, destruct/activity effects, selection bounds.
//!
//! `GenericAppearance` (`mclib/genactor.cpp`) is used only for the sky dome (code/gamecam.cpp:418) and has no simulation
//! effect; it is not ported.

use mc2_assets::stuff::quat_from_euler;
use mc2_assets::tgl::decode_animation;
use mc2_assets::tgl_instance::{AnimSet, ShapeInstance, ShapeType};
use mc2_formats::vfs::Vfs;

use super::fx::FxCall;
use super::mech::{object_status, read_shape_file, NodeData, UpdateCtx, TGL_PATH};
use super::tables::{BASE_NODE_RECYCLE_TIME, DEGREES_TO_RADS, MAX_LODS};
use super::AppearanceError;
use crate::events::{FxAction, FxEffect, FxPlacement, FxSlot};
use crate::math::Vec3;
use crate::rng::SimRng;
use crate::time::STUFF_SMALL;

/// `MAX_BD_ANIMATIONS` (bdactor.h:52).
pub const MAX_BD_ANIMATIONS: usize = 10;
/// `MAX_WEAPON_NODES` (bdactor.cpp:86).
pub const BD_WEAPON_NODES: usize = 4;
/// `SPINRATE` (bdactor.cpp:84), degrees per second.
pub const SPINRATE: f32 = 90.0;
/// `metersPerWorldUnit` (mclib/terrain.cpp:64).
pub const METERS_PER_WORLD_UNIT: f32 = 0.2;

/// The move-map cells the footprint marking reads and writes (`land->worldToCell`, `GameMap->getCell`).
pub trait MapCells {
    /// `land->worldToCell(pos, row, col)`.
    fn world_to_cell(&self, pos: Vec3) -> (i32, i32);
    /// `Terrain::realVerticesMapSide * MAPCELL_DIM` (cells per map side; rows/columns outside are skipped).
    fn cells_per_side(&self) -> i32;
    /// `MapCell::setPassable`.
    fn set_passable(&mut self, row: i32, col: i32, passable: bool);
    /// `MapCell::getLocalHeight`.
    fn local_height(&self, row: i32, col: i32) -> u32;
    /// `MapCell::setLocalHeight(DWORD)`: ORs the value in at bit 18 after clearing the 4-bit field.
    fn set_local_height(&mut self, row: i32, col: i32, height: u32);
}

/// Which type shape the instance shape was created from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BdShapeKind {
    /// LOD shape.
    Lod(usize),
    /// Destroyed shape.
    Damage,
}

/// `BldgAppearanceType`.
#[derive(Clone, Debug, Default)]
pub struct BuildingAppearanceType {
    /// `name`.
    pub name: String,
    /// `bldgShape[MAX_LODS]`.
    pub shapes: [Option<ShapeType>; MAX_LODS],
    /// `lodDistance`.
    pub lod_distance: [f32; MAX_LODS],
    /// `bldgDmgShape`.
    pub damage: Option<ShapeType>,
    /// `destructEffect`.
    pub destruct_effect: String,
    /// `bdAnimData`.
    pub anims: Vec<Option<AnimSet>>,
    /// `bdAnimLoop`.
    pub anim_loop: [bool; MAX_BD_ANIMATIONS],
    /// `bdReverse`.
    pub anim_reverse: [bool; MAX_BD_ANIMATIONS],
    /// `bdRandom`.
    pub anim_random: [bool; MAX_BD_ANIMATIONS],
    /// `bdStartF`.
    pub anim_start: [i32; MAX_BD_ANIMATIONS],
    /// `rotationalNodeId` (`NONE` when the ini has no `[AnimationNode]`).
    pub rotational_node_id: String,
    /// `spinMe` (spinning markers).
    pub spin_me: bool,
    /// `isForestClump`.
    pub is_forest_clump: bool,
    /// `nodeData` (`WeaponNodeId0..3`, `NONE` when missing; all `ANY`), empty without a `[WeaponNode]` block.
    pub node_data: Vec<NodeData>,
}

impl BuildingAppearanceType {
    /// `BldgAppearanceType::init(fileName)` (bdactor.cpp:89).
    pub fn load(vfs: &mut Vfs, name: &str) -> Result<BuildingAppearanceType, AppearanceError> {
        let ini_path = format!("{TGL_PATH}{}.ini", name.to_ascii_lowercase());
        let mut ini = vfs.read_fit(&ini_path)?.ok_or_else(|| {
            AppearanceError(format!(
                "Could not find building appearance INI file {ini_path}"
            ))
        })?;
        let mut t = BuildingAppearanceType {
            name: name.to_string(),
            ..Default::default()
        };
        ini.seek_block("TGLData").map_err(|_| {
            AppearanceError(format!(
                "{name}: Could not find block in building appearance INI file"
            ))
        })?;
        t.spin_me = ini.read_bool("SpinMe").unwrap_or(false);
        let frame_rate_override = ini.read_f32("FrameRate").unwrap_or(0.0);
        t.is_forest_clump = ini.read_bool("ForestClump").unwrap_or(false);
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
                "building {name}: base shape file missing"
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
        for i in 0..MAX_BD_ANIMATIONS {
            let mut set = None;
            if ini.seek_block(&format!("Animation:{i}")).is_ok() {
                // The C++ ignores every result here (a missing key keeps the initial value).
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
            ini.read_string("AnimationNodeId", 24).unwrap_or_default()
        } else {
            "NONE".to_string()
        };
        if frame_rate_override != 0.0 {
            for set in t.anims.iter_mut().flatten() {
                if let Some(a) = set.entries.first_mut() {
                    a.frame_rate = frame_rate_override;
                }
            }
        }
        if ini.seek_block("WeaponNode").is_ok() {
            for i in 0..BD_WEAPON_NODES {
                let node_id = ini
                    .read_string(&format!("WeaponNodeId{i}"), 511)
                    .unwrap_or_else(|_| "NONE".into());
                t.node_data.push(NodeData {
                    node_id,
                    weapon_type: 0,
                    ..Default::default()
                });
            }
        }
        Ok(t)
    }

    fn anim(&self, g: i32) -> Option<&AnimSet> {
        if (0..MAX_BD_ANIMATIONS as i32).contains(&g) {
            self.anims[g as usize].as_ref()
        } else {
            None
        }
    }

    /// `getNumFrames`.
    pub fn num_frames(&self, g: i32) -> i32 {
        self.anim(g).map_or(0, |a| a.num_frames())
    }

    /// `getFrameRate`.
    pub fn frame_rate(&self, g: i32) -> f32 {
        self.anim(g).map_or(0.0, |a| a.frame_rate())
    }

    fn flag(&self, g: i32, table: &[bool; MAX_BD_ANIMATIONS]) -> bool {
        self.anim(g).is_some() && table[g as usize]
    }

    /// `numWeaponNodes`.
    pub fn num_weapon_nodes(&self) -> i32 {
        self.node_data.len() as i32
    }

    /// Node table of a shape kind.
    pub fn shape(&self, kind: BdShapeKind) -> &ShapeType {
        static EMPTY: ShapeType = ShapeType { nodes: Vec::new() };
        match kind {
            BdShapeKind::Lod(i) => self.shapes.get(i).and_then(|s| s.as_ref()),
            BdShapeKind::Damage => self.damage.as_ref(),
        }
        .unwrap_or(&EMPTY)
    }

    fn set_animation(&mut self, shape: &mut ShapeInstance, kind: BdShapeKind, g: i32) {
        let ty = self.shape(kind).clone();
        match self.anims.get_mut(g as usize).and_then(|a| a.as_mut()) {
            Some(set) => shape.set_animation_state(&ty, set, g as usize),
            None => shape.clear_animation(),
        }
    }
}

/// The effect bookkeeping of `BldgAppearance` (`activity`, `activity1`) and the calls it made since the owner took them.
/// Presentation only: not saved, not in the digest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildingFxState {
    /// The `effects.csv` row of `activity` (created by `startActivity`).
    pub activity: Option<i32>,
    /// `activity1` exists (the shape has an `activity_node1` that is not at the building's position).
    pub activity1: bool,
    /// `useNonWeaponEffects` (the owner sets it before each update).
    pub non_weapon: bool,
    /// The calls, oldest first.
    pub calls: Vec<FxCall>,
}

/// `BldgAppearance` logic state.
#[derive(Clone, Debug)]
pub struct BuildingAppearance {
    /// Index into [`super::AppearanceSystem::building_types`].
    pub type_index: usize,
    /// `bldgShape`.
    pub shape: ShapeInstance,
    /// Its source.
    pub shape_kind: BdShapeKind,
    /// `inView`.
    pub in_view: bool,
    /// `position`.
    pub position: Vec3,
    /// `rotation`.
    pub rotation: f32,
    /// `turretYaw`.
    pub turret_yaw: f32,
    /// `turretPitch`.
    pub turret_pitch: f32,
    /// `status` (initialised NORMAL: alariq's fix of an uninitialised member, bdactor.cpp:544).
    pub status: i32,
    /// `bdAnimationState` (-1 = none).
    pub current_gesture: i32,
    /// `currentFrame`.
    pub current_frame: f32,
    /// `bdFrameRate`.
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
    /// The LOD the last frame's visibility input supplied (`Visibility::structure_lod`); `None` = that input was unavailable,
    /// so `currentLOD` is only what an earlier frame selected (not a camera result of this frame).
    pub lod_input: Option<usize>,
    /// `nodeUsed`.
    pub node_used: Vec<i32>,
    /// `nodeRecycle`.
    pub node_recycle: Vec<f32>,
    /// `rotationalNodeId` (shape index; -1 unknown, -2 none).
    pub rotational_node_index: i32,
    /// `hitNodeId`.
    pub hit_node_id: i32,
    /// `isActivitying` (`startActivity`).
    pub is_activitying: bool,
    /// `OBBRadius` (`getRadius`), fixed at init.
    pub obb_radius: f32,
    /// `highZ` (`getTopZ`), fixed at init.
    pub high_z: f32,
    /// Attached-effect state, see [`BuildingFxState`].
    pub fx: BuildingFxState,
}

impl BuildingAppearance {
    /// `BldgAppearance::init(type, obj)` (bdactor.cpp:538).
    pub fn new(ty: &BuildingAppearanceType, type_index: usize) -> BuildingAppearance {
        let nw = ty.node_data.len();
        let (obb_radius, high_z) = init_box_radius(ty.shape(BdShapeKind::Lod(0)), Vec3::default());
        BuildingAppearance {
            obb_radius,
            high_z,
            type_index,
            shape: ShapeInstance::create_from(ty.shape(BdShapeKind::Lod(0))),
            shape_kind: BdShapeKind::Lod(0),
            in_view: false,
            position: Vec3::default(),
            rotation: 0.0,
            turret_yaw: 0.0,
            turret_pitch: 0.0,
            status: object_status::NORMAL,
            current_gesture: -1,
            current_frame: 0.0,
            frame_rate: 0.0,
            is_reversed: false,
            is_looping: false,
            set_first_frame: false,
            can_transition: true,
            current_lod: 0,
            lod_input: None,
            node_used: vec![0; nw],
            node_recycle: vec![0.0; nw],
            rotational_node_index: -1,
            hit_node_id: -1,
            is_activitying: false,
            fx: BuildingFxState::default(),
        }
    }

    fn recreate(&mut self, ty: &mut BuildingAppearanceType, kind: BdShapeKind) {
        self.shape = ShapeInstance::create_from(ty.shape(kind));
        self.shape_kind = kind;
        if self.current_gesture != -1 {
            ty.set_animation(&mut self.shape, kind, self.current_gesture);
        }
    }

    /// `setGesture(gestureId)` (bdactor.cpp:982): ignored when out of range, destroyed/disabled, or already playing.
    /// A random-start animation draws `RandomNumber(numFrames - 1)` from `rng` (bdactor.cpp:1016 is listed in
    /// `oracle/visual_rng_sites.txt`).
    pub fn set_gesture(&mut self, ty: &mut BuildingAppearanceType, g: u32, rng: &mut SimRng) {
        if g as usize >= MAX_BD_ANIMATIONS
            || self.status == object_status::DESTROYED
            || self.status == object_status::DISABLED
            || g as i32 == self.current_gesture
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
        self.can_transition = self.frame_rate <= STUFF_SMALL;
    }

    /// `setObjectParameters(pos, rot, ...)` (bdactor.cpp:1038).
    pub fn set_object_parameters(&mut self, pos: Vec3, rot: f32) {
        self.rotation = rot;
        self.position = pos;
    }

    /// `setMoverParameters(turretRot, lArmRot, ...)` (bdactor.cpp:1031): turret yaw and pitch.
    pub fn set_mover_parameters(&mut self, yaw: f32, pitch: f32) {
        self.turret_yaw = yaw;
        self.turret_pitch = pitch;
    }

    /// `setObjStatus(oStatus)` (bdactor.cpp:759). Note the normal branch recreates the base shape without resetting
    /// `currentLOD`.
    pub fn set_obj_status(&mut self, ty: &mut BuildingAppearanceType, o: i32) {
        if self.status != o {
            if o == object_status::DESTROYED || o == object_status::DISABLED {
                if ty.damage.is_some() {
                    self.recreate(ty, BdShapeKind::Damage);
                    self.current_lod = 0;
                }
                if self.is_activitying {
                    self.kill_activity();
                }
                self.is_activitying = false;
            }
            if o == object_status::NORMAL {
                self.recreate(ty, BdShapeKind::Lod(0));
            }
        }
        self.status = o;
    }

    /// Where the activity effects stand: `activity_node` (`activity_node1` for the second one), or the rotational node when the
    /// type has one (bdactor.cpp:2159-2178, 2301-2375).
    fn activity_pos(&mut self, ty: &BuildingAppearanceType, second: bool) -> Vec3 {
        let name = if second {
            "activity_node1"
        } else {
            "activity_node"
        };
        let mut pos = self.node_name_position(ty, name);
        self.resolve_rotational_none(ty);
        if self.rotational_node_index >= 0 {
            pos = self.node_id_position(self.rotational_node_index);
        }
        pos
    }

    /// `startActivity(effectId, loop)` (bdactor.cpp:2273): the building's activity effect (and its twin at `activity_node1`) starts
    /// once at its node.
    pub fn start_activity(&mut self, ty: &BuildingAppearanceType, effect_id: i32, looping: bool) {
        if self.fx.activity.is_none() && self.fx.non_weapon {
            self.fx.activity = Some(effect_id);
            let test = self.node_name_position(ty, "activity_node1");
            self.fx.activity1 = test != self.position;
        }
        if let (false, Some(id)) = (self.is_activitying, self.fx.activity) {
            for second in [false, true] {
                if second && !self.fx.activity1 {
                    continue;
                }
                let at = self.activity_pos(ty, second);
                self.fx.calls.push(FxCall {
                    slot: if second {
                        FxSlot::Activity1
                    } else {
                        FxSlot::Activity
                    },
                    effect: FxEffect::Id(id),
                    action: FxAction::Start {
                        place: FxPlacement::plain(at),
                        looping,
                    },
                });
            }
            self.is_activitying = true;
        }
    }

    /// The activity flag alone (`isActivitying`), as the oracle replay drives it.
    pub fn set_activity(&mut self, on: bool) {
        self.is_activitying = on;
    }

    /// `stopActivity()` (bdactor.cpp:2381).
    pub fn stop_activity(&mut self) {
        if self.is_activitying {
            self.kill_activity();
        }
        self.is_activitying = false;
    }

    fn kill_activity(&mut self) {
        if let Some(id) = self.fx.activity {
            for (slot, on) in [
                (FxSlot::Activity, true),
                (FxSlot::Activity1, self.fx.activity1),
            ] {
                if on {
                    self.fx.calls.push(FxCall {
                        slot,
                        effect: FxEffect::Id(id),
                        action: FxAction::Kill,
                    });
                }
            }
        }
    }

    /// The activity effects follow their nodes while the building is active (bdactor.cpp:2157-2255).
    fn update_attached(&mut self, ty: &BuildingAppearanceType) {
        let Some(id) = self.fx.activity.filter(|_| self.is_activitying) else {
            return;
        };
        for second in [false, true] {
            if second && !self.fx.activity1 {
                continue;
            }
            let at = self.activity_pos(ty, second);
            self.fx.calls.push(FxCall {
                slot: if second {
                    FxSlot::Activity1
                } else {
                    FxSlot::Activity
                },
                effect: FxEffect::Id(id),
                action: FxAction::Move {
                    place: FxPlacement::plain(at),
                    looping: None,
                },
            });
        }
    }

    /// `playDestruction()` (bdactor.cpp:1449): the type's `[TGLDestructEffect]` at the building, turned half a circle from it.
    /// False when the type names none.
    pub fn play_destruction(&mut self, ty: &BuildingAppearanceType) -> bool {
        if ty.destruct_effect.is_empty() {
            return false;
        }
        self.fx.calls.push(FxCall {
            slot: FxSlot::Destruct,
            effect: FxEffect::Named(ty.destruct_effect.clone()),
            action: FxAction::Start {
                place: FxPlacement {
                    at: self.position,
                    pitch: 0.0,
                    yaw: 180.0 + self.rotation,
                    base: crate::events::FxBase::Plain,
                },
                looping: false,
            },
        });
        true
    }

    /// `recalcBounds()` (bdactor.cpp:1073) with the camera results as inputs.
    pub fn recalc_bounds(
        &mut self,
        ty: &mut BuildingAppearanceType,
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
            self.recreate(ty, BdShapeKind::Lod(lod));
        }
        self.in_view
    }

    fn resolve_rotational_none(&mut self, ty: &BuildingAppearanceType) {
        if self.rotational_node_index == -1 {
            self.rotational_node_index = if !ty.rotational_node_id.eq_ignore_ascii_case("NONE") {
                ty.shape(self.shape_kind)
                    .node_name_id(&ty.rotational_node_id)
            } else {
                -2
            };
        }
    }

    /// `BldgAppearance::update(animate)` (bdactor.cpp:1872): frames always advance; the node matrices only update in
    /// view.
    pub fn update(
        &mut self,
        ty: &mut BuildingAppearanceType,
        animate: bool,
        ctx: &mut UpdateCtx,
    ) -> i32 {
        let fl = ctx.frame_length;
        for r in self.node_recycle.iter_mut() {
            if *r > 0.0 {
                *r -= fl;
                if *r < 0.0 {
                    *r = 0.0;
                }
            }
        }
        let mut xlat = [0.0f32; 3];
        let mut rot = mc2_assets::stuff::Quat::IDENTITY;
        if self.in_view {
            if ty.spin_me {
                self.rotation += SPINRATE * fl;
            }
            if self.rotation > 180.0 {
                self.rotation -= 360.0;
            }
            if self.rotation < -180.0 {
                self.rotation += 360.0;
            }
            rot = quat_from_euler(0.0, self.rotation * DEGREES_TO_RADS, 0.0);
            if ty.spin_me {
                if let Some(t) = ctx.terrain {
                    let water = t.water_elevation();
                    if self.position.z < water {
                        self.position.z = water;
                    }
                }
            }
            xlat = [-self.position.x, self.position.z, self.position.y];
            let turret_rot = quat_from_euler(
                self.turret_pitch * DEGREES_TO_RADS,
                self.turret_yaw * DEGREES_TO_RADS,
                0.0,
            );
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
        if animate && self.frame_rate != 0.0 {
            let frame_inc = self.frame_rate * fl;
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
        if self.in_view {
            self.shape
                .transform(ty.shape(self.shape_kind), &ty.anims, xlat, &rot, ctx.turn);
            if self.is_activitying {
                self.resolve_rotational_none(ty);
            }
            self.update_attached(ty);
        }
        1
    }

    fn or_position(&self, p: [f32; 3]) -> Vec3 {
        if p == [0.0, 0.0, 0.0] {
            self.position
        } else {
            Vec3 {
                x: p[0],
                y: p[1],
                z: p[2],
            }
        }
    }

    /// `getWeaponNodePosition(node)` (bdactor.cpp:454): no in-view test; applies the turret yaw (without pitch) to the
    /// turret node first.
    pub fn weapon_node_position(&mut self, ty: &BuildingAppearanceType, node: i32) -> Vec3 {
        if node < 0 || node >= ty.num_weapon_nodes() {
            return self.position;
        }
        let torso = quat_from_euler(0.0, self.turret_yaw * DEGREES_TO_RADS, 0.0);
        self.resolve_rotational_none(ty);
        if self.rotational_node_index >= 0 {
            self.shape
                .set_node_rotation(self.rotational_node_index, &torso);
        }
        let p = self.shape.node_position_by_name(
            ty.shape(self.shape_kind),
            &ty.node_data[node as usize].node_id,
        );
        self.or_position(p)
    }

    /// `getNodeNamePosition(name)` (bdactor.cpp:924).
    pub fn node_name_position(&self, ty: &BuildingAppearanceType, name: &str) -> Vec3 {
        self.or_position(
            self.shape
                .node_position_by_name(ty.shape(self.shape_kind), name),
        )
    }

    /// `getNodeIdPosition(id)` (bdactor.cpp:950).
    pub fn node_id_position(&self, id: i32) -> Vec3 {
        self.or_position(self.shape.node_position(id))
    }

    /// `getHitNode()` (bdactor.cpp:497).
    pub fn hit_node(&mut self, ty: &BuildingAppearanceType) -> Vec3 {
        if self.hit_node_id == -1 {
            self.hit_node_id = ty.shape(self.shape_kind).node_name_id("hitnode");
        }
        self.node_id_position(self.hit_node_id)
    }

    /// `getWeaponNode(type)` (bdactor.cpp:507): least used node, index from 0 (no type test).
    pub fn weapon_node(&self, ty: &BuildingAppearanceType) -> i32 {
        let mut least = 999_999_999;
        let mut best = -1;
        for i in 0..ty.num_weapon_nodes() {
            if self.node_used[i as usize] < least {
                least = self.node_used[i as usize];
                best = i;
            }
        }
        if best < 0 || best >= ty.num_weapon_nodes() {
            -1
        } else {
            best
        }
    }

    /// `setWeaponNodeUsed(node)` (bdactor.cpp:443): subtracts `numWeaponNodes` (not a smoke count), so only node ids
    /// `numWeaponNodes..2*numWeaponNodes` count.
    pub fn set_weapon_node_used(&mut self, ty: &BuildingAppearanceType, node: i32) {
        let n = node - ty.num_weapon_nodes();
        if n >= 0 && n < ty.num_weapon_nodes() {
            self.node_used[n as usize] += 1;
            self.node_recycle[n as usize] = BASE_NODE_RECYCLE_TIME;
        }
    }

    /// `getWeaponNodeRecycle(node)` (bdactor.cpp:529): 0 for non-weapon nodes.
    pub fn weapon_node_recycle(&self, ty: &BuildingAppearanceType, node: i32) -> f32 {
        if node >= 0 && node < ty.num_weapon_nodes() {
            self.node_recycle[node as usize]
        } else {
            0.0
        }
    }

    /// `hasAnimationData(gestureId)` (bdactor.h:391).
    pub fn has_animation_data(&self, ty: &BuildingAppearanceType, g: i32) -> bool {
        ty.anim(g).is_some()
    }

    /// Force the base LOD before footprint work (`calcCellsCovered`, `markTerrain`, `calcAdjCell`).
    fn force_base_lod(&mut self, ty: &mut BuildingAppearanceType) {
        if self.current_lod != 0 {
            self.current_lod = 0;
            self.recreate(ty, BdShapeKind::Lod(0));
        }
    }

    /// `calcCellsCovered(pos, cellList)` (bdactor.cpp:2460): cells under the vertices above 1 wu (forest clumps: at or
    /// below), skipping `_PAB` nodes; forces the base LOD.
    pub fn calc_cells_covered(
        &mut self,
        ty: &mut BuildingAppearanceType,
        pos: Vec3,
        map: &dyn MapCells,
    ) -> Vec<(i16, i16)> {
        self.force_base_lod(ty);
        let shape_ty = ty.shape(self.shape_kind);
        let side = map.cells_per_side();
        let mut out = Vec::new();
        for (i, node) in shape_ty.nodes.iter().enumerate() {
            if starts_with_ci(&node.id, "_PAB") {
                continue;
            }
            for j in 0..node.vertices.len() {
                let v = shape_ty.shape_vertex_in_editor(i, j, -self.rotation);
                let world = Vec3 {
                    x: pos.x + v[0],
                    y: pos.y + v[1],
                    z: pos.z + v[2],
                };
                let record = if ty.is_forest_clump {
                    v[2] <= 1.0
                } else {
                    v[2] >= 1.0
                };
                if record {
                    let (r, c) = map.world_to_cell(world);
                    if r < 0 || r >= side || c < 0 || c >= side {
                        continue;
                    }
                    out.push((r as i16, c as i16));
                }
            }
        }
        out
    }

    /// `calcAdjCell(row, col)` (bdactor.cpp:2995): only its base-LOD side effect is observable.
    pub fn calc_adj_cell(&mut self, ty: &mut BuildingAppearanceType) {
        self.force_base_lod(ty);
    }

    /// `markMoveMap(passable, lineOfSightRect, useHeight, cellList)` (bdactor.cpp:2790). Vertices go through the
    /// current node matrices (`GetShapeVertexInWorld`: identity until the building was first transformed, i.e. seen),
    /// so the marked cells depend on what the camera has shown. At a non-zero LOD the C++ builds a fresh temporary of the
    /// *same* LOD (identity matrices). With `collect`, cells are returned instead of written (the `cellList` mode).
    /// Returns (cells marked or collected, `lineOfSightRect` = min row, min col, max row, max col, collected cells).
    pub fn mark_move_map(
        &self,
        ty: &mut BuildingAppearanceType,
        passable: bool,
        collect: bool,
        map: &mut dyn MapCells,
    ) -> (i32, [i32; 4], Vec<(i16, i16)>) {
        let temp;
        let shape: &ShapeInstance = if self.current_lod != 0 {
            let mut t = ShapeInstance::create_from(ty.shape(BdShapeKind::Lod(self.current_lod)));
            if self.current_gesture != -1 {
                ty.set_animation(
                    &mut t,
                    BdShapeKind::Lod(self.current_lod),
                    self.current_gesture,
                );
            }
            temp = t;
            &temp
        } else {
            &self.shape
        };
        let shape_ty = if self.current_lod != 0 {
            ty.shape(BdShapeKind::Lod(self.current_lod))
        } else {
            ty.shape(self.shape_kind)
        };
        let side = map.cells_per_side();
        let (mut min_row, mut max_row, mut min_col, mut max_col) = (9999, 0, 9999, 0);
        let mut cells = Vec::new();
        let mut marked = 0;
        for (i, node) in shape_ty.nodes.iter().enumerate() {
            if starts_with_ci(&node.id, "_PAB") {
                continue;
            }
            for j in 0..node.vertices.len() {
                let v = shape.shape_vertex_in_world(shape_ty, i, j, -self.rotation);
                let world = Vec3 {
                    x: self.position.x + v[0],
                    y: self.position.y + v[1],
                    z: self.position.z + v[2],
                };
                let (r, c) = map.world_to_cell(world);
                if r < 0 || r >= side || c < 0 || c >= side {
                    continue;
                }
                min_row = min_row.min(r);
                max_row = max_row.max(r);
                min_col = min_col.min(c);
                max_col = max_col.max(c);
                if collect {
                    cells.push((r as i16, c as i16));
                    marked += 1;
                } else if (ty.is_forest_clump && v[2] <= 1.0)
                    || (!ty.is_forest_clump && v[2] >= 1.0)
                {
                    map.set_passable(r, c, passable);
                }
            }
        }
        (marked, [min_row, min_col, max_row, max_col], cells)
    }

    /// `markLOS(clearIt)` (bdactor.cpp:2925): raises (or clears) the cell LOS heights under the vertices, skipping `LOS_`
    /// and `SpotLight_` nodes; uses the base LOD (a temporary when another LOD is current).
    pub fn mark_los(&self, ty: &BuildingAppearanceType, clear: bool, map: &mut dyn MapCells) {
        let shape_ty = if self.current_lod != 0 {
            ty.shape(BdShapeKind::Lod(0))
        } else {
            ty.shape(self.shape_kind)
        };
        mark_los_vertices(shape_ty, self.position, self.rotation, clear, map);
    }
}

/// The vertex loop shared by `BldgAppearance::markLOS` and `TreeAppearance::markLOS` (bdactor.cpp:2938, 4255):
/// `GetShapeVertexInEditor` (no matrices), cell height `min(z * metersPerWorldUnit * 0.25, 15)`, written as `h + 0.5`
/// truncated when it exceeds the cell's.
fn mark_los_vertices(
    shape_ty: &ShapeType,
    position: Vec3,
    rotation: f32,
    clear: bool,
    map: &mut dyn MapCells,
) {
    let side = map.cells_per_side();
    for (i, node) in shape_ty.nodes.iter().enumerate() {
        if starts_with_ci(&node.id, "LOS_") || starts_with_ci(&node.id, "SpotLight_") {
            continue;
        }
        for j in 0..node.vertices.len() {
            let v = shape_ty.shape_vertex_in_editor(i, j, -rotation);
            let world = Vec3 {
                x: position.x + v[0],
                y: position.y + v[1],
                z: position.z + v[2],
            };
            let (r, c) = map.world_to_cell(world);
            if r < 0 || r >= side || c < 0 || c >= side {
                continue;
            }
            if clear {
                map.set_local_height(r, c, 0);
            } else {
                let current = map.local_height(r, c) as f32;
                let mut h = v[2] * METERS_PER_WORLD_UNIT * 0.25;
                if h > 15.0 {
                    h = 15.0;
                }
                if h > current {
                    map.set_local_height(r, c, (h + 0.5) as u32);
                }
            }
        }
    }
}

/// The `OBBRadius` / `highZ` computation of `BldgAppearance::init` and `TreeAppearance::init` (bdactor.cpp:688-728, 3348-3385):
/// the eight corners of the type's bounding box (axes swapped to simulation space) around the root node centre, at the
/// appearance position the init sees (still zero). Returns (`OBBRadius`, `highZ`), both starting at -1.
pub fn init_box_radius(shape: &ShapeType, position: Vec3) -> (f32, f32) {
    let (min, max) = shape.bounding_box();
    let nc = shape.root_node_center();
    let corner = |bx: [f32; 3], by: [f32; 3], bz: [f32; 3]| -> [f32; 3] {
        // x from box x, y from box z, z from box y (bdactor.cpp:690-713).
        [
            position.x + bx[0] + nc[0],
            position.y + by[2] + nc[2],
            position.z + bz[1] + nc[1],
        ]
    };
    let corners = [
        corner(min, min, max),
        corner(min, max, max),
        corner(max, max, max),
        corner(max, min, max),
        corner(min, min, min),
        corner(max, min, min),
        corner(max, max, min),
        corner(min, max, min),
    ];
    let mut radius = -1.0f32;
    let mut high_z = -1.0f32;
    for c in corners {
        let test = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        if radius < test {
            radius = test;
        }
        if c[2] > high_z {
            high_z = c[2];
        }
    }
    (radius, high_z)
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// `TreeAppearanceType` (bdactor.cpp:3037): shapes, damage shape, forest clump flag.
#[derive(Clone, Debug, Default)]
pub struct TreeAppearanceType {
    /// `name`.
    pub name: String,
    /// `treeShape[MAX_LODS]`.
    pub shapes: [Option<ShapeType>; MAX_LODS],
    /// `lodDistance`.
    pub lod_distance: [f32; MAX_LODS],
    /// `treeDmgShape`.
    pub damage: Option<ShapeType>,
    /// `isForestClump`.
    pub is_forest_clump: bool,
}

impl TreeAppearanceType {
    /// `TreeAppearanceType::init(fileName)` (bdactor.cpp:3037).
    pub fn load(vfs: &mut Vfs, name: &str) -> Result<TreeAppearanceType, AppearanceError> {
        let ini_path = format!("{TGL_PATH}{}.ini", name.to_ascii_lowercase());
        let mut ini = vfs.read_fit(&ini_path)?.ok_or_else(|| {
            AppearanceError(format!(
                "Could not find tree appearance INI file {ini_path}"
            ))
        })?;
        let mut t = TreeAppearanceType {
            name: name.to_string(),
            ..Default::default()
        };
        ini.seek_block("TGLData").map_err(|_| {
            AppearanceError(format!(
                "{name}: Could not find block in tree appearance INI file"
            ))
        })?;
        t.is_forest_clump = ini.read_bool("ForestClump").unwrap_or(false);
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
                "tree {name}: base shape file missing"
            )));
        }
        if ini.seek_block("TGLDamage").is_ok() {
            let file = ini
                .read_string("FileName", 511)
                .map_err(|_| AppearanceError(format!("{name}: [TGLDamage] has no FileName")))?;
            t.damage = read_shape_file(vfs, &file)?.filter(|s| !s.is_empty());
        }
        Ok(t)
    }

    fn shape(&self, kind: BdShapeKind) -> &ShapeType {
        static EMPTY: ShapeType = ShapeType { nodes: Vec::new() };
        match kind {
            BdShapeKind::Lod(i) => self.shapes.get(i).and_then(|s| s.as_ref()),
            BdShapeKind::Damage => self.damage.as_ref(),
        }
        .unwrap_or(&EMPTY)
    }
}

/// `TreeAppearance` logic state.
#[derive(Clone, Debug)]
pub struct TreeAppearance {
    /// Index into [`super::AppearanceSystem::tree_types`].
    pub type_index: usize,
    /// Source of `treeShape` (trees have no animation, so the instance matrices are not needed).
    pub shape_kind: BdShapeKind,
    /// `inView`.
    pub in_view: bool,
    /// `position`.
    pub position: Vec3,
    /// `rotation`.
    pub rotation: f32,
    /// `pitch` (`setMoverParameters`).
    pub pitch: f32,
    /// `status`.
    pub status: i32,
    /// `currentLOD`.
    pub current_lod: usize,
    /// The LOD the last frame's visibility input supplied (`Visibility::structure_lod`); `None` = that input was unavailable,
    /// so `currentLOD` is only what an earlier frame selected (not a camera result of this frame).
    pub lod_input: Option<usize>,
    /// `OBBRadius` (`getRadius`), fixed at init.
    pub obb_radius: f32,
}

impl TreeAppearance {
    /// `TreeAppearance::init` (bdactor.cpp:3221).
    pub fn new(ty: &TreeAppearanceType, type_index: usize) -> TreeAppearance {
        TreeAppearance {
            obb_radius: init_box_radius(ty.shape(BdShapeKind::Lod(0)), Vec3::default()).0,
            type_index,
            shape_kind: BdShapeKind::Lod(0),
            in_view: false,
            position: Vec3::default(),
            rotation: 0.0,
            pitch: 0.0,
            status: object_status::NORMAL,
            current_lod: 0,
            lod_input: None,
        }
    }

    /// `setObjStatus(oStatus)` (bdactor.cpp:3405).
    pub fn set_obj_status(&mut self, ty: &TreeAppearanceType, o: i32) {
        if self.status != o {
            if (o == object_status::DESTROYED || o == object_status::DISABLED)
                && ty.damage.is_some()
            {
                self.shape_kind = BdShapeKind::Damage;
            }
            if o == object_status::NORMAL {
                self.shape_kind = BdShapeKind::Lod(0);
            }
        }
        self.status = o;
    }

    /// `recalcBounds()` (bdactor.cpp:3595) with the camera results as inputs.
    pub fn recalc_bounds(&mut self, ty: &TreeAppearanceType, in_view: bool, lod: usize) -> bool {
        self.in_view = in_view;
        if in_view
            && self.status != object_status::DESTROYED
            && self.status != object_status::DISABLED
            && lod != self.current_lod
            && lod < MAX_LODS
            && ty.shapes[lod].is_some()
        {
            self.current_lod = lod;
            self.shape_kind = BdShapeKind::Lod(lod);
        }
        self.in_view
    }

    /// `markLOS(clearIt)` (bdactor.cpp:4243): forces the base LOD (recreating the shape) first.
    pub fn mark_los(&mut self, ty: &TreeAppearanceType, clear: bool, map: &mut dyn MapCells) {
        if self.current_lod != 0 {
            self.current_lod = 0;
            self.shape_kind = BdShapeKind::Lod(0);
        }
        mark_los_vertices(
            ty.shape(self.shape_kind),
            self.position,
            self.rotation,
            clear,
            map,
        );
    }
}

// ------------------------------------------------------------------------------------------------ save/load

use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};

// Static (not saved): `BuildingAppearanceType` and `TreeAppearanceType` (shapes, damage shapes, lod distances, animation
// entries and flags, node ids, forest/spin flags). The `AnimSet::shape_ids` cache of the building type is saved by the
// owner, `AppearanceSystem` (mod.rs).

impl Snapshot for BdShapeKind {
    fn save(&self, w: &mut Writer) {
        match *self {
            BdShapeKind::Lod(i) => {
                w.u8(0);
                i.save(w);
            }
            BdShapeKind::Damage => w.u8(1),
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        match r.u8()? {
            0 => Ok(BdShapeKind::Lod(usize::load(r)?)),
            1 => Ok(BdShapeKind::Damage),
            tag => Err(SaveError(format!("save: bad building shape tag {tag}"))),
        }
    }
}

// Every field is dynamic: no field is static or derived. `obb_radius` and `high_z` are set at init, but they are read
// through `getRadius`/`getTopZ` and saved as they are.
crate::snapshot_struct!(BuildingAppearance {
    type_index,
    shape,
    shape_kind,
    in_view,
    position,
    rotation,
    turret_yaw,
    turret_pitch,
    status,
    current_gesture,
    current_frame,
    frame_rate,
    is_reversed,
    is_looping,
    set_first_frame,
    can_transition,
    current_lod,
    lod_input,
    node_used,
    node_recycle,
    rotational_node_index,
    hit_node_id,
    is_activitying,
    obb_radius,
    high_z,
    fx => BuildingFxState::default(),
});

// Every field is dynamic: no field is static or derived (`obb_radius` is saved, see above).
crate::snapshot_struct!(TreeAppearance {
    type_index,
    shape_kind,
    in_view,
    position,
    rotation,
    pitch,
    status,
    current_lod,
    lod_input,
    obb_radius,
});
