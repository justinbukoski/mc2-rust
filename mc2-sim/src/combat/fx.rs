// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! What the damage code asks of the pilot and the presentation: `pilot->injure`, `eject`, `triggerAlarm`, `handleAlarm`,
//! `radioMessage` (warrior.cpp:1945, 2009, 4558, 4632, 1304).
//!
//! `injure`, `eject` and `triggerAlarm` only change pilot data and are applied here on the [`Pilot`](crate::ai::Pilot) of the mover
//! (`status`, `wounds`, the alarm lists). `handleAlarm` runs game-system effects and then the brain's ABL alarm handler, and
//! `SensorSystem::disable` clears a contact list: both are lane D's code, so the call is recorded in [`Pilot::pending_calls`] in
//! the order the C++ made it and lane D's consumer ([`crate::ai::calls::run_pending_calls`]) runs it at once, so each effect lands
//! where the C++ made the direct call. Radio chatter is sound only: a [`SimEvent::Radio`] with the `RADIO_*` id (the radio log
//! that throttles chatter belongs to the audio side).

use crate::ai::pilot::{PilotCall, MAX_ALARM_TRIGGERS};
use crate::combat::stats::LegEffects;
use crate::events::SimEvent;
use crate::ids::ObjectHandle;
use crate::object::Kind;

/// `RADIO_EJECTING` (dradio.h).
pub const RADIO_EJECTING: i32 = 18;
/// `RADIO_DEATH`.
pub const RADIO_DEATH: i32 = 19;
/// `RADIO_CRIPPLED`.
pub const RADIO_CRIPPLED: i32 = 20;
/// `RADIO_PILOT_HURT`.
pub const RADIO_PILOT_HURT: i32 = 21;
/// `RADIO_WEAPON_DOWN`.
pub const RADIO_WEAPON_DOWN: i32 = 22;
/// `RADIO_WEAPONS_OUT`.
pub const RADIO_WEAPONS_OUT: i32 = 23;
/// `RADIO_AMMO_OUT`.
pub const RADIO_AMMO_OUT: i32 = 24;
/// `RADIO_ARMORHOLE`.
pub const RADIO_ARMORHOLE: i32 = 30;

/// `PILOT_ALARM_*` (warrior.h:288).
pub mod alarm {
    /// `PILOT_ALARM_TARGET_OF_WEAPONFIRE`.
    pub const TARGET_OF_WEAPONFIRE: u8 = 0;
    /// `PILOT_ALARM_FIRED_WEAPON`.
    pub const FIRED_WEAPON: u8 = 17;
    /// `PILOT_ALARM_HIT_BY_WEAPONFIRE`.
    pub const HIT_BY_WEAPONFIRE: u8 = 1;
    /// `PILOT_ALARM_VEHICLE_INCAPACITATED`.
    pub const VEHICLE_INCAPACITATED: u8 = 6;
    /// `PILOT_ALARM_VEHICLE_DESTROYED`.
    pub const VEHICLE_DESTROYED: u8 = 7;
    /// `PILOT_ALARM_VEHICLE_WITHDRAWN`.
    pub const VEHICLE_WITHDRAWN: u8 = 8;
    /// `PILOT_ALARM_COLLISION`.
    pub const COLLISION: u8 = 10;
    /// `PILOT_ALARM_KILLED_TARGET`.
    pub const KILLED_TARGET: u8 = 12;
}

/// `WARRIOR_STATUS_*` (warrior.h:262), as the typed [`WarriorStatus`] lane E's `Pilot::status` uses (same tags 0-4).
pub mod warrior_status {
    use crate::mission::outcome::WarriorStatus;
    /// Normal.
    pub const NORMAL: WarriorStatus = WarriorStatus::Normal;
    /// Withdrawing.
    pub const WITHDRAWING: WarriorStatus = WarriorStatus::Withdrawing;
    /// Withdrawn.
    pub const WITHDRAWN: WarriorStatus = WarriorStatus::Withdrawn;
    /// Ejected.
    pub const EJECTED: WarriorStatus = WarriorStatus::Ejected;
    /// Dead.
    pub const DEAD: WarriorStatus = WarriorStatus::Dead;
}

/// `DeathCause` (mover.h:140).
pub mod death {
    /// `ENGINE_DEATH`.
    pub const ENGINE: u32 = 1;
    /// `PILOT_DEATH`.
    pub const PILOT: u32 = 2;
    /// `EJECTION_DEATH`.
    pub const EJECTION: u32 = 3;
    /// `POWER_USED_UP`.
    pub const POWER_USED_UP: u32 = 5;
}

