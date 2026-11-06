// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The consumer of [`Pilot::pending_calls`](super::pilot::Pilot::pending_calls): the calls lane C's damage code makes on a pilot
//! (`handleAlarm`'s ABL callback, `handleOwnVehicleIncapacitation`'s order clean-up, `sensorSystem->disable()`), run in the order
//! the C++ made them (`combat/fx.rs` queues them in that order and runs this right after each push, so the effects are as immediate
//! as the direct calls in `warrior.cpp:4632-4690, 6941-6990`).

use super::pilot::{AttackOrders, OrderState, PilotCall};
use crate::ids::ObjectHandle;

/// `pilotAlarmFunctionName[NUM_PILOT_ALARMS]` (warrior.cpp:151-171): the ABL function `brainAlarmCallback[alarm]` resolves.
pub const PILOT_ALARM_FUNCTION_NAMES: [&str; super::pilot::NUM_ALARMS] = [
    "handletargetofweaponfire",
    "handlehitbyweaponfire",
    "handledamagetakenrate",
    "handledeathofmate",
    "handlecripplingoffriendlyvehicle",
    "handledestructionoffriendlyvehicle",
    "handleincapacitationofvehicle",
    "handledestructionofvehicle",
    "handlewithdraw",
    "handleattackorder",
    "handlecollision",
    "handleguardbreach",
    "handlekilledtarget",
    "handlematefiredweapon",
    "handleplayerorder",
    "handlenomovepath",
    "handlegateclosing",
    "handlefiredweapon",
    "handlenewmover",
];

/// Run (and drain) the pending calls of the pilot flying `mover`, oldest first.
pub fn run_pending_calls(w: &mut crate::World, mover: ObjectHandle) {
    let Some(slot) = crate::combat::fx::pilot_slot(w, mover) else {
        return;
    };
    let calls = match w.pilots.get_mut(slot) {
        Some(pilot) => std::mem::take(&mut pilot.pending_calls),
        None => return,
    };
    link_pilot(w, mover, slot);
    for call in calls {
        match call {
            PilotCall::BrainAlarm { alarm, .. } => {
                super::brain::call_alarm_handler(w, slot, usize::from(alarm))
            }
            PilotCall::ClearOrders => clear_orders(w, mover, slot),
            PilotCall::UpdateAttackerStatus {
                attacker,
                time_bits,
            } => {
                super::warrior::update_attacker_status(w, slot, attacker, f32::from_bits(time_bits))
            }
            PilotCall::SensorDisable { broken } => super::sensors::disable(w, mover, broken),
        }
    }
}

/// `MechWarrior::vehicleWID` of the pilot flying `mover` points at `mover` (set by `Mover::setPilotHandle`; a world assembled by hand
/// may not have the link yet, and every call here comes from a mover that knows its pilot).
fn link_pilot(w: &mut crate::World, mover: ObjectHandle, slot: u16) {
    let wid = w.objects.watch_id(mover);
    if let Some(p) = w.pilots.get_mut(slot) {
        if p.mover != wid {
            p.mover = wid;
        }
    }
}

/// `Mover::setGroupId(groupId, setup)` (mover.cpp:2738-2747), reached from `MoverGroup::add` / `remove` / `disband` with `setup` true
/// (group.cpp:111, 128, 158): a mover that joins a group has its orders cleared like an incapacitated one (so its pilot logs one
/// `PILOT_ALARM_NO_MOVEPATH`), and its order state returns to general.
pub fn set_group_id(w: &mut crate::World, mover: ObjectHandle, group_id: i32, setup: bool) {
    if let Some(core) = w
        .objects
        .get_mut(mover)
        .and_then(|obj| match &mut obj.kind {
            crate::object::Kind::Mech(m) => Some(&mut m.m),
            crate::object::Kind::Vehicle(v) => Some(&mut v.m),
            _ => None,
        })
    {
        core.group_id = group_id as i8;
    }
    let Some(slot) = crate::combat::fx::pilot_slot(w, mover) else {
        return;
    };
    link_pilot(w, mover, slot);
    if setup && group_id > -1 {
        super::warrior::clear_cur_tac_order(w, slot, false);
        if let Some(pilot) = w.pilots.get_mut(slot) {
            pilot.order_state = OrderState::General;
        }
    }
}

/// `MechWarrior::clearCurTacOrder(false)` on the pilot flying `mover` (warrior.cpp:4397-4422).
pub fn clear_cur_tac_order(w: &mut crate::World, _mover: ObjectHandle, slot: u16) {
    super::warrior::clear_cur_tac_order(w, slot, false);
}

