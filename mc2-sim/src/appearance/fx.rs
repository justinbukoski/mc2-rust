// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Effects attached to appearances (`gosFX` effects the appearance classes own): the appearances queue their `Start` / `Execute` /
//! `Stop` / `Kill` calls as [`FxCall`]s and the world entry points turn them into [`SimEvent::AppearanceFx`]; the application
//! resolves the effect and plays it. Nothing here is read back by the simulation, saved or digested.

use crate::events::{FxAction, FxEffect, FxSlot, SimEvent};
use crate::ids::ObjectHandle;
use crate::World;

/// One call into an attached gosFX effect the appearance made, before the owner turns it into [`SimEvent::AppearanceFx`].
#[derive(Clone, Debug, PartialEq)]
pub struct FxCall {
    pub slot: FxSlot,
    pub effect: FxEffect,
    pub action: FxAction,
}

/// Pushes the queued calls of `h` as events, oldest first.
pub fn emit(w: &mut World, h: ObjectHandle, calls: Vec<FxCall>) {
    for c in calls {
        w.events.push(SimEvent::AppearanceFx {
            object: h,
            slot: c.slot,
            effect: c.effect,
            action: c.action,
        });
    }
}

/// Takes the calls any appearance of `h` has queued.
pub fn flush(w: &mut World, h: ObjectHandle) {
    let calls = w.appearance.take_fx_calls(h);
    emit(w, h, calls);
}

/// `Mech3DAppearance::copyFrom` (mech3d.cpp:5008): a mech saved while smoking starts the smoke again. The events go to the first
/// frame after the load.
pub fn restore_after_load(w: &mut World) {
    w.appearance.reset_fx_after_load();
    let smoking: Vec<(ObjectHandle, i32)> = (0..w.appearance.by_handle.len())
        .map(|i| ObjectHandle(i as _))
        .filter_map(|h| w.appearance.mech(h).map(|m| (h, m.is_smoking)))
        .filter(|(_, level)| *level >= 0)
        .collect();
    for (h, level) in smoking {
        start_smoking(w, h, level);
    }
}

/// `startSmoking(level)` of a mech (mech.cpp:6150, 7009, mover.cpp:6768).
pub fn start_smoking(w: &mut World, h: ObjectHandle, level: i32) {
    let non_weapon = w.objects.use_non_weapon_effects;
    if let Some((m, t)) = w.appearance.mech_mut(h) {
        m.fx.non_weapon = non_weapon;
        m.start_smoking(t, level);
    }
    flush(w, h);
}

/// `startWaterWake()` / `stopWaterWake()` of a mech or vehicle, by whether its cell is water (mech.cpp:6290-6301, gvehicl.cpp:3736).
pub fn update_water_wake(w: &mut World, h: ObjectHandle, in_water: bool) {
    let non_weapon = w.objects.use_non_weapon_effects;
    let level = w.terrain.water_elevation;
    w.appearance.set_wake(h, in_water, level, non_weapon);
    flush(w, h);
}

/// `playEjection()` of a mech (mech.cpp:4811, 4843).
pub fn play_ejection(w: &mut World, h: ObjectHandle) {
    let non_weapon = w.objects.use_non_weapon_effects;
    // The cloud stands where the craft is: the appearance has not been given this frame's position yet when the shut-down
    // and start-up branches run (a craft just dropped still holds the origin).
    let position = w.objects.get(h).map(|o| o.c.position);
    if let Some((m, _)) = w.appearance.mech_mut(h) {
        m.fx.non_weapon = non_weapon;
        if let Some(p) = position {
            m.position = p;
        }
        m.play_ejection();
    }
    flush(w, h);
}

/// Keeps the player's `UseNonWeaponEffects` choice in the mech's effect state before an update.
pub fn sync_preference(w: &mut World, h: ObjectHandle) {
    let non_weapon = w.objects.use_non_weapon_effects;
    if let Some((v, _)) = w.appearance.vehicle_mut(h) {
        v.fx.non_weapon = non_weapon;
    } else if let Some((m, _)) = w.appearance.mech_mut(h) {
        m.fx.non_weapon = non_weapon;
    } else if let Some((b, _)) = w.appearance.building_mut(h) {
        b.fx.non_weapon = non_weapon;
    }
}

/// Whether the cell under `at` is deep or shallow water (`GameMap->getDeepWater || getShallowWater`).
pub fn over_water_cell(w: &World, at: &crate::math::Vec3) -> bool {
    let (r, c) = w.terrain.world_to_cell(at);
    w.terrain.get_deep_water(r, c) || w.terrain.get_shallow_water(r, c)
}

/// `startActivity(effectId, loop)` of a vehicle or building; `stopActivity()` with `None`.
pub fn set_activity(w: &mut World, h: ObjectHandle, effect_id: Option<i32>, looping: bool) {
    sync_preference(w, h);
    w.appearance.set_activity_effect(h, effect_id, looping);
    flush(w, h);
}

/// `appearance->playDestruction()` of a building or vehicle (bdactor.cpp:1449, gvactor.cpp:1888): starts the type's
/// `[TGLDestructEffect]` once and tells whether the type has one (the caller then draws no generic explosion). The application
/// looks the named effect up in the effect library; a name it does not have is reported there.
pub fn play_destruction(w: &mut World, h: ObjectHandle) -> bool {
    let played = if let Some((b, t)) = w.appearance.building_mut(h) {
        b.play_destruction(t)
    } else if let Some((v, t)) = w.appearance.vehicle_mut(h) {
        v.play_destruction_fx(t)
    } else {
        false
    };
    flush(w, h);
    played
}
