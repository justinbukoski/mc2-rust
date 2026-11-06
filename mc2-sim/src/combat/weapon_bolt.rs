// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! WeaponBolt creation and lifecycle helpers (`createWeaponBolt`, `connect`, hot spot positions).

use mc2_sim_data::objtype::{ObjectTypeData, WeaponBoltType};

use crate::appearance::tables::BASE_NODE_RECYCLE_TIME;
use crate::combat::{BoltState, WeaponShotInfo};
use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::object::store::type_object_class;
use crate::object::{flags, hooks, Kind, ObjectClass, ObjectStore, Pool};
use crate::World;

/// The fields of a `WeaponBoltType` the bolt code reads, whichever projectile block the data came from. A `[BeamProjectileData]`
/// type has the constructor defaults (weaponbolt.h:97-122) for everything it does not read: no arc (`arcHeight = -1`), no hit
/// time, no area effect, no hit effect object, and velocity and projectile length 1.0 (weaponbolt.cpp:173).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoltParams {
    /// `isBeam`.
    pub is_beam: bool,
    /// `beamDuration`.
    pub beam_duration: f32,
    /// `beamWiggle`.
    pub beam_wiggle: f32,
    /// `velocity`.
    pub velocity: f32,
    /// `arcEffect`.
    pub arc_effect: bool,
    /// `arcHeight`.
    pub arc_height: f32,
    /// `afterHitTime`.
    pub after_hit_time: f32,
    /// `hitEffectObjNum`.
    pub hit_effect: i32,
    /// `areaEffectDmg`.
    pub area_damage: f32,
    /// `areaEffectRad`.
    pub area_radius: f32,
    /// `fireSoundFX` (a `sounds.h` id, 0 = none).
    pub fire_sound: i32,
}

impl BoltParams {
    /// The type data of a bolt type.
    pub fn of_type(t: &WeaponBoltType) -> BoltParams {
        match t {
            WeaponBoltType::Bolt(b) => BoltParams {
                is_beam: b.is_beam,
                beam_duration: b.beam_duration,
                beam_wiggle: b.beam_wiggle,
                velocity: b.velocity,
                arc_effect: b.arc_effect,
                arc_height: b.arc_height,
                after_hit_time: b.after_hit_time,
                hit_effect: b.hit_effect,
                area_damage: b.area_effect_damage,
                area_radius: b.area_effect_radius,
                fire_sound: b.firing_sound_fx,
            },
            WeaponBoltType::Beam(b) => BoltParams {
                is_beam: b.is_beam,
                beam_duration: b.beam_duration,
                beam_wiggle: b.beam_wiggle,
                velocity: b.velocity,
                arc_effect: false,
                arc_height: -1.0,
                after_hit_time: 0.0,
                hit_effect: 0,
                area_damage: 0.0,
                area_radius: 0.0,
                fire_sound: b.firing_sound_fx,
            },
        }
    }

    /// `hitEffectObjNum && areaEffectDmg > 0 && areaEffectRad > 0`: the weapon is an area-effect weapon (hit or miss, the explosion does
    /// the damage).
    pub fn is_area_effect(&self) -> bool {
        self.hit_effect != 0 && self.area_damage > 0.0 && self.area_radius > 0.0
    }
}

/// The [`BoltParams`] of the type a bolt object was initialised with.
pub fn bolt_params(w: &World, h: ObjectHandle) -> Option<BoltParams> {
    match &w.objects.type_of(h)?.data {
        ObjectTypeData::WeaponBolt(t) => Some(BoltParams::of_type(t)),
        _ => None,
    }
}

/// The bolt state behind a handle.
pub fn bolt_state(w: &World, h: ObjectHandle) -> Option<&BoltState> {
    match &w.objects.get(h)?.kind {
        Kind::WeaponBolt(b) => Some(b),
        _ => None,
    }
}