/// `MechWarrior::handleOwnVehicleIncapacitation` after the ejection (warrior.cpp:6957-6975): clear the current order, general order
/// state, zero both paths' step counts, re-initialise the move orders, drop the path-manager entry, re-initialise the attack
/// orders and forget the last target.
fn clear_orders(w: &mut crate::World, _mover: ObjectHandle, slot: u16) {
    super::warrior::clear_cur_tac_order(w, slot, false);
    let now = w.clock.scenario_time;
    if let Some(pilot) = w.pilots.get_mut(slot) {
        pilot.order_state = OrderState::General;
        for path in &mut pilot.lane_b_move.paths {
            path.num_steps = 0;
        }
        pilot.lane_b_move.init(now);
    }
    w.paths.remove(slot);
    if let Some(pilot) = w.pilots.get_mut(slot) {
        pilot.attack_orders = AttackOrders::init(now);
    }
    super::warrior::set_last_target(w, slot, None, false, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::pilot::Pilot;
    use crate::ai::sensors::Sensor;
    use crate::combat::fx;
    use crate::command::{OrderCode, OrderOrigin, TacticalOrder};
    use crate::ids::{CommanderId, WatchId};
    use crate::mover::{MechState, MoverCore};
    use crate::object::{flags, Common, Kind, Obj, ObjectClass};
    use crate::rng::RngMode;

    /// `PILOT_ALARM_NO_MOVEPATH` (warrior.h:304).
    const PILOT_ALARM_NO_MOVEPATH: u8 = 15;

    fn mech(handle: ObjectHandle, commander: i8) -> Obj {
        let mut c = Common::new(ObjectClass::BattleMech, handle);
        c.flags |= flags::EXISTS | flags::AWAKE;
        let state = MechState {
            m: MoverCore {
                pilot_slot: 1,
                commander: CommanderId(commander),
                suppression_fire: 1,
                ..MoverCore::default()
            },
            ..MechState::default()
        };
        Obj {
            c,
            kind: Kind::Mech(Box::new(state)),
        }
    }

    /// One mech (handle 1, pilot slot 1) in an otherwise empty world.
    fn world() -> (crate::World, ObjectHandle) {
        let mut w = crate::World::new(RngMode::LegacyMsvc, 1);
        let h = ObjectHandle(1);
        w.objects.list = vec![None, Some(mech(h, 1))];
        w.pilots.slots = vec![Pilot::default(), Pilot::default()];
        (w, h)
    }

    /// `handleOwnVehicleIncapacitation` (warrior.cpp:6941-6975) for a cause that does not eject (5): the combat order, its counter, the
    /// move orders, both paths, the path-manager entry, the attack orders and the last target are cleared; the order state goes back
    /// to general; `triggerAlarm(PILOT_ALARM_NO_MOVEPATH, MOVEPATH_ERR_TACORDER_CLEARED)` (warrior.cpp:4408) is recorded.
    #[test]
    fn incapacitation_clears_orders_paths_attack_orders_and_last_target() {
        let (mut w, h) = world();
        w.clock.scenario_time = 12.5;
        w.pilots.num_warriors_in_combat = 1;
        {
            let p = &mut w.pilots.slots[1];
            p.cur_order =
                TacticalOrder::new(OrderOrigin::Player, OrderCode::AttackObject, false, 3.0);
            p.order_state = OrderState::Player;
            p.last_target = (WatchId(77), 2.0);
            p.attack_orders.target = WatchId(77);
            p.attack_orders.pursue = true;
            p.lane_b_move.goal_type = 0;
            p.lane_b_move.has_goal = true;
            p.lane_b_move.speed_throttle = 40;
            p.lane_b_move.run = true;
            p.lane_b_move.move_state = 3;
            p.lane_b_move.wait_for_point_time = 9.0;
            p.lane_b_move.is_yielding = true;
            for path in &mut p.lane_b_move.paths {
                path.init(3);
            }
        }
        w.paths.request(crate::mover::manager::PathRequest {
            pilot: 1,
            selector: 0,
            params: 0,
            source: 0,
        });
        fx::own_vehicle_incapacitation(&mut w, h, 5);

        assert_eq!(w.pilots.num_warriors_in_combat, 0); // warrior.cpp:4399-4402
        assert_eq!(
            w.objects.get(h).map(|o| match &o.kind {
                Kind::Mech(m) => m.m.suppression_fire,
                _ => -1,
            }),
            Some(0)
        ); // warrior.cpp:4406
        let p = &w.pilots.slots[1];
        assert!(p.pending_calls.is_empty());
        assert_eq!(p.cur_order.code, OrderCode::None);
        assert_eq!(p.cur_order.origin, OrderOrigin::Commander);
        assert_eq!(p.cur_order.stage, 1); // curTacOrder.init()
        assert_eq!(p.last_order.last_time, 12.5); // warrior.cpp:4412
        assert_eq!(p.order_state, OrderState::General); // warrior.cpp:6964
        assert_eq!(p.lane_b_move.goal_type, 0xFFFF_FFFF); // MoveOrders::init / clearMoveGoal
        assert!(!p.lane_b_move.has_goal);
        assert_eq!(p.lane_b_move.goal.x, -999_999.0);
        assert_eq!(p.lane_b_move.speed_throttle, 100); // MoveOrders::init (warrior.cpp:1217-1224)
        assert_eq!(p.lane_b_move.speed_state, 2);
        assert!(!p.lane_b_move.run);
        assert_eq!(p.lane_b_move.move_state, 1);
        assert_eq!(p.lane_b_move.wait_for_point_time, -1.0);
        assert!(!p.lane_b_move.is_yielding);
        for path in &p.lane_b_move.paths {
            assert_eq!((path.num_steps, path.num_steps_when_not_paused), (0, 0));
        }
        assert!(w.paths.queue.is_empty()); // PathManager->remove(this)
        assert_eq!(p.alarms[15], vec![-14]); // PILOT_ALARM_NO_MOVEPATH, MOVEPATH_ERR_TACORDER_CLEARED
        assert_eq!(p.alarm_history[15], vec![-14]);
        assert_eq!(p.attack_orders, AttackOrders::init(12.5)); // warrior.cpp:6974
        assert_eq!(p.last_target, (WatchId(0), 12.5)); // setLastTarget(NULL)
    }

    /// A non-combat current order does not touch `numWarriorsInCombat` (warrior.cpp:4399).
    #[test]
    fn clearing_a_move_order_leaves_the_combat_counter_alone() {
        let (mut w, h) = world();
        w.pilots.num_warriors_in_combat = 2;
        w.pilots.slots[1].cur_order =
            TacticalOrder::new(OrderOrigin::Player, OrderCode::MoveToPoint, false, 0.0);
        clear_cur_tac_order(&mut w, h, 1);
        assert_eq!(w.pilots.num_warriors_in_combat, 2);
        assert_eq!(w.pilots.slots[1].cur_order.code, OrderCode::None);
        // clearAttackOrders (not init): targetTime -1, time/targetPoint untouched (warrior.cpp:2406-2414)
        assert_eq!(w.pilots.slots[1].attack_orders.target_time, -1.0);
    }

    /// The home commander's pilot releasing a live last target decrements its count.
    #[test]
    fn home_pilot_releasing_a_live_target_decrements_the_attacker_count() {
        let (mut w, h) = world();
        let target = ObjectHandle(2);
        w.objects.list.push(Some(mech(target, 3)));
        if let Some(Obj {
            kind: Kind::Mech(m),
            ..
        }) = w.objects.list[1].as_mut()
        {
            m.m.commander = CommanderId(0);
        }
        let wid = w.objects.watch_id(target);
        w.pilots.slots[1].last_target = (wid, 1.0);
        fx::own_vehicle_incapacitation(&mut w, h, 5);
        assert_eq!(w.objects.get(target).unwrap().c.num_attackers, 0);
        w.events.clear();
        w.pilots.slots[1].last_target = (wid, 1.0);
        fx::own_vehicle_incapacitation(&mut w, h, 5);
        assert_eq!(w.objects.get(target).unwrap().c.num_attackers, 0);
    }

    #[test]
    fn update_attacker_status_keeps_unique_watch_ids_and_refreshes_time() {
        let (mut w, _) = world();
        let attacker = crate::ids::WatchId(77);
        super::super::warrior::update_attacker_status(&mut w, 1, attacker, 2.0);
        super::super::warrior::update_attacker_status(&mut w, 1, attacker, 5.0);
        assert_eq!(w.pilots.slots[1].attackers, [(attacker, 5.0)]);
    }

    #[test]
    fn update_attacker_status_caps_the_list_at_fifty_entries() {
        let (mut w, _) = world();
        for id in 1..=50 {
            super::super::warrior::update_attacker_status(
                &mut w,
                1,
                crate::ids::WatchId(id),
                id as f32,
            );
        }
        super::super::warrior::update_attacker_status(&mut w, 1, crate::ids::WatchId(51), 51.0);

        let attackers = &w.pilots.slots[1].attackers;
        assert_eq!(attackers.len(), 50);
        assert_eq!(attackers.first(), Some(&(crate::ids::WatchId(1), 1.0)));
        assert_eq!(attackers.last(), Some(&(crate::ids::WatchId(50), 50.0)));
    }

    fn sensor_world() -> (crate::World, ObjectHandle, [ObjectHandle; 3]) {
        let (mut w, h) = world();
        let targets = [ObjectHandle(2), ObjectHandle(3), ObjectHandle(4)];
        for t in targets {
            w.objects.list.push(Some(mech(t, 3)));
        }
        w.sensors.pool = vec![
            Sensor {
                id: 0,
                owner: h,
                team_index: 0,
                not_shutdown: true,
                team_contacts_enabled: true,
                contacts: vec![(targets[0], 1), (targets[1], 1), (targets[2], 1)],
                ..Sensor::default()
            },
            Sensor {
                id: 1,
                owner: ObjectHandle(9),
                team_index: 1,
                not_shutdown: true,
                team_contacts_enabled: true,
                contacts: vec![(targets[1], 1)],
                ..Sensor::default()
            },
        ];
        w.sensors.teams[0] = vec![0, 1];
        w.sensors.team_contact_order[0] = targets.to_vec();
        w.sensors.team_contact_status.insert(targets[0], {
            let mut s = [0; crate::ids::MAX_TEAMS];
            s[0] = 1;
            s
        });
        (w, h, targets)
    }

    /// `SensorSystem::disable` = `clearContacts` (contact.cpp:162, 455): every private contact goes (`removeContact(0)` repeatedly,
    /// swap-filling slot 0), and each leaves the team list unless another sensor of the team still holds it; `broken` marks a
    /// destroyed vehicle's sensor (gvehicl.cpp:562).
    #[test]
    fn sensor_disable_clears_private_contacts_and_keeps_contacts_other_sensors_hold() {
        let (mut w, h, targets) = sensor_world();
        fx::sensor_disable(&mut w, h, true);
        assert!(w.sensors.pool[0].contacts.is_empty());
        assert!(w.sensors.pool[0].broken);
        // sensor 1 of the same team still sees targets[1]; the team list keeps only it
        assert_eq!(w.sensors.team_contact_order[0], vec![targets[1]]);
        assert_eq!(w.sensors.pool[1].contacts.len(), 1);
        // the aggregate status is rebuilt from the remaining sensor
        assert_eq!(
            w.sensors
                .team_contact_status
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![targets[1]]
        );
        // a mech's `disable` alone does not break the sensor
        let (mut w, h, _) = sensor_world();
        fx::sensor_disable(&mut w, h, false);
        assert!(!w.sensors.pool[0].broken);
        assert!(w.sensors.pool[0].contacts.is_empty());
    }

    /// A shut-down sensor's `removeContact` skips the master (contact.cpp:440): the team list is left alone.
    #[test]
    fn disabling_a_shut_down_sensor_does_not_touch_the_team_list() {
        let (mut w, h, targets) = sensor_world();
        w.sensors.pool[0].not_shutdown = false;
        fx::sensor_disable(&mut w, h, false);
        assert!(w.sensors.pool[0].contacts.is_empty());
        assert_eq!(w.sensors.team_contact_order[0], targets.to_vec());
    }

    /// `brainAlarmCallback[alarm]` is NULL for a pilot without a brain (warrior.cpp:4675): nothing runs, nothing is reported.
    #[test]
    fn a_pilot_without_a_brain_handles_an_alarm_silently() {
        let (mut w, h) = world();
        fx::handle_alarm(&mut w, h, 10, 1);
        assert!(w.events.is_empty());
        assert!(w.pilots.slots[1].pending_calls.is_empty());
    }

    /// `pilotAlarmFunctionName` (warrior.cpp:151-171) has one name per alarm code.
    #[test]
    fn alarm_function_names_cover_every_alarm_code() {
        assert_eq!(PILOT_ALARM_FUNCTION_NAMES.len(), 19);
        assert_eq!(
            PILOT_ALARM_FUNCTION_NAMES[PILOT_ALARM_NO_MOVEPATH as usize],
            "handlenomovepath"
        );
        assert_eq!(PILOT_ALARM_FUNCTION_NAMES[18], "handlenewmover");
    }
}
