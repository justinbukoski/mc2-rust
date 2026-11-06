// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `gosFX::Effect`: the base class and the dispatch to every effect type.
//!
//! An [`Effect`] is a running instance of an [`EffectSpec`]. The life cycle is the C++ one:
//! [`Effect::start`] once (age 0, the parent's transform), then [`Effect::execute`] every
//! frame with the current time, which ages the effect, spawns the scheduled child events,
//! executes the children and reports back whether the effect is still alive. The class-specific
//! work (particle clouds, cards, tubes, ...) lives in the sibling modules; each virtual method
//! of the C++ (`Start`, `Execute`, `Kill`, `HasFinished`, `Draw`) is a `match` on the
//! specification kind here, and calls the base-class version (`effect_start` and friends)
//! exactly where the C++ calls `Effect::Start`.

use crate::card::CardState;
use crate::cloud::Cloud;
use crate::debris::DebrisState;
use crate::draw::{DrawInfo, DrawList};
use crate::fcurve::FCurve;
use crate::library::EffectLibrary;
use crate::light::LightState;
use crate::math::{Mat, Obb};
use crate::rng::VisualRng;
use crate::singleton::ShapeState;
use crate::spec::{EffectSpec, SpecKind};
use crate::state::MlrState;
use crate::tube::TubeState;
use std::sync::Arc;

pub const EXECUTE_FLAG: u32 = 1;
pub const LOOP_FLAG: u32 = 2;
pub const LOCAL_SPACE_SIMULATION: u32 = 0;
pub const DYNAMIC_WORLD_SPACE_SIMULATION: u32 = 4;
pub const STATIC_WORLD_SPACE_SIMULATION: u32 = 8;
pub const PARENT_SIMULATION: u32 = 12;
pub const SIMULATION_MODE_MASK: u32 = 12;

/// Shared services for `start`/`execute`: the library (child events and effect clouds create
/// effects by id) and the visual random stream.
pub struct Ctx<'a> {
    pub lib: &'a EffectLibrary,
    pub rng: &'a mut VisualRng,
}

/// `Effect::ExecuteInfo`.
#[derive(Clone, Debug)]
pub struct ExecuteInfo {
    pub time: f64,
    pub parent_to_world: Mat,
    /// `-1.0` asks `start` to draw a fresh random seed.
    pub seed: f32,
    /// `-1.0` means "start at age 0 with this effect's own life span".
    pub age: f32,
    pub age_rate: f32,
}

impl ExecuteInfo {
    pub fn new(time: f64, parent_to_world: Mat) -> ExecuteInfo {
        ExecuteInfo {
            time,
            parent_to_world,
            seed: -1.0,
            age: -1.0,
            age_rate: -1.0,
        }
    }
}

/// The state every effect has (the data members of `gosFX::Effect`).
pub(crate) struct Base {
    pub local_to_world: Mat,
    pub local_to_parent: Mat,
    pub last_ran: f64,
    pub age: f32,
    pub age_rate: f32,
    pub seed: f32,
    pub flags: u32,
    pub children: Vec<Effect>,
    pub event_idx: usize,
}

pub(crate) enum Data {
    None,
    Cloud(Box<Cloud>),
    Card(CardState),
    Shape(ShapeState),
    Tube(Box<TubeState>),
    Debris(Box<DebrisState>),
    Light(LightState),
}

pub struct Effect {
    pub(crate) spec: Arc<EffectSpec>,
    pub(crate) b: Base,
    pub(crate) data: Data,
}

impl Effect {
    /// `EffectLibrary::MakeEffect(index, flags)`. `None` when the id is out of range.
    pub fn new(lib: &EffectLibrary, id: usize, flags: u32) -> Option<Effect> {
        let spec = lib.spec(id)?.clone();
        Some(Effect::from_spec(spec, flags))
    }

