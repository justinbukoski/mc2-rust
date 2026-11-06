// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! ABL capture queries and setters (`code/ablmc2.cpp:3790-3960,4629-4705`).

use crate::ids::{ObjectHandle, PartId, WatchId};
use crate::mission::bindings::SimBindings;
use crate::object::{flags, Kind};

fn object(b: &SimBindings<'_>, id: i32) -> Option<ObjectHandle> {
    if id == -1 {
        b.world.objects.by_watch_id(b.cur_object)
    } else {
        b.world.objects.find_by_part_id(PartId(id))
    }
}

/// `execSetCaptured`: `GameObject::setCaptured` only changes the flag.
pub fn set_captured(b: &mut SimBindings<'_>, id: i32) {
    if let Some(h) = object(b, id) {
        if let Some(o) = b.world.objects.get_mut(h) {
            o.c.set_flag(flags::CAPTURED, true);
        }
    }
}

/// `execSetCapturable` (`ablmc2.cpp:3838`): the C++ ignores its boolean argument and
/// makes buildings and ground vehicles capturable. Multiplayer-only clearing is absent
/// in this single-player simulation.
pub fn set_capturable(b: &mut SimBindings<'_>, id: i32, _capturable: bool) {
    if let Some(h) = object(b, id) {
        if let Some(o) = b.world.objects.get_mut(h) {
            if matches!(o.kind, Kind::Building(_) | Kind::Vehicle(_)) {
                o.c.set_flag(flags::CAPTURABLE, true);
            }
        }
    }
}

fn movers_for_id(b: &SimBindings<'_>, id: i32) -> Vec<ObjectHandle> {
    if (500..=508).contains(&id) {
        let team = (id - 500) as usize;
        return b
            .world
            .teams
            .teams
            .get(team)
            .and_then(Option::as_ref)
            .map(|t| {
                t.roster
                    .iter()
                    .filter_map(|&wid| b.world.objects.by_watch_id(wid))
                    .collect()
            })
            .unwrap_or_default();
    }
    if (1..=511).contains(&id) {
        return b
            .world
            .objects
            .movers
            .iter()
            .copied()
            .filter(|&h| {
                b.world
                    .objects
                    .get(h)
                    .and_then(crate::combat::core_of)
                    .is_some_and(|m| m.squad_id == id)
            })
            .collect();
    }
    Vec::new()
}

/// `execIsCaptured`: unit IDs count captured members; other IDs query one object.
pub fn is_captured(b: &SimBindings<'_>, id: i32) -> i32 {
    if (1..=511).contains(&id) {
        movers_for_id(b, id)
            .iter()
            .filter(|&&h| {
                b.world
                    .objects
                    .get(h)
                    .is_some_and(|o| o.c.flag(flags::CAPTURED))
            })
            .count() as i32
    } else {
        i32::from(
            object(b, id)
                .and_then(|h| b.world.objects.get(h))
                .is_some_and(|o| o.c.flag(flags::CAPTURED)),
        )
    }
}

/// `GameObject::isCaptureable` for the supported single-player object classes.
/// `execIsCapturable`; team -2 asks only for the capturable flag and not-destroyed state.
pub fn is_capturable(b: &SimBindings<'_>, id: i32, team: i32) -> bool {
    let Some(h) = object(b, id) else { return false };
    if team == -2 {
        return b
            .world
            .objects
            .get(h)
            .is_some_and(|o| o.c.flag(flags::CAPTURABLE) && !o.c.is_destroyed());
    }
    crate::ai::capture::is_captureable(b.world, h, team)
}

/// `execIsTeamTargeting` delegates to `Team::isTargeting` (`ablmc2.cpp:4654-4658`,
/// `team.cpp:343-363`): query each roster mover's pilot current target by watch ID.
/// Team 0 instead uses the object's attacker count. `except_id` is a watch ID.
pub fn is_team_targeting(
    b: &mut SimBindings<'_>,
    team_id: i32,
    target_id: i32,
    except_id: i32,
) -> bool {
    if team_id == 0 {
        // `ObjectManager::get` consumes an object handle here, unlike the other
        // capture natives that resolve their object argument as a part ID.
        // (`ablmc2.cpp:4659-4663`, `objmgr.cpp:1916-1923`).
        let target = ObjectHandle(target_id as u32);
        return b
            .world
            .objects
            .get(target)
            .is_some_and(|o| o.c.num_attackers > 0);
    }
    if !(500..=508).contains(&team_id) {
        return false;
    }
    // `Team::isTargeting` receives the raw target watch ID from ABL; do not resolve this
    // argument as a part ID (`ablmc2.cpp:4654-4658`, `team.cpp:343-363`).
    let target_wid = WatchId(target_id as u32);
    if target_wid.is_none() {
        return false;
    }
    crate::ai::team::is_targeting(
        b.world,
        (team_id - 500) as usize,
        target_wid,
        WatchId(except_id as u32),
    )
}

/// `execIsTeamCapturing`: the original explicitly ignores `except_id`.
pub fn is_team_capturing(
    b: &SimBindings<'_>,
    team_id: i32,
    target_id: i32,
    _except_id: i32,
) -> bool {
    let Some(target) =
        object(b, target_id).and_then(|h| b.world.objects.get(h).map(|o| o.c.watch_id))
    else {
        return false;
    };
    if target.is_none() {
        return false;
    }
    match team_id {
        0 => b.world.objects.is_team_capturing(None, target),
        500..=508 => b
            .world
            .objects
            .is_team_capturing(Some((team_id - 500) as usize), target),
        _ => false,
    }
}
