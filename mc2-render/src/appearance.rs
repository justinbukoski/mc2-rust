// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Which shape an object draws and how it is posed: level of detail, damage and gesture animation.
//!
//! The render classes pick the shape in their `recalcBounds` (LOD by the eye distance, only while on screen) and swap
//! it in their `setObjStatus` (damage, when the status changes): `BldgAppearance` (`mclib/bdactor.cpp`),
//! `TreeAppearance` (`bdactor.cpp`), `GVAppearance` (`mclib/gvactor.cpp`) and `Mech3DAppearance` (`mclib/mech3d.cpp`).
//! Here [`advance`] applies those rules to the instance's [`ShapeChoice`] once per frame, through
//! [`ShapeInstance::select_shape`](crate::scene::ShapeInstance::select_shape).  The shape is replaced only when the C++
//! would replace it, so it can outlast the rule that chose it (a tree that comes back from damage keeps its LOD counter).
//! The pose is [`state_pose`]: the animation of the bound gesture at the state's frame.

use crate::math::{Mat4, Vec3};
use crate::scene::ModelId;
use crate::shape::ShapeModel;
use std::collections::HashMap;

/// `MAX_LODS` (`mclib/apprtype.h:37`).
pub const MAX_LODS: usize = 3;

/// `OBJECT_STATUS_*` (`mclib/objstatus.h:13-15`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ObjectStatus {
    #[default]
    Normal,
    Disabled,
    Destroyed,
}

/// The part of the appearance state the renderer reads: what shape to show and how to pose it.  The
/// simulation owns the values (lane F's `MechAppearance` and `BuildingAppearance`); the renderer only
/// looks at them.  Time enters through `frame`, which the simulation advances each step, so the same
/// state always gives the same picture.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct AppearanceState {
    pub status: ObjectStatus,
    /// The animation bound to the shape: a mech's `bound_gesture` (`currentGestureId` at the last `setAnimation`), or the
    /// building's animation state (`bdAnimationState`); `None` where nothing is bound and the object shows its bind
    /// pose.  Not the gesture the mech is in now: a destroyed mech keeps its shape's binding (`mech3d.cpp:3137-3140`).
    pub gesture: Option<usize>,
    /// The shape's `frameNum` (a mech's `shape.frame_num`, latched by `updateGeometry`, not its `currentFrame`).
    /// Sampled at its integer part, as `TransformMultiShape` does with `frameNum` (`msl.cpp`).
    pub frame: f32,
    /// The fall damage shape the simulation latched when the mech was destroyed: `Some(0)` forward, `Some(1)`
    /// backward, an index into [`ShapeSet::fall_damage`].  The caller passes lane F's `MechAppearance::shape_kind`
    /// here: `ForwardDamage` is `Some(0)`, `BackwardDamage` is `Some(1)`, `Lod(_)` is `None`.  The simulation swaps
    /// the shape once, in `setObjStatus` (`mech3d.cpp:2879-2897`), so the latch does not follow the gesture after that.
    pub fall_shape: Option<usize>,
    /// `baseRootNodeHeight`, measured by lane F on the first appearance update
    /// (`Mech3DAppearance::update`, `mech3d.cpp:4160`).  The renderer uses this supplied
    /// simulation state—not the bind pose—when the animated `joint_root` moves the mech's
    /// selection box (`mech3d.cpp:2153-2203`).  `None` is for non-mechs and legacy callers.
    pub base_root_node_height: Option<f32>,
    /// `isHelicopter` from `Mech3DAppearance::setMoverParameters`.  C++ lowers an in-view
    /// helicopter selection box by `HELICOPTER_FACTOR` (25) after refreshing its root offset
    /// (`mech3d.cpp:2153-2160`).
    pub is_helicopter: bool,
}