/// Mutable [`bolt_state`].
pub fn bolt_state_mut(w: &mut World, h: ObjectHandle) -> Option<&mut BoltState> {
    match &mut w.objects.get_mut(h)?.kind {
        Kind::WeaponBolt(b) => Some(b),
        _ => None,
    }
}

/// `GameObject::getPositionFromHS(hotSpot)`: the hot spot position of any object. A mech's is the weapon node position; a ground
/// vehicle's the hit node unless a node is asked for (or the hit node is the object position); a turret's the weapon node 0; a
/// building's the hit node (raised by half the top z when it is the object position); every other object has no hot spots
/// (bldng.h:225, gvehicl.cpp:2538, turret.cpp:1227, mech.cpp:5345, gameobj.h:519).
pub fn position_from_hs(w: &mut World, h: ObjectHandle, hot_spot: i32) -> Vec3 {
    let Some(o) = w.objects.get(h) else {
        return Vec3::default();
    };
    let position = o.c.position;
    match &o.kind {
        Kind::Mech(_) => w.appearance.weapon_node_position(h, hot_spot),
        Kind::Vehicle(_) => {
            if w.appearance.get(h).is_none() {
                return position;
            }
            let hit = w.appearance.hit_node(h);
            if hot_spot != -1 || hit == position {
                w.appearance.weapon_node_position(h, hot_spot)
            } else {
                hit
            }
        }
        Kind::Turret(_) => {
            let hit = hooks::hit_node(w, h);
            if hit == position || hot_spot != -1 {
                hooks::weapon_node_position(w, h, 0)
            } else {
                hit
            }
        }
        Kind::Building(_) => {
            if w.appearance.get(h).is_none() {
                return position;
            }
            let mut hs = hooks::hit_node(w, h);
            if hs == position {
                hs.z += hooks::top_z(w, h) * 0.5;
            }
            hs
        }
        _ => position,
    }
}

/// The point a bolt aims at on its target each frame (weaponbolt.cpp:430-445): a non-mech target's hot spot, a mech's left or right
/// hit node by the target hot spot.
pub fn target_aim_point(w: &mut World, target: ObjectHandle, target_hot_spot: i32) -> Vec3 {
    let is_mech = matches!(w.objects.get(target).map(|o| &o.kind), Some(Kind::Mech(_)));
    if is_mech {
        w.appearance.hit_node_side(target, target_hot_spot != 0)
    } else {
        position_from_hs(w, target, target_hot_spot)
    }
}

/// `GameObjectManager::createWeaponBolt(effectId)` (objmgr.cpp:2734): the next pool slot (`getWeapon()`, which `finishNow()`s its
/// previous occupant), then `init(effectId)`, `init(false, type)` and `setExists(true)`. Returns `None` when the effect's object type
/// is not a weapon bolt (the C++ `STOP`s when the type does not exist at all; that also returns `None` here). The pool slot is
/// consumed either way.
///
/// `WeaponBolt::init(false, type)` resets no state of its own: a reused slot keeps its old `hit_target`, positions and shot until the
/// first update (the C++ constructor set them once). The sim RNG is not touched.
pub fn create(w: &mut World, effect_id: i32) -> Option<ObjectHandle> {
    let h = ObjectStore::next_pool_slot(w, Pool::Weapons)?;
    let type_num = w.objects.effect_object_number(effect_id);
    let t = w.objects.type_by_num(type_num)?;
    if type_object_class(t) != ObjectClass::WeaponBolt {
        return None;
    }
    let shot_id = w.allocate_weapon_shot_id();
    let o = w.objects.get_mut(h)?;
    if let Kind::WeaponBolt(b) = &mut o.kind {
        b.shot_id = shot_id;
        b.effect_id = effect_id;
        b.just_created = true;
    } else {
        return None;
    }
    o.c.type_handle = type_num;
    o.c.class = ObjectClass::WeaponBolt;
    o.c.set_flag(flags::JUSTCREATED, true);
    o.set_exists(true);
    Some(h)
}

