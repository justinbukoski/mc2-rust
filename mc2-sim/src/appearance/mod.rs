// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! LANE F - appearance model: the animation and geometry state the simulation reads.
//!
//! The C++ keeps simulation-relevant state inside render classes (`mclib/mech3d.cpp` `Mech3DAppearance`, `gvactor.cpp`,
//! `bdactor.cpp`, `genactor.cpp`, `tgl.cpp`): gesture state machines whose current frame decides how fast a mech walks, when a
//! turret can fire or a gate is open, and node/hot-spot positions (weapon muzzles, hit nodes, `getTopZ`) that decide where
//! bolts start and what line of sight uses. None of that needs a GPU. This lane ports the *logic* of those classes headless:
//! gesture tables (`mclib/mstates.h`, [`tables`]), gesture selection (`setGestureGoal`, `checkStop`), frame advance
//! (`mechFrameRate * frameLength`), jump timing, per-gesture velocity (`StartVel/EndVel` from the mech `.ini`), and the
//! skeleton: node matrices evaluated from the TGL shapes and AGL animations exactly as `TG_MultiShape::TransformMultiShape`
//! does ([`mc2_assets::tgl_instance`]), so weapon muzzles and hit nodes are the C++ values bit for bit (DESIGN.md 14.1).
//!
//! Camera coupling (see [`crate::visibility`]): `updateGeometry()` (node positions) only runs when `turn < 3 || inView ||
//! jumping`; otherwise the node matrices keep their last values and node queries return the object position while not in
//! view (`mech3d.cpp:706-860`). `recalcBounds` is replaced by its camera results: [`AppearanceSystem::recalc_bounds`] takes
//! the final `inView` and the LOD the camera distance selected (an LOD switch recreates the shape, which changes node
//! matrices and, through the C++ index caches, even which nodes animate).
//!
//! Call order is behaviour: the API mirrors the C++ virtual calls one to one (first accepted gesture goal per frame wins,
//! getters re-apply the torso twist, `update` consumes the per-frame flags). `oracle --app-trace` records the exact call
//! sequence the C++ simulation makes on every mech appearance; `tests/appearance_replay.rs` replays it.

pub mod fx;
pub mod mech;
pub mod structure;
pub mod tables;
pub mod vehicle;

use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::rng::SimRng;
pub use mech::{MechAppearance, MechAppearanceType, UpdateCtx};
pub use structure::{
    BuildingAppearance, BuildingAppearanceType, MapCells, TreeAppearance, TreeAppearanceType,
};
pub use vehicle::{AppearanceTerrain, VehicleAppearance, VehicleAppearanceType};

/// Loading error (missing or malformed appearance data).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppearanceError(pub String);

impl std::fmt::Display for AppearanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "appearance: {}", self.0)
    }
}

impl std::error::Error for AppearanceError {}

impl From<mc2_formats::error::FormatError> for AppearanceError {
    fn from(e: mc2_formats::error::FormatError) -> Self {
        AppearanceError(e.to_string())
    }
}

impl From<mc2_assets::AssetError> for AppearanceError {
    fn from(e: mc2_assets::AssetError) -> Self {
        AppearanceError(e.0)
    }
}

/// Appearance state of one object.
#[derive(Clone, Debug)]
pub enum Appearance {
    /// `Mech3DAppearance`.
    Mech(Box<MechAppearance>),
    /// `GVAppearance` (ground vehicles; its gesture machine only runs when `!MPlayer`).
    Vehicle(Box<VehicleAppearance>),
    /// No appearance (carnage, bolts).
    None,
    /// Buildings, turrets, gates and building-type terrain objects: `BldgAppearance` ([`structure`]).
    Building(Box<BuildingAppearance>),
    /// Trees and other tree-type terrain objects: `TreeAppearance` ([`structure`]).
    Tree(Box<TreeAppearance>),
}

/// Hit direction for [`AppearanceSystem::hit`] (`hitFront`, `hitBack`, `hitLeft`, `hitRight`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitDirection {
    /// `hitFront`.
    Front,
    /// `hitBack`.
    Back,
    /// `hitLeft`.
    Left,
    /// `hitRight`.
    Right,
}

/// Appearances of all objects, indexed by [`ObjectHandle`], plus the shared appearance types (`appearanceTypeList`).
#[derive(Clone, Debug, Default)]
pub struct AppearanceSystem {
    /// Slot per handle.
    pub by_handle: Vec<Option<Appearance>>,
    /// Mech appearance types in load order (`AppearanceTypeList`, looked up by name, case-insensitive).
    pub mech_types: Vec<MechAppearanceType>,
    /// Ground vehicle appearance types (`GVAppearanceType`).
    pub vehicle_types: Vec<VehicleAppearanceType>,
    /// Building appearance types (`BldgAppearanceType`).
    pub building_types: Vec<BuildingAppearanceType>,
    /// Tree appearance types (`TreeAppearanceType`).
    pub tree_types: Vec<TreeAppearanceType>,
}

