// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The effect manager the app talks to: spawn an effect by name at a transform, attach it to a
//! moving object, step it with the clock, draw it.
//!
//! This is the role `WeaponBolt`, `Carnage` and `TerrainObject` play in the C++ (each owns a few
//! `gosFX::Effect*` it creates with `EffectLibrary::MakeEffect`, starts with the parent matrix,
//! executes every frame and kills when the owner dies); here one [`FxSystem`] owns them all.
//! Effects are purely visual: nothing here reads or writes simulation state, the random numbers
//! come from the system's own [`VisualRng`], and the only input is the time and the transforms
//! the app hands in.
//!
//! Space: gosFX works in the `Stuff` frame (x = -world x, y = world z, z = world y; the game
//! spells the swap out at every call site, see `mc2-render/src/camera.rs`). Transforms given to
//! the system and the draw data it produces are in that frame; [`game_to_fx`] and
//! [`FxTransform`] build the matrices the C++ callers build.

use crate::draw::{DrawInfo, DrawList};
use crate::effect::{Ctx, Effect, ExecuteInfo, EXECUTE_FLAG, LOOP_FLAG};
use crate::error::{FxError, Result};
use crate::library::{EffectLibrary, LIBRARY_PATH};
use crate::math::{Mat, Obb, Vec3};
use crate::rng::VisualRng;
use crate::spec::EffectSpec;
use crate::weaponfx::{WeaponEffects, EFFECTS_CSV_PATH, NONE};
use mc2_formats::vfs::Vfs;
use std::collections::HashMap;
use std::sync::Arc;

/// Game world position (x east, y north, z up) to the gosFX frame, as `weaponbolt.cpp` does:
/// `x = -position.x; y = position.z; z = position.y`.
pub fn game_to_fx(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(-x, z, y)
}

/// `gosFX::Effect_Into_Motion` (`gosfx.cpp`): points the effect's +Z along the object's +Z.
pub const EFFECT_INTO_MOTION: Mat = Mat {
    m: [
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 0.0],
    ],
};

/// `gosFX::Effect_Against_Motion`.
pub const EFFECT_AGAINST_MOTION: Mat = Mat {
    m: [
        [1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
    ],
};

/// The transforms the game builds for its effects.
pub struct FxTransform;

impl FxTransform {
    /// `Effect_Into_Motion * Rot(yaw) * Translate(position)`: how `weaponbolt.cpp` places muzzle
    /// flashes and hit effects (`yaw_degrees` is the object's `getRotation()`; the position is
    /// already in the gosFX frame, see [`game_to_fx`]).
    pub fn into_motion(position: Vec3, yaw_degrees: f32) -> Mat {
        Self::with_base(&EFFECT_INTO_MOTION, position, yaw_degrees)
    }

    /// `Effect_Against_Motion * Rot(yaw) * Translate(position)`: bolts flying along their
    /// aim direction (`rotation = -world_angle_between(position, target)`).
    pub fn against_motion(position: Vec3, yaw_degrees: f32) -> Mat {
        Self::with_base(&EFFECT_AGAINST_MOTION, position, yaw_degrees)
    }

    /// How a bolt's in-flight effect is placed each frame (`WeaponBolt::update`): at the
    /// projectile `position` (game world), turned to fly along the line to `target`
    /// (`rotation = -world_angle_between(position, target)`). For beams the C++ uses the target
    /// as the position.
    pub fn bolt(position: Vec3, target: Vec3) -> Mat {
        let rotation = -world_angle_between(position, target);
        Self::against_motion(game_to_fx(position.x, position.y, position.z), rotation)
    }

    /// A muzzle flash on a mech at `position` (game world) facing `owner_rotation_degrees`.
    pub fn muzzle(position: Vec3, owner_rotation_degrees: f32) -> Mat {
        Self::into_motion(
            game_to_fx(position.x, position.y, position.z),
            owner_rotation_degrees,
        )
    }

    /// A plain placement (`shapeOrigin`: no rotation, just the position).
    pub fn at(position: Vec3) -> Mat {
        Mat::from_translation(position)
    }

    fn with_base(base: &Mat, position: Vec3, yaw_degrees: f32) -> Mat {
        let mut rot = Mat::IDENTITY;
        rot.build_rotation_euler(0.0, yaw_degrees.to_radians(), 0.0);
        let local_to_world = Mat::multiply(base, &rot);
        Mat::multiply(&local_to_world, &Mat::from_translation(position))
    }
}

/// `world_angle_between(v1, v2)` (`mclib/mathfunc.cpp`): the angle in degrees from +Y to
/// `v1 - v2`, negative when that vector points to -X. Positions are in game world coordinates.
pub fn world_angle_between(v1: Vec3, v2: Vec3) -> f32 {
    let facing = Vec3::new(0.0, 1.0, 0.0);
    let goal = v1 - v2;
    let mag_product = facing.length() * goal.length();
    let mut angle = if mag_product == 0.0 {
        0.0
    } else {
        // `my_acos` clamps its argument.
        (facing.dot(goal) / mag_product)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    };
    let z = facing.x * goal.y - facing.y * goal.x;
    if z > 0.0 {
        angle = -angle;
    }
    angle
}

/// A reference to a running effect. Stale handles (the effect finished and its slot was reused)
/// are detected by the generation and simply report "not alive".
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FxHandle {
    index: u32,
    generation: u32,
}

/// The four effect names `effects.csv` gives a weapon.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WeaponFx {
    /// The in-flight effect (`PP2K Effect Name`): spawned looping, killed when the bolt ends.
    Trail,
    MuzzleFlash,
    Hit,
    Miss,
}