/// `setTargetPosition(pos)`: allocates the position on first use, then stores it.
pub fn set_target_position(bolt: &mut BoltState, pos: Vec3) {
    bolt.has_target_position = true;
    bolt.target_position = pos;
}

/// The tail both `connect`s share: the shot is a copy assignment (`weaponShot = *shotInfo` runs `WeaponShotInfo::init` again, so the
/// difficulty scaling is applied a second time), and the owner's weapon node recycle is reset to the beam duration or
/// `BASE_NODE_RECYCLE_TIME`.
fn connect_tail(
    w: &mut World,
    h: ObjectHandle,
    owner: ObjectHandle,
    shot: Option<&WeaponShotInfo>,
    source_hs: i32,
) {
    if let Some(s) = shot {
        // `operator =` is `init(copy.attackerWID, ...)`: `assign` onto a scratch shot
        let mut copy = *s;
        copy.assign(w, *s);
        if let Some(b) = bolt_state_mut(w, h) {
            b.weapon_shot = Some(copy);
        }
    }
    let recycle = match bolt_params(w, h) {
        Some(p) if p.is_beam => p.beam_duration,
        _ => BASE_NODE_RECYCLE_TIME,
    };
    w.appearance
        .set_weapon_node_recycle(owner, source_hs, recycle);
}

/// `WeaponBolt::connect(source, dest, shotInfo, sourceHS, targetHS)` (weaponbolt.h:304). `target_hs` is overwritten as in the C++: a
/// mech target gets 0, or 1 on `RollDice(50)` (one sim draw), every other target -1. An area-effect weapon aims at the target's
/// current position and drops the target watch id.
pub fn connect_target(
    w: &mut World,
    h: ObjectHandle,
    owner: ObjectHandle,
    target: ObjectHandle,
    shot: Option<&WeaponShotInfo>,
    source_hs: i32,
    _target_hs: i32,
) {
    let owner_wid = w.objects.watch_id(owner);
    let target_wid = w.objects.watch_id(target);
    let is_mech = matches!(w.objects.get(target).map(|o| &o.kind), Some(Kind::Mech(_)));
    let target_hot_spot = if is_mech {
        let r = w.rng.0.clone();
        let left = r.borrow_mut().sim.roll_dice(50);
        i32::from(left)
    } else {
        // Vehicles need to know to use the hit node for this hot spot and NOT the weapon node
        -1
    };
    let area = bolt_params(w, h).is_some_and(|p| p.is_area_effect());
    let target_loc = w.objects.get(target).map(|o| o.c.position);
    if let Some(b) = bolt_state_mut(w, h) {
        b.owner = owner_wid;
        b.target = target_wid;
        b.hot_spots = (source_hs, target_hot_spot);
        if area {
            // If this is an AreaEffect weapon, NO target WID, hit the target's CURRENT LOCATION
            if let Some(loc) = target_loc {
                set_target_position(b, loc);
            }
            b.target = crate::ids::WatchId::NONE;
        }
    }
    connect_tail(w, h, owner, shot, source_hs);
}

/// `WeaponBolt::connect(source, targetLoc, shotInfo, sourceHS)` (weaponbolt.h:344): a shot at a point on the ground.
pub fn connect_point(
    w: &mut World,
    h: ObjectHandle,
    owner: ObjectHandle,
    target_loc: Vec3,
    shot: Option<&WeaponShotInfo>,
    source_hs: i32,
) {
    let owner_wid = w.objects.watch_id(owner);
    if let Some(b) = bolt_state_mut(w, h) {
        b.owner = owner_wid;
        b.target = crate::ids::WatchId::NONE;
        b.hot_spots.0 = source_hs;
        set_target_position(b, target_loc);
    }
    connect_tail(w, h, owner, shot, source_hs);
}