impl AppearanceSystem {
    /// `appearanceTypeList->getAppearance(MECH_TYPE << 24, name)`: the index of the mech type `name`, loading it on first
    /// use (`Mech3DAppearanceType::init`).
    pub fn mech_type(
        &mut self,
        vfs: &mut mc2_formats::vfs::Vfs,
        name: &str,
    ) -> Result<usize, AppearanceError> {
        if let Some(i) = self
            .mech_types
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(name))
        {
            return Ok(i);
        }
        let t = MechAppearanceType::load(vfs, name)?;
        self.mech_types.push(t);
        Ok(self.mech_types.len() - 1)
    }

    /// `appearance = new Mech3DAppearance; appearance->init(type, this)` (mech.cpp:1304-1310). Draws the idle timer
    /// `RandomNumber(idleMAX)` (mech3d.cpp:1023) from `sim_rng`, the SIM stream: the timer gates the stand->idle gesture
    /// switch, so `oracle/visual_rng_sites.txt` does not list it; one draw per mech in creation order.
    pub fn create_mech(&mut self, h: ObjectHandle, type_index: usize, sim_rng: &mut SimRng) {
        let a = MechAppearance::new(&mut self.mech_types[type_index], type_index, sim_rng);
        self.put(h, Appearance::Mech(Box::new(a)));
    }

    /// `appearanceTypeList->getAppearance(VEHICLE_TYPE << 24, name)` (`GVAppearanceType::init` on first use).
    pub fn vehicle_type(
        &mut self,
        vfs: &mut mc2_formats::vfs::Vfs,
        name: &str,
    ) -> Result<usize, AppearanceError> {
        if let Some(i) = self
            .vehicle_types
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(name))
        {
            return Ok(i);
        }
        let t = VehicleAppearanceType::load(vfs, name)?;
        self.vehicle_types.push(t);
        Ok(self.vehicle_types.len() - 1)
    }

    /// `appearance = new GVAppearance; appearance->init(type, this)` (gvehicl.cpp).
    pub fn create_vehicle(&mut self, h: ObjectHandle, type_index: usize) {
        let a = VehicleAppearance::new(&self.vehicle_types[type_index], type_index);
        self.put(h, Appearance::Vehicle(Box::new(a)));
    }

    /// `appearanceTypeList->getAppearance(BLDG_TYPE << 24, name)`.
    pub fn building_type(
        &mut self,
        vfs: &mut mc2_formats::vfs::Vfs,
        name: &str,
    ) -> Result<usize, AppearanceError> {
        if let Some(i) = self
            .building_types
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(name))
        {
            return Ok(i);
        }
        let t = BuildingAppearanceType::load(vfs, name)?;
        self.building_types.push(t);
        Ok(self.building_types.len() - 1)
    }

    /// `appearanceTypeList->getAppearance(TREED_TYPE << 24, name)`.
    pub fn tree_type(
        &mut self,
        vfs: &mut mc2_formats::vfs::Vfs,
        name: &str,
    ) -> Result<usize, AppearanceError> {
        if let Some(i) = self
            .tree_types
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(name))
        {
            return Ok(i);
        }
        let t = TreeAppearanceType::load(vfs, name)?;
        self.tree_types.push(t);
        Ok(self.tree_types.len() - 1)
    }

    /// `appearance = new BldgAppearance; appearance->init(type, this)`.
    pub fn create_building(&mut self, h: ObjectHandle, type_index: usize) {
        let a = BuildingAppearance::new(&self.building_types[type_index], type_index);
        self.put(h, Appearance::Building(Box::new(a)));
    }

    /// `appearance = new TreeAppearance; appearance->init(type, this)`.
    pub fn create_tree(&mut self, h: ObjectHandle, type_index: usize) {
        let a = TreeAppearance::new(&self.tree_types[type_index], type_index);
        self.put(h, Appearance::Tree(Box::new(a)));
    }

    /// Mutable building appearance plus its type.
    pub fn building_mut(
        &mut self,
        h: ObjectHandle,
    ) -> Option<(&mut BuildingAppearance, &mut BuildingAppearanceType)> {
        match self
            .by_handle
            .get_mut(h.0 as usize)
            .and_then(|a| a.as_mut())
        {
            Some(Appearance::Building(b)) => self
                .building_types
                .get_mut(b.type_index)
                .map(|t| (&mut **b, t)),
            _ => None,
        }
    }

    /// Mutable tree appearance plus its type.
    pub fn tree_mut(
        &mut self,
        h: ObjectHandle,
    ) -> Option<(&mut TreeAppearance, &TreeAppearanceType)> {
        match self
            .by_handle
            .get_mut(h.0 as usize)
            .and_then(|a| a.as_mut())
        {
            Some(Appearance::Tree(b)) => self.tree_types.get(b.type_index).map(|t| (&mut **b, t)),
            _ => None,
        }
    }

    /// Mutable vehicle appearance plus its type.
    pub fn vehicle_mut(
        &mut self,
        h: ObjectHandle,
    ) -> Option<(&mut VehicleAppearance, &mut VehicleAppearanceType)> {
        match self
            .by_handle
            .get_mut(h.0 as usize)
            .and_then(|a| a.as_mut())
        {
            // A scaffold `VehicleAppearance::default()` has no type: treated like no appearance.
            Some(Appearance::Vehicle(v)) => self
                .vehicle_types
                .get_mut(v.type_index)
                .map(|t| (&mut **v, t)),
            _ => None,
        }
    }

    /// The vehicle appearance of `h`, if it is one.
    pub fn vehicle(&self, h: ObjectHandle) -> Option<&VehicleAppearance> {
        match self.get(h) {
            Some(Appearance::Vehicle(v)) => Some(v),
            _ => None,
        }
    }

    /// Store an appearance for a handle (replacing any previous one).
    pub fn put(&mut self, h: ObjectHandle, a: Appearance) {
        let i = h.0 as usize;
        if self.by_handle.len() <= i {
            self.by_handle.resize_with(i + 1, || None);
        }
        self.by_handle[i] = Some(a);
    }

    /// Take the appearance out of its slot (leaves `None`); pair with [`AppearanceSystem::put`].
    pub fn take(&mut self, h: ObjectHandle) -> Option<Appearance> {
        self.by_handle.get_mut(h.0 as usize).and_then(|a| a.take())
    }

    /// Drop an object's appearance (`delete appearance`).
    pub fn remove(&mut self, h: ObjectHandle) {
        if let Some(slot) = self.by_handle.get_mut(h.0 as usize) {
            *slot = None;
        }
    }

    /// The appearance of `h`, if any.
    pub fn get(&self, h: ObjectHandle) -> Option<&Appearance> {
        self.by_handle.get(h.0 as usize).and_then(|a| a.as_ref())
    }

    /// The mech appearance of `h`, if it is one.
    pub fn mech(&self, h: ObjectHandle) -> Option<&MechAppearance> {
        match self.get(h) {
            Some(Appearance::Mech(m)) => Some(m),
            _ => None,
        }
    }

    /// Mutable mech appearance plus its (shared, mutable) type.
    pub fn mech_mut(
        &mut self,
        h: ObjectHandle,
    ) -> Option<(&mut MechAppearance, &mut MechAppearanceType)> {
        match self
            .by_handle
            .get_mut(h.0 as usize)
            .and_then(|a| a.as_mut())
        {
            Some(Appearance::Mech(m)) => {
                self.mech_types.get_mut(m.type_index).map(|t| (&mut **m, t))
            }
            _ => None,
        }
    }

    fn mech_ref(&self, h: ObjectHandle) -> Option<(&MechAppearance, &MechAppearanceType)> {
        self.mech(h)
            .and_then(|m| self.mech_types.get(m.type_index).map(|t| (m, t)))
    }

    // ------------------------------------------------------------------------------------------------ frame driving

    /// `appearance->update(animate)`: gesture/frame advance, jump timing, and node geometry when
    /// `turn < 3 || inView || jumping`. Returns the C++ result (1).
    pub fn update(&mut self, h: ObjectHandle, animate: bool, ctx: &mut UpdateCtx) -> i32 {
        if let Some((b, t)) = self.building_mut(h) {
            return b.update(t, animate, ctx);
        }
        if let Some((v, t)) = self.vehicle_mut(h) {
            return v.update(t, animate, ctx);
        }
        match self.mech_mut(h) {
            Some((m, t)) => m.update(t, animate, ctx),
            None => 0,
        }
    }

    /// The footprints a mech has raised since the last call (type, foot position, rotation); empty for other appearances.
    pub fn take_footprints(&mut self, h: ObjectHandle) -> Vec<(i32, Vec3, f32)> {
        match self.mech_mut(h) {
            Some((m, _)) => std::mem::take(&mut m.foot.pending),
            None => Vec::new(),
        }
    }

    /// `appearance->recalcBounds()` with the camera results as inputs: `in_view` (the final `inView`) and `lod` (the
    /// level of detail the camera distance selects, 0 for the base shape; only read when in view). Returns `inView`.
    /// Record the structure LOD input of this frame's `recalcBounds` (`None`: unavailable). Buildings, turrets, gates and trees only.
    pub fn set_structure_lod_input(&mut self, h: ObjectHandle, lod: Option<usize>) {
        match self
            .by_handle
            .get_mut(h.0 as usize)
            .and_then(|a| a.as_mut())
        {
            Some(Appearance::Building(b)) => b.lod_input = lod,
            Some(Appearance::Tree(t)) => t.lod_input = lod,
            _ => {}
        }
    }

    /// The structure LOD input recorded by the last [`Self::set_structure_lod_input`] (`None`: unavailable or no appearance).
    pub fn structure_lod_input(&self, h: ObjectHandle) -> Option<usize> {
        match self.get(h) {
            Some(Appearance::Building(b)) => b.lod_input,
            Some(Appearance::Tree(t)) => t.lod_input,
            _ => None,
        }
    }

    pub fn recalc_bounds(&mut self, h: ObjectHandle, in_view: bool, lod: usize) -> bool {
        if let Some((b, t)) = self.building_mut(h) {
            return b.recalc_bounds(t, in_view, lod);
        }
        if let Some((b, t)) = self.tree_mut(h) {
            return b.recalc_bounds(t, in_view, lod);
        }
        if let Some((v, t)) = self.vehicle_mut(h) {
            return v.recalc_bounds(t, in_view, lod);
        }
        match self.mech_mut(h) {
            Some((m, t)) => m.recalc_bounds(t, in_view, lod),
            None => {
                self.set_in_view(h, in_view);
                in_view
            }
        }
    }

    /// `setInView(viewStatus)`.
    pub fn set_in_view(&mut self, h: ObjectHandle, in_view: bool) {
        match self
            .by_handle
            .get_mut(h.0 as usize)
            .and_then(|a| a.as_mut())
        {
            Some(Appearance::Mech(m)) => m.in_view = in_view,
            Some(Appearance::Vehicle(v)) => v.in_view = in_view,
            Some(Appearance::Building(b)) => b.in_view = in_view,
            Some(Appearance::Tree(b)) => b.in_view = in_view,
            _ => {}
        }
    }

    /// `canBeSeen()` (`inView`).
    pub fn in_view(&self, h: ObjectHandle) -> bool {
        match self.get(h) {
            Some(Appearance::Mech(m)) => m.in_view,
            Some(Appearance::Vehicle(v)) => v.in_view,
            Some(Appearance::Building(b)) => b.in_view,
            Some(Appearance::Tree(b)) => b.in_view,
            _ => false,
        }
    }

    /// `setObjectParameters(pos, rot, selected, team, homeRelations)` (only position and rotation matter).
    pub fn set_object_parameters(&mut self, h: ObjectHandle, pos: Vec3, rot: f32) {
        if let Some((b, _)) = self.building_mut(h) {
            b.set_object_parameters(pos, rot);
            return;
        }
        if let Some((b, _)) = self.tree_mut(h) {
            b.position = pos;
            b.rotation = rot;
            return;
        }
        if let Some((v, _)) = self.vehicle_mut(h) {
            v.set_object_parameters(pos, rot);
        } else if let Some((m, _)) = self.mech_mut(h) {
            m.set_object_parameters(pos, rot);
        }
    }

    /// `setMoverParameters(turretRot, lArmRot, rArmRot, isAirborne)`.
    pub fn set_mover_parameters(
        &mut self,
        h: ObjectHandle,
        turret_rot: f32,
        l_arm: f32,
        r_arm: f32,
        airborne: bool,
    ) {
        if let Some((b, _)) = self.building_mut(h) {
            b.set_mover_parameters(turret_rot, l_arm);
            return;
        }
        if let Some((b, _)) = self.tree_mut(h) {
            b.pitch = turret_rot;
            return;
        }
        if let Some((v, _)) = self.vehicle_mut(h) {
            v.set_mover_parameters(turret_rot, l_arm, r_arm, airborne);
        } else if let Some((m, _)) = self.mech_mut(h) {
            m.set_mover_parameters(turret_rot, l_arm, r_arm, airborne);
        }
    }

    /// `setObjStatus(status)`.
    pub fn set_obj_status(&mut self, h: ObjectHandle, status: i32) {
        if let Some((b, t)) = self.building_mut(h) {
            b.set_obj_status(t, status);
            return;
        }
        if let Some((b, t)) = self.tree_mut(h) {
            b.set_obj_status(t, status);
            return;
        }
        if let Some((v, t)) = self.vehicle_mut(h) {
            v.set_obj_status(t, status);
        } else if let Some((m, t)) = self.mech_mut(h) {
            m.set_obj_status(t, status);
        }
    }

    /// The C++ `appearance->setObjStatus(getStatus())` hand-over of `BattleMech::update` (mech.cpp:6229-6233, 6282-6283) into a mech
    /// appearance's [`MechAppearance::cpp_status`]; other appearances have no such field.
    pub fn set_mech_cpp_status(&mut self, h: ObjectHandle, status: i32) {
        if let Some((m, _)) = self.mech_mut(h) {
            m.cpp_status = status;
        }
    }

    // ------------------------------------------------------------------------------------------------ gestures

    /// `setGestureGoal(stateGoal)` ([`tables::state`]): -1 accepted, 0 ignored (first accepted goal per frame wins;
    /// ignored during hit gestures, while a goal is in progress, and for jumping without `setJumpParameters`).
    pub fn set_gesture_goal(&mut self, h: ObjectHandle, goal: i32) -> i32 {
        match self.mech_mut(h) {
            Some((m, _)) => m.set_gesture_goal(goal),
            None => 0,
        }
    }

    /// `setGesture(gestureId)` (forces a gesture; runs an in-view `update` first).
    pub fn set_gesture(&mut self, h: ObjectHandle, g: u32, ctx: &mut UpdateCtx) {
        if let Some((b, t)) = self.building_mut(h) {
            match ctx.gesture_rng.as_deref_mut() {
                Some(rng) => b.set_gesture(t, g, rng),
                None => b.set_gesture(t, g, ctx.sim_rng),
            }
            return;
        }
        if let Some((v, t)) = self.vehicle_mut(h) {
            match ctx.gesture_rng.as_deref_mut() {
                Some(rng) => v.set_gesture(t, g, rng),
                None => v.set_gesture(t, g, ctx.sim_rng),
            }
        } else if let Some((m, t)) = self.mech_mut(h) {
            m.set_gesture(t, g, ctx);
        }
    }

    /// `setBrake(brake)`.
    pub fn set_brake(&mut self, h: ObjectHandle, brake: bool) {
        if let Some((m, _)) = self.mech_mut(h) {
            m.set_brake(brake);
        }
    }

    /// `setCombatMode(combat)`.
    pub fn set_combat_mode(&mut self, h: ObjectHandle, combat: bool) {
        if let Some((m, _)) = self.mech_mut(h) {
            m.set_combat_mode(combat);
        }
    }

    /// `hitFront/hitBack/hitLeft/hitRight`.
    pub fn hit(&mut self, h: ObjectHandle, dir: HitDirection) {
        if let Some((m, t)) = self.mech_mut(h) {
            let g = match dir {
                HitDirection::Front => tables::gesture::HIT_FRONT,
                HitDirection::Back => tables::gesture::HIT_BACK,
                HitDirection::Left => tables::gesture::HIT_LEFT,
                HitDirection::Right => tables::gesture::HIT_RIGHT,
            };
            m.hit(t, g);
        }
    }

    /// `setJumpParameters(end)`: false while already jumping.
    pub fn set_jump_parameters(&mut self, h: ObjectHandle, end: Vec3) -> bool {
        match self.mech_mut(h) {
            Some((m, _)) => m.set_jump_parameters(end),
            None => false,
        }
    }

    /// The effect calls an appearance has queued since the last call (mechs for now).
    pub fn take_fx_calls(&mut self, h: ObjectHandle) -> Vec<fx::FxCall> {
        if let Some((v, _)) = self.vehicle_mut(h) {
            return std::mem::take(&mut v.fx.calls);
        }
        if let Some((b, _)) = self.building_mut(h) {
            return std::mem::take(&mut b.fx.calls);
        }
        match self.mech_mut(h) {
            Some((m, _)) => std::mem::take(&mut m.fx.calls),
            None => Vec::new(),
        }
    }

    /// After a save is read: the effect state is presentation and is not saved, so an activity effect the save says is running
    /// is started again by its owner's next update (`start_activity` does nothing while `is_activitying` is set).
    pub fn reset_fx_after_load(&mut self) {
        for a in self.by_handle.iter_mut().flatten() {
            if let Appearance::Building(b) = a {
                b.is_activitying = false;
            }
        }
    }

    /// `startWaterWake()` / `stopWaterWake()` for a mech or a vehicle.
    pub fn set_wake(&mut self, h: ObjectHandle, wading: bool, level: f32, non_weapon: bool) {
        if let Some((v, _)) = self.vehicle_mut(h) {
            v.fx.non_weapon = non_weapon;
            v.set_wake(wading, level);
            return;
        }
        if let Some((m, _)) = self.mech_mut(h) {
            m.fx.non_weapon = non_weapon;
            if wading {
                m.start_water_wake(level);
            } else {
                m.stop_water_wake();
            }
        }
    }

    /// `blowLeftArm()` / `blowRightArm()`: six VISUAL draws.
    pub fn blow_arm(&mut self, h: ObjectHandle, left: bool, visual_rng: &mut SimRng) {
        if let Some((m, t)) = self.mech_mut(h) {
            m.blow_arm(t, left, visual_rng);
        }
    }

    /// `getCurrentGestureId()` (0 for appearances without gestures).
    pub fn current_gesture_id(&self, h: ObjectHandle) -> i32 {
        match self.get(h) {
            Some(Appearance::Mech(m)) => m.current_gesture,
            Some(Appearance::Vehicle(v)) => v.current_gesture,
            Some(Appearance::Building(b)) => b.current_gesture,
            _ => 0,
        }
    }

    /// `getOldGestureGoal()`: `Mech3DAppearance` does not override it, so it is -1 for every appearance.
    pub fn old_gesture_goal(&self, _h: ObjectHandle) -> i32 {
        -1
    }

    /// `haveFallen()`.
    pub fn have_fallen(&self, h: ObjectHandle) -> bool {
        self.mech(h).is_some_and(|m| m.fallen)
    }

    /// `isInJump()`.
    pub fn is_in_jump(&self, h: ObjectHandle) -> bool {
        self.mech(h).is_some_and(|m| m.in_jump)
    }

    /// `isJumpSetup()`.
    pub fn is_jump_setup(&self, h: ObjectHandle) -> bool {
        self.mech(h).is_some_and(|m| m.jump_setup)
    }

    /// `isJumpAirborne()`.
    pub fn is_jump_airborne(&self, h: ObjectHandle) -> bool {
        self.mech(h).is_some_and(|m| m.jump_airborne)
    }

    /// `getVelocity()`: the jump velocity vector (zero on the ground).
    pub fn velocity(&self, h: ObjectHandle) -> Vec3 {
        self.mech(h).map_or_else(Vec3::default, |m| m.jump_velocity)
    }

    /// `getVelocityMagnitude()` in metres per second: the current gesture's speed, interpolated `StartVel..EndVel` by
    /// frame; 0 while airborne (the C++ `velocity` member is never assigned).
    pub fn velocity_magnitude(&self, h: ObjectHandle) -> f32 {
        self.mech_ref(h)
            .map_or(0.0, |(m, t)| m.velocity_magnitude(t))
    }

    /// `appearance->getVelocityOfGesture(g)` *as the simulation calls it*: through `Appearance*`, whose
    /// `getVelocityOfGesture(long)` `Mech3DAppearance` does not override (its own takes `unsigned long`), so the result is
    /// the base class's 0 (`BattleMech::calcMaxSpeed`, `calcSpriteSpeed`, mech.cpp:8814-8860).
    pub fn velocity_of_gesture(&self, _h: ObjectHandle, _g: i32) -> f32 {
        0.0
    }

    /// `Mech3DAppearance::getVelocityOfGesture(g)` (the mech's own, used inside the appearance).
    pub fn mech_velocity_of_gesture(&self, h: ObjectHandle, g: i32) -> f32 {
        self.mech_ref(h)
            .map_or(0.0, |(m, t)| m.velocity_of_gesture(t, g))
    }

    // ------------------------------------------------------------------------------------------------ nodes

    /// `getWeaponNodePosition(node)`: the muzzle position, or the object position when not in view / not a weapon node.
    pub fn weapon_node_position(&mut self, h: ObjectHandle, node: i32) -> Vec3 {
        if let Some((b, t)) = self.building_mut(h) {
            return b.weapon_node_position(t, node);
        }
        if let Some((v, t)) = self.vehicle_mut(h) {
            return v.weapon_node_position(t, node);
        }
        match self.mech_mut(h) {
            Some((m, t)) => m.weapon_node_position(t, node),
            None => Vec3::default(),
        }
    }

    /// `getNodePosition(nodeId)` (node table index).
    pub fn node_position(&mut self, h: ObjectHandle, node: i32) -> Vec3 {
        match self.mech_mut(h) {
            Some((m, t)) => m.node_position(t, node),
            None => Vec3::default(),
        }
    }

    /// `getNodeNamePosition(name)`.
    pub fn node_name_position(&mut self, h: ObjectHandle, name: &str) -> Vec3 {
        if let Some((b, t)) = self.building_mut(h) {
            return b.node_name_position(t, name);
        }
        if let Some((v, t)) = self.vehicle_mut(h) {
            return v.node_name_position(t, name);
        }
        match self.mech_mut(h) {
            Some((m, t)) => m.node_name_position(t, name),
            None => Vec3::default(),
        }
    }

    /// `getHitNodeLeft()` / `getHitNodeRight()`.
    pub fn hit_node_side(&mut self, h: ObjectHandle, left: bool) -> Vec3 {
        match self.mech_mut(h) {
            Some((m, t)) => m.hit_node(t, left),
            None => Vec3::default(),
        }
    }

    /// `getLowestWeaponNode()`.
    pub fn lowest_weapon_node(&mut self, h: ObjectHandle) -> i32 {
        if let Some((v, t)) = self.vehicle_mut(h) {
            return v.lowest_weapon_node(t);
        }
        match self.mech_mut(h) {
            Some((m, t)) => m.lowest_weapon_node(t),
            None => -1,
        }
    }

    /// `getWeaponNode(weaponType)` (marks the node used).
    pub fn weapon_node(&mut self, h: ObjectHandle, weapon_type: i32) -> i32 {
        if let Some((b, t)) = self.building_mut(h) {
            return b.weapon_node(t);
        }
        if let Some((v, t)) = self.vehicle_mut(h) {
            return v.weapon_node(t, weapon_type);
        }
        match self.mech_mut(h) {
            Some((m, t)) => m.weapon_node(t, weapon_type),
            None => -1,
        }
    }

    /// `getWeaponNodeRecycle(node)` (9999 for non-weapon nodes).
    pub fn weapon_node_recycle(&self, h: ObjectHandle, node: i32) -> f32 {
        if let Some(Appearance::Building(b)) = self.get(h) {
            return b.weapon_node_recycle(&self.building_types[b.type_index], node);
        }
        if let Some(v) = self.vehicle(h) {
            return v.weapon_node_recycle(node);
        }
        self.mech_ref(h)
            .map_or(9999.0, |(m, t)| m.weapon_node_recycle(t, node))
    }

    /// `setWeaponNodeUsed(node)`.
    pub fn set_weapon_node_used(&mut self, h: ObjectHandle, node: i32) {
        if let Some((b, t)) = self.building_mut(h) {
            b.set_weapon_node_used(t, node);
            return;
        }
        if let Some((v, _)) = self.vehicle_mut(h) {
            v.set_weapon_node_used(node);
        } else if let Some((m, t)) = self.mech_mut(h) {
            m.set_weapon_node_used(t, node);
        }
    }

    /// `setWeaponNodeRecycle(node, time)`.
    pub fn set_weapon_node_recycle(&mut self, h: ObjectHandle, node: i32, time: f32) {
        if let Some((m, t)) = self.mech_mut(h) {
            m.set_weapon_node_recycle(t, node, time);
        }
    }

    /// `resetWeaponNodes()`.
    pub fn reset_weapon_nodes(&mut self, h: ObjectHandle) {
        if let Some((m, _)) = self.mech_mut(h) {
            m.reset_weapon_nodes();
        }
    }

    /// `getHitNode()`: the vehicle `hitnode`; zero for mechs (base class).
    pub fn hit_node(&mut self, h: ObjectHandle) -> Vec3 {
        if let Some((b, t)) = self.building_mut(h) {
            return b.hit_node(t);
        }
        match self.vehicle_mut(h) {
            Some((v, t)) => v.hit_node(t),
            None => Vec3::default(),
        }
    }

    /// `getNodeIdPosition(id)` (shape index).
    pub fn node_id_position(&mut self, h: ObjectHandle, id: i32) -> Vec3 {
        if let Some((b, _)) = self.building_mut(h) {
            return b.node_id_position(id);
        }
        if let Some((v, t)) = self.vehicle_mut(h) {
            return v.node_id_position(t, id);
        }
        match self.mech_mut(h) {
            Some((m, t)) => m.node_id_position(t, id),
            None => Vec3::default(),
        }
    }

    /// `getSmokeNodePosition(node)` (vehicles; zero otherwise).
    pub fn smoke_node_position(&mut self, h: ObjectHandle, node: i32) -> Vec3 {
        match self.vehicle_mut(h) {
            Some((v, t)) => v.smoke_node_position(t, node),
            None => Vec3::default(),
        }
    }

    /// `getDustNodePosition(node)` (vehicles; zero otherwise).
    pub fn dust_node_position(&mut self, h: ObjectHandle, node: i32) -> Vec3 {
        match self.vehicle_mut(h) {
            Some((v, t)) => v.dust_node_position(t, node),
            None => Vec3::default(),
        }
    }

    /// `getInTransition()`: a vehicle animation that has not finished one cycle; false for mechs (base class).
    pub fn in_transition(&self, h: ObjectHandle) -> bool {
        match self.get(h) {
            Some(Appearance::Vehicle(v)) => !v.can_transition,
            Some(Appearance::Building(b)) => !b.can_transition,
            _ => false,
        }
    }

    /// `playDestruction()`: whether the appearance plays its own destruction effect (`effect_exists` answers whether the
    /// effect library has a named effect); false for mechs (base class).
    pub fn play_destruction(&self, h: ObjectHandle, effect_exists: &dyn Fn(&str) -> bool) -> bool {
        if let Some(Appearance::Building(b)) = self.get(h) {
            let t = &self.building_types[b.type_index];
            return !t.destruct_effect.is_empty() && effect_exists(&t.destruct_effect);
        }
        match self.vehicle(h) {
            Some(v) => self
                .vehicle_types
                .get(v.type_index)
                .is_some_and(|t| v.play_destruction(t, effect_exists)),
            None => false,
        }
    }

    /// `hasAnimationData(gestureId)` (buildings; false otherwise, base class).
    pub fn has_animation_data(&self, h: ObjectHandle, g: i32) -> bool {
        match self.get(h) {
            Some(Appearance::Building(b)) => {
                b.has_animation_data(&self.building_types[b.type_index], g)
            }
            _ => false,
        }
    }

    /// `startActivity` / `stopActivity` reduced to the building's activity flag (the oracle replay drives it directly).
    pub fn set_activity(&mut self, h: ObjectHandle, on: bool) {
        if let Some((b, _)) = self.building_mut(h) {
            b.set_activity(on);
        }
    }

    /// `startActivity(effectId, loop)` / `stopActivity()` (buildings and vehicles: the activity effect, plus the building's
    /// activity flag). `effect_id` is the `effects.csv` row (`None` to stop).
    pub fn set_activity_effect(&mut self, h: ObjectHandle, effect_id: Option<i32>, looping: bool) {
        if let Some((v, t)) = self.vehicle_mut(h) {
            match effect_id {
                Some(id) => v.start_activity(t, id, looping),
                None => v.stop_activity(),
            }
        } else if let Some((b, t)) = self.building_mut(h) {
            match effect_id {
                Some(id) => b.start_activity(t, id, looping),
                None => b.stop_activity(),
            }
        }
    }

    /// `markMoveMap(passable, lineOfSightRect, false, cellList)`: see [`BuildingAppearance::mark_move_map`]. Returns
    /// (cells counted, line-of-sight rectangle, collected cells); (0, default, empty) for other appearances.
    pub fn mark_move_map(
        &mut self,
        h: ObjectHandle,
        passable: bool,
        collect: bool,
        map: &mut dyn MapCells,
    ) -> (i32, [i32; 4], Vec<(i16, i16)>) {
        match self.building_mut(h) {
            Some((b, t)) => b.mark_move_map(t, passable, collect, map),
            None => (0, [9999, 9999, 0, 0], Vec::new()),
        }
    }

    /// `markLOS(clearIt)` for buildings and trees.
    pub fn mark_los(&mut self, h: ObjectHandle, clear: bool, map: &mut dyn MapCells) {
        if let Some((b, t)) = self.building_mut(h) {
            b.mark_los(t, clear, map);
        } else if let Some((b, t)) = self.tree_mut(h) {
            b.mark_los(t, clear, map);
        }
    }

    /// `calcCellsCovered(pos, cellList)` (buildings; forces the base LOD).
    pub fn calc_cells_covered(
        &mut self,
        h: ObjectHandle,
        pos: Vec3,
        map: &dyn MapCells,
    ) -> Vec<(i16, i16)> {
        match self.building_mut(h) {
            Some((b, t)) => b.calc_cells_covered(t, pos, map),
            None => Vec::new(),
        }
    }

    /// `calcAdjCell(row, col)` (buildings; only forces the base LOD).
    pub fn calc_adj_cell(&mut self, h: ObjectHandle) {
        if let Some((b, t)) = self.building_mut(h) {
            b.calc_adj_cell(t);
        }
    }

    /// `getLeftArmOff()` / `getRightArmOff()`.
    pub fn arm_off(&self, h: ObjectHandle, left: bool) -> bool {
        self.mech(h).is_some_and(|m| {
            if left {
                m.left_arm_off
            } else {
                m.right_arm_off
            }
        })
    }

    /// `getTopZ()`: a building's `highZ` (computed at init from the shape box; code/bldng.h:237 and code/terrobj.h:294 put
    /// LOS hot spots at half of it), 0 for every other appearance (base class).
    pub fn top_z(&self, h: ObjectHandle) -> f32 {
        match self.get(h) {
            Some(Appearance::Building(b)) => b.high_z,
            _ => 0.0,
        }
    }

    /// `getRadius()`: `OBBRadius` for buildings and trees (computed at init from the shape box; code/bldng.h:316,
    /// code/terrobj.h:257), 0 for movers (base class).
    pub fn radius(&self, h: ObjectHandle) -> f32 {
        match self.get(h) {
            Some(Appearance::Building(b)) => b.obb_radius,
            Some(Appearance::Tree(t)) => t.obb_radius,
            _ => 0.0,
        }
    }
}