/// The pool slot of the pilot flying `h` (`Mover::pilot`), `None` for a mover without one.
pub fn pilot_slot(w: &crate::World, h: ObjectHandle) -> Option<u16> {
    let o = w.objects.get(h)?;
    let slot = match &o.kind {
        Kind::Mech(m) => m.m.pilot_slot,
        Kind::Vehicle(v) => v.m.pilot_slot,
        _ => return None,
    };
    (slot != 0 && usize::from(slot) < w.pilots.slots.len()).then_some(slot)
}

/// `MechWarrior::radioMessage(message)` (warrior.cpp:1304): a [`SimEvent::Radio`] for the pilot's slot. The gates that need the
/// radio log or the sound system (`active()`, the mission turn, the per-message rate limits, `radio->playMessage`) are the audio
/// side's (`mc2_audio::radio`), which is given `underHomeCommand()` (warrior.cpp:1316) with the message.
pub fn radio_message(w: &mut crate::World, mover: ObjectHandle, message: i32) {
    let Some(slot) = pilot_slot(w, mover) else {
        return;
    };
    let under_home_command = w
        .objects
        .get(mover)
        .is_some_and(|o| o.commander_id() == crate::combat::HOME_COMMANDER);
    w.events.push(SimEvent::Radio {
        pilot: u32::from(slot),
        message,
        under_home_command,
    });
}

/// `MechWarrior::anyPlayerInCombat` (warrior.cpp:4498-4511): a pilot of the player's force (`commanderId == home`) whose vehicle
/// has a current target (`getCurrentTarget` = `getLastTarget`, which drops a destroyed, disabled or friendly one). The mission
/// scripts read it (`getplayerincombat`, ablmc2.cpp:2789) to switch to the combat tune. Read-only: the C++ `getLastTarget` also
/// clears a stale target and its combat order; here the pilot does that on its own next call (rust/docs/DIFFERENCES.md).
pub fn any_player_in_combat(w: &crate::World) -> bool {
    for slot in 1..w.pilots.slots.len() {
        let slot = u16::try_from(slot).unwrap_or(u16::MAX);
        if crate::ai::warrior::vehicle(w, slot).is_none()
            || !crate::ai::warrior::under_home_command(w, slot)
        {
            continue;
        }
        if crate::ai::warrior::peek_current_target(w, slot).is_some() {
            return true;
        }
    }
    false
}

/// `MechWarrior::triggerAlarm(alarmCode, triggerId)` (warrior.cpp:4558): the trigger goes on the alarm list and into the history; a
/// full list (15) drops it. Returns 0, or -1 when dropped.
pub fn trigger_alarm(w: &mut crate::World, mover: ObjectHandle, alarm: u8, trigger_id: i32) -> i32 {
    let Some(slot) = pilot_slot(w, mover) else {
        return -1;
    };
    let Some(p) = w.pilots.get_mut(slot) else {
        return -1;
    };
    let code = usize::from(alarm);
    if p.alarms[code].len() == MAX_ALARM_TRIGGERS
        || p.alarm_history[code].len() == MAX_ALARM_TRIGGERS
    {
        return -1;
    }
    p.alarms[code].push(trigger_id);
    p.alarm_history[code].push(trigger_id);
    0
}

/// `MechWarrior::handleAlarm(alarmCode, triggerId)` (warrior.cpp:4632): a sleeping mover ignores it; the alarms that cannot wait for the
/// pilot's own update are handled at once (incapacitation, withdrawal), then the brain's ABL handler is recorded for lane D.
pub fn handle_alarm(w: &mut crate::World, mover: ObjectHandle, alarm: u8, trigger_id: u32) {
    let awake = w.objects.get(mover).is_some_and(|o| o.c.awake());
    if !awake {
        return;
    }
    match alarm {
        alarm::VEHICLE_INCAPACITATED => own_vehicle_incapacitation(w, mover, trigger_id),
        // handleOwnVehicleDestruction does nothing; handleOwnVehicleWithdrawn marks the pilot withdrawn
        alarm::VEHICLE_WITHDRAWN => {
            if let Some(slot) = pilot_slot(w, mover) {
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.status = warrior_status::WITHDRAWN;
                }
            }
        }
        _ => {}
    }
    if let Some(slot) = pilot_slot(w, mover) {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.pending_calls
                .push(PilotCall::BrainAlarm { alarm, trigger_id });
        }
    }
    crate::ai::calls::run_pending_calls(w, mover);
}