    pub fn from_spec(spec: Arc<EffectSpec>, flags: u32) -> Effect {
        let data = match &spec.kind {
            SpecKind::Effect => Data::None,
            SpecKind::PointLight(_) => Data::Light(LightState::default()),
            SpecKind::PointCloud(_)
            | SpecKind::ShardCloud(_)
            | SpecKind::PertCloud(_)
            | SpecKind::CardCloud(_)
            | SpecKind::ShapeCloud(_)
            | SpecKind::EffectCloud(_) => Data::Cloud(Box::new(Cloud::new(&spec))),
            SpecKind::Card(_) => Data::Card(CardState::default()),
            SpecKind::Shape(s) => Data::Shape(ShapeState::new(s.radius)),
            SpecKind::Tube(t) => Data::Tube(Box::new(TubeState::new(t))),
            SpecKind::DebrisCloud(d) => Data::Debris(Box::new(DebrisState::new(d))),
        };
        Effect {
            spec,
            b: Base {
                local_to_world: Mat::IDENTITY,
                local_to_parent: Mat::IDENTITY,
                last_ran: -1.0,
                age: 0.0,
                age_rate: 0.0,
                seed: 0.0,
                flags,
                children: Vec::new(),
                event_idx: 0,
            },
            data,
        }
    }

    pub fn spec(&self) -> &Arc<EffectSpec> {
        &self.spec
    }
    pub fn name(&self) -> &str {
        self.spec.name()
    }
    pub fn age(&self) -> f32 {
        self.b.age
    }
    pub fn seed(&self) -> f32 {
        self.b.seed
    }
    pub fn local_to_world(&self) -> &Mat {
        &self.b.local_to_world
    }
    pub fn local_to_parent(&self) -> &Mat {
        &self.b.local_to_parent
    }
    pub fn set_local_to_parent(&mut self, m: Mat) {
        self.b.local_to_parent = m;
    }
    pub fn children(&self) -> &[Effect] {
        &self.b.children
    }

    // --- flags (effect.hpp inline accessors)
    pub fn set_execute_on(&mut self) {
        self.b.flags |= EXECUTE_FLAG;
    }
    pub fn set_execute_off(&mut self) {
        self.b.flags &= !EXECUTE_FLAG;
    }
    pub fn is_executed(&self) -> bool {
        self.b.flags & EXECUTE_FLAG != 0
    }
    pub fn set_loop_on(&mut self) {
        self.b.flags |= LOOP_FLAG;
    }
    pub fn set_loop_off(&mut self) {
        self.b.flags &= !LOOP_FLAG;
    }
    pub fn is_looped(&self) -> bool {
        self.b.flags & LOOP_FLAG != 0
    }
    pub fn simulation_mode(&self) -> u32 {
        self.b.flags & SIMULATION_MODE_MASK
    }
    pub fn flags(&self) -> u32 {
        self.b.flags
    }

    // --- virtual methods

    /// `Effect::Start` and its overrides.
    pub fn start(&mut self, ctx: &mut Ctx, info: &ExecuteInfo) {
        match &self.spec.kind {
            SpecKind::PointCloud(_)
            | SpecKind::ShardCloud(_)
            | SpecKind::PertCloud(_)
            | SpecKind::CardCloud(_)
            | SpecKind::ShapeCloud(_)
            | SpecKind::EffectCloud(_) => self.cloud_start(ctx, info),
            SpecKind::Card(_) => self.card_start(ctx, info),
            SpecKind::Tube(_) => self.tube_start(ctx, info),
            SpecKind::DebrisCloud(_) => self.debris_start(ctx, info),
            SpecKind::PointLight(_) => self.light_start(ctx, info),
            SpecKind::Effect | SpecKind::Shape(_) => self.effect_start(ctx, info),
        }
    }

    /// `Effect::Execute` and its overrides. `bounds` receives the bounding sphere of the
    /// effect and its children in the parent's space. Returns whether the effect is still
    /// executing; the caller drops it when this is `false`.
    pub fn execute(&mut self, ctx: &mut Ctx, info: &ExecuteInfo, bounds: &mut Obb) -> bool {
        match &self.spec.kind {
            SpecKind::PointCloud(_)
            | SpecKind::ShardCloud(_)
            | SpecKind::PertCloud(_)
            | SpecKind::CardCloud(_)
            | SpecKind::ShapeCloud(_)
            | SpecKind::EffectCloud(_) => self.cloud_execute(ctx, info, bounds),
            SpecKind::Card(_) => self.card_execute(ctx, info, bounds),
            SpecKind::Shape(_) => self.shape_execute(ctx, info, bounds),
            SpecKind::Tube(_) => self.tube_execute(ctx, info, bounds),
            SpecKind::DebrisCloud(_) => self.debris_execute(ctx, info, bounds),
            SpecKind::PointLight(_) => self.light_execute(ctx, info, bounds),
            SpecKind::Effect => self.effect_execute(ctx, info, bounds),
        }
    }