/// How to place and run a new effect.
#[derive(Clone, Debug)]
pub struct SpawnParams {
    /// The parent-to-world matrix the C++ passes in `ExecuteInfo` (see [`FxTransform`]).
    pub transform: Mat,
    /// Run forever until killed (`LoopFlag`), as the bolt trail does.
    pub looped: bool,
    /// Simulation-space bits (`LOCAL`, `DYNAMIC_WORLD`, `STATIC_WORLD`); local is what every
    /// weapon effect uses.
    pub simulation_mode: u32,
    /// Follow an anchor (see [`FxSystem::set_anchor`]) through an offset matrix.
    pub attach: Option<Attachment>,
    /// Fixed effect seed instead of a random one.
    pub seed: Option<f32>,
}

impl SpawnParams {
    pub fn at(transform: Mat) -> SpawnParams {
        SpawnParams {
            transform,
            looped: false,
            simulation_mode: 0,
            attach: None,
            seed: None,
        }
    }
    pub fn looped(mut self) -> SpawnParams {
        self.looped = true;
        self
    }
    pub fn attached(mut self, anchor: u64, offset: Mat) -> SpawnParams {
        self.attach = Some(Attachment { anchor, offset });
        self
    }
}

/// Follow `anchor` (the id the app registered with [`FxSystem::set_anchor`], for example an
/// object id) with `offset` applied first: parent-to-world = `offset * anchor`.
#[derive(Copy, Clone, Debug)]
pub struct Attachment {
    pub anchor: u64,
    pub offset: Mat,
}

struct Instance {
    effect: Effect,
    ground_fade: bool,
    parent_to_world: Mat,
    attach: Option<Attachment>,
    bounds: Obb,
    /// The owner's draw condition is false: the effect keeps running (and following) but is not drawn.
    hidden: bool,
}

struct Slot {
    generation: u32,
    instance: Option<Instance>,
}

pub struct FxSystem {
    library: Arc<EffectLibrary>,
    weapons: WeaponEffects,
    rng: VisualRng,
    slots: Vec<Slot>,
    free: Vec<u32>,
    anchors: HashMap<u64, Mat>,
    time: f64,
}

