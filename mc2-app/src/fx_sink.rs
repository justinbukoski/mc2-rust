// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Weapon and combat effects: the sim's presentation events turned into `mc2-fx` gosFX effects, and the scene pass that draws them.
//!
//! The C++ sim creates gosFX effects straight from game code; the port's sim only pushes [`SimEvent`]s. [`FxHub::new`] builds the
//! pair the game needs: a [`FxEventSink`] (push it into `GameCore::sinks`) and a [`FxScene`] ([`SceneEffects`], assign it to
//! `GameCore::effects`), sharing one [`mc2_fx::system::FxSystem`]. Effects are purely visual and draw their random numbers from the
//! fx system's own generator, never from the simulation's.
//!
//! What each event becomes (names come from `data/objects/effects.csv` through [`WeaponEffects`], never from a table here):
//!
//! | event | effect | C++ |
//! |---|---|---|
//! | [`SimEvent::Explosion`] | `GetEffectName(effectId)`, once (`ExecuteFlag`), at `(-x, z, y)` with no rotation | `Carnage::init` carnage.cpp:904-916, started in `Carnage::update` carnage.cpp:654-675 |
//! | [`SimEvent::AirstrikeContrail`] | `GetEffectName(JET_CONTRAIL_ID)`, once, at the bomber's position | `Artillery::update` artlry.cpp:938-962 |
//! | [`SimEvent::WeaponEffect`] muzzle | `GetEffectMuzzleFlashName(effectId)`, once, `Effect_Into_Motion * Rot(owner yaw)` at the muzzle, only with the non-weapon-effects preference | `WeaponBolt::init` weaponbolt.cpp:2382-2398, started weaponbolt.cpp:477-502 |
//! | [`SimEvent::WeaponEffect`] `Hit` | `GetEffectHitName(effectId)`, once, plain translation at the impact point | `WeaponBolt::init` weaponbolt.cpp:2402-2418, started weaponbolt.cpp:627-641 |
//! | [`SimEvent::WeaponEffect`] `Miss` on land | `GetEffectMissName(effectId)`, once, plain translation at the ground point, only with the non-weapon-effects preference | `WeaponBolt::init` weaponbolt.cpp:2422-2437, started weaponbolt.cpp:696-708 |
//! | [`SimEvent::WeaponEffect`] `Miss` on water | `GetEffectName(WATER_MISS_FX = 76)` (not for effect 11, machine guns), once, at `(-x, waterElevation, y)`, only with the preference | weaponbolt.cpp:2440-2450, started weaponbolt.cpp:661-682 |
//!
//! The in-flight trail and beam are not event driven: [`FxHub::handle_events`] reads every live `WeaponBolt` (`BoltState`) from the
//! sim world after each step. A bolt that has no effect yet starts `GetEffectName(effectId)` looping (`ExecuteFlag|LoopFlag`,
//! `WeaponBolt::init` weaponbolt.cpp:2362-2378; first `Start` weaponbolt.cpp:457-474); every later step moves it
//! (`WeaponBolt::update` weaponbolt.cpp:1154-1193): `Effect_Against_Motion * Rot(-world_angle_between(laserPosition, targetPosition))`
//! at `laserPosition`, or at the target position for a beam (`isBeam`). A bolt that has ended winds its effect down
//! (`FxSystem::stop`; the C++ kills it `afterHitTime` after the hit, weaponbolt.cpp:1345).
//!
//! The faces `WeaponBolt::render` draws itself (every live beam from the muzzle to the target, a projectile whose trail is gone) are
//! built by [`crate::bolt_geom`] and added to the same scene pass.
//!
//! Craters and footprints are not gosFX effects: [`SimEvent::Crater`] (`CraterManager::addCrater`) goes into the hub's
//! [`CraterPool`] (the C++ ring of 1000, dropped onto the terrain of the event's world) and [`FxHub::decal_batches`] draws it
//! through `mc2_render::crater` in the scene pass; a new mission empties it. Events that arrive without a world (no terrain to drop
//! them on) or with the non-weapon effects off (`addCrater` returns at once) add nothing.
//!
//! [`SimEvent::Flash`] (`flashBuilding`) is not an effect: it is an appearance tint, kept by `LiveSim` and drawn through
//! `mc2_render::flash::BuildingFlash`, so the hub plans nothing for it and reports nothing unavailable.

use crate::bolt_geom::{self, BeamLook, BoltLook};
use crate::bomber::BomberRig;
use crate::layers::SceneEffects;
use crate::sim::{FrameContext, SimDriver, SimEventSink};
use mc2_formats::vfs::Vfs;
use mc2_fx::draw::DrawList;
use mc2_fx::math::{Mat, Vec3 as FxVec3};
use mc2_fx::render::{self, FxTextures};
use mc2_fx::rng::VisualRng;
use mc2_fx::system::{
    game_to_fx, world_angle_between, FxHandle, FxSystem, FxTransform, SpawnParams, WeaponFx,
};
use mc2_fx::weaponfx::{WeaponEffects, NONE};
use mc2_render::camera::GameCamera;
use mc2_render::crater::{
    crater_batches, CraterTextures, Decal, CRATER_TEXTURE, FOOTPRINT_TEXTURE,
};
use mc2_render::fx::{FxBatch, FxTextureId};
use mc2_render::scene::SceneRenderer;
use mc2_sim::combat::weapon_bolt::{bolt_params, bolt_state};
use mc2_sim::crater::CraterPool;
use mc2_sim::math::Vec3;
use mc2_sim::ObjectHandle;
use mc2_sim::{FxAction, FxBase, FxEffect, FxPlacement, FxSlot, SimEvent, WeaponImpact};
use mc2_sim_data::objtype::ObjectTypeData;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

/// Why an effect was spawned.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FxRole {
    /// An effect attached to an appearance (`Mech3DAppearance`, `GVAppearance`, `BldgAppearance`, the support VTOL).
    Appearance(FxSlot),
    /// A carnage explosion (`SimEvent::Explosion`).
    Explosion,
    /// A weapon's muzzle flash.
    MuzzleFlash,
    /// A weapon's hit effect.
    Hit,
    /// A weapon's miss effect on land.
    Miss,
    /// The water-miss effect.
    WaterMiss,
    /// The bomber's jet contrail (`SimEvent::AirstrikeContrail`).
    Contrail,
}

/// One effect the mapping asks for, before it is spawned.
#[derive(Clone, Debug)]
pub struct Planned {
    pub role: FxRole,
    /// The `effects.csv` effect name.
    pub name: String,
    /// Where it is (game world), for tests and diagnostics.
    pub at: Vec3,
    /// The parent-to-world matrix the C++ passes in `ExecuteInfo`.
    pub transform: Mat,
    /// The strike a bomber contrail belongs to: `(strike, turn the bomb run started on, strike position)`. Set by the hub (it
    /// knows the turn); the pure mapping leaves it empty.
    pub strike: Option<(mc2_sim::WatchId, i64, Vec3)>,
}

/// A bomber contrail that follows the bomber's animated `activity_node` until the strike is over (`artlry.cpp:986-1041`).
struct Trail {
    start_turn: i64,
    at: Vec3,
    handle: FxHandle,
}

/// The parent-to-world matrix of an attached effect: `localToWorld * shapeOrigin` of the C++, the base rotation (none, `Into_Motion`
/// or `Against_Motion`) in front of `EulerAngles(pitch, yaw, 0)`, then the translation to `(-x, z, y)`.
pub fn placement_matrix(p: &FxPlacement) -> Mat {
    let mut rot = Mat::IDENTITY;
    rot.build_rotation_euler(p.pitch.to_radians(), p.yaw.to_radians(), 0.0);
    let local_to_world = match p.base {
        FxBase::Plain => rot,
        FxBase::IntoMotion => Mat::multiply(&mc2_fx::system::EFFECT_INTO_MOTION, &rot),
        FxBase::AgainstMotion => Mat::multiply(&mc2_fx::system::EFFECT_AGAINST_MOTION, &rot),
    };
    Mat::multiply(
        &local_to_world,
        &Mat::from_translation(game_to_fx(p.at.x, p.at.y, p.at.z)),
    )
}

/// One call on an effect attached to an appearance, in event order.
#[derive(Clone, Debug)]
struct AttachedOp {
    owner: ObjectHandle,
    slot: FxSlot,
    effect: FxEffect,
    action: FxAction,
}

/// What one event maps to: effects to spawn and the parts that cannot be shown.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub spawn: Vec<Planned>,
    pub unavailable: Vec<String>,
}

/// `WATER_MISS_FX` (mclib/weaponfx.h:41).
const WATER_MISS_FX: i64 = 76;
/// Whether a miss point is over a deep or shallow water cell of the map: `GameMap->getDeepWater(cellR, cellC) ||
/// GameMap->getShallowWater(cellR, cellC)` after `land->worldToCell` (weaponbolt.cpp:661-667), not the interpolated elevation.
pub fn over_water_cell(terrain: &mc2_sim::terrain::Terrain, at: &Vec3) -> bool {
    let (r, c) = terrain.world_to_cell(at);
    terrain.get_deep_water(r, c) || terrain.get_shallow_water(r, c)
}

/// Machine guns have no water miss (weaponbolt.cpp:2440).
const MACHINE_GUN_EFFECT: i64 = 11;