    /// `Effect::Kill` and its overrides: kills the children and stops executing.
    pub fn kill(&mut self) {
        match &self.spec.kind {
            SpecKind::PointCloud(_)
            | SpecKind::ShardCloud(_)
            | SpecKind::PertCloud(_)
            | SpecKind::CardCloud(_)
            | SpecKind::ShapeCloud(_)
            | SpecKind::EffectCloud(_) => self.cloud_kill(),
            SpecKind::Card(_) => self.card_kill(),
            SpecKind::Tube(_) => self.tube_kill(),
            SpecKind::DebrisCloud(_) => self.debris_kill(),
            SpecKind::PointLight(_) => self.light_kill(),
            SpecKind::Effect | SpecKind::Shape(_) => self.effect_kill(),
        }
    }

    /// `Effect::HasFinished` and its overrides.
    pub fn has_finished(&self) -> bool {
        match &self.spec.kind {
            SpecKind::PointCloud(_)
            | SpecKind::ShardCloud(_)
            | SpecKind::PertCloud(_)
            | SpecKind::CardCloud(_)
            | SpecKind::ShapeCloud(_)
            | SpecKind::EffectCloud(_) => self.effect_has_finished() && self.cloud_active() == 0,
            SpecKind::Tube(_) => self.effect_has_finished() && self.tube_active() == 0,
            _ => self.effect_has_finished(),
        }
    }

    /// `Effect::Stop`: ends the effect's own life (age 1, no more events); the children and
    /// particles run out.
    pub fn stop(&mut self) {
        self.b.age = 1.0;
        self.b.event_idx = self.spec.base.events.len();
    }

    /// `Effect::Draw` and its overrides: appends this effect's geometry, then its children's.
    pub fn draw(&self, info: &DrawInfo, out: &mut DrawList) {
        match &self.spec.kind {
            SpecKind::PointCloud(_)
            | SpecKind::ShardCloud(_)
            | SpecKind::PertCloud(_)
            | SpecKind::CardCloud(_)
            | SpecKind::ShapeCloud(_)
            | SpecKind::EffectCloud(_) => self.cloud_draw(info, out),
            SpecKind::Card(_) => self.card_draw(info, out),
            SpecKind::Shape(_) => self.shape_draw(info, out),
            SpecKind::Tube(_) => self.tube_draw(info, out),
            SpecKind::DebrisCloud(_) => self.debris_draw(info, out),
            SpecKind::PointLight(_) => self.light_draw(info, out),
            SpecKind::Effect => {}
        }
        self.effect_draw(info, out);
    }

    // --- the base class

    /// `Effect::Start`.
    pub(crate) fn effect_start(&mut self, ctx: &mut Ctx, info: &ExecuteInfo) {
        // Don't override last_ran if Start is issued while the effect is already running.
        if !self.is_executed() || self.b.last_ran == -1.0 {
            self.b.last_ran = info.time;
        }
        self.set_execute_on();

        // No seed provided: pick one.
        self.b.seed = if info.seed == -1.0 {
            ctx.rng.fraction()
        } else {
            info.seed
        };

        if info.age == -1.0 {
            let mut lifetime = self.spec.base.life_span.value;
            if lifetime < 0.033333 {
                lifetime = 0.033333;
            }
            self.b.age_rate = 1.0 / lifetime;
            self.b.age = 0.0;
        } else {
            self.b.age = info.age;
            self.b.age_rate = info.age_rate;
        }

        self.b.local_to_world = Mat::multiply(&self.b.local_to_parent, &info.parent_to_world);
        self.b.event_idx = 0;
    }