// ------------------------------------------------------------------------------------------------ world entry points
//
// CROSS-LANE (scaffold review round 1 made these free functions on the World): thin wrappers over the AppearanceSystem
// methods above, which borrow `world.appearance` next to the clock and the RNG. Lanes may call either form.

/// `appearance->update(animate)` with the world's clock (`turn`, `frameLength`) and SIM stream.
pub fn update(w: &mut crate::World, h: ObjectHandle, animate: bool) -> i32 {
    fx::sync_preference(w, h);
    let result = {
        let mut streams = w.rng.0.borrow_mut();
        let streams = &mut *streams;
        let mut ctx = UpdateCtx {
            turn: w.clock.turn,
            frame_length: w.clock.frame_length,
            sim_rng: &mut streams.sim,
            gesture_rng: Some(&mut streams.visual),
            terrain: Some(&w.terrain),
        };
        w.appearance.update(h, animate, &mut ctx)
    };
    // `craterManager->addCrater(footprintType, footPosition, rotation)` for the feet that came down (mech3d.cpp:3217)
    let prints = w.appearance.take_footprints(h);
    fx::flush(w, h);
    if w.objects.use_non_weapon_effects {
        for (kind, at, rotation) in prints {
            w.events
                .push(crate::SimEvent::Crater { kind, at, rotation });
        }
    }
    result
}

