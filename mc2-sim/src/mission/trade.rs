// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Mission::tradeMover` (mission.cpp:1470) and `Mover::loadPilot` (mover.cpp:3364): a mover changes team and commander, and, when it
//! gets a commander, a new pilot. Used when a disabled mech is recovered (the recovery vehicle's order, the recovery craft).
//!
//! The pilot a recovered mech gets is `LogisticsData::getBestPilot(tonnage)`: campaign state the simulation does not own. The host
//! installs a [`RecoveryPilots`] (see [`crate::World::recovery_pilots`]); without one the trade is refused, never faked.

use std::rc::Rc;

use crate::ai::sensors::{add_team_sensor, remove_team_sensor};
use crate::ids::{ObjectHandle, TeamId, WatchId};
use crate::mission::pilots;
use crate::object::Kind;
use crate::World;

/// The pilot picked for a recovered mech (`getBestPilot` and the `LogisticsPilot` skills copied over the file, mover.cpp:3396).
#[derive(Clone, Debug, PartialEq)]
pub struct RecoveryPilot {
    /// The pilot file stem (`data/missions/warriors/<file>.fit`); it must be in `World::data.pilot_profiles`
    /// (`init::prepare_spawn`), like every spawned pilot.
    pub file: String,
    /// The `LogisticsPilot` gunnery and piloting skills and specialties that override the file, when the campaign changed them.
    pub skills: Option<(f32, f32, [bool; 23])>,
}

/// `LogisticsData::getBestPilot(mechWeight)`: the host's campaign roster. `in_use` is `MechWarrior::warriorInUse(file)`.
pub trait RecoveryPilots {
    /// The best free pilot for a mech of `tonnage`, marked used by the host.
    fn best_pilot(&self, tonnage: i64, in_use: &dyn Fn(&str) -> bool) -> Option<RecoveryPilot>;
    /// `LogisticsData::gotPilotsLeft()`: some free pilot remains, which enables the recovery button (controlgui.cpp:1841). Nothing
    /// is marked used.
    fn has_pilot(&self, in_use: &dyn Fn(&str) -> bool) -> bool;
}

/// `MechWarrior::warriorInUse(name)` (warrior.cpp:8530): a live pilot slot was read from that file.
pub fn warrior_in_use(w: &World, file: &str) -> bool {
    w.mission.pilot_loads.iter().any(|l| {
        l.used
            && l.profile.as_ref().is_some_and(|p| {
                w.data.pilot_profiles.iter().any(|(k, v)| {
                    k.eq_ignore_ascii_case(file) && v.as_ref().is_some_and(|q| Rc::ptr_eq(p, q))
                })
            })
    })
}

/// Why a trade was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TradeError {
    /// No [`RecoveryPilots`] is installed, or it has no pilot left.
    NoPilot,
    /// The pilot file is not loaded in the world, or the brain failed to compile.
    Pilot(String),
    /// The mover is not a mech or vehicle, or the team does not exist.
    BadMover,
}

fn core_mut(w: &mut World, h: ObjectHandle) -> Option<&mut crate::mover::MoverCore> {
    match &mut w.objects.get_mut(h)?.kind {
        Kind::Mech(m) => Some(&mut m.m),
        Kind::Vehicle(v) => Some(&mut v.m),
        _ => None,
    }
}