fn named(name: &str) -> bool {
    // a shipped cell like `NONE ` (row 64) is not in the effect library either way: no effect, so no report
    let name = name.trim();
    !name.is_empty() && name != NONE
}

/// The mapping of one event (pure: no effect is created). `owner_yaw` is the owner's `getRotation()` in degrees (`None` when the
/// owner is gone: the C++ then uses 0). `water` is `(impact point is over water, water elevation)` for a `Miss` (`None` = not known:
/// the miss is then reported unavailable rather than guessed).
pub fn plan_event(
    event: &SimEvent,
    weapons: &WeaponEffects,
    use_non_weapon_effects: bool,
    owner_yaw: Option<f32>,
    water: Option<(bool, f32)>,
) -> Plan {
    let mut plan = Plan::default();
    match event {
        SimEvent::Explosion { effect_id, at, .. } => {
            let name = weapons.effect_name(i64::from(*effect_id));
            if named(name) {
                plan.spawn.push(Planned {
                    role: FxRole::Explosion,
                    strike: None,
                    name: name.to_string(),
                    at: *at,
                    transform: FxTransform::at(game_to_fx(at.x, at.y, at.z)),
                });
            }
        }
        SimEvent::AirstrikeContrail { effect_id, at, .. } => {
            // `rightContrail->Start` at the bomber's node, plain translation, no rotation (artlry.cpp:950-962)
            let name = weapons.effect_name(i64::from(*effect_id));
            if named(name) {
                plan.spawn.push(Planned {
                    role: FxRole::Contrail,
                    strike: None,
                    name: name.to_string(),
                    at: *at,
                    transform: FxTransform::at(game_to_fx(at.x, at.y, at.z)),
                });
            }
        }
        SimEvent::WeaponEffect {
            effect_id,
            from,
            to,
            impact,
            ..
        } => {
            let id = i64::from(*effect_id);
            let muzzle = weapons.muzzle_flash_name(id);
            if *impact == WeaponImpact::Launch && named(muzzle) && use_non_weapon_effects {
                plan.spawn.push(Planned {
                    role: FxRole::MuzzleFlash,
                    strike: None,
                    name: muzzle.to_string(),
                    at: *from,
                    transform: FxTransform::muzzle(
                        FxVec3::new(from.x, from.y, from.z),
                        owner_yaw.unwrap_or(0.0),
                    ),
                });
            }
            match impact {
                WeaponImpact::Launch => {}
                WeaponImpact::Hit => {
                    let hit = weapons.hit_effect_name(id);
                    if named(hit) {
                        plan.spawn.push(Planned {
                            role: FxRole::Hit,
                            strike: None,
                            name: hit.to_string(),
                            at: *to,
                            transform: FxTransform::at(game_to_fx(to.x, to.y, to.z)),
                        });
                    }
                }
                WeaponImpact::Miss if !use_non_weapon_effects => {}
                WeaponImpact::Miss => match water {
                    None => plan
                        .unavailable
                        .push("weapon miss effect (no terrain to tell land from water)".into()),
                    Some((true, elevation)) => {
                        let name = weapons.effect_name(WATER_MISS_FX);
                        if id != MACHINE_GUN_EFFECT && named(name) {
                            plan.spawn.push(Planned {
                                role: FxRole::WaterMiss,
                                strike: None,
                                name: name.to_string(),
                                at: *to,
                                transform: FxTransform::at(game_to_fx(to.x, to.y, elevation)),
                            });
                        }
                    }
                    Some((false, _)) => {
                        let miss = weapons.miss_effect_name(id);
                        if named(miss) {
                            plan.spawn.push(Planned {
                                role: FxRole::Miss,
                                strike: None,
                                name: miss.to_string(),
                                at: *to,
                                transform: FxTransform::at(game_to_fx(to.x, to.y, to.z)),
                            });
                        }
                    }
                },
            }
        }
        _ => {}
    }
    plan
}

/// Where a bolt's effect stands this frame (weaponbolt.cpp:1154-1187).
fn bolt_transform(b: &BoltFlight) -> Mat {
    let fx = |v: Vec3| FxVec3::new(v.x, v.y, v.z);
    let rotation = -world_angle_between(fx(b.position), fx(b.target));
    // a beam is played at the hit location, a projectile at `laserPosition`
    let at = if b.is_beam { b.target } else { b.position };
    FxTransform::against_motion(game_to_fx(at.x, at.y, at.z), rotation)
}

/// How the type of bolt `h` looks.
fn bolt_look(world: &mc2_sim::World, h: ObjectHandle) -> Option<BoltLook> {
    match &world.objects.type_of(h)?.data {
        ObjectTypeData::WeaponBolt(t) => Some(BoltLook::of_type(t)),
        _ => None,
    }
}

/// Every bolt that is flying in `world` (`WeaponBolt`s that exist and have a target to fly to).
fn live_bolts(world: &mc2_sim::World) -> Vec<BoltFlight> {
    let mut out = Vec::new();
    for &h in &world.objects.weapons {
        let Some(o) = world.objects.get(h) else {
            continue;
        };
        if !o.exists() {
            continue;
        }
        let (Some(b), Some(p)) = (bolt_state(world, h), bolt_params(world, h)) else {
            continue;
        };
        let Some(look) = bolt_look(world, h) else {
            continue;
        };
        // a bolt not yet connected to a target is not updated by the C++ either (weaponbolt.cpp:436-441)
        if b.just_created || !b.has_target_position || b.hit_target {
            continue;
        }
        out.push(BoltFlight {
            key: b.shot_id,
            effect_id: b.effect_id,
            is_beam: p.is_beam,
            position: b.laser_position,
            target: b.target_position,
            origin: o.c.position,
            elapsed: p.beam_duration - b.time_left,
            look,
        });
    }
    out
}

/// An effect the hub started (diagnostics and tests).
#[derive(Clone, Debug)]
pub struct Spawned {
    pub role: FxRole,
    pub name: String,
    pub at: Vec3,
    pub handle: FxHandle,
}

struct Inner {
    fx: FxSystem,
    textures: HashMap<String, FxTextureId>,
    use_non_weapon_effects: bool,
    /// Effect time at game time 0 of the running mission (a new mission restarts the game clock).
    base: f64,
    pending: Vec<Planned>,
    /// Calls on attached effects not applied yet.
    attached_ops: Vec<AttachedOp>,
    /// The effect each appearance member runs (`rightDustPoofEffect[1]`, `waterWake`, ...).
    attached: HashMap<(ObjectHandle, FxSlot), FxHandle>,
    /// The effect name each member last started, kept after the effect ran out: `SetLoopOn` + `SetExecuteOn` runs it again
    /// (gvactor.cpp:2473, mech3d.cpp:3580). Dropped on `Stop` and `Kill`.
    attached_name: HashMap<(ObjectHandle, FxSlot), String>,
    /// Members whose draw condition is false (`FxAction::Draw(false)`); a started effect of the member is hidden.
    hidden: HashSet<(ObjectHandle, FxSlot)>,
    spawned: Vec<Spawned>,
    /// The airstrike bomber's animation (where its contrail node is on each frame), once a mission has loaded it.
    bomber: Option<Rc<BomberRig>>,
    trails: Vec<Trail>,
    unavailable: BTreeMap<String, u64>,
    logged: HashSet<String>,
    /// The live bolts the sim reported after its last step.
    bolts: Vec<BoltFlight>,
    /// The in-flight effect of each bolt that has one.
    bolt_fx: Vec<(BoltKey, FxHandle)>,
    /// Bolts whose effect was started once (a finished effect is not started again).
    started_bolts: Vec<BoltKey>,
    /// Trails of bolts that ended, stopped but still fading (a mission reset kills them).
    winding: Vec<FxHandle>,
    /// Beam texture names the object types name that are not uploaded yet.
    pending_textures: Vec<String>,
    /// Library effects the appearance types name (`destructEffect`) whose textures are not uploaded yet.
    pending_effects: Vec<String>,
    /// Whether the faces `WeaponBolt::render` draws itself (beams, trail-less projectiles) are drawn (a diagnostic switch for tests).
    draw_bolt_faces: bool,
    /// The visual stream the beam wiggle draws from (`weaponbolt.cpp:1304-1306`); not the sim's.
    wiggle_rng: VisualRng,
    /// `craterManager`: the decals of the running mission.
    craters: CraterPool,
    /// The uploaded `feet0000` / `feet0001` textures (`None` before they are loaded or when the data lacks them).
    crater_textures: CraterTextures,
}

/// Which shot an in-flight effect belongs to. Pool handles are reused; shot IDs are not.
type BoltKey = u32;

/// One live bolt as the scene needs it.
#[derive(Clone, Debug)]
struct BoltFlight {
    key: BoltKey,
    effect_id: i32,
    is_beam: bool,
    /// `laserPosition`.
    position: Vec3,
    /// `*targetPosition`.
    target: Vec3,
    /// The bolt's own position (the muzzle for a beam, `follow_owner`).
    origin: Vec3,
    /// `beamDuration - timeLeft` (beams).
    elapsed: f32,
    /// The type data the faces come from.
    look: BoltLook,
}

impl Inner {
    fn note(&mut self, what: &str) {
        *self.unavailable.entry(what.to_string()).or_insert(0) += 1;
        if self.logged.insert(what.to_string()) {
            log::warn!("fx unavailable: {what}");
        }
    }