/// Which render class's rules apply.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AppearanceKind {
    /// Buildings and the other structures (`BldgAppearance`, `bdactor.cpp`): no LOD while destroyed or disabled
    /// (`bdactor.cpp:1302`); the `[TGLDamage]` shape replaces the LOD.
    #[default]
    Building,
    /// Trees (`TreeAppearance`, `bdactor.cpp:3405-3455`, `3595`): the same LOD rule, but a status change never
    /// resets `currentLOD`.
    Tree,
    /// Ground vehicles (`GVAppearance`, `gvactor.cpp:1023-1055`, `1543`): every status change resets `currentLOD`.
    Vehicle,
    /// Mechs (`Mech3DAppearance`, `mech3d.cpp:2047`, `2871`): LOD is selected until the mech is destroyed
    /// (`mech3d.cpp:2244`); the fall damage shape the simulation latched ([`AppearanceState::fall_shape`]) replaces
    /// the LOD once, when it is destroyed.
    Mech,
}

/// The shapes of one appearance, as loaded into the scene.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ShapeSet {
    pub kind: AppearanceKind,
    /// `bldgShape[i]` / `mechShape[i]` (index 0 is the full detail shape); `None` where the appearance has no such LOD.
    pub lods: [Option<ModelId>; MAX_LODS],
    /// `Distance<i>`: LOD `i` is used past this eye distance.  Unused for LOD 0.
    pub lod_distance: [f32; MAX_LODS],
    /// `[3DBounds]` override of the derived type box, in the Stuff frame (`apprtype.cpp:80-121`).
    pub selection_bounds: Option<(Vec3, Vec3)>,
    /// The building's type box established when its base shape is initialized.  A damaged shape
    /// changes the transition frame's rough extent, but never replaces this box
    /// (`bdactor.cpp:736-741`, `759-820`).
    pub building_type_bounds: Option<(Vec3, Vec3)>,
    /// `bldgDmgShape` / `treeDmgShape` / `gvDmgShape` (`[TGLDamage]`): shown while destroyed or disabled.
    pub damaged: Option<ModelId>,
    /// `mechForwardDmgShape` and `mechBackwardDmgShape` (mechs only).
    pub fall_damage: [Option<ModelId>; 2],
}

/// The shape an object shows this frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// An LOD of [`ShapeSet::lods`].
    Lod(usize),
    /// [`ShapeSet::damaged`].
    Damaged,
    /// [`ShapeSet::fall_damage`]: 0 forward, 1 backward.
    Fall(usize),
}

impl ShapeSet {
    /// The model a [`Shown`] refers to.
    pub fn model(&self, shown: Shown) -> Option<ModelId> {
        match shown {
            Shown::Lod(i) => self.lods.get(i).copied().flatten(),
            Shown::Damaged => self.damaged,
            Shown::Fall(i) => self.fall_damage.get(i).copied().flatten(),
        }
    }
}

/// The LOD an object at eye distance `distance` uses (`BldgAppearance::recalcBounds`,
/// `bdactor.cpp:1308-1325`; `Mech3DAppearance::recalcBounds`, `mech3d.cpp:2248-2267`; the same in
/// `gvactor.cpp:1738-1755` and `TreeAppearance`, `bdactor.cpp:3776-3793`).
///
/// With high object detail every LOD `i >= 1` that exists and whose `Distance<i>` is strictly exceeded
/// is a candidate, and the last candidate wins.  Without it, LOD 1 is used whenever it exists.
pub fn select_lod(set: &ShapeSet, distance: f32, high_object_detail: bool) -> usize {
    if !high_object_detail {
        return if set.lods[1].is_some() { 1 } else { 0 };
    }
    let mut lod = 0;
    for i in 1..MAX_LODS {
        if set.lods[i].is_some() && distance > set.lod_distance[i] {
            lod = i;
        }
    }
    lod
}

/// What an instance keeps between frames: the shape it draws, the appearance's `currentLOD`, and the status the shape
/// was last set for.  C++ replaces the shape in `setObjStatus` when the status changes and in `recalcBounds` when the
/// selected LOD differs from `currentLOD`; nothing else replaces it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ShapeChoice {
    pub shown: Shown,
    pub current_lod: usize,
    pub status: ObjectStatus,
}