/// `MechWarrior::handleOwnVehicleIncapacitation(cause)` (warrior.cpp:6941): a pilot whose center torso is gone (cause 0), whose engine
/// blew (1) or whose legs are gone (66) ejects if he can; then his orders are cleared (recorded for lane D).
pub fn own_vehicle_incapacitation(w: &mut crate::World, mover: ObjectHandle, cause: u32) {
    if matches!(cause, 0 | 1 | 66) {
        crate::combat::damage::handle_ejection(w, mover);
    }
    if let Some(slot) = pilot_slot(w, mover) {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.pending_calls.push(PilotCall::ClearOrders);
        }
    }
    crate::ai::calls::run_pending_calls(w, mover);
}

/// `sensorSystem->disable()` (and `broken = true` for vehicles), recorded for lane D.
pub fn sensor_disable(w: &mut crate::World, mover: ObjectHandle, broken: bool) {
    if let Some(slot) = pilot_slot(w, mover) {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.pending_calls.push(PilotCall::SensorDisable { broken });
        }
    }
    crate::ai::calls::run_pending_calls(w, mover);
}

/// The pilot-side effects of [`crate::combat::stats::calc_leg_status`].
pub fn leg_effects(w: &mut crate::World, mover: ObjectHandle, fx: LegEffects) {
    if fx.incapacitated_alarm {
        trigger_alarm(w, mover, alarm::VEHICLE_INCAPACITATED, 66);
    }
    if fx.crippled_message {
        radio_message(w, mover, RADIO_CRIPPLED);
    }
}

/// `MechWarrior::injure(numWounds, checkEject)` (warrior.cpp:1945). Returns true when the pilot died.
pub fn injure(
    w: &mut crate::World,
    mover: ObjectHandle,
    num_wounds: f32,
    check_eject: bool,
) -> bool {
    let Some(slot) = pilot_slot(w, mover) else {
        return false;
    };
    let skill_try = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or([0.0; 4], |t| t.skills.skill_try);
    let skill_success = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or([0.0; 4], |t| t.skills.skill_success);
    let piloting = mc2_sim_data::pilot::Skill::Piloting.index();
    {
        let Some(p) = w.pilots.get(slot) else {
            return false;
        };
        if p.status != warrior_status::NORMAL {
            return false;
        }
    }
    let (wounds, escapes) = {
        let p = w.pilots.get_mut(slot).expect("checked above");
        p.wounds += num_wounds;
        (p.wounds, p.escapes_thru_ejection)
    };
    if wounds >= 6.0 {
        if check_eject || escapes {
            if let Some(l) = w.mission.pilot_loads.get_mut(usize::from(slot)) {
                l.skill_points[piloting] += skill_try[piloting];
            }
            if escapes {
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.wounds = 3.0;
                }
                if let Some(l) = w.mission.pilot_loads.get_mut(usize::from(slot)) {
                    l.skill_points[piloting] += skill_success[piloting];
                }
                // If we can't eject, we're dead... (the ejection always succeeds)
                if !crate::combat::damage::handle_ejection(w, mover) {
                    if let Some(p) = w.pilots.get_mut(slot) {
                        p.wounds = 6.0;
                    }
                }
            }
        }
        // The warrior's vehicle is immediately disabled...
        let still_dying = w.pilots.get(slot).is_some_and(|p| p.wounds >= 6.0) && !escapes;
        if still_dying {
            radio_message(w, mover, RADIO_DEATH);
            if let Some(p) = w.pilots.get_mut(slot) {
                p.status = warrior_status::DEAD;
            }
            crate::combat::damage::disable(w, mover, death::PILOT);
            return true;
        }
    } else if num_wounds > 0.0 {
        // DON'T play this if they die!
        radio_message(w, mover, RADIO_PILOT_HURT);
    }
    false
}