    /// Moves effect time to `game_time` and starts what the sim asked for since the last call, at that time.
    fn advance_to(&mut self, game_time: f64) {
        self.fx.update(self.base + game_time);
        for p in std::mem::take(&mut self.pending) {
            let h = self.fx.spawn(&p.name, SpawnParams::at(p.transform));
            match h {
                Some(handle) => {
                    if let Some((_, start_turn, at)) = p.strike {
                        self.trails.push(Trail {
                            start_turn,
                            at,
                            handle,
                        });
                    }
                    self.spawned.push(Spawned {
                        role: p.role,
                        name: p.name,
                        at: p.at,
                        handle,
                    });
                }
                None => self.note(&format!("effect '{}' is not in the effect library", p.name)),
            }
        }
        for op in std::mem::take(&mut self.attached_ops) {
            self.apply_attached(op);
        }
        let fx = &self.fx;
        self.attached.retain(|_, h| fx.is_alive(*h));
        self.sync_bolt_effects();
    }

    /// Re-places every bomber contrail at the bomber's animated `activity_node` (`rightContrail->Execute` with the node position,
    /// `artlry.cpp:1013-1041`); the contrail ends with the bomb run, not with the sim strike object (`rightContrail->Kill`, `artlry.cpp:922-927`).
    fn sync_trails(&mut self, world: &mc2_sim::World) {
        let Some(rig) = self.bomber.clone() else {
            return;
        };
        let trails = std::mem::take(&mut self.trails);
        for t in trails {
            let steps = world.clock.turn - t.start_turn;
            let over = rig.run_finished(steps, world.clock.frame_length);
            if over || !self.fx.is_alive(t.handle) {
                self.fx.kill(t.handle);
                continue;
            }
            let frame = rig.frame_after(steps, world.clock.frame_length);
            let p = rig.activity_offset(frame);
            let m = FxTransform::at(game_to_fx(t.at.x + p.x, t.at.y + p.y, t.at.z + p.z));
            self.fx.set_transform(t.handle, m);
            self.trails.push(t);
        }
    }

    /// Ends the attached effects whose owner no longer exists: the appearance that owned them is gone, so nothing would stop a
    /// looping dust cloud or wake. Support-unit effects (no owner object) stay.
    fn end_effects_of_gone_owners(&mut self, w: &mc2_sim::World) {
        // the one-shot destruct effect outlives the `exists` flag of a destroyed object
        let gone =
            |o: ObjectHandle| !o.is_none() && w.objects.get(o).is_none_or(|obj| !obj.exists());
        let doomed: Vec<_> = self
            .attached
            .keys()
            .filter(|(o, s)| gone(*o) && *s != FxSlot::Destruct)
            .copied()
            .collect();
        for key in doomed {
            if let Some(h) = self.attached.remove(&key) {
                self.fx.kill(h);
            }
        }
        self.attached_name.retain(|(o, _), _| !gone(*o));
        self.hidden.retain(|(o, _)| !gone(*o));
    }

    /// One call on an effect attached to an appearance: `Start` makes (or restarts) the member's effect, `Execute` moves it,
    /// `Stop` winds it down, `Kill` ends it.
    fn apply_attached(&mut self, op: AttachedOp) {
        let key = (op.owner, op.slot);
        match op.action {
            FxAction::Start { place, looping } => {
                let name = match &op.effect {
                    FxEffect::Id(id) => self.fx.weapons().effect_name(i64::from(*id)).to_string(),
                    FxEffect::Named(n) => n.clone(),
                };
                if !named(&name) {
                    return; // `GetEffectName(id)` is `NONE`: no effect, as the C++
                }
                if let Some(old) = self.attached.remove(&key) {
                    self.fx.kill(old); // the C++ restarts its one `gosFX::Effect*`
                }
                let transform = placement_matrix(&place);
                let mut params = SpawnParams::at(transform);
                if looping {
                    params = params.looped();
                }
                match self.fx.spawn(&name, params) {
                    Some(handle) => {
                        self.fx.set_hidden(handle, self.hidden.contains(&key));
                        self.attached.insert(key, handle);
                        self.attached_name.insert(key, name.clone());
                        self.spawned.push(Spawned {
                            role: FxRole::Appearance(op.slot),
                            name,
                            at: place.at,
                            handle,
                        });
                    }
                    None => self.note(&format!("effect '{name}' is not in the effect library")),
                }
            }
            FxAction::Move { place, looping } => {
                if let Some(&h) = self.attached.get(&key) {
                    self.fx.set_transform(h, placement_matrix(&place));
                    if let Some(l) = looping {
                        self.fx.set_looped(h, l);
                    }
                } else if looping == Some(true) {
                    // The effect ran out while the object stood still; moving again runs it again, here.
                    if let Some(name) = self.attached_name.get(&key).cloned() {
                        let params = SpawnParams::at(placement_matrix(&place)).looped();
                        if let Some(handle) = self.fx.spawn(&name, params) {
                            self.fx.set_hidden(handle, self.hidden.contains(&key));
                            self.attached.insert(key, handle);
                        }
                    }
                }
            }
            FxAction::Stop => {
                self.attached_name.remove(&key);
                if let Some(h) = self.attached.remove(&key) {
                    self.fx.stop(h);
                    self.winding.push(h);
                }
            }
            FxAction::Kill => {
                self.attached_name.remove(&key);
                if let Some(h) = self.attached.remove(&key) {
                    self.fx.kill(h);
                }
            }
            FxAction::Draw(on) => {
                if on {
                    self.hidden.remove(&key);
                } else {
                    self.hidden.insert(key);
                }
                if let Some(&h) = self.attached.get(&key) {
                    self.fx.set_hidden(h, !on);
                }
            }
        }
    }

    /// Starts the in-flight effect of bolts that have none, moves the others (`WeaponBolt::update`), and winds down those of bolts
    /// that are gone.
    fn sync_bolt_effects(&mut self) {
        let bolts = std::mem::take(&mut self.bolts);
        let tracked = std::mem::take(&mut self.bolt_fx);
        let mut next: Vec<(BoltKey, FxHandle)> = Vec::new();
        for (key, h) in tracked {
            if !bolts.iter().any(|b| b.key == key) {
                self.fx.stop(h);
                self.winding.push(h);
            } else if self.fx.is_alive(h) {
                next.push((key, h));
            }
            // a finished effect is not restarted: the C++ drops `gosEffect` when `Execute` returns false
        }
        for b in &bolts {
            let m = bolt_transform(b);
            if let Some((_, h)) = next.iter().find(|(k, _)| *k == b.key) {
                self.fx.set_transform(*h, m);
            } else if !self.bolt_started(b) {
                let started = self.fx.spawn_weapon(
                    i64::from(b.effect_id),
                    WeaponFx::Trail,
                    SpawnParams::at(m),
                );
                if let Some(h) = started {
                    next.push((b.key, h));
                    self.started_bolts.push(b.key);
                }
            }
        }
        self.started_bolts
            .retain(|k| bolts.iter().any(|b| b.key == *k));
        let fx = &self.fx;
        self.winding.retain(|h| fx.is_alive(*h));
        self.bolts = bolts;
        self.bolt_fx = next;
    }

    fn bolt_started(&self, b: &BoltFlight) -> bool {
        self.started_bolts.contains(&b.key)
    }
}

/// The shared effect state behind the sink and the scene pass.
#[derive(Clone)]
pub struct FxHub {
    inner: Rc<RefCell<Inner>>,
}

impl FxHub {
    /// Loads the effect library and uploads the textures of every effect `effects.csv` names into `scene`, so the scene pass needs
    /// no loader. Production registration is `core.sinks.push(Box::new(hub.sink())); core.effects = Some(Box::new(hub.scene()))`.
    pub fn new(
        scene: &mut SceneRenderer,
        vfs: &mut Vfs,
        seed: u32,
        use_non_weapon_effects: bool,
    ) -> Result<FxHub, String> {
        let fx = FxSystem::from_vfs(vfs, seed).map_err(|e| e.to_string())?;
        let textures = preload_textures(&fx, vfs, seed, &mut |n, w, h, px| {
            scene.add_fx_texture(n, w, h, px)
        })?;
        let hub = Self::with_system(fx, textures, use_non_weapon_effects);
        hub.upload_crater_textures(vfs, scene);
        Ok(hub)
    }

    /// Uploads `defaults/feet0000.tga` and `feet0001.tga` (`CraterManager::init`, crater.cpp:104-113). A missing file leaves that
    /// kind of decal undrawn and is noted once.
    pub fn upload_crater_textures(&self, vfs: &mut Vfs, scene: &mut SceneRenderer) {
        let mut load = |path: &str| {
            let img = vfs
                .read(path)
                .ok()
                .flatten()
                .and_then(|b| mc2_assets::tga::decode(&b).ok())?;
            Some(scene.add_fx_texture(path, img.width as u32, img.height as u32, &img.pixels))
        };
        let (crater, footprint) = (load(CRATER_TEXTURE), load(FOOTPRINT_TEXTURE));
        let mut inner = self.inner.borrow_mut();
        for (id, name) in [(crater, CRATER_TEXTURE), (footprint, FOOTPRINT_TEXTURE)] {
            if id.is_none() {
                inner.note(&format!("crater texture '{name}' could not be loaded"));
            }
        }
        inner.crater_textures = CraterTextures { crater, footprint };
    }