/// `Mission::tradeMover(mover, newTeamID, newCommanderID, pilot, "pbrain")`.
pub fn trade_mover(
    w: &mut World,
    h: ObjectHandle,
    new_team: i32,
    new_commander: i32,
    pilot: Option<&RecoveryPilot>,
) -> Result<(), TradeError> {
    if core_mut(w, h).is_none()
        || new_team < 0
        || w.teams
            .teams
            .get(new_team as usize)
            .is_none_or(|t| t.is_none())
    {
        return Err(TradeError::BadMover);
    }
    // missionInterface->removeMover(mover): off the group bar
    if let Some(part) = w.objects.get(h).map(|o| o.c.part_id) {
        w.mission.unit_groups.remove(&part);
    }
    if new_commander > -1 {
        let pilot = pilot.ok_or(TradeError::NoPilot)?;
        load_pilot(w, h, pilot, "pbrain")?;
    }
    // ObjectManager->tradeMover(mover, newTeamID, newCommanderID) (objmgr.cpp:612)
    let old_wid = w.objects.watch_id_peek(h);
    let old_team = w.objects.get(h).map_or(-1, |o| o.team_id());
    let slot = core_mut(w, h).map(|c| c.pilot_slot);
    // Mover::tradeRefresh (BattleMech::killed and lost are not modelled):
    if let Some(c) = core_mut(w, h) {
        c.time_left = 15.0;
        c.flags.exploding = false;
        c.flags.withdrawing = false;
    }
    if let Some(Kind::Mech(m)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        m.engine_blow_time = -1.0;
        m.loadout.stats.eject_order_given = false;
    }
    // Mover::setTeamId(newTeamID, true): off the old roster and team sensors, onto the new
    let sensor = w.sensors.pool.iter().position(|s| s.owner == h);
    if old_team > -1 {
        if let Some(Some(team)) = w.teams.teams.get_mut(old_team as usize) {
            if let Some(i) = team.roster.iter().position(|&x| x == old_wid) {
                team.roster.swap_remove(i);
            }
        }
        if let Some(id) = sensor {
            remove_team_sensor(w, old_team, id);
        }
    }
    if let Some(c) = core_mut(w, h) {
        c.team = TeamId(new_team as i8);
    }
    if let Some(id) = sensor {
        add_team_sensor(w, new_team, id);
    }
    if let Some(slot) = slot {
        crate::ai::warrior::set_team(w, slot, Some(new_team as usize));
        if let Some(l) = w.mission.pilot_loads.get_mut(usize::from(slot)) {
            l.team = new_team;
        }
    }
    // watchID = 0; addToRoster (a fresh watch id); setCommanderId; modifyMoverLists(MOVERLIST_TRADE)
    w.objects.trade_mover_lists(&w.teams, h);
    let wid = w.objects.watch_id(h);
    if let Some(Some(team)) = w.teams.teams.get_mut(new_team as usize) {
        team.roster.push(wid);
    }
    if let Some(slot) = slot {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.mover = wid;
        }
    }
    if let Some(c) = core_mut(w, h) {
        c.commander = crate::ids::CommanderId(new_commander as i8);
    }
    // the sensors work again (`sensorSystem->broken = false`); missionInterface->addMover puts it on the group bar and GUI
    if let Some(id) = sensor {
        w.sensors.pool[id].broken = false;
    }
    if new_commander > -1 {
        crate::mission::spawn::set_on_gui(w, h, true);
    }
    Ok(())
}

/// `Mover::loadPilot(file, brain, lPilot)`: the old pilot is freed, a new one is read from `pilot.file` and takes the mover.
fn load_pilot(
    w: &mut World,
    h: ObjectHandle,
    pilot: &RecoveryPilot,
    brain: &str,
) -> Result<(), TradeError> {
    let profile = w
        .data
        .pilot_profiles
        .get(&pilot.file.to_ascii_lowercase())
        .cloned()
        .ok_or_else(|| TradeError::Pilot(format!("pilot file {} is not loaded", pilot.file)))?;
    let old = core_mut(w, h)
        .map(|c| usize::from(c.pilot_slot))
        .ok_or(TradeError::BadMover)?;
    let radius = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or(pilots::DEFAULT_ATTACK_RADIUS, |t| {
            t.warrior.default_attack_radius
        });
    let wid = w.objects.watch_id(h);
    pilots::clear_slot(&mut w.pilots, &mut w.mission.pilot_loads, old, radius);
    w.mission.pilot_loads[old].used = false;
    let slot = pilots::new_warrior(&mut w.mission.pilot_loads)
        .ok_or_else(|| TradeError::Pilot("Too many pilots in this mission!".into()))?;
    {
        let streams = w.rng.0.clone();
        let mut r = streams.borrow_mut();
        let _ = pilots::init_pilot(
            &mut w.pilots,
            &mut w.mission.pilot_loads,
            slot,
            profile,
            brain,
            &mut r.sim,
        );
    }
    if let Some((gunnery, piloting, specialties)) = pilot.skills {
        use mc2_sim_data::pilot::Skill;
        let p = &mut w.pilots.slots[slot];
        let l = &mut w.mission.pilot_loads[slot];
        l.skill_rank[Skill::Gunnery.index()] = gunnery;
        p.skills[Skill::Gunnery.index()] = i32::from(gunnery as i8);
        l.skill_rank[Skill::Piloting.index()] = piloting;
        p.skills[Skill::Piloting.index()] = i32::from(piloting as i8);
        p.specialty = specialties;
        let avg =
            (l.skill_rank[Skill::Gunnery.index()] + l.skill_rank[Skill::Piloting.index()]) / 2.0;
        l.rank = Some(mc2_sim_data::pilot::rank_for_average(avg));
    }
    crate::mission::abl_load::make_pilot_brain(w, slot, brain).map_err(TradeError::Pilot)?;
    w.pilots.slots[slot].mover = wid;
    if let Some(c) = core_mut(w, h) {
        c.pilot_slot = slot as u16;
    }
    Ok(())
}

/// The id of the pilot of `h`, for tests.
pub fn pilot_of(w: &World, h: ObjectHandle) -> Option<WatchId> {
    crate::combat::fx::pilot_slot(w, h)
        .and_then(|s| w.pilots.get(s))
        .map(|p| p.mover)
}