impl Default for ShapeChoice {
    /// The C++ constructors start on the full detail shape with `currentLOD` 0; the status is applied on the first frame.
    fn default() -> Self {
        Self {
            shown: Shown::Lod(0),
            current_lod: 0,
            status: ObjectStatus::Normal,
        }
    }
}

/// Advances `prev` by one frame: the `setObjStatus` rules when the status changed, then the `recalcBounds` LOD rule.
///
/// `in_view` is the `inView` of `recalcBounds` ([`crate::camera::GameCamera::sphere_in_view`]).  The LOD is only
/// selected for an object on screen, so an object that left the view keeps the LOD it had (`bdactor.cpp:1302`,
/// `mech3d.cpp:2244`).  `distance` is the eye distance; `high_object_detail` is the `useHighObjectDetail` switch.
pub fn advance(
    set: &ShapeSet,
    prev: ShapeChoice,
    state: AppearanceState,
    in_view: bool,
    distance: f32,
    high_object_detail: bool,
) -> ShapeChoice {
    let mut next = prev;
    if state.status != prev.status {
        next.status = state.status;
        set_status(set, &mut next, state);
    }
    let selects_lod = match set.kind {
        AppearanceKind::Mech => state.status != ObjectStatus::Destroyed,
        _ => state.status == ObjectStatus::Normal,
    };
    if selects_lod && in_view {
        let lod = select_lod(set, distance, high_object_detail);
        if lod != next.current_lod {
            next.current_lod = lod;
            next.shown = Shown::Lod(lod);
        }
    }
    next
}

/// The `setObjStatus` rules for a status change to `state.status`.
fn set_status(set: &ShapeSet, next: &mut ShapeChoice, state: AppearanceState) {
    let damaged = matches!(
        state.status,
        ObjectStatus::Destroyed | ObjectStatus::Disabled
    );
    match set.kind {
        // bdactor.cpp:759-820: the damage shape resets `currentLOD` only when the appearance has one; back to
        // normal the base shape is shown and `currentLOD` is kept.
        AppearanceKind::Building => {
            if !damaged {
                next.shown = Shown::Lod(0);
            } else if set.damaged.is_some() {
                next.shown = Shown::Damaged;
                next.current_lod = 0;
            }
        }
        // bdactor.cpp:3405-3455: the damage and base shapes are swapped, and `currentLOD` is left alone.
        AppearanceKind::Tree => {
            if !damaged {
                next.shown = Shown::Lod(0);
            } else if set.damaged.is_some() {
                next.shown = Shown::Damaged;
            }
        }
        // gvactor.cpp:1023-1055: the shape is swapped where the appearance has one, and `currentLOD` is 0 either way.
        AppearanceKind::Vehicle => {
            if !damaged {
                next.shown = Shown::Lod(0);
            } else if set.damaged.is_some() {
                next.shown = Shown::Damaged;
            }
            next.current_lod = 0;
        }
        // mech3d.cpp:2871-2900: only DESTROYED swaps, to the latched fall shape where there is one; `currentLOD` is 0.
        AppearanceKind::Mech => {
            if state.status == ObjectStatus::Destroyed {
                if let Some(i) = state
                    .fall_shape
                    .filter(|&i| set.fall_damage.get(i).is_some_and(Option::is_some))
                {
                    next.shown = Shown::Fall(i);
                }
                next.current_lod = 0;
            }
        }
    }
}

/// The node pose of an object in `state`: the animation bound to its gesture, at its frame, or the bind pose
/// when none is bound.  A gesture without an animation file has no clip, which is the bind pose too
/// (`Mech3DAppearanceType::setAnimation` clears the animation, `mech3d.cpp:655-662`).  The pose depends on
/// `model` and `state` only.
pub fn state_pose(model: &ShapeModel, state: &AppearanceState) -> Vec<Mat4> {
    let clip = state
        .gesture
        .and_then(|g| model.gestures.get(g))
        .and_then(Option::as_ref);
    match clip {
        // `TransformMultiShape` samples `frameNum` with `float2long` (`msl.cpp:1347`), which rounds to the nearest
        // integer with ties to even (`GameOS/include/pch.hpp:150`), so the frame steps to that key.
        Some(clip) => model.pose(
            Some(clip),
            state.frame.round_ties_even().max(0.0),
            &HashMap::new(),
        ),
        None => model.bind_pose(),
    }
}