    /// The decals of the running mission.
    pub fn craters(&self) -> CraterPool {
        self.inner.borrow().craters.clone()
    }

    /// Draw data for the crater and footprint decals, lit by the scene's `ambient` (`CraterManager::render`).
    pub fn decal_batches(&self, ambient: [f32; 3], out: &mut Vec<FxBatch>) {
        let inner = self.inner.borrow();
        // `CraterManager::render` draws nothing while the player has the non-weapon effects off
        if !inner.use_non_weapon_effects {
            return;
        }
        let decals: Vec<Decal> = inner
            .craters
            .iter()
            .map(|c| {
                let (uv, extent) = c.uv();
                Decal {
                    crater: c.is_crater(),
                    corners: c.corners.map(|p| [p.x, p.y, p.z]),
                    uv,
                    extent,
                }
            })
            .collect();
        out.extend(crater_batches(
            decals.iter(),
            inner.crater_textures,
            ambient,
        ));
    }

    /// A hub over a ready effect system; `textures` maps an effect texture file base name to its uploaded id.
    pub fn with_system(
        fx: FxSystem,
        textures: HashMap<String, FxTextureId>,
        use_non_weapon_effects: bool,
    ) -> FxHub {
        FxHub {
            inner: Rc::new(RefCell::new(Inner {
                fx,
                textures,
                use_non_weapon_effects,
                base: 0.0,
                pending: Vec::new(),
                attached_ops: Vec::new(),
                attached: HashMap::new(),
                attached_name: HashMap::new(),
                hidden: HashSet::new(),
                spawned: Vec::new(),
                bomber: None,
                trails: Vec::new(),
                unavailable: BTreeMap::new(),
                logged: HashSet::new(),
                bolts: Vec::new(),
                bolt_fx: Vec::new(),
                started_bolts: Vec::new(),
                winding: Vec::new(),
                pending_textures: Vec::new(),
                pending_effects: Vec::new(),
                draw_bolt_faces: true,
                wiggle_rng: VisualRng::new(0x6265_616d),
                craters: CraterPool::default(),
                crater_textures: CraterTextures::default(),
            })),
        }
    }

    /// The player's `UseNonWeaponEffects` preference (the options screen), for the events that arrive from now on.
    pub fn set_use_non_weapon_effects(&self, on: bool) {
        self.inner.borrow_mut().use_non_weapon_effects = on;
    }

    /// The airstrike bomber the running mission loaded ([`crate::mission_view::MissionView::bomber`]): its contrail follows the
    /// bomber's animated node. Without it the contrail stays where the strike is.
    pub fn set_bomber(&self, bomber: Option<Rc<BomberRig>>) {
        self.inner.borrow_mut().bomber = bomber;
    }

    /// The `SimEventSink` half: `GameCore::sinks.push(Box::new(hub.sink()))`.
    pub fn sink(&self) -> FxEventSink {
        FxEventSink { hub: self.clone() }
    }

    /// The `SceneEffects` half: `GameCore::effects = Some(Box::new(hub.scene()))`.
    pub fn scene(&self) -> FxScene {
        FxScene { hub: self.clone() }
    }

    /// Maps and queues events; the effects start at the next [`advance_to`](Self::advance_to).
    pub fn handle_events(&self, world: Option<&mc2_sim::World>, events: &[SimEvent]) {
        let mut inner = self.inner.borrow_mut();
        if let Some(w) = world {
            inner.bolts = live_bolts(w);
            inner.end_effects_of_gone_owners(w);
        }
        for e in events {
            if let SimEvent::AppearanceFx {
                object,
                slot,
                effect,
                action,
            } = e
            {
                inner.attached_ops.push(AttachedOp {
                    owner: *object,
                    slot: *slot,
                    effect: effect.clone(),
                    action: *action,
                });
                continue;
            }
            if let SimEvent::Crater { kind, at, rotation } = e {
                // `addCrater` returns at once without the non-weapon effects; the corners need the world's terrain
                match world {
                    Some(w) if inner.use_non_weapon_effects => {
                        inner
                            .craters
                            .add(*kind, at, *rotation, &|p| w.terrain.elevation(p));
                    }
                    None => inner.note("crater decal (no terrain to drop it on)"),
                    _ => {}
                }
                continue;
            }
            let yaw = match (e, world) {
                (SimEvent::WeaponEffect { owner, .. }, Some(w)) => w
                    .objects
                    .by_watch_id(*owner)
                    .and_then(|h| w.objects.get(h))
                    .map(|o| o.c.rotation),
                _ => None,
            };
            let water = match (e, world) {
                (
                    SimEvent::WeaponEffect {
                        impact: WeaponImpact::Miss,
                        to,
                        ..
                    },
                    Some(w),
                ) => Some((over_water_cell(&w.terrain, to), w.terrain.water_elevation)),
                _ => None,
            };
            let mut plan = plan_event(
                e,
                inner.fx.weapons(),
                inner.use_non_weapon_effects,
                yaw,
                water,
            );
            if let (SimEvent::AirstrikeContrail { strike, at, .. }, Some(w)) = (e, world) {
                // the contrail starts at the bomber's `activity_node` on the first frame of the run
                let off = inner.bomber.as_ref().map_or(Vec3::default(), |b| {
                    let o = b.activity_offset(b.frame_after(0, w.clock.frame_length));
                    Vec3 {
                        x: o.x,
                        y: o.y,
                        z: o.z,
                    }
                });
                for p in &mut plan.spawn {
                    p.strike = Some((*strike, w.clock.turn, *at));
                    p.at = Vec3 {
                        x: at.x + off.x,
                        y: at.y + off.y,
                        z: at.z + off.z,
                    };
                    p.transform = FxTransform::at(game_to_fx(p.at.x, p.at.y, p.at.z));
                }
            }
            inner.pending.extend(plan.spawn);
            for u in &plan.unavailable {
                inner.note(u);
            }
        }
        if let Some(w) = world {
            inner.sync_trails(w);
        }
        // Flight effects use the sim-assigned shot identity; a reused object-pool slot is a new bolt.
        if world.is_some() {
            inner.sync_bolt_effects();
        }
    }

    /// Notes the beam textures the object types of `world` name, to be uploaded by [`upload_pending_textures`](Self::upload_pending_textures).
    pub fn collect_bolt_types(&self, world: &mc2_sim::World) {
        {
            // the destruct effects the appearance types name are not rows of `effects.csv`: their textures are uploaded here too
            let mut inner = self.inner.borrow_mut();
            let names = world
                .appearance
                .building_types
                .iter()
                .map(|t| &t.destruct_effect)
                .chain(
                    world
                        .appearance
                        .vehicle_types
                        .iter()
                        .map(|t| &t.destruct_effect),
                );
            for n in names {
                if named(n) && !inner.pending_effects.contains(n) {
                    inner.pending_effects.push(n.clone());
                }
            }
        }
        let Some(table) = world.objects.types.as_ref() else {
            return;
        };
        let mut inner = self.inner.borrow_mut();
        for t in table.types.values() {
            if let ObjectTypeData::WeaponBolt(b) = &t.data {
                if let BoltLook::Beam(BeamLook {
                    texture: Some(name),
                    ..
                }) = BoltLook::of_type(b)
                {
                    if !inner.textures.contains_key(&name)
                        && !inner.pending_textures.contains(&name)
                    {
                        inner.pending_textures.push(name);
                    }
                }
            }
        }
    }

    /// Uploads the beam textures [`collect_bolt_types`](Self::collect_bolt_types) found (`data/tgl/128/<name>.tga`, weaponbolt.cpp:2471-2481).
    pub fn upload_pending_textures(&self, vfs: &mut Vfs, scene: &mut SceneRenderer) {
        let mut inner = self.inner.borrow_mut();
        let effects = std::mem::take(&mut inner.pending_effects);
        if !effects.is_empty() {
            let mut ids = std::mem::take(&mut inner.textures);
            let seed = 1;
            if let Err(e) = probe_textures(
                &effects,
                vfs,
                seed,
                &mut |n, w, h, px| scene.add_fx_texture(n, w, h, px),
                &mut ids,
            ) {
                inner.note(&format!(
                    "destruct effect textures could not be loaded: {e}"
                ));
            }
            inner.textures = ids;
        }
        let names = std::mem::take(&mut inner.pending_textures);
        let mut loader = FxTextures::default();
        for name in names {
            let tex = mc2_fx::state::TextureRef {
                name: name.clone(),
                instance: 0,
            };
            match loader.resolve(vfs, &tex, &mut |n, w, h, px| {
                scene.add_fx_texture(n, w, h, px)
            }) {
                Some(id) => {
                    inner.textures.insert(name, id);
                }
                None => inner.note(&format!("beam texture '{name}' could not be loaded")),
            }
        }
    }

    /// Runs the effects to `game_time` (seconds of the running mission) and starts the queued ones.
    pub fn advance_to(&self, game_time: f64) {
        self.inner.borrow_mut().advance_to(game_time);
    }