/// `appearance = new Mech3DAppearance; appearance->init(type, this)`: the idle timer draw comes from the world's SIM stream.
pub fn create_mech(w: &mut crate::World, h: ObjectHandle, type_index: usize) {
    let mut streams = w.rng.0.borrow_mut();
    w.appearance.create_mech(h, type_index, &mut streams.sim);
}

/// `appearance->recalcBounds()` with the camera's results (`in_view`, `lod`) as inputs.
pub fn recalc_bounds(w: &mut crate::World, h: ObjectHandle, in_view: bool, lod: usize) -> bool {
    w.appearance.recalc_bounds(h, in_view, lod)
}

/// `appearance->setGestureGoal(goal)`: -1 accepted, 0 ignored (first accepted goal per frame wins).
pub fn set_gesture_goal(w: &mut crate::World, h: ObjectHandle, goal: i32) -> i32 {
    w.appearance.set_gesture_goal(h, goal)
}

/// `appearance->getVelocityMagnitude()` (metres per second).
pub fn velocity_magnitude(w: &crate::World, h: ObjectHandle) -> f32 {
    w.appearance.velocity_magnitude(h)
}

/// `appearance->getWeaponNodePosition(node)` (object position when not in view).
pub fn weapon_node_position(w: &mut crate::World, h: ObjectHandle, node: i32) -> Vec3 {
    w.appearance.weapon_node_position(h, node)
}