/// The mech torso twist `Mech3DAppearance` applies to `joint_torso` (`mech3d.cpp:729-733`): a yaw of `torso_degrees`
/// built as `EulerAngles(0, yaw, 0)`, as the node name and rotation to keep on the instance.  `None` when the model
/// has no `joint_torso` node.  The arm angles are stored by `setMoverParameters` but no node uses them.
pub fn torso_twist(model: &ShapeModel, torso_degrees: f32) -> Option<(String, crate::math::Quat)> {
    let idx = model.node_index("joint_torso")?;
    let q = mc2_assets::stuff::quat_from_euler(0.0, torso_degrees.to_radians(), 0.0);
    let q = crate::math::Quat {
        x: q.x,
        y: q.y,
        z: q.z,
        w: q.w,
    };
    Some((model.nodes[idx].id.clone(), q))
}

/// [`state_pose`] with the rotation of one node applied on top of the gesture animation.
pub fn state_pose_with_node(
    model: &ShapeModel,
    state: &AppearanceState,
    node: Option<&(String, crate::math::Quat)>,
) -> Vec<Mat4> {
    let Some((idx, q)) = node.and_then(|(id, q)| model.node_index(id).map(|i| (i, *q))) else {
        return state_pose(model, state);
    };
    let clip = state
        .gesture
        .and_then(|g| model.gestures.get(g))
        .and_then(Option::as_ref);
    let frame = if clip.is_some() {
        state.frame.round_ties_even().max(0.0)
    } else {
        0.0
    };
    model.pose(clip, frame, &HashMap::from([(idx, q)]))
}

impl crate::scene::ShapeInstance {
    /// Applies a status-driven model transition without running `recalcBounds`' LOD logic.
    /// Buildings call this before their visibility gates because `Building::handleDestruction`
    /// calls `setObjStatus` before `recalcBounds` (`code/bldng.cpp:1497-1499`).
    pub fn apply_status(&mut self) {
        if self.state.status != self.choice.status {
            self.choice.status = self.state.status;
            set_status(&self.shapes, &mut self.choice, self.state);
            if let Some(model) = self.shapes.model(self.choice.shown) {
                self.model = model;
            }
        }
    }