    /// A new mission: running effects end, the game clock restarts.
    pub fn reset(&self) {
        let mut inner = self.inner.borrow_mut();
        let mut handles: Vec<FxHandle> = inner.spawned.drain(..).map(|s| s.handle).collect();
        handles.extend(inner.bolt_fx.drain(..).map(|(_, h)| h));
        handles.append(&mut inner.winding);
        handles.extend(inner.attached.drain().map(|(_, h)| h));
        for h in handles {
            inner.fx.kill(h);
        }
        inner.pending.clear();
        inner.trails.clear();
        inner.attached_ops.clear();
        inner.attached_name.clear();
        inner.hidden.clear();
        inner.craters.clear();
        inner.bolts.clear();
        inner.started_bolts.clear();
        let t = inner.fx.time();
        inner.fx.update(t);
        inner.base = inner.fx.time();
    }

    /// Everything started so far (including finished effects).
    pub fn spawned(&self) -> Vec<Spawned> {
        self.inner.borrow().spawned.clone()
    }

    /// Whether a started effect is still running.
    pub fn is_alive(&self, h: FxHandle) -> bool {
        self.inner.borrow().fx.is_alive(h)
    }

    /// Running effects.
    pub fn active_count(&self) -> usize {
        self.inner.borrow().fx.active_count()
    }

    /// Parts of the effect pipeline that could not be shown, with how often they were reached.
    pub fn unavailable(&self) -> BTreeMap<String, u64> {
        self.inner.borrow().unavailable.clone()
    }

    /// The in-flight effects of the bolts now flying (their `gosEffect`), for diagnostics and tests.
    pub fn bolt_effects(&self) -> Vec<FxHandle> {
        self.inner
            .borrow()
            .bolt_fx
            .iter()
            .map(|(_, h)| *h)
            .collect()
    }

    /// Ends every in-flight bolt effect at once (they stop drawing immediately and are not started again).
    pub fn kill_bolt_effects(&self) {
        let mut inner = self.inner.borrow_mut();
        let mut handles: Vec<FxHandle> = inner.bolt_fx.iter().map(|(_, h)| *h).collect();
        handles.extend(inner.winding.iter().copied());
        for h in handles {
            inner.fx.kill(h);
        }
    }

    /// Ends every effect attached to an appearance at once (for the frame comparisons of the tests).
    pub fn kill_attached_effects(&self) {
        let mut inner = self.inner.borrow_mut();
        let handles: Vec<FxHandle> = inner.attached.drain().map(|(_, h)| h).collect();
        for h in handles {
            inner.fx.kill(h);
        }
    }

    /// The members of appearances that run an effect now (diagnostics and tests).
    pub fn attached_slots(&self) -> Vec<(ObjectHandle, FxSlot)> {
        let mut v: Vec<_> = self.inner.borrow().attached.keys().copied().collect();
        v.sort();
        v
    }

    /// Diagnostics: whether the beam and trail-less projectile faces are drawn (on by default).
    pub fn set_draw_bolt_faces(&self, on: bool) {
        self.inner.borrow_mut().draw_bolt_faces = on;
    }

    /// Ends one started effect at once (it stops drawing immediately).
    pub fn kill(&self, h: FxHandle) {
        self.inner.borrow_mut().fx.kill(h);
    }

    /// The draw data of the one effect `h` alone.
    pub fn draw_list_of(&self, h: FxHandle, camera: &GameCamera) -> DrawList {
        let inner = self.inner.borrow();
        let mut list = DrawList::default();
        inner
            .fx
            .draw_one(h, &render::camera_to_world(camera), &mut list);
        list
    }

    /// The draw data of every running effect in the gosFX frame (`x = -world x, y = z, z = y`).
    pub fn draw_list(&self, camera: &GameCamera) -> DrawList {
        let inner = self.inner.borrow();
        let mut list = DrawList::default();
        inner.fx.draw(&render::camera_to_world(camera), &mut list);
        list
    }
}

/// Spawns effects from the frame's events.
pub struct FxEventSink {
    hub: FxHub,
}

impl<D> SimEventSink<D> for FxEventSink
where
    D: SimDriver<Event = SimEvent, World = mc2_sim::World>,
{
    fn mission_start(&mut self, world: &D::World) {
        self.hub.reset();
        self.hub.collect_bolt_types(world);
    }

    fn mission_end(&mut self) {
        self.hub.reset();
    }

    fn on_frame(&mut self, ctx: &FrameContext<'_, D>, events: &[SimEvent]) {
        self.hub.handle_events(Some(ctx.world), events);
    }
}

/// Draws the effects in the scene pass.
pub struct FxScene {
    hub: FxHub,
}

impl SceneEffects for FxScene {
    fn decals(&mut self, ambient: [f32; 3], out: &mut Vec<FxBatch>) {
        self.hub.decal_batches(ambient, out);
    }

    fn batches(&mut self, camera: &GameCamera, time: f32, out: &mut Vec<FxBatch>) {
        self.hub.advance_to(f64::from(time));
        let list = self.hub.draw_list(camera);
        let mut inner = self.hub.inner.borrow_mut();
        let inner = &mut *inner;
        let mut missing = Vec::new();
        let textures = &inner.textures;
        out.extend(render::to_fx_batches(&list, |t| {
            let id = textures.get(&t.name).copied();
            if id.is_none() {
                missing.push(t.name.clone());
            }
            id
        }));
        for name in missing {
            inner.note(&format!("effect texture '{name}' was not preloaded"));
        }
        // the faces `WeaponBolt::render` draws itself: every live beam, and a projectile that has no gosFX trail
        let mut faces = Vec::new();
        let mut untextured = Vec::new();
        let Inner {
            bolts,
            draw_bolt_faces,
            wiggle_rng,
            ..
        } = inner;
        for b in bolts.iter().filter(|_| *draw_bolt_faces) {
            match &b.look {
                BoltLook::Beam(look) => {
                    let texture = look.texture.as_ref().and_then(|n| {
                        let id = inner.textures.get(n).copied();
                        if id.is_none() {
                            untextured.push(n.clone());
                        }
                        id
                    });
                    let wiggle = bolt_geom::wiggle_offset(look, || wiggle_rng.fraction());
                    faces.extend(bolt_geom::beam_batch(
                        look, b.origin, b.target, wiggle, b.elapsed, texture,
                    ));
                }
                BoltLook::Shot(look) => {
                    if !inner.bolt_fx.iter().any(|(k, _)| *k == b.key) {
                        faces.extend(bolt_geom::shot_batch(look, b.position, b.target));
                    }
                }
            }
        }
        for name in untextured {
            inner.note(&format!("beam texture '{name}' was not uploaded"));
        }
        out.extend(faces);
    }
}

/// Uploads every texture the effects of `effects.csv` draw. Each named effect is run on a throwaway system for a few seconds and its
/// draw data collected; the texture ids come from `upload`.
fn preload_textures(
    fx: &FxSystem,
    vfs: &mut Vfs,
    seed: u32,
    upload: render::Upload,
) -> Result<HashMap<String, FxTextureId>, String> {
    let mut names: Vec<String> = Vec::new();
    for i in 0..fx.weapons().len() {
        if let Some(e) = fx.weapons().get(i) {
            for n in [
                &e.effect_name,
                &e.muzzle_flash_name,
                &e.hit_effect_name,
                &e.miss_effect_name,
            ] {
                if named(n) && !names.contains(n) {
                    names.push(n.clone());
                }
            }
        }
    }
    let mut ids = HashMap::new();
    probe_textures(&names, vfs, seed, upload, &mut ids)?;
    Ok(ids)
}