/// `appearance->getNodePosition(node)`.
pub fn node_position(w: &mut crate::World, h: ObjectHandle, node: i32) -> Vec3 {
    w.appearance.node_position(h, node)
}

/// `appearance->getTopZ()` (buildings: `highZ`; 0 otherwise).
pub fn top_z(w: &crate::World, h: ObjectHandle) -> f32 {
    w.appearance.top_z(h)
}

/// `appearance->getRadius()`.
pub fn radius(w: &crate::World, h: ObjectHandle) -> f32 {
    w.appearance.radius(h)
}

/// `appearance->setGesture(g)`: a vehicle's random start frame draws from the VISUAL stream (gvactor.cpp:1383 is listed in
/// `oracle/visual_rng_sites.txt`); a mech runs an in-view update.
pub fn set_gesture(w: &mut crate::World, h: ObjectHandle, g: u32) {
    let mut streams = w.rng.0.borrow_mut();
    let streams = &mut *streams;
    let mut ctx = UpdateCtx {
        turn: w.clock.turn,
        frame_length: w.clock.frame_length,
        sim_rng: &mut streams.sim,
        gesture_rng: Some(&mut streams.visual),
        terrain: Some(&w.terrain),
    };
    w.appearance.set_gesture(h, g, &mut ctx)
}