impl FxSystem {
    pub fn new(library: EffectLibrary, weapons: WeaponEffects, seed: u32) -> FxSystem {
        FxSystem {
            library: Arc::new(library),
            weapons,
            rng: VisualRng::new(seed),
            slots: Vec::new(),
            free: Vec::new(),
            anchors: HashMap::new(),
            time: 0.0,
        }
    }

    /// Loads `data/effects/mc2.fx` and `data/objects/effects.csv` through the game VFS.
    pub fn from_vfs(vfs: &mut Vfs, seed: u32) -> Result<FxSystem> {
        let lib_bytes = vfs
            .read(LIBRARY_PATH)
            .map_err(|e| FxError::new(format!("{LIBRARY_PATH}: {e}")))?
            .ok_or_else(|| FxError::new(format!("{LIBRARY_PATH} not found")))?;
        let library = EffectLibrary::load(&lib_bytes)?;
        let csv = vfs
            .read(EFFECTS_CSV_PATH)
            .map_err(|e| FxError::new(format!("{EFFECTS_CSV_PATH}: {e}")))?
            .ok_or_else(|| FxError::new(format!("{EFFECTS_CSV_PATH} not found")))?;
        let weapons = WeaponEffects::from_csv_bytes(&csv)?;
        Ok(FxSystem::new(library, weapons, seed))
    }

    pub fn library(&self) -> &EffectLibrary {
        &self.library
    }

    pub fn weapons(&self) -> &WeaponEffects {
        &self.weapons
    }

    /// The system clock: the time of the last [`update`](Self::update).
    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn active_count(&self) -> usize {
        self.slots.iter().filter(|s| s.instance.is_some()).count()
    }

    /// Creates the effect called `name` (case-insensitive, `EffectLibrary::Find`) and starts it
    /// at the current time. `None` for a name the library does not have, including `NONE`.
    pub fn spawn(&mut self, name: &str, params: SpawnParams) -> Option<FxHandle> {
        if name == NONE {
            return None;
        }
        let spec = self.library.find(name)?.clone();
        Some(self.spawn_spec(spec, params))
    }

    /// Starts a specific specification (the library may hold several with the same name).
    pub fn spawn_spec(&mut self, spec: Arc<EffectSpec>, params: SpawnParams) -> FxHandle {
        let mut flags =
            EXECUTE_FLAG | (params.simulation_mode & crate::effect::SIMULATION_MODE_MASK);
        if params.looped {
            flags |= LOOP_FLAG;
        }
        let ground_fade = spec.base.name.to_ascii_lowercase().contains("explosion");
        let mut effect = Effect::from_spec(spec, flags);
        let mut parent_to_world = params.transform;
        if let Some(a) = &params.attach {
            if let Some(anchor) = self.anchors.get(&a.anchor) {
                parent_to_world = Mat::multiply(&a.offset, anchor);
            }
        }
        let mut info = ExecuteInfo::new(self.time, parent_to_world);
        if let Some(seed) = params.seed {
            info.seed = seed;
        }
        let mut ctx = Ctx {
            lib: &self.library,
            rng: &mut self.rng,
        };
        effect.start(&mut ctx, &info);
        let instance = Instance {
            effect,
            ground_fade,
            parent_to_world,
            attach: params.attach,
            bounds: Obb::IDENTITY,
            hidden: false,
        };
        self.insert(instance)
    }

    /// Spawns one of a weapon's effects by `effects.csv` row (`WeaponEffects::GetEffect*Name`).
    /// The trail loops (`ExecuteFlag|LoopFlag`), the others run once, as `WeaponBolt` makes them.
    pub fn spawn_weapon(
        &mut self,
        effect_id: i64,
        which: WeaponFx,
        mut params: SpawnParams,
    ) -> Option<FxHandle> {
        let name = match which {
            WeaponFx::Trail => self.weapons.effect_name(effect_id),
            WeaponFx::MuzzleFlash => self.weapons.muzzle_flash_name(effect_id),
            WeaponFx::Hit => self.weapons.hit_effect_name(effect_id),
            WeaponFx::Miss => self.weapons.miss_effect_name(effect_id),
        }
        .to_string();
        params.looped |= which == WeaponFx::Trail;
        self.spawn(&name, params)
    }