/// Runs each named effect on a throwaway system and uploads the textures it draws (into `ids`, which keeps those already there).
fn probe_textures(
    names: &[String],
    vfs: &mut Vfs,
    seed: u32,
    upload: render::Upload,
    ids: &mut HashMap<String, FxTextureId>,
) -> Result<(), String> {
    let mut probe = FxSystem::from_vfs(vfs, seed).map_err(|e| e.to_string())?;
    let mut loader = FxTextures::default();
    let cam = Mat::from_translation(FxVec3::new(0.0, 50.0, 200.0));
    for name in names {
        let Some(h) = probe.spawn(name, SpawnParams::at(Mat::from_translation(FxVec3::ZERO)))
        else {
            continue;
        };
        let mut t = probe.time();
        for _ in 0..80 {
            t += 0.05;
            probe.update(t);
            let mut list = DrawList::default();
            probe.draw(&cam, &mut list);
            for b in &list.batches {
                if let Some(tex) = &b.state.texture {
                    if !ids.contains_key(&tex.name) {
                        if let Some(id) = loader.resolve(vfs, tex, upload) {
                            ids.insert(tex.name.clone(), id);
                        }
                    }
                }
            }
            if !probe.is_alive(h) {
                break;
            }
        }
        probe.kill(h);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_sim::ids::WatchId;

    /// A fixture table written from the C++ column order (`weaponfx.cpp:36-60`): effect name, muzzle flash, hit, object number, miss.
    const CSV: &str = "n,Effect,Muzzle,Hit,Obj,Miss,Weapon\r\n\
        1,NONE,NONE,NONE,0,NONE,No weapon\r\n\
        2,ppc_trail,PPC_flare,ppc_hit,467,PPC_Miss,PPC\r\n\
        3,Mech_Explosion,NONE,NONE,0,NONE,Boom\r\n\
        4,NONE,NONE,NONE,0,NONE,Last\r\n";

    fn table() -> WeaponEffects {
        WeaponEffects::from_csv_bytes(CSV.as_bytes()).unwrap()
    }

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    #[test]
    fn explosion_starts_the_effect_name_once_at_the_swapped_position() {
        // carnage.cpp:904 GetEffectName(effectId); carnage.cpp:654-662 x = -x, y = z, z = y
        let at = v(10.0, 20.0, 30.0);
        let p = plan_event(
            &SimEvent::Explosion {
                effect_id: 2,
                sound_id: None,
                at,
                radius: 5.0,
            },
            &table(),
            false,
            None,
            None,
        );
        assert_eq!(p.spawn.len(), 1);
        assert_eq!(p.spawn[0].name, "Mech_Explosion");
        assert_eq!(p.spawn[0].role, FxRole::Explosion);
        assert_eq!(
            p.spawn[0].transform.translation(),
            FxVec3::new(-10.0, 30.0, 20.0)
        );
        assert!(p.unavailable.is_empty());
    }

    #[test]
    fn explosion_with_effect_name_none_spawns_nothing_like_the_cpp() {
        // carnage.cpp:904: the `NONE` row creates no gosEffect and that is not a failure
        let p = plan_event(
            &SimEvent::Explosion {
                effect_id: 0,
                sound_id: None,
                at: v(0.0, 0.0, 0.0),
                radius: 1.0,
            },
            &table(),
            true,
            None,
            None,
        );
        assert!(p.spawn.is_empty() && p.unavailable.is_empty());
    }

    fn weapon(effect_id: i32) -> SimEvent {
        weapon_with(effect_id, WeaponImpact::Launch)
    }

    fn weapon_with(effect_id: i32, impact: WeaponImpact) -> SimEvent {
        SimEvent::WeaponEffect {
            shot_id: 1,
            impact,
            effect_id,
            owner: WatchId::NONE,
            from: v(1.0, 2.0, 3.0),
            to: v(100.0, 200.0, 5.0),
        }
    }

    #[test]
    fn launch_gives_only_the_muzzle_flash_and_hit_only_the_hit_effect() {
        let p = plan_event(&weapon(1), &table(), true, Some(90.0), None);
        let roles: Vec<_> = p.spawn.iter().map(|s| (s.role, s.name.as_str())).collect();
        assert_eq!(roles, vec![(FxRole::MuzzleFlash, "PPC_flare")]);
        // weaponbolt.cpp:490-501: Into_Motion * Rot(owner yaw) at (-x, z, y) of the muzzle
        assert_eq!(
            p.spawn[0].transform,
            FxTransform::into_motion(FxVec3::new(-1.0, 3.0, 2.0), 90.0)
        );
        // the trail is drawn from the bolt's flight, not reported unavailable
        assert!(p.unavailable.is_empty(), "{:?}", p.unavailable);
        let h = plan_event(
            &weapon_with(1, WeaponImpact::Hit),
            &table(),
            true,
            None,
            None,
        );
        let roles: Vec<_> = h.spawn.iter().map(|s| (s.role, s.name.as_str())).collect();
        assert_eq!(roles, vec![(FxRole::Hit, "ppc_hit")]);
        // weaponbolt.cpp:627-641: the hit effect is a plain translation at the impact
        assert_eq!(
            h.spawn[0].transform.translation(),
            FxVec3::new(-100.0, 5.0, 200.0)
        );
        assert!(h.unavailable.is_empty());
    }

    #[test]
    fn miss_on_land_starts_the_miss_effect_not_the_hit_effect() {
        // weaponbolt.cpp:696-708: missEffect (GetEffectMissName, 2422-2437) at (-x, z, y); the hit effect belongs to the target branch
        let p = plan_event(
            &weapon_with(1, WeaponImpact::Miss),
            &table(),
            true,
            None,
            Some((false, 0.0)),
        );
        let roles: Vec<_> = p.spawn.iter().map(|s| (s.role, s.name.as_str())).collect();
        assert_eq!(roles, vec![(FxRole::Miss, "PPC_Miss")]);
        assert_eq!(
            p.spawn[0].transform.translation(),
            FxVec3::new(-100.0, 5.0, 200.0)
        );
    }

    #[test]
    fn water_is_the_cell_flags_not_the_elevation() {
        // weaponbolt.cpp:661-667: GameMap deep/shallow flags decide, whatever the elevation says
        let mut t = mc2_sim::terrain::Terrain::from_elevations(60, &vec![0.0; 60 * 60]);
        t.water_elevation = 10.0; // every point is below the water line by elevation
        let at = |r: i32, c: i32| t.cell_to_world(r, c);
        let (dry, deep, shallow) = (at(10, 10), at(20, 20), at(30, 30));
        assert_ne!(t.get_water(&dry), 0, "elevation says water");
        t.set_deep_water(20, 20, true);
        t.set_shallow_water(30, 30, true);
        assert!(
            !over_water_cell(&t, &dry),
            "flag clear, elevation wet: land"
        );
        assert!(over_water_cell(&t, &deep));
        assert!(over_water_cell(&t, &shallow));
        // flag set, elevation dry: still water
        t.water_elevation = -10.0;
        assert_eq!(t.get_water(&deep), 0);
        assert!(over_water_cell(&t, &deep));
    }

    #[test]
    fn miss_on_water_starts_effect_76_at_the_water_surface_and_not_for_machine_guns() {
        // weaponbolt.cpp:661-682, 2440 (`effectId != 11`); WATER_MISS_FX 76 (weaponfx.h:41)
        let csv = format!(
            "{}{}",
            CSV,
            (5..=78)
                .map(|i| format!(
                    "{i},{},NONE,NONE,0,NONE,x\r\n",
                    if i == 77 { "WaterSplash" } else { "NONE" }
                ))
                .collect::<String>()
        );
        let t = WeaponEffects::from_csv_bytes(csv.as_bytes()).unwrap();
        let p = plan_event(
            &weapon_with(1, WeaponImpact::Miss),
            &t,
            true,
            None,
            Some((true, -3.0)),
        );
        assert_eq!(p.spawn.len(), 1, "{p:?}");
        assert_eq!(
            (p.spawn[0].role, p.spawn[0].name.as_str()),
            (FxRole::WaterMiss, "WaterSplash")
        );
        assert_eq!(
            p.spawn[0].transform.translation(),
            FxVec3::new(-100.0, -3.0, 200.0)
        );
        // no non-weapon-effects preference: no miss effects at all (2422, 2440)
        let off = plan_event(
            &weapon_with(1, WeaponImpact::Miss),
            &t,
            false,
            None,
            Some((true, -3.0)),
        );
        assert!(off.spawn.is_empty(), "{off:?}");
        let mg = plan_event(
            &weapon_with(11, WeaponImpact::Miss),
            &t,
            true,
            None,
            Some((true, 0.0)),
        );
        assert!(
            mg.spawn.iter().all(|s| s.role != FxRole::WaterMiss),
            "{mg:?}"
        );
    }

    #[test]
    fn miss_without_terrain_is_reported_unavailable() {
        let p = plan_event(
            &weapon_with(1, WeaponImpact::Miss),
            &table(),
            true,
            None,
            None,
        );
        assert!(p
            .spawn
            .iter()
            .all(|s| s.role != FxRole::Miss && s.role != FxRole::WaterMiss));
        assert_eq!(p.unavailable.len(), 1, "{:?}", p.unavailable);
    }

    #[test]
    fn muzzle_flash_needs_the_non_weapon_effects_preference() {
        // weaponbolt.cpp:2382: `&& useNonWeaponEffects`; the hit effect has no such test (2402)
        let off = plan_event(&weapon(1), &table(), false, None, None);
        assert!(off.spawn.is_empty(), "{off:?}");
        let hit = plan_event(
            &weapon_with(1, WeaponImpact::Hit),
            &table(),
            false,
            None,
            None,
        );
        assert_eq!(hit.spawn.len(), 1);
    }

    #[test]
    fn flash_and_crater_are_not_effects() {
        let c = plan_event(
            &SimEvent::Crater {
                kind: 1,
                at: v(0.0, 0.0, 0.0),
                rotation: 0.0,
            },
            &table(),
            true,
            None,
            None,
        );
        assert!(c.spawn.is_empty() && c.unavailable.is_empty());
        let f = plan_event(
            &SimEvent::Flash {
                object: mc2_sim::ObjectHandle(0),
                argb: 0,
            },
            &table(),
            true,
            None,
            None,
        );
        assert!(f.spawn.is_empty() && f.unavailable.is_empty());
    }

    #[test]
    fn other_events_are_not_effects() {
        let p = plan_event(&SimEvent::Betty(1), &table(), true, None, None);
        assert!(p.spawn.is_empty() && p.unavailable.is_empty());
    }

    /// The `effects.csv` field `col` (0-based after the leading id column) of the row for `effect_id`, parsed here straight from the
    /// shipped bytes, independent of `WeaponEffects`.
    fn csv_field(csv: &str, effect_id: usize, col: usize) -> String {
        let line = csv.lines().nth(effect_id + 1).expect("row");
        line.split(',').nth(col).expect("column").trim().to_string()
    }

    /// A hub over the shipped effect library (no GPU), and an effect id whose `effects.csv` row names a trail.
    fn data_hub() -> Option<(FxHub, i32)> {
        let root = std::env::var_os("MC2_DATA")?;
        let mut vfs = Vfs::from_system_cfg(root).unwrap();
        let fx = FxSystem::from_vfs(&mut vfs, 7).unwrap();
        let id = (0..fx.weapons().len())
            .find(|&i| fx.weapons().get(i).is_some_and(|e| named(&e.effect_name)))
            .expect("a weapon with a trail") as i32;
        Some((FxHub::with_system(fx, HashMap::new(), true), id))
    }

    #[test]
    fn death_explosions_reach_the_production_sink_without_a_gpu() {
        let Some((hub, _)) = data_hub() else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        let world = mc2_sim::World::new(mc2_sim::rng::RngMode::Pcg32, 7);
        let ctx = FrameContext::<crate::live_sim::LiveSim> {
            frame: 1,
            frame_length: 1.0 / 30.0,
            turn: 1,
            scenario_time: 1.0 / 30.0,
            listener: mc2_render::math::Vec3::default(),
            listener_rotation: 0.0,
            world: &world,
        };
        let events: Vec<_> = [39, 40, 49]
            .into_iter()
            .map(|effect_id| SimEvent::Explosion {
                effect_id,
                at: v(effect_id as f32, 2.0, 3.0),
                radius: 0.0,
                sound_id: None,
            })
            .collect();
        let names: Vec<_> = {
            let inner = hub.inner.borrow();
            [39, 40, 49]
                .map(|id| inner.fx.weapons().effect_name(id).to_owned())
                .into()
        };
        let mut sink = hub.sink();
        <FxEventSink as SimEventSink<crate::live_sim::LiveSim>>::on_frame(&mut sink, &ctx, &events);
        hub.advance_to(1.0);
        let spawned = hub.spawned();
        for (id, name) in [39, 40, 49].into_iter().zip(names) {
            assert!(
                spawned.iter().any(|s| s.role == FxRole::Explosion
                    && s.name == name
                    && s.at == v(id as f32, 2.0, 3.0)),
                "effect {id}: {spawned:?}"
            );
        }
    }

    fn flight(id: i32, _owner: WatchId, at: Vec3) -> BoltFlight {
        BoltFlight {
            key: 1,
            effect_id: id,
            is_beam: false,
            position: at,
            target: v(at.x + 300.0, at.y, at.z),
            origin: at,
            elapsed: 0.0,
            look: BoltLook::Shot(bolt_geom::ShotLook {
                length: 10.0,
                bulge_length: 3.0,
                bulge_width: 1.0,
                front_rgb: 0xffffff,
                mid_edge_rgb: 0xffffff,
                middle_rgb: 0xffffff,
                back_rgb: 0xffffff,
                edge_alpha: 255,
                bolt_alpha: 255,
            }),
        }
    }

    #[test]
    fn slot_three_reuse_ends_old_shot_and_starts_new_shot_by_identity() {
        let Some((hub, id)) = data_hub() else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        let owner = WatchId(9);
        // Slot 3 starts as shot 41, then the pool reuses it for shot 42. The older shot has the same owner and effect.
        let old_slot = ObjectHandle(3);
        let mut old_slot_shot = flight(id, owner, v(100.0, 10.0, 5.0));
        old_slot_shot.key = 41;
        old_slot_shot.target = v(300.0, 10.0, 5.0);
        let mut older_shot = flight(id, owner, v(10.0, 10.0, 5.0));
        older_shot.key = 40;
        older_shot.target = v(900.0, 10.0, 5.0);
        hub.inner.borrow_mut().bolts = vec![old_slot_shot.clone(), older_shot.clone()];
        hub.advance_to(1.0);
        let old_handles = hub.bolt_effects();
        assert_eq!(old_handles.len(), 2);
        let old_slot_fx = hub
            .inner
            .borrow()
            .bolt_fx
            .iter()
            .find(|(key, _)| *key == 41)
            .unwrap()
            .1;
        let older_fx = hub
            .inner
            .borrow()
            .bolt_fx
            .iter()
            .find(|(key, _)| *key == 40)
            .unwrap()
            .1;

        // Sim reports the new live shot in reused slot 3. It has its own launch point and identity.
        let new_slot = ObjectHandle(3);
        assert_eq!(new_slot, old_slot, "the sim pool reuses slot 3");
        let mut new_slot_shot = flight(id, owner, v(25.0, 10.0, 5.0));
        new_slot_shot.key = 42;
        new_slot_shot.target = v(500.0, 10.0, 5.0);
        older_shot.position = v(20.0, 10.0, 5.0);
        {
            let mut inner = hub.inner.borrow_mut();
            inner.bolts = vec![new_slot_shot.clone(), older_shot];
            inner.sync_bolt_effects();
        }
        let new_fx = hub
            .inner
            .borrow()
            .bolt_fx
            .iter()
            .find(|(key, _)| *key == 42)
            .unwrap()
            .1;
        assert_ne!(new_fx, old_slot_fx, "slot reuse starts a new effect handle");
        assert!(
            hub.inner
                .borrow()
                .bolt_fx
                .iter()
                .any(|(key, h)| *key == 40 && *h == older_fx),
            "older same-owner, same-effect shot keeps its handle"
        );
        assert!(
            !hub.inner.borrow().bolt_fx.iter().any(|(key, _)| *key == 41),
            "the old shot identity is no longer tracked"
        );
        assert_eq!(
            hub.inner
                .borrow()
                .bolts
                .iter()
                .find(|b| b.key == 42)
                .unwrap()
                .position,
            v(25.0, 10.0, 5.0),
            "the new trail follows the reused slot's own launch position"
        );
        hub.advance_to(60.0);
        assert!(!hub.is_alive(old_slot_fx), "the old shot trail ended");
    }

    #[test]
    fn reset_ends_the_trail_of_a_bolt_in_flight() {
        let Some((hub, id)) = data_hub() else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        hub.inner.borrow_mut().bolts = vec![flight(id, WatchId(9), v(10.0, 10.0, 5.0))];
        hub.advance_to(1.0);
        let trail = hub.bolt_effects();
        assert_eq!(trail.len(), 1);
        hub.reset();
        assert!(hub.bolt_effects().is_empty());
        assert!(
            !hub.is_alive(trail[0]),
            "no trail survives into the next mission"
        );
        assert_eq!(hub.active_count(), 0);
    }

    #[test]
    fn reset_kills_a_stopped_trail_that_is_still_fading() {
        let Some((hub, id)) = data_hub() else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        hub.inner.borrow_mut().bolts = vec![flight(id, WatchId(9), v(10.0, 10.0, 5.0))];
        hub.advance_to(1.0);
        let trail = hub.bolt_effects();
        // the bolt ends: its trail is stopped and fades
        hub.inner.borrow_mut().bolts.clear();
        hub.advance_to(1.05);
        assert!(hub.bolt_effects().is_empty());
        let fading = trail[0];
        if hub.is_alive(fading) {
            hub.reset();
            assert!(
                !hub.is_alive(fading),
                "no fading trail survives into the next mission"
            );
        }
        assert_eq!(hub.active_count(), 0);
    }

    #[test]
    fn shipped_explosion_ids_map_to_the_names_in_effects_csv() {
        let Some(root) = std::env::var_os("MC2_DATA") else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        let mut vfs = Vfs::from_system_cfg(root).unwrap();
        let bytes = vfs
            .read(mc2_fx::weaponfx::EFFECTS_CSV_PATH)
            .unwrap()
            .expect("effects.csv");
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let weapons = WeaponEffects::from_csv_bytes(&bytes).unwrap();
        // the ids weaponfx.h hard-codes (mclib/weaponfx.h:12-30)
        for (id, define) in [
            (45, "BUILDING_EXPLOSION_ID"),
            (40, "VEHICLE_EXPLOSION_ID"),
            (46, "SPLASH_VEHICLE_EXPLOSION_ID"),
            (39, "MECH_EXPLOSION_ID"),
            (44, "TURRET_EXPLOSION_ID"),
            (49, "MECH_DISABLE_EXPLOSION_ID"),
            (53, "MINE_EXPLOSION_ID"),
            (54, "EMPTY_EXPLOSION_ID"),
        ] {
            let expect = csv_field(&text, id, 1);
            let p = plan_event(
                &SimEvent::Explosion {
                    effect_id: id as i32,
                    sound_id: None,
                    at: v(0.0, 0.0, 0.0),
                    radius: 1.0,
                },
                &weapons,
                true,
                None,
                None,
            );
            if expect == "NONE" {
                assert!(p.spawn.is_empty(), "{define}");
            } else {
                assert_eq!(p.spawn.len(), 1, "{define}");
                assert_eq!(p.spawn[0].name, expect, "{define} ({id})");
            }
        }
        // every weapon row: muzzle (column 2) and hit (column 3) names straight from the file
        for id in 1..weapons.len() {
            for (role, col, impact) in [
                (FxRole::MuzzleFlash, 2, WeaponImpact::Launch),
                (FxRole::Hit, 3, WeaponImpact::Hit),
            ] {
                let p = plan_event(&weapon_with(id as i32, impact), &weapons, true, None, None);
                let expect = csv_field(&text, id, col);
                let got = p
                    .spawn
                    .iter()
                    .find(|s| s.role == role)
                    .map(|s| s.name.clone());
                if expect == "NONE" || expect.is_empty() {
                    assert_eq!(got, None, "row {id} {role:?}");
                } else {
                    assert_eq!(got.as_deref(), Some(expect.as_str()), "row {id} {role:?}");
                }
            }
        }
        // the miss column (5) on land, and WATER_MISS_FX (76) over water, straight from the file (machine gun 11 has none)
        for id in 1..weapons.len() {
            let land = plan_event(
                &weapon_with(id as i32, WeaponImpact::Miss),
                &weapons,
                true,
                None,
                Some((false, 0.0)),
            );
            let expect = csv_field(&text, id, 5);
            let got = land.spawn.first().map(|s| s.name.clone());
            if expect == "NONE" || expect.is_empty() {
                assert_eq!(got, None, "row {id} miss");
            } else {
                assert_eq!(got.as_deref(), Some(expect.as_str()), "row {id} miss");
            }
            let water = plan_event(
                &weapon_with(id as i32, WeaponImpact::Miss),
                &weapons,
                true,
                None,
                Some((true, 0.0)),
            );
            let splash = csv_field(&text, 76, 1);
            let got = water.spawn.first().map(|s| s.name.clone());
            if id == 11 || splash == "NONE" || splash.is_empty() {
                assert_eq!(got, None, "row {id} water miss");
            } else {
                assert_eq!(got.as_deref(), Some(splash.as_str()), "row {id} water miss");
            }
        }
    }
}

/// The effects attached to appearances through the hub: what the hub does with `AppearanceFx` calls over time (C++ gvactor.cpp:2473,
/// mech3d.cpp:2432, 2579, 3580).
#[cfg(test)]
mod attached_tests {
    use super::*;

    fn data_hub() -> Option<FxHub> {
        let root = std::env::var_os("MC2_DATA")?;
        let mut vfs = Vfs::from_system_cfg(root).unwrap();
        let fx = FxSystem::from_vfs(&mut vfs, 7).unwrap();
        Some(FxHub::with_system(fx, HashMap::new(), true))
    }

    fn call(object: u32, slot: FxSlot, id: i32, action: FxAction) -> SimEvent {
        SimEvent::AppearanceFx {
            object: ObjectHandle(object),
            slot,
            effect: FxEffect::Id(id),
            action,
        }
    }

    fn place(x: f32, y: f32) -> FxPlacement {
        FxPlacement::plain(Vec3 { x, y, z: 0.0 })
    }

    fn handle_of(hub: &FxHub, object: u32, slot: FxSlot) -> Option<FxHandle> {
        hub.inner
            .borrow()
            .attached
            .get(&(ObjectHandle(object), slot))
            .copied()
    }

    fn triangles(hub: &FxHub, h: FxHandle) -> usize {
        let mut list = DrawList::default();
        hub.inner.borrow().fx.draw_one(h, &Mat::IDENTITY, &mut list);
        list.triangle_count()
    }

    /// A vehicle's dust cloud runs out while it stands (`SetLoopOff`) and runs again when it moves (`SetLoopOn` + `SetExecuteOn`):
    /// the cloud comes back at the vehicle's new position.
    #[test]
    fn dust_that_ran_out_runs_again_where_the_vehicle_is_now() {
        let Some(hub) = data_hub() else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        let slot = FxSlot::DustCloud;
        let start = FxAction::Start {
            place: place(100.0, 200.0),
            looping: false,
        };
        hub.handle_events(None, &[call(5, slot, 70, start)]);
        hub.advance_to(1.0);
        let first = handle_of(&hub, 5, slot).expect("started");
        let mut t = 1.0;
        while hub.is_alive(first) && t < 120.0 {
            t += 1.0;
            hub.advance_to(t);
        }
        assert!(!hub.is_alive(first), "the one-shot cloud runs out");
        // standing still: told to run out, nothing to bring back
        let stand = FxAction::Move {
            place: place(100.0, 200.0),
            looping: Some(false),
        };
        hub.handle_events(None, &[call(5, slot, 70, stand)]);
        hub.advance_to(t + 1.0);
        assert!(handle_of(&hub, 5, slot).is_none_or(|h| !hub.is_alive(h)));
        // moving again, 300 units east
        let go = FxAction::Move {
            place: place(400.0, 200.0),
            looping: Some(true),
        };
        hub.handle_events(None, &[call(5, slot, 70, go)]);
        hub.advance_to(t + 2.0);
        let again = handle_of(&hub, 5, slot).expect("the cloud runs again");
        assert!(hub.is_alive(again));
        let (centre, _) = hub.inner.borrow().fx.bounds(again).expect("bounds");
        let want = game_to_fx(400.0, 200.0, 0.0);
        assert!(
            (centre.x - want.x).abs() < 150.0,
            "the cloud is at the vehicle: {centre:?} vs {want:?}"
        );
    }

    /// An effect the owner stopped or killed is not brought back by a later move.
    #[test]
    fn a_killed_wake_stays_dead() {
        let Some(hub) = data_hub() else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        let slot = FxSlot::WaterWake;
        let start = FxAction::Start {
            place: place(0.0, 0.0),
            looping: true,
        };
        hub.handle_events(None, &[call(6, slot, 66, start)]);
        hub.advance_to(1.0);
        hub.handle_events(None, &[call(6, slot, 66, FxAction::Kill)]);
        let mv = FxAction::Move {
            place: place(10.0, 0.0),
            looping: Some(true),
        };
        hub.handle_events(None, &[call(6, slot, 66, mv)]);
        hub.advance_to(2.0);
        assert!(handle_of(&hub, 6, slot).is_none());
    }

    /// `FxAction::Draw(false)` keeps the effect running but draws nothing (foot dust while wading, a wake while jumping, the
    /// dust of a vehicle in the water); `Draw(true)` shows it again; an effect started while hidden starts hidden.
    #[test]
    fn a_hidden_effect_keeps_running_but_is_not_drawn() {
        let Some(hub) = data_hub() else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        let slot = FxSlot::WaterWake;
        let start = FxAction::Start {
            place: place(0.0, 0.0),
            looping: true,
        };
        hub.handle_events(None, &[call(7, slot, 34, start)]);
        hub.advance_to(0.2);
        for t in 1..8 {
            hub.advance_to(0.2 + f64::from(t) * 0.25);
        }
        let h = handle_of(&hub, 7, slot).expect("started");
        let shown = triangles(&hub, h);
        assert!(shown > 0, "the wake has geometry once it is running");
        hub.handle_events(None, &[call(7, slot, 34, FxAction::Draw(false))]);
        hub.advance_to(3.0);
        assert!(hub.is_alive(h), "still running");
        assert_eq!(triangles(&hub, h), 0, "not drawn");
        hub.handle_events(None, &[call(7, slot, 34, FxAction::Draw(true))]);
        let mut again = 0;
        for t in 1..8 {
            hub.advance_to(3.0 + f64::from(t) * 0.25);
            again = again.max(triangles(&hub, h));
        }
        assert!(again > 0, "drawn again");
        // a restart while the condition holds comes up hidden
        hub.handle_events(None, &[call(7, slot, 34, FxAction::Draw(false))]);
        hub.handle_events(None, &[call(7, slot, 34, start)]);
        hub.advance_to(5.2);
        let h2 = handle_of(&hub, 7, slot).expect("restarted");
        let mut drawn = 0;
        for t in 1..8 {
            hub.advance_to(5.2 + f64::from(t) * 0.25);
            drawn += triangles(&hub, h2);
        }
        assert_eq!(drawn, 0);
    }

    /// A looping effect has no one left to stop it when its owner is gone: the hub ends it.
    #[test]
    fn effects_of_a_removed_owner_end() {
        use mc2_sim::mission::init::{load_world, LoadSpec};
        use mc2_sim::rng::RngMode;
        let Some(hub) = data_hub() else {
            eprintln!("MC2_DATA not set; skipping");
            return;
        };
        let root = std::env::var_os("MC2_DATA").unwrap();
        let mut vfs = Vfs::from_system_cfg(root).unwrap();
        let mut world = load_world(
            &mut vfs,
            &LoadSpec::quickstart("mc2_01", 1, RngMode::LegacyMsvc),
        )
        .expect("load mc2_01")
        .world;
        let owner = world.objects.movers[0];
        let slot = FxSlot::DustCloud;
        let start = FxAction::Start {
            place: place(0.0, 0.0),
            looping: true,
        };
        hub.handle_events(Some(&world), &[call(owner.0, slot, 70, start)]);
        // a support-unit effect has no owner object and stays
        hub.handle_events(
            Some(&world),
            &[call(
                ObjectHandle::NONE.0,
                FxSlot::SupportDust(0),
                56,
                FxAction::Start {
                    place: place(0.0, 0.0),
                    looping: true,
                },
            )],
        );
        hub.advance_to(1.0);
        let h = handle_of(&hub, owner.0, slot).expect("started");
        let support =
            handle_of(&hub, ObjectHandle::NONE.0, FxSlot::SupportDust(0)).expect("support");
        hub.handle_events(Some(&world), &[]);
        assert!(hub.is_alive(h), "the owner still exists");
        world.objects.get_mut(owner).unwrap().set_exists(false);
        hub.handle_events(Some(&world), &[]);
        hub.advance_to(1.5);
        assert!(!hub.is_alive(h), "the owner is gone: the cloud ends");
        assert!(hub.is_alive(support));
    }
}