    /// `Effect::Execute`.
    pub(crate) fn effect_execute(
        &mut self,
        ctx: &mut Ctx,
        info: &ExecuteInfo,
        bounds: &mut Obb,
    ) -> bool {
        if !self.is_executed() {
            return false;
        }
        if info.seed != -1.0 {
            self.b.seed = info.seed;
        }

        // New age; clear the bounds.
        let age = self.b.age + (info.time - self.b.last_ran) as f32 * self.b.age_rate;
        *bounds = Obb::IDENTITY;

        self.b.local_to_world = Mat::multiply(&self.b.local_to_parent, &info.parent_to_world);

        // Spawn the events that are due.
        while self.b.event_idx < self.spec.base.events.len() {
            let event = &self.spec.base.events[self.b.event_idx];
            if event.time > self.b.age {
                break;
            }
            let mut flags = EXECUTE_FLAG;
            if event.flags & SIMULATION_MODE_MASK == PARENT_SIMULATION {
                flags |= self.b.flags & SIMULATION_MODE_MASK;
            } else {
                flags |= event.flags & SIMULATION_MODE_MASK;
            }
            let (effect_id, local_to_parent) = (event.effect_id, event.local_to_parent);
            self.b.event_idx += 1;
            if let Some(mut child) = Effect::new(ctx.lib, effect_id as usize, flags) {
                child.b.local_to_parent = local_to_parent;
                let min_seed = self
                    .spec
                    .base
                    .min_child_seed
                    .compute(self.b.age, self.b.seed);
                let seed_range = self
                    .spec
                    .base
                    .max_child_seed
                    .compute(self.b.age, self.b.seed)
                    - min_seed;
                let seed = (ctx.rng.fraction() * seed_range + min_seed).clamp(0.0, 1.0);
                let mut local_info = ExecuteInfo::new(info.time, self.b.local_to_world);
                local_info.seed = seed;
                child.start(ctx, &local_info);
                self.b.children.push(child);
            }
        }

        // Execute the children; the ones that finish are dropped.
        let mut child_obb = Obb::IDENTITY;
        let child_info = ExecuteInfo::new(info.time, self.b.local_to_world);
        let mut i = 0;
        while i < self.b.children.len() {
            let alive = self.b.children[i].execute(ctx, &child_info, &mut child_obb);
            if !alive {
                self.b.children.remove(i);
            } else {
                i += 1;
            }
            // Merge the child's sphere into ours (the C++ does this for dead children too).
            let parent_bounds = Obb::multiply(&child_obb, &self.b.local_to_parent);
            *bounds = Obb::union(bounds, &parent_bounds);
        }

        // Advance; at the end of life either loop or wait for the children and die.
        self.b.last_ran = info.time;
        self.b.age = age;
        if self.b.age >= 1.0 {
            if self.is_looped() {
                self.start(ctx, info);
            } else if self.has_finished() {
                self.kill();
            }
        }
        self.is_executed()
    }

    /// `Effect::Kill`.
    pub(crate) fn effect_kill(&mut self) {
        for child in self.b.children.iter_mut() {
            child.kill();
        }
        self.set_execute_off();
    }

    /// `Effect::HasFinished`.
    pub(crate) fn effect_has_finished(&self) -> bool {
        if self.is_executed() && self.b.age < 1.0 {
            return false;
        }
        self.b.children.is_empty()
    }

    /// `Effect::Draw`: the children, with this effect's state combined in.
    pub(crate) fn effect_draw(&self, info: &DrawInfo, out: &mut DrawList) {
        if self.b.children.is_empty() {
            return;
        }
        let new_info = DrawInfo {
            parent_to_world: Mat::multiply(&self.b.local_to_parent, &info.parent_to_world),
            state: MlrState::combine(&info.state, &self.spec.base.state),
            camera_to_world: info.camera_to_world,
        };
        for child in &self.b.children {
            child.draw(&new_info, out);
        }
    }

    /// Resolved state and effect-to-world matrix every class's `Draw` starts with.
    pub(crate) fn draw_frame(&self, info: &DrawInfo) -> (crate::draw::DrawState, Mat) {
        let state = MlrState::combine(&info.state, &self.spec.base.state);
        let local_to_world = Mat::multiply(&self.b.local_to_parent, &info.parent_to_world);
        (crate::draw::DrawState::from_mlr(&state), local_to_world)
    }
}