    /// Advances the shape this object draws by one frame for the eye at `eye` (game world coordinates); see
    /// [`advance`].  `in_view` is the appearance's `inView` for this frame.  The distance is the one the appearance
    /// measures (`objectCenter.GetApproximateLength()`, `bdactor.cpp:1098`; `eyeDistance`, `mech3d.cpp:2072`): a
    /// rotation and axis swap leave lengths unchanged.
    pub fn select_shape(&mut self, eye: Vec3, in_view: bool, high_object_detail: bool) {
        let distance = (self.position - eye).length();
        self.in_view = in_view;
        self.choice = advance(
            &self.shapes,
            self.choice,
            self.state,
            in_view,
            distance,
            high_object_detail,
        );
        if let Some(model) = self.shapes.model(self.choice.shown) {
            self.model = model;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(kind: AppearanceKind, lods: [bool; MAX_LODS], distances: [f32; MAX_LODS]) -> ShapeSet {
        let mut s = ShapeSet {
            kind,
            lod_distance: distances,
            ..Default::default()
        };
        for (i, present) in lods.iter().enumerate() {
            if *present {
                s.lods[i] = Some(ModelId(10 + i));
            }
        }
        s
    }

    fn normal() -> AppearanceState {
        AppearanceState::default()
    }

    #[test]
    fn lod_switches_only_past_the_distance_strictly() {
        // bdactor.cpp:1315 `distanceToEye > appearType->lodDistance[i]`
        let s = set(
            AppearanceKind::Building,
            [true, true, true],
            [0.0, 500.0, 1500.0],
        );
        assert_eq!(select_lod(&s, 0.0, true), 0);
        assert_eq!(select_lod(&s, 500.0, true), 0);
        assert_eq!(select_lod(&s, 500.5, true), 1);
        assert_eq!(select_lod(&s, 1500.0, true), 1);
        assert_eq!(select_lod(&s, 1500.5, true), 2);
    }

    #[test]
    fn the_last_exceeded_lod_wins_even_when_distances_are_not_ordered() {
        // The loop has no break: LOD 2 wins over LOD 1 whenever both are exceeded.
        let s = set(
            AppearanceKind::Building,
            [true, true, true],
            [0.0, 1000.0, 200.0],
        );
        assert_eq!(select_lod(&s, 150.0, true), 0);
        assert_eq!(select_lod(&s, 500.0, true), 2);
        assert_eq!(select_lod(&s, 1500.0, true), 2);
    }

    #[test]
    fn a_missing_lod_is_skipped() {
        // bdactor.cpp:1315 requires `appearType->bldgShape[i]`.
        let s = set(
            AppearanceKind::Building,
            [true, false, true],
            [0.0, 1000.0, 200.0],
        );
        assert_eq!(select_lod(&s, 500.0, true), 2);
    }

    #[test]
    fn without_high_object_detail_lod_1_is_used_whenever_it_exists() {
        // bdactor.cpp:1324 `selectLOD = 1` for every distance.
        let s = set(
            AppearanceKind::Building,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        assert_eq!(select_lod(&s, 0.0, false), 1);
        let no_lod1 = set(
            AppearanceKind::Building,
            [true, false, true],
            [0.0, 1000.0, 0.0],
        );
        assert_eq!(select_lod(&no_lod1, 5000.0, false), 0);
    }

    /// The shape after one frame from a fresh instance.
    fn fresh(s: &ShapeSet, state: AppearanceState, distance: f32, hod: bool) -> Shown {
        advance(s, ShapeChoice::default(), state, true, distance, hod).shown
    }

    fn with_status(status: ObjectStatus) -> AppearanceState {
        AppearanceState { status, ..normal() }
    }

    #[test]
    fn mech_lod_follows_the_detail_switch() {
        // mech3d.cpp:88 `useHighObjectDetail = true` (prefs.cpp:52 keeps it on by default).
        let s = set(
            AppearanceKind::Mech,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        assert_eq!(fresh(&s, normal(), 2000.0, true), Shown::Lod(1));
        assert_eq!(fresh(&s, normal(), 2000.0, false), Shown::Lod(1));
        assert_eq!(fresh(&s, normal(), 500.0, true), Shown::Lod(0));
    }

    #[test]
    fn a_structure_shows_its_damage_shape_while_destroyed_or_disabled() {
        // bdactor.cpp:759-790: the damage shape replaces the LOD on DESTROYED or DISABLED.
        let mut s = set(
            AppearanceKind::Building,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        s.damaged = Some(ModelId(99));
        let start = advance(&s, ShapeChoice::default(), normal(), true, 1500.0, true);
        assert_eq!(start.shown, Shown::Lod(1));
        for status in [ObjectStatus::Destroyed, ObjectStatus::Disabled] {
            let hit = advance(&s, start, with_status(status), true, 1500.0, true);
            assert_eq!(hit.shown, Shown::Damaged);
            assert_eq!(hit.current_lod, 0, "bdactor.cpp:779 resets currentLOD");
            assert_eq!(s.model(hit.shown), Some(ModelId(99)));
        }
        // Back to normal the base shape is shown, and the LOD is selected again from the distance.
        let back = advance(
            &s,
            advance(
                &s,
                start,
                with_status(ObjectStatus::Destroyed),
                true,
                1500.0,
                true,
            ),
            normal(),
            true,
            1500.0,
            true,
        );
        assert_eq!(back.shown, Shown::Lod(1));
    }

    #[test]
    fn a_destroyed_structure_without_damage_shape_keeps_its_lod() {
        // bdactor.cpp:759-820: no damage shape is swapped in, and no LOD is selected while destroyed (bdactor.cpp:1302).
        let s = set(
            AppearanceKind::Building,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        let at_lod_1 = advance(&s, ShapeChoice::default(), normal(), true, 1500.0, true);
        let destroyed = advance(
            &s,
            at_lod_1,
            with_status(ObjectStatus::Destroyed),
            true,
            10.0,
            true,
        );
        assert_eq!(destroyed.shown, Shown::Lod(1));
        assert_eq!(destroyed.current_lod, 1);
        let far = advance(
            &s,
            destroyed,
            with_status(ObjectStatus::Destroyed),
            true,
            5000.0,
            true,
        );
        assert_eq!(far.shown, Shown::Lod(1));
    }

    #[test]
    fn a_lod_chosen_while_visible_is_kept_while_the_object_is_off_screen() {
        // bdactor.cpp:1300-1302: recalcBounds selects an LOD only for an object on screen.  Seen at LOD 0, pulled
        // back out of the view past the LOD 1 distance, then destroyed (no damage shape) before it returns: C++
        // still shows LOD 0 and keeps it.
        let s = set(
            AppearanceKind::Building,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        let seen = advance(&s, ShapeChoice::default(), normal(), true, 300.0, true);
        assert_eq!(seen.shown, Shown::Lod(0));
        let off = advance(&s, seen, normal(), false, 5000.0, true);
        assert_eq!(off, seen, "no LOD change while off screen");
        let destroyed = advance(
            &s,
            off,
            with_status(ObjectStatus::Destroyed),
            false,
            5000.0,
            true,
        );
        assert_eq!(destroyed.shown, Shown::Lod(0));
        assert_eq!(destroyed.current_lod, 0);
        // Back in view, still destroyed: nothing re-selects, so LOD 0 stays.
        let back = advance(
            &s,
            destroyed,
            with_status(ObjectStatus::Destroyed),
            true,
            5000.0,
            true,
        );
        assert_eq!(back.shown, Shown::Lod(0));
        // Back in view and normal: the distance selects LOD 1 again.
        let normal_again = advance(&s, back, normal(), true, 5000.0, true);
        assert_eq!(normal_again.shown, Shown::Lod(1));
    }

    #[test]
    fn a_tree_keeps_its_lod_counter_through_damage_and_back() {
        // bdactor.cpp:3405-3455: a tree's status change swaps the shape and leaves currentLOD alone, so a tree at
        // distant LOD 1 that is disabled and back to normal shows its base shape with currentLOD 1, and the LOD is
        // not re-selected until the selected LOD differs from it (bdactor.cpp:3797).
        let mut s = set(
            AppearanceKind::Tree,
            [true, true, true],
            [0.0, 1000.0, 2000.0],
        );
        s.damaged = Some(ModelId(99));
        let distant = advance(&s, ShapeChoice::default(), normal(), true, 1500.0, true);
        assert_eq!(distant.shown, Shown::Lod(1));
        let disabled = advance(
            &s,
            distant,
            with_status(ObjectStatus::Disabled),
            true,
            1500.0,
            true,
        );
        assert_eq!(disabled.shown, Shown::Damaged);
        assert_eq!(disabled.current_lod, 1);
        let normal_again = advance(&s, disabled, normal(), true, 1500.0, true);
        assert_eq!(
            normal_again.shown,
            Shown::Lod(0),
            "the base shape is recreated"
        );
        assert_eq!(normal_again.current_lod, 1, "currentLOD is retained");
        // Still at LOD 1's distance: the selected LOD equals currentLOD, so the base shape stays.
        let same = advance(&s, normal_again, normal(), true, 1500.0, true);
        assert_eq!(same.shown, Shown::Lod(0));
        // Past LOD 2's distance the selected LOD differs, and the LOD is swapped in.
        let further = advance(&s, same, normal(), true, 2500.0, true);
        assert_eq!(further.shown, Shown::Lod(2));
        assert_eq!(further.current_lod, 2);
    }

    #[test]
    fn a_building_recreates_its_base_shape_at_lod_0_when_it_comes_back() {
        // bdactor.cpp:759-820: the damage shape resets currentLOD to 0, so back to normal the LOD is selected again.
        let mut s = set(
            AppearanceKind::Building,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        s.damaged = Some(ModelId(99));
        let distant = advance(&s, ShapeChoice::default(), normal(), true, 1500.0, true);
        let disabled = advance(
            &s,
            distant,
            with_status(ObjectStatus::Disabled),
            true,
            1500.0,
            true,
        );
        assert_eq!(disabled.current_lod, 0);
        let normal_again = advance(&s, disabled, normal(), true, 1500.0, true);
        assert_eq!(normal_again.shown, Shown::Lod(1));
    }

    #[test]
    fn a_vehicle_resets_its_lod_on_every_status_change() {
        // gvactor.cpp:1023-1055: currentLOD is 0 after any status change, damaged or back to normal.
        let mut s = set(
            AppearanceKind::Vehicle,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        s.damaged = Some(ModelId(99));
        let distant = advance(&s, ShapeChoice::default(), normal(), true, 1500.0, true);
        let back = advance(
            &s,
            distant,
            with_status(ObjectStatus::Destroyed),
            true,
            1500.0,
            true,
        );
        assert_eq!(back.current_lod, 0);
        let normal_again = advance(&s, back, normal(), true, 1500.0, true);
        assert_eq!(
            normal_again.shown,
            Shown::Lod(1),
            "the LOD is re-selected from the distance"
        );
    }

    #[test]
    fn a_mech_shows_the_fall_shape_the_simulation_latched() {
        // mech3d.cpp:2879-2897 swaps once, in setObjStatus; the caller passes lane F's shape_kind as the latch.
        const FALLEN_FORWARD: usize = 23;
        const FALLEN_BACKWARD: usize = 24;
        let mut s = set(
            AppearanceKind::Mech,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        s.fall_damage = [Some(ModelId(70)), Some(ModelId(71))];
        let destroyed = |latch, g| AppearanceState {
            status: ObjectStatus::Destroyed,
            gesture: Some(g),
            frame: 0.0,
            fall_shape: latch,
            ..Default::default()
        };
        let fell = |latch, g| fresh(&s, destroyed(latch, g), 0.0, true);
        assert_eq!(fell(Some(0), FALLEN_FORWARD), Shown::Fall(0));
        assert_eq!(fell(Some(1), FALLEN_BACKWARD), Shown::Fall(1));
        assert_eq!(s.model(Shown::Fall(1)), Some(ModelId(71)));
        // The gesture moving on after the swap does not change the shape: lane F keeps shape_kind.
        let hit = advance(
            &s,
            ShapeChoice::default(),
            destroyed(Some(0), FALLEN_FORWARD),
            true,
            0.0,
            true,
        );
        let moved_on = advance(&s, hit, destroyed(Some(0), 4), true, 0.0, true);
        assert_eq!(moved_on.shown, Shown::Fall(0));
        // No latch, destroyed in a fall gesture: no swap happened, so the LOD it had stays
        // (mech3d.cpp:2244 skips recalcBounds).
        let lod_1 = advance(&s, ShapeChoice::default(), normal(), true, 1500.0, true);
        assert_eq!(
            advance(&s, lod_1, destroyed(None, FALLEN_FORWARD), true, 0.0, true).shown,
            Shown::Lod(1)
        );
        // A latch without a shape behind it is no swap either.
        let mut no_shape = s;
        no_shape.fall_damage = [None, None];
        assert_eq!(
            advance(
                &no_shape,
                lod_1,
                destroyed(Some(0), FALLEN_FORWARD),
                true,
                0.0,
                true
            )
            .shown,
            Shown::Lod(1)
        );
    }

    #[test]
    fn a_mech_keeps_selecting_its_lod_while_disabled() {
        // mech3d.cpp:2244 only skips the LOD for DESTROYED; a disabled mech is not swapped.
        let s = set(
            AppearanceKind::Mech,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        assert_eq!(
            fresh(&s, with_status(ObjectStatus::Disabled), 2000.0, true),
            Shown::Lod(1)
        );
    }

    #[test]
    fn a_destroyed_mech_off_screen_keeps_its_lod() {
        // mech3d.cpp:2244 (`status != DESTROYED`) and the inView gate: the LOD it had stays.
        let s = set(
            AppearanceKind::Mech,
            [true, true, false],
            [0.0, 1000.0, 0.0],
        );
        let seen = advance(&s, ShapeChoice::default(), normal(), true, 300.0, true);
        let destroyed_off = advance(
            &s,
            seen,
            with_status(ObjectStatus::Destroyed),
            false,
            5000.0,
            true,
        );
        assert_eq!(destroyed_off.shown, Shown::Lod(0));
    }

    #[test]
    fn the_pose_follows_the_gesture_at_the_rounded_frame() {
        // TransformMultiShape samples `frameNum` with float2long (msl.cpp:1347): round to nearest, ties to even
        // (pch.hpp:150). A gesture without a clip is the bind pose (mech3d.cpp:655-662).
        use crate::math::Quat;
        use crate::shape::{AnimationClip, NodeInfo, NodeKind, Track};
        let mut model = ShapeModel {
            name: "m".into(),
            nodes: vec![NodeInfo {
                id: "root".into(),
                parent: None,
                center: Vec3::ZERO,
                relative_center: Vec3::ZERO,
                kind: NodeKind::Normal,
            }],
            texture_names: Vec::new(),
            texture_alpha: Vec::new(),
            vertices: Vec::new(),
            batches: Vec::new(),
            bounds: (Vec3::ZERO, Vec3::ZERO),
            cone_extent: 0.0,
            building_bounds: (Vec3::ZERO, Vec3::ZERO),
            gestures: Vec::new(),
        };
        // Four keys, a quarter turn apart: key k points x along (cos, sin) of k * 90 degrees.
        let turn = |k: f32| {
            Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), k * std::f32::consts::FRAC_PI_2)
        };
        model.gestures = vec![
            None,
            Some(AnimationClip {
                frames: 4,
                frame_rate: 30.0,
                tracks: vec![Some(Track {
                    rotations: vec![turn(0.0), turn(1.0), turn(2.0), turn(3.0)],
                    positions: Vec::new(),
                })],
            }),
        ];
        let pose = |gesture, frame| {
            state_pose(
                &model,
                &AppearanceState {
                    status: ObjectStatus::Normal,
                    gesture,
                    frame,
                    fall_shape: None,
                    ..Default::default()
                },
            )
        };
        let x = Vec3::new(1.0, 0.0, 0.0);
        assert_eq!(pose(None, 1.0), model.bind_pose());
        assert_eq!(pose(Some(0), 1.0), model.bind_pose());
        assert_eq!(pose(Some(1), 0.0)[0].transform_point(x), x);
        // Each frame is one key, a quarter turn apart: frame 1 points along +y, frame 2 along -x.
        let at = |frame| pose(Some(1), frame)[0].transform_point(x);
        assert!((at(1.0) - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5);
        assert!((at(2.0) - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-5);
        // float2long rounds to the nearest frame (msl.cpp:1347, pch.hpp:150): 0.4 is frame 0 and 0.6 is frame 1,
        // on either side of the boundary at 0.5.
        assert_eq!(pose(Some(1), 0.4), pose(Some(1), 0.0));
        assert_eq!(pose(Some(1), 0.6), pose(Some(1), 1.0));
        assert_eq!(pose(Some(1), 1.4), pose(Some(1), 1.0));
        assert_eq!(pose(Some(1), 1.6), pose(Some(1), 2.0));
        assert_ne!(pose(Some(1), 0.6), pose(Some(1), 0.0));
        // Ties go to the even frame, as the FPU's round-to-nearest-even does in float2long.
        assert_eq!(pose(Some(1), 0.5), pose(Some(1), 0.0));
        assert_eq!(pose(Some(1), 1.5), pose(Some(1), 2.0));
        assert_eq!(pose(Some(1), 2.5), pose(Some(1), 2.0));
        assert_ne!(pose(Some(1), 1.5), pose(Some(1), 1.0));
        // Negative fractions round the same way and then hold the first key (the frame is never negative).
        assert_eq!(pose(Some(1), -0.6), pose(Some(1), 0.0));
    }
}
