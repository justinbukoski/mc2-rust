// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Team`, `Commander`, relations. In single player: team 0 friendly to itself, teams 0 and 2 neutral to each other (value 1),
//! everything else enemy (value 2) (`Mission::init`, mission.cpp:1655).

use crate::ids::{MAX_COMMANDERS, MAX_TEAMS};

/// `Team::relations` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Relation {
    /// 0.
    Friendly = 0,
    /// 1.
    Neutral = 1,
    /// 2.
    Enemy = 2,
}

/// `Team`.
#[derive(Clone, Debug, Default)]
pub struct Team {
    /// Roster of mover watch ids (`MAX_MOVERS_PER_TEAM` = 120), in `buildRoster` order with swap-remove.
    pub roster: Vec<crate::ids::WatchId>,
    /// Objectives of the team (only the home team's are evaluated).
    pub num_primary_objectives: i32,
}

/// `Commander`.
#[derive(Clone, Debug, Default)]
pub struct Commander {
    /// Team index.
    pub team: i8,
}

/// All teams and commanders plus the relation matrix.
#[derive(Clone, Debug)]
pub struct Teams {
    /// `Team::teams[]` (None = not created).
    pub teams: Vec<Option<Team>>,
    /// `Commander::commanders[]`.
    pub commanders: Vec<Option<Commander>>,
    /// `Team::relations[8][8]`.
    pub relations: [[Relation; MAX_TEAMS]; MAX_TEAMS],
    /// `Team::home` index.
    pub home: usize,
}

/// `Team::isTargeting` (`team.cpp:343-363`): visit roster watch IDs, resolve each to a mover and its pilot, and ask the
/// pilot for its current target. A nonzero `except` watch ID is excluded.
pub fn is_targeting(
    world: &mut crate::World,
    team_index: usize,
    target: crate::ids::WatchId,
    except: crate::ids::WatchId,
) -> bool {
    let Some(roster) = world
        .teams
        .teams
        .get(team_index)
        .and_then(Option::as_ref)
        .map(|team| team.roster.clone())
    else {
        return false;
    };
    for mover_wid in roster {
        if !except.is_none() && mover_wid == except {
            continue;
        }
        let Some(slot) = world
            .objects
            .by_watch_id(mover_wid)
            .and_then(|mover| crate::combat::fx::pilot_slot(world, mover))
        else {
            continue;
        };
        if crate::ai::warrior::get_current_target(world, slot)
            .and_then(|current| world.objects.get(current).map(|obj| obj.c.watch_id))
            == Some(target)
        {
            return true;
        }
    }
    false
}

impl Default for Teams {
    fn default() -> Self {
        Teams {
            teams: vec![None; MAX_TEAMS],
            commanders: vec![None; MAX_COMMANDERS],
            relations: [[Relation::Enemy; MAX_TEAMS]; MAX_TEAMS],
            home: 0,
        }
    }
}

// ---- save/load (lane D). Everything here is dynamic: relations and rosters change during play, objective counts are set by the
// mission init and read later, and `Commander::team` is set at init. ----

use crate::command::snapshot_enum;

snapshot_enum!(Relation {
    Friendly = 0,
    Neutral = 1,
    Enemy = 2,
});

crate::snapshot_struct!(Team {
    roster,
    num_primary_objectives,
});

crate::snapshot_struct!(Commander { team });

crate::snapshot_struct!(Teams {
    teams,
    commanders,
    relations,
    home,
});