/// Lane T supplies the vehicle pitch/roll terrain: `MapData::terrainAngle`'s normal (mapdata.cpp:1070) on dry cells. Where
/// `terrainAngle` writes no normal, the normal is the caller's zero `Vector3D` (see `terrain::Terrain::angle_normal`).
impl AppearanceTerrain for crate::terrain::Terrain {
    fn terrain_angle_normal(&self, position: Vec3) -> Option<Vec3> {
        let (r, c) = self.world_to_cell(&position);
        let map = self.map();
        let cell = map.cell(r, c);
        if cell.deep_water() || cell.shallow_water() {
            None
        } else {
            Some(self.angle_normal(&position))
        }
    }
    fn water_elevation(&self) -> f32 {
        self.water_elevation
    }
    fn elevation(&self, position: Vec3) -> f32 {
        crate::terrain::Terrain::elevation(self, &position)
    }
}

/// The appearance half of the turret firing gate in `Turret::updateAnimations` (code/turret.cpp:444-475), for an awake
/// turret with a target: open (gesture 2), or opening (gesture 1) with the pop-up animation finished. A closed (0), closing
/// (3) or not yet updated (-1) turret cannot fire. The awake/target test and the gesture switches (`setGesture(0/1/2/3)`)
/// stay with the caller (lane A's turret update), which must call this before it switches gestures.
pub fn can_fire(w: &crate::World, h: ObjectHandle) -> bool {
    match w.appearance.current_gesture_id(h) {
        2 => true,
        1 => !w.appearance.in_transition(h),
        _ => false,
    }
}