    fn insert(&mut self, instance: Instance) -> FxHandle {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.instance = Some(instance);
            FxHandle {
                index,
                generation: slot.generation,
            }
        } else {
            self.slots.push(Slot {
                generation: 0,
                instance: Some(instance),
            });
            FxHandle {
                index: (self.slots.len() - 1) as u32,
                generation: 0,
            }
        }
    }

    fn instance_mut(&mut self, h: FxHandle) -> Option<&mut Instance> {
        let slot = self.slots.get_mut(h.index as usize)?;
        if slot.generation != h.generation {
            return None;
        }
        slot.instance.as_mut()
    }

    fn instance(&self, h: FxHandle) -> Option<&Instance> {
        let slot = self.slots.get(h.index as usize)?;
        if slot.generation != h.generation {
            return None;
        }
        slot.instance.as_ref()
    }

    /// Whether the effect is still running (it finishes on its own unless looped).
    pub fn is_alive(&self, h: FxHandle) -> bool {
        self.instance(h).is_some_and(|i| i.effect.is_executed())
    }

    /// Moves a free-standing effect (a bolt trail following the projectile).
    pub fn set_transform(&mut self, h: FxHandle, parent_to_world: Mat) {
        if let Some(i) = self.instance_mut(h) {
            i.parent_to_world = parent_to_world;
            i.attach = None;
        }
    }

    /// Attaches the effect to an anchor from now on.
    pub fn attach(&mut self, h: FxHandle, anchor: u64, offset: Mat) {
        if let Some(i) = self.instance_mut(h) {
            i.attach = Some(Attachment { anchor, offset });
        }
    }

    /// Stops following; the effect stays where it is.
    pub fn detach(&mut self, h: FxHandle) {
        if let Some(i) = self.instance_mut(h) {
            i.attach = None;
        }
    }

    /// The app reports where a moving object is, every frame, before [`update`](Self::update).
    /// Attached effects follow it; if the anchor is never set again (the object is gone) they
    /// keep the last placement.
    pub fn set_anchor(&mut self, anchor: u64, transform: Mat) {
        self.anchors.insert(anchor, transform);
    }

    pub fn remove_anchor(&mut self, anchor: u64) {
        self.anchors.remove(&anchor);
    }

    /// `SetLoopOn()` / `SetLoopOff()` followed by `SetExecuteOn()`: how an effect that follows its object is told to keep going or
    /// to run out (`waterWake`, a vehicle's `dustCloud`).
    pub fn set_looped(&mut self, h: FxHandle, looped: bool) {
        if let Some(i) = self.instance_mut(h) {
            if looped {
                i.effect.set_loop_on();
            } else {
                i.effect.set_loop_off();
            }
        }
    }

    /// Whether the effect is drawn: the appearances skip the `Draw` of an effect while a condition holds (a wading mech's foot
    /// dust) but keep executing it.
    pub fn set_hidden(&mut self, h: FxHandle, hidden: bool) {
        if let Some(i) = self.instance_mut(h) {
            i.hidden = hidden;
        }
    }

    /// `Effect::Kill`: stop now, children and particles included.
    pub fn kill(&mut self, h: FxHandle) {
        if let Some(i) = self.instance_mut(h) {
            i.effect.kill();
        }
    }

    /// `Effect::Stop`: end the effect's own life but let particles and children run out. How a
    /// looping trail is wound down when its bolt hits.
    pub fn stop(&mut self, h: FxHandle) {
        if let Some(i) = self.instance_mut(h) {
            i.effect.set_loop_off();
            i.effect.stop();
        }
    }

    /// The effect's bounding sphere from the last update (centre and radius in the gosFX frame).
    pub fn bounds(&self, h: FxHandle) -> Option<(Vec3, f32)> {
        let i = self.instance(h)?;
        let b = &i.bounds;
        let centre = Mat::multiply(&b.local_to_parent, &i.parent_to_world).translation();
        Some((centre, b.sphere_radius))
    }

    /// `Effect::Execute` for everything at `time` (seconds, the same clock the effects were
    /// started on); finished effects are dropped. Time never runs backwards: an earlier time is
    /// treated as no time having passed.
    pub fn update(&mut self, time: f64) {
        let time = time.max(self.time);
        self.time = time;
        for index in 0..self.slots.len() {
            let Some(inst) = self.slots[index].instance.as_mut() else {
                continue;
            };
            if let Some(a) = &inst.attach {
                if let Some(anchor) = self.anchors.get(&a.anchor) {
                    inst.parent_to_world = Mat::multiply(&a.offset, anchor);
                }
            }
            let info = ExecuteInfo::new(time, inst.parent_to_world);
            let mut ctx = Ctx {
                lib: &self.library,
                rng: &mut self.rng,
            };
            let mut bounds = Obb::IDENTITY;
            let alive = inst.effect.execute(&mut ctx, &info, &mut bounds);
            inst.bounds = bounds;
            if !alive {
                let slot = &mut self.slots[index];
                slot.instance = None;
                slot.generation = slot.generation.wrapping_add(1);
                self.free.push(index as u32);
            }
        }
    }

    /// Appends the draw data of every running effect.
    pub fn draw(&self, camera_to_world: &Mat, out: &mut DrawList) {
        for slot in &self.slots {
            if let Some(inst) = &slot.instance {
                // A killed effect is gone at once, before the next update drops it.
                if !inst.effect.is_executed() || inst.hidden {
                    continue;
                }
                let info = DrawInfo::new(inst.parent_to_world, *camera_to_world);
                let previous_ground_fade = out.ground_fade;
                out.ground_fade = inst.ground_fade;
                inst.effect.draw(&info, out);
                out.ground_fade = previous_ground_fade;
            }
        }
    }

    /// Appends the draw data of the one effect `h` (nothing if it is gone or killed).
    pub fn draw_one(&self, h: FxHandle, camera_to_world: &Mat, out: &mut DrawList) {
        if let Some(inst) = self.instance(h) {
            if inst.effect.is_executed() && !inst.hidden {
                let info = DrawInfo::new(inst.parent_to_world, *camera_to_world);
                let previous_ground_fade = out.ground_fade;
                out.ground_fade = inst.ground_fade;
                inst.effect.draw(&info, out);
                out.ground_fade = previous_ground_fade;
            }
        }
    }

    /// Direct access for tests and tools.
    pub fn effect(&self, h: FxHandle) -> Option<&Effect> {
        self.instance(h).map(|i| &i.effect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_angle_between_matches_the_c_plus_plus() {
        let o = Vec3::ZERO;
        // v1 - v2 = (0, 1, 0): straight along +Y -> 0
        assert!((world_angle_between(Vec3::new(0.0, 1.0, 0.0), o)).abs() < 1e-4);
        // (1, 0, 0): 90 degrees, z = -goal.x = -1 <= 0 so positive
        assert!((world_angle_between(Vec3::new(1.0, 0.0, 0.0), o) - 90.0).abs() < 1e-4);
        // (-1, 0, 0): 90 degrees, z = 1 > 0 so negated
        assert!((world_angle_between(Vec3::new(-1.0, 0.0, 0.0), o) + 90.0).abs() < 1e-4);
        // zero vector: 0
        assert_eq!(world_angle_between(o, o), 0.0);
    }

    #[test]
    fn bolt_placement_translates_to_the_stuff_frame() {
        let m = FxTransform::bolt(Vec3::new(10.0, 20.0, 30.0), Vec3::new(10.0, 120.0, 30.0));
        assert_eq!(m.translation(), Vec3::new(-10.0, 30.0, 20.0));
    }
}
