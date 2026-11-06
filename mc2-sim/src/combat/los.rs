// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Mover::getLOSPosition` (mover.cpp:3459) and `GameObject::lineOfSight(point)` (gameobj.cpp:1828): the line of sight test a missed shot
//! uses to pick a visible spot beside its target.

use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::World;

/// `Mover::getLOSPosition()` of the mover's own position: the position raised to the lowest weapon node's height above the ground
/// (`lowestWeaponNodeZ`), the jump gesture's own height while jumping, or 15 units when the mover never had a weapon node.
///
/// `lowestWeaponNodeID/Z` are ONE cache in the C++, shared by `hasWeaponNode`, the target line of sight and this miss-point test; it is
/// refreshed on every call before turn 6 (mover.cpp:3447-3458, 3470-3487). This delegates to [`crate::mover::mover_los_position_for_candidate`],
/// which owns that cache (an earlier second cache in the loadout was first filled at the first miss, from whatever animation frame the mech
/// was in then, so mc2_24's mech 512 saw the ground from node 4 at 23.3 instead of node 1 at 29.1).
pub fn mover_los_position(w: &mut World, h: ObjectHandle) -> Vec3 {
    let Some(position) = w.objects.get(h).map(|o| o.c.position) else {
        return Vec3::default();
    };
    crate::mover::mover_los_position_for_candidate(w, h, position)
}

/// `GameObject::lineOfSight(point, checkVisibleBits)`: inside the playable map, then the team ray from the object's LOS position to the
/// point with the target radius 15. The C++ passes `checkVisibleBits` (true) into `Team::lineOfSight`'s `startExtRad` slot, so the start
/// radius is 1.0; the fog-of-war branch of that function is `#if 0`. A mover's LOS position is [`mover_los_position`]; other objects
/// (turrets) use their own ([`hooks::los_position`](crate::object::hooks::los_position)).
pub fn line_of_sight_point(w: &mut World, h: ObjectHandle, point: Vec3) -> bool {
    let is_mover = w.objects.get(h).is_some_and(|o| o.is_mover());
    let from = if is_mover {
        mover_los_position(w, h)
    } else {
        crate::object::hooks::los_position(w, h)
    };
    let team = w.objects.get(h).map_or(-1, |o| o.team_id());
    w.terrain.is_game_select_position(&point)
        && crate::ai::sensors::line_of_sight_ray(w, from, point, team, 15.0, 1.0)
}