// ------------------------------------------------------------------------------------------------ save/load
//
// Overlay: `AppearanceSystem::load_into` runs on a freshly initialised base world of the same mission. The type lists are
// static: the save carries their names (a check) and the run-time caches that live inside the types (see below); the base
// keeps the types themselves. Save order: four name lists, `by_handle`, mech type caches, vehicle and building animation
// caches. `load_into` reads in the same order and fails with a message on any mismatch (the world is then half-overlaid and
// must be discarded).

use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};
use mc2_assets::stuff::{Mat, Quat};
use mc2_assets::tgl_instance::{AnimSet, ShapeInstance, ShapeRec};

// Foreign types (the trait is local). Every field of the node state is dynamic.
impl Snapshot for Mat {
    fn save(&self, w: &mut Writer) {
        self.0.save(w);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(Mat(Snapshot::load(r)?))
    }
}

crate::snapshot_struct!(Quat { x, y, z, w });
crate::snapshot_struct!(ShapeRec {
    local,
    shape_to_world,
    calced,
    anim,
    base_rotation,
    process_me,
});
crate::snapshot_struct!(ShapeInstance { recs, frame_num });
impl Snapshot for Appearance {
    fn save(&self, w: &mut Writer) {
        match self {
            Appearance::Mech(a) => {
                w.u8(0);
                a.save(w);
            }
            Appearance::Vehicle(a) => {
                w.u8(1);
                a.save(w);
            }
            Appearance::None => w.u8(3),
            Appearance::Building(a) => {
                w.u8(4);
                a.save(w);
            }
            Appearance::Tree(a) => {
                w.u8(5);
                a.save(w);
            }
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(match r.u8()? {
            0 => Appearance::Mech(Snapshot::load(r)?),
            1 => Appearance::Vehicle(Snapshot::load(r)?),
            3 => Appearance::None,
            4 => Appearance::Building(Snapshot::load(r)?),
            5 => Appearance::Tree(Snapshot::load(r)?),
            tag => return Err(SaveError(format!("save: bad appearance tag {tag}"))),
        })
    }
}

/// Names of a type list, in order.
fn type_names<'a>(names: impl Iterator<Item = &'a String>) -> Vec<String> {
    names.cloned().collect()
}