/// `MechWarrior::eject` (warrior.cpp:2009).
pub fn eject(w: &mut crate::World, mover: ObjectHandle) {
    let Some(slot) = pilot_slot(w, mover) else {
        return;
    };
    {
        let Some(p) = w.pilots.get(slot) else { return };
        if p.status != warrior_status::NORMAL && p.status != warrior_status::WITHDRAWING {
            return;
        }
    }
    let died = {
        let p = w.pilots.get_mut(slot).expect("checked above");
        if p.wounds < 6.0 {
            p.wounds += 1.0; // pilots take one point on ejection
        }
        p.wounds >= 6.0
    };
    if died {
        radio_message(w, mover, RADIO_DEATH);
        if let Some(p) = w.pilots.get_mut(slot) {
            p.status = warrior_status::DEAD;
        }
    } else {
        radio_message(w, mover, RADIO_EJECTING);
        if let Some(p) = w.pilots.get_mut(slot) {
            p.status = warrior_status::EJECTED;
        }
    }
    // The warrior's vehicle is immediately disabled...
    crate::combat::damage::disable(w, mover, death::EJECTION);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::{BodyLocationState, Loadout};
    use crate::ids::CommanderId;
    use crate::mover::{MechState, MoverCore};
    use crate::object::{flags, Common, Obj, ObjectClass};
    use crate::rng::RngMode;

    /// A mech with eight empty body locations and a pilot in slot 1, in a world without game data.
    fn world_with_mech(escapes: bool) -> (crate::World, ObjectHandle) {
        let mut w = crate::World::new(RngMode::LegacyMsvc, 1);
        let h = ObjectHandle(1);
        let body = (0..8)
            .map(|i| BodyLocationState {
                max_internal_structure: 10,
                cur_internal_structure: 10.0,
                critical_spaces: vec![(255, false); if i == 0 || i >= 6 { 6 } else { 12 }],
                case: true,
                ..BodyLocationState::default()
            })
            .collect();
        let state = MechState {
            m: MoverCore {
                pilot_slot: 1,
                commander: CommanderId(1),
                ..MoverCore::default()
            },
            loadout: Loadout {
                armor: vec![(5.0, 5); 11],
                body,
                ..Loadout::default()
            },
            ..MechState::default()
        };
        let mut c = Common::new(ObjectClass::BattleMech, h);
        c.flags |= flags::AWAKE;
        w.objects.list = vec![
            None,
            Some(Obj {
                c,
                kind: crate::object::Kind::Mech(Box::new(state)),
            }),
        ];
        w.pilots.slots = vec![Default::default(), Default::default()];
        w.pilots.slots[1].escapes_thru_ejection = escapes;
        (w, h)
    }

    #[test]
    fn the_alarm_lists_hold_fifteen_triggers() {
        let (mut w, h) = world_with_mech(false);
        for i in 0..15 {
            assert_eq!(trigger_alarm(&mut w, h, alarm::HIT_BY_WEAPONFIRE, i), 0);
        }
        assert_eq!(trigger_alarm(&mut w, h, alarm::HIT_BY_WEAPONFIRE, 99), -1);
        let p = &w.pilots.slots[1];
        assert_eq!(p.alarms[1].len(), 15);
        assert_eq!(p.alarm_history[1].len(), 15);
        // another alarm still works
        assert_eq!(trigger_alarm(&mut w, h, alarm::COLLISION, 7), 0);
    }

    #[test]
    fn a_pilot_with_six_wounds_dies_and_takes_the_mech_with_him() {
        let (mut w, h) = world_with_mech(false);
        assert!(!injure(&mut w, h, 2.0, true));
        assert_eq!(w.pilots.slots[1].wounds, 2.0);
        assert_eq!(w.pilots.slots[1].status, warrior_status::NORMAL);
        assert!(injure(&mut w, h, 4.0, true));
        assert_eq!(w.pilots.slots[1].status, warrior_status::DEAD);
        assert!(w.objects.get(h).unwrap().c.is_disabled());
        // a dead pilot cannot be hurt again
        assert!(!injure(&mut w, h, 6.0, true));
        assert!(w.events.iter().any(|e| matches!(
            e,
            crate::SimEvent::Radio {
                message: RADIO_DEATH,
                ..
            }
        )));
    }

    #[test]
    fn a_pilot_who_escapes_through_ejection_is_left_at_three_wounds() {
        let (mut w, h) = world_with_mech(true);
        assert!(!injure(&mut w, h, 6.0, false));
        let p = &w.pilots.slots[1];
        // wounds = 3, then the ejection costs one more point
        assert_eq!(p.wounds, 4.0);
        assert_eq!(p.status, warrior_status::EJECTED);
        let o = w.objects.get(h).unwrap();
        assert!(o.c.is_disabled());
        // the head blew with the ejection
        assert_eq!(
            crate::combat::loadout_of(o).unwrap().body[0].damage_state,
            crate::combat::stats::IS_DAMAGE_DESTROYED
        );
    }

    #[test]
    fn handling_an_alarm_of_a_sleeping_mover_does_nothing() {
        let (mut w, h) = world_with_mech(false);
        w.objects.get_mut(h).unwrap().c.flags &= !flags::AWAKE;
        handle_alarm(&mut w, h, alarm::VEHICLE_INCAPACITATED, 1);
        assert!(w.pilots.slots[1].pending_calls.is_empty());
        assert_eq!(w.pilots.slots[1].status, warrior_status::NORMAL);
    }
}
