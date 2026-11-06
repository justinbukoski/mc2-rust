// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Capture eligibility and blockers (`gameobj.cpp:2064-2099`, `bldng.cpp:1041`, `mech.cpp:8923`).

use crate::ids::{ObjectHandle, TeamId, WatchId};
use crate::object::{flags, Kind};
use crate::World;

/// `GameObject::isCaptureable` for the single-player classes shipped in missions.
pub fn is_captureable(w: &World, h: ObjectHandle, team: i32) -> bool {
    let Some(o) = w.objects.get(h) else {
        return false;
    };
    if !o.c.flag(flags::CAPTURABLE) || o.c.is_destroyed() {
        return false;
    }
    match o.kind {
        Kind::Building(_) => o.c.awake() && o.team_id() != team,
        Kind::Vehicle(_) => o.team_id() != team,
        // The shipped BattleMech override ignores its parameter and tests Team::home.
        Kind::Mech(_) => o.team_id() == w.teams.home as i32,
        _ => false,
    }
}

/// `GameObject::getCaptureBlocker(capturingMover, blockerList)`; result order is mover-list order.
pub fn blockers(w: &mut World, target: ObjectHandle, capturer: ObjectHandle) -> Vec<ObjectHandle> {
    let Some((capturing_team, capturer_pos)) =
        w.objects.get(capturer).map(|o| (o.team_id(), o.c.position))
    else {
        return Vec::new();
    };
    let close_to_target = crate::ai::mover_ops::distance_from(w, target, &capturer_pos) <= 30.0;
    let range = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or(0.0, |g| g.mover_general.block_capture_range);
    let candidates = w.objects.movers.clone();
    let mut result = Vec::new();
    for mover in candidates {
        let Some((team, position, armed, active)) = w.objects.get(mover).map(|m| {
            (
                m.team_id(),
                m.c.position,
                crate::combat::loadout_of(m).is_some_and(|l| l.num_weapons() > 0),
                !m.c.is_disabled() && m.c.awake(),
            )
        }) else {
            continue;
        };
        let eligible_team = if close_to_target {
            crate::object::hooks::team_relation(w, capturing_team, team)
                == Some(crate::ai::Relation::Enemy)
        } else {
            team < 0
                || crate::object::hooks::team_relation(w, capturing_team, team)
                    == Some(crate::ai::Relation::Enemy)
        };
        let in_range = crate::ai::mover_ops::distance_from(w, target, &position) < range;
        if eligible_team
            && armed
            && active
            && in_range
            && (close_to_target
                || crate::ai::targeting::team_is_contact(w, capturer, mover, 32 | 1 | 64))
        {
            result.push(mover);
        }
    }
    result
}

/// `Building::setCommanderId` plus `setTeamId(team, false)` for supported capture targets.
pub fn capture_target(w: &mut World, target: ObjectHandle, capturer: ObjectHandle, team: i32) {
    let Some(kind) = w.objects.get(target).map(|o| o.c.class) else {
        return;
    };
    match kind {
        crate::object::ObjectClass::Building => {
            let commander = w.objects.get(capturer).map_or(-1, |o| o.commander_id());
            if let Some(o) = w.objects.get_mut(target) {
                if let Kind::Building(b) = &mut o.kind {
                    b.commander_id = commander as i8;
                }
            }
            crate::object::structure::set_team_id(w, target, team, false);
        }
        crate::object::ObjectClass::GroundVehicle => {
            let old_team = w.objects.get(target).map_or(-1, |o| o.team_id());
            let resource_points = w
                .objects
                .get(target)
                .and_then(|o| w.objects.types.as_ref()?.get(o.c.type_handle as u32))
                .and_then(|ty| match &ty.data {
                    mc2_sim_data::objtype::ObjectTypeData::GroundVehicle(v) => {
                        Some(v.resource_points)
                    }
                    _ => None,
                })
                .unwrap_or(0);
            crate::ai::sensors::mover_leave_team(w, target, old_team);
            if let Some(roster) = usize::try_from(old_team)
                .ok()
                .and_then(|t| w.teams.teams.get_mut(t))
                .and_then(Option::as_mut)
            {
                let wid = w
                    .objects
                    .get(target)
                    .map_or(WatchId::NONE, |o| o.c.watch_id);
                roster.roster.retain(|&id| id != wid);
            }
            let slot = crate::ai::warrior::slot_of(w, target);
            if let Some(o) = w.objects.get_mut(target) {
                if let Kind::Vehicle(v) = &mut o.kind {
                    v.m.team = TeamId(team as i8);
                }
            }
            if let Some(slot) = slot {
                crate::ai::warrior::set_team(w, slot, None);
            }
            if w.clock.turn > 10 && resource_points != 0 && team == w.teams.home as i32 {
                crate::object::hooks::add_resource_points(w, resource_points);
            }
        }
        _ => {}
    }
}