/// Reads a saved name list and fails unless it equals the base world's list (same names, same order).
fn expect_types(what: &str, base: &[String], r: &mut Reader<'_>) -> SaveResult<()> {
    let saved = Vec::<String>::load(r)?;
    if saved.as_slice() == base {
        return Ok(());
    }
    let first = saved
        .iter()
        .zip(base)
        .position(|(a, b)| a != b)
        .unwrap_or(saved.len().min(base.len()));
    Err(SaveError(format!(
        "save: {what} type list differs from the base world (save has {} types, base has {}); first difference at index \
         {first}: save {:?}, base {:?}. The base must be the same mission, loaded in the same order.",
        saved.len(),
        base.len(),
        saved.get(first),
        base.get(first),
    )))
}

/// `AnimSet::shape_ids`: the node-index cache that `SetAnimationState` fills on the first call ever on a set (msl.cpp:2512),
/// so it depends on run history and is saved. The table's presence pattern and length come from the static data (checked).
fn save_anim_caches(anims: &[Option<AnimSet>], w: &mut Writer) {
    w.len(anims.len());
    for a in anims {
        match a {
            None => w.u8(0),
            Some(set) => {
                w.u8(1);
                set.shape_ids.save(w);
            }
        }
    }
}

fn load_anim_caches(anims: &mut [Option<AnimSet>], r: &mut Reader<'_>) -> SaveResult<()> {
    let n = r.length()?;
    if n != anims.len() {
        return Err(SaveError(format!(
            "save: animation table has {n} entries, the base world has {}",
            anims.len()
        )));
    }
    for a in anims.iter_mut() {
        match (r.u8()?, a.as_mut()) {
            (0, None) => {}
            (1, Some(set)) => set.shape_ids = Snapshot::load(r)?,
            _ => {
                return Err(SaveError(
                    "save: animation presence differs from the base world".into(),
                ))
            }
        }
    }
    Ok(())
}

/// `node_data` arm flags: `Mech3DAppearance::init` rewrites them for every instance (mech3d.cpp:1207-1216). Their values are
/// a function of the static shapes, but they are saved and checked like the caches above.
fn save_mech_caches(t: &MechAppearanceType, w: &mut Writer) {
    save_anim_caches(&t.anims, w);
    w.len(t.node_data.len());
    for n in &t.node_data {
        n.is_r_arm_node.save(w);
        n.is_l_arm_node.save(w);
    }
}

fn load_mech_caches(t: &mut MechAppearanceType, r: &mut Reader<'_>) -> SaveResult<()> {
    load_anim_caches(&mut t.anims, r)?;
    let n = r.length()?;
    if n != t.node_data.len() {
        return Err(SaveError(format!(
            "save: mech {} has {n} node entries, the base world has {}",
            t.name,
            t.node_data.len()
        )));
    }
    for node in t.node_data.iter_mut() {
        node.is_r_arm_node = Snapshot::load(r)?;
        node.is_l_arm_node = Snapshot::load(r)?;
    }
    Ok(())
}

impl Snapshot for AppearanceSystem {
    fn save(&self, w: &mut Writer) {
        type_names(self.mech_types.iter().map(|t| &t.name)).save(w);
        type_names(self.vehicle_types.iter().map(|t| &t.name)).save(w);
        type_names(self.building_types.iter().map(|t| &t.name)).save(w);
        type_names(self.tree_types.iter().map(|t| &t.name)).save(w);
        self.by_handle.save(w);
        for t in &self.mech_types {
            save_mech_caches(t, w);
        }
        for t in &self.vehicle_types {
            save_anim_caches(&t.anims, w);
        }
        for t in &self.building_types {
            save_anim_caches(&t.anims, w);
        }
    }

    /// Not an overlay: the static type lists cannot come from the save. Use [`AppearanceSystem::load_into`].
    fn load(_r: &mut Reader<'_>) -> SaveResult<Self> {
        Err(SaveError(
            "save: AppearanceSystem cannot be loaded without its static type lists; use load_into on a base world".into(),
        ))
    }

    fn load_into(&mut self, r: &mut Reader<'_>) -> SaveResult<()> {
        expect_types(
            "mech",
            &type_names(self.mech_types.iter().map(|t| &t.name)),
            r,
        )?;
        expect_types(
            "vehicle",
            &type_names(self.vehicle_types.iter().map(|t| &t.name)),
            r,
        )?;
        expect_types(
            "building",
            &type_names(self.building_types.iter().map(|t| &t.name)),
            r,
        )?;
        expect_types(
            "tree",
            &type_names(self.tree_types.iter().map(|t| &t.name)),
            r,
        )?;
        self.by_handle = Snapshot::load(r)?;
        for t in self.mech_types.iter_mut() {
            load_mech_caches(t, r)?;
        }
        for t in self.vehicle_types.iter_mut() {
            load_anim_caches(&mut t.anims, r)?;
        }
        for t in self.building_types.iter_mut() {
            load_anim_caches(&mut t.anims, r)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;
    use mech::NodeData;

    fn base() -> AppearanceSystem {
        let mut s = AppearanceSystem::default();
        s.mech_types.push(MechAppearanceType {
            name: "test".into(),
            anims: vec![Some(AnimSet::new(Vec::new())), None],
            node_data: vec![NodeData::default()],
            ..Default::default()
        });
        s
    }

    fn bytes(s: &AppearanceSystem) -> Vec<u8> {
        let mut w = Writer::new();
        s.save(&mut w);
        w.into_bytes()
    }

    #[test]
    fn round_trip_is_byte_identical() {
        let mut a = base();
        a.mech_types[0].anims[0].as_mut().unwrap().shape_ids = Some(vec![Some(1), None]);
        a.mech_types[0].node_data[0].is_r_arm_node = true;
        a.put(ObjectHandle(1), Appearance::None);
        a.put(ObjectHandle(2), Appearance::None);
        a.put(ObjectHandle(3), Appearance::Vehicle(Box::default()));
        let saved = bytes(&a);

        let mut b = base();
        let mut r = Reader::new(&saved);
        b.load_into(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(bytes(&b), saved);
        assert_eq!(
            b.mech_types[0].anims[0].as_ref().unwrap().shape_ids,
            Some(vec![Some(1), None])
        );
        assert!(b.mech_types[0].node_data[0].is_r_arm_node);
    }

    #[test]
    fn different_type_list_is_rejected() {
        let saved = bytes(&base());
        let mut other = AppearanceSystem::default();
        other.mech_types.push(MechAppearanceType {
            name: "other".into(),
            anims: vec![Some(AnimSet::new(Vec::new())), None],
            node_data: vec![NodeData::default()],
            ..Default::default()
        });
        let err = other.load_into(&mut Reader::new(&saved)).unwrap_err();
        assert!(err.0.contains("mech type list differs"), "{err}");
    }
}
