// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MechWarrior` data: the pilot pool and per-pilot state. Slot numbers matter: `brainUpdate = (slot % 30) * 0.2`,
//! `combatUpdate = (slot % 15) * 0.1`, `movementUpdate = (slot % 15) * 0.2` initial staggers, and `clear()` does not reset
//! them for recycled slots. The pool has 120 slots; slot 0 is never handed out (`newWarrior`).

use crate::command::{Tactic, TacticalOrder};
use crate::ids::WatchId;
use crate::math::Vec3;

/// Pilot alarm codes (`PILOT_ALARM_*`, 19 codes). ABL handlers are called in ascending code order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alarm(pub u8);

/// Number of alarm codes.
pub const NUM_ALARMS: usize = 19;
/// Triggers kept per alarm before new ones are silently dropped.
pub const MAX_ALARM_TRIGGERS: usize = 15;
/// Pilot pool size (`MAX_WARRIORS`).
pub const MAX_WARRIORS: usize = 120;

/// The single-player queue's lossy `TacticalOrder::pack` payload plus the point and tactic sidecars kept by the UI marker.
#[derive(Clone, Debug, Default)]
pub struct QueuedOrder {
    /// The two packed 32-bit fields (`TacticalOrder::data[2]`).
    pub data: [u32; 2],
    /// Point retained by `QueuedTacOrder` after cell quantization.
    pub point: Vec3,
    /// Tactic retained separately because it is not represented in the packed fields.
    pub tactic: Tactic,
}

crate::snapshot_struct!(QueuedOrder {
    data,
    point,
    tactic
});

crate::snapshot_struct!(AttackOrders {
    time,
    origin,
    attack_type,
    target,
    target_point,
    aim_location,
    pursue,
    target_time,
});

/// `AttackOrders` (warrior.h): what the pilot is currently shooting at. Nothing reads it until the combat decision tree is ported; it
/// exists so `clearAttackOrders` / `AttackOrders::init` (run when orders are cleared) have their C++ target.
#[derive(Clone, Debug, PartialEq)]
pub struct AttackOrders {
    /// `time`.
    pub time: f32,
    /// `origin`.
    pub origin: crate::command::OrderOrigin,
    /// `type` (`ATTACK_NONE` = 0).
    pub attack_type: i32,
    /// `targetWID`.
    pub target: WatchId,
    /// `targetPoint`.
    pub target_point: Vec3,
    /// `aimLocation`.
    pub aim_location: i32,
    /// `pursue`.
    pub pursue: bool,
    /// `targetTime`.
    pub target_time: f32,
}

impl AttackOrders {
    /// `AttackOrders::init()` (warrior.cpp:1273-1284).
    pub fn init(scenario_time: f32) -> AttackOrders {
        AttackOrders {
            time: scenario_time,
            origin: crate::command::OrderOrigin::Commander,
            attack_type: 0,
            target: WatchId(0),
            target_point: Vec3::default(),
            aim_location: -1,
            pursue: false,
            target_time: 0.0,
        }
    }

    /// `MechWarrior::clearAttackOrders()` (warrior.cpp:2406-2414): unlike `init` it leaves `time` and `targetPoint` alone and sets
    /// `targetTime` to -1.
    pub fn clear(&mut self) {
        self.origin = crate::command::OrderOrigin::Commander;
        self.attack_type = 0;
        self.target = WatchId(0);
        self.aim_location = -1;
        self.pursue = false;
        self.target_time = -1.0;
    }
}

impl Default for AttackOrders {
    fn default() -> Self {
        AttackOrders::init(0.0)
    }
}

/// Which of the three order slots is active (`orderState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OrderState {
    /// General (commander/brain) order.
    #[default]
    General,
    /// Player order.
    Player,
    /// Alarm order.
    Alarm,
}

/// One pilot.
#[derive(Clone, Debug, Default)]
pub struct Pilot {
    /// Pool slot (0 unused).
    pub slot: u16,
    /// Mover this pilot flies.
    pub mover: WatchId,
    /// `wounds` (0..6), dead at 6.
    pub wounds: f32,
    /// `skills[3]` (gunnery, piloting, ...) and accumulated skill points.
    pub skills: [i32; 4],
    /// `specialtySkills[23]`.
    pub specialty: [bool; 23],
    /// `escapesThruEjection` (a C++ `bool`: `roll <= piloting + 30 && roll < 95` from one `RandomNumber(100)` at pilot load,
    /// slot order; warrior.cpp:1203-1206).
    pub escapes_thru_ejection: bool,
    /// `attackRadius`.
    pub attack_radius: f32,
    /// `attackOrders` (`AttackOrders::init`, warrior.cpp:1273).
    pub attack_orders: AttackOrders,
    /// Bit set of [`crate::ai::AiUnavailable`] kinds already reported through [`crate::SimEvent::AiUnavailable`] for this pilot, so a
    /// per-frame unported branch is reported once instead of every frame.
    pub unavailable_reported: Vec<crate::ai::AiUnavailable>,
    /// Three order slots + current + last (`tacOrder[3]`, `curTacOrder`, `lastTacOrder`).
    pub tac_orders: [TacticalOrder; 3],
    /// `curTacOrder`.
    pub cur_order: TacticalOrder,
    /// `lastTacOrder`.
    pub last_order: TacticalOrder,
    /// `orderState`.
    pub order_state: OrderState,
    /// `newTacOrderReceived[GENERAL, PLAYER, ALARM]`.
    pub new_order_received: [bool; 3],
    /// `alarmPriority`.
    pub alarm_priority: i32,
    /// `curPlayerOrderFromQueue`.
    pub player_order_from_queue: bool,
    /// `curPlayerOrderFromQueue` for the currently executing order.
    pub cur_player_order_from_queue: bool,
    /// Order queue (<= 16 per pilot, shared pool of 2000): stores LOSSY packed orders (see `TacticalOrder::pack/unpack`).
    pub queue: Vec<QueuedOrder>,
    /// `tacOrderQueueExecuting` and `tacOrderQueueIndex`.
    pub queue_executing: bool,
    /// The Rust queue is shifted on pop, so its active loop index is always zero.
    pub queue_index: usize,
    /// `brainUpdate`, `combatUpdate`, `movementUpdate` next-run times (scenario seconds).
    pub timers: [f32; 3],
    /// ABL brain instance (module state), created by `mission::init`.
    pub brain: Option<mc2_abl::InstId>,
    /// `memory[60]` cells.
    pub memory: Vec<i32>,
    /// `alarm[19]` trigger lists and `alarmHistory`.
    pub alarms: [Vec<i32>; NUM_ALARMS],
    /// Historical trigger lists read by the ABL event-history natives. Unlike `alarms`, these are not consumed by alarm handling.
    pub alarm_history: [Vec<i32>; NUM_ALARMS],
    /// `lastTarget` (watch id, time).
    pub last_target: (WatchId, f32),
    /// `challenger` returned by `getChallenger`; cleared when the challenger is no longer valid.
    pub challenger: WatchId,
    /// `keepMoving`, `willHelp`, `useGoalPlan`.
    pub keep_moving: bool,
    // ---- appended by lane E (mission objectives and the campaign outcome read these) ----
    /// `status` (`WARRIOR_STATUS_*`, warrior.h:265), typed. Lane C's damage code (`combat/fx.rs`, `warrior_status` consts) writes it
    /// through the same type.
    pub status: crate::mission::outcome::WarriorStatus,
    /// CROSS-LANE (lane C -> D): calls into lane D's code that lane C's damage code made, in call order; `ai::calls::run_pending_calls`
    /// runs and drains them right after each is queued, so the queue is empty between calls.
    pub pending_calls: Vec<PilotCall>,
    /// `moveOrders` (the field keeps lane B's name; it is lane D's full [`MoveOrders`](super::moveorders::MoveOrders)).
    pub lane_b_move: LaneBMoveStandin,
    /// `weaponsStatus[MAX_WEAPONS_PER_MOVER]` (`WEAPON_STATUS_*` per weapon or the chance percent).
    pub weapons_status: Vec<i32>,
    /// `weaponsStatusResult` (`WEAPONS_STATUS_*` or the number of weapons with a shot).
    pub weapons_status_result: i32,
    /// `useGoalPlan`.
    pub use_goal_plan: bool,
    /// `mainGoalAction`, `mainGoalObjectWID`, `mainGoalLocation`, `mainGoalControlRadius`.
    pub main_goal: MainGoal,
    /// `timeOfLastOrders` (when the pilot started to be without orders; -1 when he has some).
    pub time_of_last_orders: f32,
    /// `attackers[numAttackers]` (at most 50).
    pub attackers: Vec<(WatchId, f32)>,
    /// `brainState`.
    pub brain_state: i32,
    /// `willHelp`.
    pub will_help: bool,
    /// `coreScanTargetWID`.
    pub core_scan_target: WatchId,
    /// `targetPriorityList`.
    pub target_priorities: TargetPriorityList,
    /// `situationOrders` (only the fields something reads).
    pub situation: SituationOrders,
    /// `lastTargetObliterate`, `lastTargetFriendly`, `lastTargetConserveAmmo`.
    pub last_target_flags: [bool; 3],
    /// `nextTacOrderId`, `lastTacOrderId`.
    pub tac_order_ids: [i32; 2],
    /// `tacOrderQueueLooping`.
    pub queue_looping: bool,
    /// `numMechKills[NUM_VEHICLE_CLASSES][COMBAT_STAT_CAREER, COMBAT_STAT_MISSION]` (mechclass.h: none, light, medium, heavy, assault,
    /// ground vehicle, elemental).
    pub num_mech_kills: [[i32; 2]; 7],
    /// `killed[MAX_MOVERS / 3]` (watch ids of the movers this pilot destroyed this mission, at most 85).
    pub killed: Vec<WatchId>,
    /// Port bookkeeping, not C++ state: the number of ABL statements the last brain run or alarm handler of this pilot executed
    /// (`ABLModule::execute`'s return value), so a test can tell that a handler really ran.
    pub brain_statements: u32,
    /// `lastGoalPathSize` / `lastGoalPath[]`: the `thruArea` of the steps of the last global path the goal plan computed.
    pub last_goal_path: Vec<i16>,
    /// `brainAlarmCallback[alarm]` is non-NULL: the brain defines `pilotAlarmFunctionName[alarm]` (resolved by `setBrain`).
    pub brain_alarm_handlers: [bool; NUM_ALARMS],
}

/// `GoalActionType` (warrior.h:633).
pub mod goal_action {
    /// `GOAL_ACTION_NONE`.
    pub const NONE: i32 = 0;
    /// `GOAL_ACTION_UNDECIDED`.
    pub const UNDECIDED: i32 = 1;
    /// `GOAL_ACTION_MOVE`.
    pub const MOVE: i32 = 2;
    /// `GOAL_ACTION_ATTACK`.
    pub const ATTACK: i32 = 3;
    /// `GOAL_ACTION_CAPTURE`.
    pub const CAPTURE: i32 = 4;
    /// `GOAL_ACTION_GUARD`.
    pub const GUARD: i32 = 5;
}

/// The main goal of the goal plan (`MechWarrior::setMainGoal`, warrior.cpp:7479).
#[derive(Clone, Debug, PartialEq)]
pub struct MainGoal {
    /// `mainGoalAction`.
    pub action: i32,
    /// `mainGoalObjectWID`.
    pub object: WatchId,
    /// `mainGoalLocation`.
    pub location: Vec3,
    /// `mainGoalControlRadius`.
    pub control_radius: f32,
}

impl Default for MainGoal {
    /// `MechWarrior::init(bool)` (warrior.cpp:924-929).
    fn default() -> Self {
        MainGoal {
            action: goal_action::NONE,
            object: WatchId(0),
            location: Vec3::default(),
            control_radius: -1.0,
        }
    }
}

/// `SituationOrders` (warrior.h:555): `SituationOrders::init()` (warrior.cpp:1286-1297).
#[derive(Clone, Debug, PartialEq)]
pub struct SituationOrders {
    /// `openFire`.
    pub open_fire: bool,
    /// `fireRange` (set by `setSituationFireRange`).
    pub fire_range: f32,
    /// `guardObjectWID`.
    pub guard_object: WatchId,
}

impl Default for SituationOrders {
    fn default() -> Self {
        SituationOrders {
            open_fire: false,
            fire_range: -1.0,
            guard_object: WatchId(0),
        }
    }
}

/// `MAX_TARGET_PRIORITIES` (warrior.h:352).
pub const MAX_TARGET_PRIORITIES: usize = 20;

/// `TargetPriority` (warrior.h:397).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetPriority {
    /// `type` (`TARGET_PRIORITY_*`).
    pub kind: i32,
    /// `params[3]`.
    pub params: [i32; 3],
}

/// `TargetPriorityList` (warrior.h:403): `size` entries of the fixed list of 20.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetPriorityList {
    /// `size`.
    pub size: i32,
    /// `list[MAX_TARGET_PRIORITIES]`.
    pub list: [TargetPriority; MAX_TARGET_PRIORITIES],
}

impl TargetPriorityList {
    /// `TargetPriorityList::insert` (warrior.cpp:419-437): 0, or 1 when full / 2 when `index` is past the end. `index` -1 appends.
    pub fn insert(&mut self, index: i32, priority: TargetPriority) -> i32 {
        if self.size as usize == MAX_TARGET_PRIORITIES {
            return 1;
        }
        let index = if index == -1 { self.size } else { index };
        if index > self.size {
            return 2;
        }
        let (index, size) = (index as usize, self.size as usize);
        if index < size {
            self.list.copy_within(index..size, index + 1);
        }
        self.list[index] = priority;
        self.size += 1;
        0
    }

    /// `TargetPriorityList::remove` (warrior.cpp:440-451).
    pub fn remove(&mut self, index: i32) {
        if index > -1 && index < self.size {
            let (index, size) = (index as usize, self.size as usize);
            if index < size - 1 {
                self.list.copy_within(index + 1..size, index);
            }
            self.size -= 1;
        }
    }
}

/// Lane B's historical name for [`MoveOrders`](super::moveorders::MoveOrders), the full `MoveOrders` of the pilot (every C++
/// field; the ones lane B reads keep their names).
pub type LaneBMoveStandin = super::moveorders::MoveOrders;

/// A call into lane D that lane C's damage code makes on a mover's pilot (see `combat::fx`). Recorded in call order in
/// [`Pilot::pending_calls`] and run at once by [`crate::ai::calls::run_pending_calls`]. The state changes `handleAlarm` makes on the pilot and
/// the mover (ejection, withdrawal) are applied by lane C; what is left to lane D is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotCall {
    /// `handleAlarm`'s last step (warrior.cpp:4675): the brain's ABL alarm handler `brainAlarmCallback[alarm]`.
    BrainAlarm {
        /// `PILOT_ALARM_*`.
        alarm: u8,
        /// Trigger id.
        trigger_id: u32,
    },
    /// The order clean-up of `handleOwnVehicleIncapacitation` (warrior.cpp:6957): `clearCurTacOrder(false)`, general order state,
    /// move orders and path manager entry cleared, attack orders re-initialised, `setLastTarget(NULL)`.
    ClearOrders,
    /// `MechWarrior::updateAttackerStatus(attackerWid, scenarioTime)` on the target's pilot, made by `fireWeapon` (warrior.cpp).
    UpdateAttackerStatus {
        /// The attacker's watch id.
        attacker: crate::ids::WatchId,
        /// `scenarioTime` as the bits of the `f32`.
        time_bits: u32,
    },
    /// `sensorSystem->disable()` (`clearContacts`); `broken` for a ground vehicle (`sensorSystem->broken = true`).
    SensorDisable {
        /// Vehicle flavour.
        broken: bool,
    },
}

/// The pool (`MechWarrior::warriorList`).
#[derive(Clone, Debug, Default)]
pub struct PilotPool {
    /// 120 slots.
    pub slots: Vec<Pilot>,
    /// `brainsEnabled[team]`.
    pub brains_enabled: [bool; crate::ids::MAX_TEAMS],
    /// `MechWarrior::curEventID` / `curEventTrigger` (static, warrior.cpp:285): the alarm the brain's `getNextPilotEvent` is at.
    pub cur_event_id: i32,
    /// See `cur_event_id`.
    pub cur_event_trigger: i32,
    /// `MechWarrior::numWarriorsInCombat` (warrior.cpp:279): +1 in `setCurTacOrder` of a combat order, -1 in `clearCurTacOrder` of one.
    pub num_warriors_in_combat: i32,
}

impl PilotPool {
    /// Pilot in `slot` (0 is never handed out).
    pub fn get_mut(&mut self, slot: u16) -> Option<&mut Pilot> {
        self.slots.get_mut(slot as usize)
    }
    /// Shared access.
    pub fn get(&self, slot: u16) -> Option<&Pilot> {
        self.slots.get(slot as usize)
    }
}

/// `MAX_WEAPONS_PER_MOVER` (dmover.h:16).
pub const MAX_WEAPONS_PER_MOVER: usize = 32;
/// `WEAPONS_STATUS_CANNOT_FIRE` (dwarrior.h:17).
pub const WEAPONS_STATUS_CANNOT_FIRE: i32 = -1;
/// `WEAPONS_STATUS_NO_TARGET`.
pub const WEAPONS_STATUS_NO_TARGET: i32 = -2;
/// `WEAPONS_STATUS_OUT_OF_RANGE`.
pub const WEAPONS_STATUS_OUT_OF_RANGE: i32 = -3;
/// `WEAPON_STATUS_NOT_READY` (dwarrior.h:21).
pub const WEAPON_STATUS_NOT_READY: i32 = -1;
/// `WEAPON_STATUS_OUT_OF_AMMO`.
pub const WEAPON_STATUS_OUT_OF_AMMO: i32 = -2;
/// `WEAPON_STATUS_OUT_OF_RANGE`.
pub const WEAPON_STATUS_OUT_OF_RANGE: i32 = -3;
/// `WEAPON_STATUS_NOT_LOCKED`.
pub const WEAPON_STATUS_NOT_LOCKED: i32 = -4;
/// `WEAPON_STATUS_NO_CHANCE`.
pub const WEAPON_STATUS_NO_CHANCE: i32 = -5;
/// `WEAPON_STATUS_TOO_HOT`.
pub const WEAPON_STATUS_TOO_HOT: i32 = -6;

impl Pilot {
    /// A pilot as `MechWarrior::init(bool create)` leaves it (warrior.cpp:844-1017): pool `slot`, the three update timers staggered
    /// by the pool index, three initialised order slots, no alarms, `weaponsStatusResult` = no target, `willHelp`, no orders
    /// pending, `lastTargetTime` -1, initialised move orders (two zero filled paths).
    pub fn fresh(slot: u16, attack_radius: f32, timers: [f32; 3]) -> Pilot {
        Pilot {
            slot,
            attack_radius,
            timers,
            memory: vec![0; 60],
            tac_orders: std::array::from_fn(|_| TacticalOrder::init()),
            cur_order: TacticalOrder::init(),
            last_order: TacticalOrder::init(),
            weapons_status: vec![0; MAX_WEAPONS_PER_MOVER],
            weapons_status_result: WEAPONS_STATUS_NO_TARGET,
            will_help: true,
            time_of_last_orders: -1.0,
            last_target: (WatchId(0), -1.0),
            tac_order_ids: [0, -1],
            ..Pilot::default()
        }
    }
}

// ---- save/load (lane D) ----
// Every field of `Pilot` is dynamic: nothing here is static. `Pilot` is manual (not `snapshot_struct!`) because `status` (lane E's
// `WarriorStatus`) and `brain` (an `mc2_abl::InstId`) are encoded by the helpers below, so no trait impl is added for types that
// other lanes own. Save order = struct field order = load literal order.

use crate::command::snapshot_enum;
use crate::mission::outcome::WarriorStatus;
use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};

snapshot_enum!(OrderState {
    General = 0,
    Player = 1,
    Alarm = 2,
});

/// A pending call is saved as a tag (0 `BrainAlarm`, 1 `ClearOrders`, 2 `SensorDisable`, 3 `UpdateAttackerStatus`) and its fields, in call order with the list.
impl Snapshot for PilotCall {
    fn save(&self, w: &mut Writer) {
        match *self {
            PilotCall::BrainAlarm { alarm, trigger_id } => {
                w.u8(0);
                alarm.save(w);
                trigger_id.save(w);
            }
            PilotCall::ClearOrders => w.u8(1),
            PilotCall::SensorDisable { broken } => {
                w.u8(2);
                broken.save(w);
            }
            PilotCall::UpdateAttackerStatus {
                attacker,
                time_bits,
            } => {
                w.u8(3);
                attacker.save(w);
                time_bits.save(w);
            }
        }
    }

    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(match r.u8()? {
            0 => PilotCall::BrainAlarm {
                alarm: Snapshot::load(r)?,
                trigger_id: Snapshot::load(r)?,
            },
            1 => PilotCall::ClearOrders,
            2 => PilotCall::SensorDisable {
                broken: Snapshot::load(r)?,
            },
            3 => PilotCall::UpdateAttackerStatus {
                attacker: Snapshot::load(r)?,
                time_bits: Snapshot::load(r)?,
            },
            other => return Err(SaveError(format!("save: bad PilotCall tag {other}"))),
        })
    }
}

/// `WarriorStatus` tag (outcome.rs is lane E's file).
fn save_status(s: WarriorStatus, w: &mut Writer) {
    w.u8(match s {
        WarriorStatus::Normal => 0,
        WarriorStatus::Withdrawing => 1,
        WarriorStatus::Withdrawn => 2,
        WarriorStatus::Ejected => 3,
        WarriorStatus::Dead => 4,
        WarriorStatus::Mia => 5,
        WarriorStatus::Captured => 6,
        WarriorStatus::Basecamp => 7,
    });
}

fn load_status(r: &mut Reader<'_>) -> SaveResult<WarriorStatus> {
    Ok(match r.u8()? {
        0 => WarriorStatus::Normal,
        1 => WarriorStatus::Withdrawing,
        2 => WarriorStatus::Withdrawn,
        3 => WarriorStatus::Ejected,
        4 => WarriorStatus::Dead,
        5 => WarriorStatus::Mia,
        6 => WarriorStatus::Captured,
        7 => WarriorStatus::Basecamp,
        other => return Err(SaveError(format!("save: bad WarriorStatus tag {other}"))),
    })
}

/// `brain`: the ABL instance id. The instance itself is saved by the ABL environment (lane E), so only the id is written.
fn save_brain(b: Option<mc2_abl::InstId>, w: &mut Writer) {
    b.map(|i| i.0).save(w);
}

fn load_brain(r: &mut Reader<'_>) -> SaveResult<Option<mc2_abl::InstId>> {
    Ok(Option::<u32>::load(r)?.map(mc2_abl::InstId))
}

impl Snapshot for Pilot {
    fn save(&self, w: &mut Writer) {
        self.slot.save(w);
        self.mover.save(w);
        self.wounds.save(w);
        self.skills.save(w);
        self.specialty.save(w);
        self.escapes_thru_ejection.save(w);
        self.attack_radius.save(w);
        self.attack_orders.save(w);
        self.tac_orders.save(w);
        self.cur_order.save(w);
        self.last_order.save(w);
        self.order_state.save(w);
        self.queue.save(w);
        self.queue_executing.save(w);
        self.queue_index.save(w);
        self.timers.save(w);
        save_brain(self.brain, w);
        self.memory.save(w);
        self.alarms.save(w);
        self.last_target.save(w);
        self.keep_moving.save(w);
        save_status(self.status, w);
        self.new_order_received.save(w);
        self.alarm_priority.save(w);
        self.player_order_from_queue.save(w);
        self.cur_player_order_from_queue.save(w);
        self.alarm_history.save(w);
        self.challenger.save(w);
        self.pending_calls.save(w);
        self.lane_b_move.save(w);
        self.weapons_status.save(w);
        self.weapons_status_result.save(w);
        self.use_goal_plan.save(w);
        self.main_goal.save(w);
        self.time_of_last_orders.save(w);
        self.attackers.save(w);
        self.brain_state.save(w);
        self.will_help.save(w);
        self.core_scan_target.save(w);
        self.target_priorities.save(w);
        self.situation.save(w);
        self.last_target_flags.save(w);
        self.tac_order_ids.save(w);
        self.queue_looping.save(w);
        self.num_mech_kills.save(w);
        self.killed.save(w);
        self.brain_statements.save(w);
        self.last_goal_path.save(w);
        self.brain_alarm_handlers.save(w);
    }

    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(Pilot {
            slot: Snapshot::load(r)?,
            mover: Snapshot::load(r)?,
            wounds: Snapshot::load(r)?,
            skills: Snapshot::load(r)?,
            specialty: Snapshot::load(r)?,
            escapes_thru_ejection: Snapshot::load(r)?,
            attack_radius: Snapshot::load(r)?,
            attack_orders: Snapshot::load(r)?,
            unavailable_reported: Vec::new(),
            tac_orders: Snapshot::load(r)?,
            cur_order: Snapshot::load(r)?,
            last_order: Snapshot::load(r)?,
            order_state: Snapshot::load(r)?,
            queue: Snapshot::load(r)?,
            queue_executing: Snapshot::load(r)?,
            queue_index: Snapshot::load(r)?,
            timers: Snapshot::load(r)?,
            brain: load_brain(r)?,
            memory: Snapshot::load(r)?,
            alarms: Snapshot::load(r)?,
            last_target: Snapshot::load(r)?,
            keep_moving: Snapshot::load(r)?,
            status: load_status(r)?,
            new_order_received: Snapshot::load(r)?,
            alarm_priority: Snapshot::load(r)?,
            player_order_from_queue: Snapshot::load(r)?,
            cur_player_order_from_queue: Snapshot::load(r)?,
            alarm_history: Snapshot::load(r)?,
            challenger: Snapshot::load(r)?,
            pending_calls: Snapshot::load(r)?,
            lane_b_move: Snapshot::load(r)?,
            weapons_status: Snapshot::load(r)?,
            weapons_status_result: Snapshot::load(r)?,
            use_goal_plan: Snapshot::load(r)?,
            main_goal: Snapshot::load(r)?,
            time_of_last_orders: Snapshot::load(r)?,
            attackers: Snapshot::load(r)?,
            brain_state: Snapshot::load(r)?,
            will_help: Snapshot::load(r)?,
            core_scan_target: Snapshot::load(r)?,
            target_priorities: Snapshot::load(r)?,
            situation: Snapshot::load(r)?,
            last_target_flags: Snapshot::load(r)?,
            tac_order_ids: Snapshot::load(r)?,
            queue_looping: Snapshot::load(r)?,
            num_mech_kills: Snapshot::load(r)?,
            killed: Snapshot::load(r)?,
            brain_statements: Snapshot::load(r)?,
            last_goal_path: Snapshot::load(r)?,
            brain_alarm_handlers: Snapshot::load(r)?,
        })
    }
}

crate::snapshot_struct!(MainGoal {
    action,
    object,
    location,
    control_radius,
});

crate::snapshot_struct!(SituationOrders {
    open_fire,
    fire_range,
    guard_object,
});

crate::snapshot_struct!(TargetPriority { kind, params });

crate::snapshot_struct!(TargetPriorityList { size, list });

crate::snapshot_struct!(super::moveorders::GlobalStep {
    thru_area,
    goal_door,
    cost_to_goal,
    goal_cell,
});

crate::snapshot_struct!(super::moveorders::MoveOrders {
    time,
    origin,
    speed_type,
    speed_velocity,
    speed_state,
    speed_throttle,
    goal_type,
    goal_object_wid,
    goal_object_position,
    goal,
    has_goal,
    next_update,
    new_goal,
    way_path,
    cur_way_pt,
    cur_way_dir,
    path_type,
    original_global_goal,
    global_goal_location,
    global_path,
    num_global_steps,
    cur_global_step,
    paths,
    time_of_last_step,
    move_state,
    move_state_goal,
    twisting,
    yield_time,
    yield_state,
    wait_for_point_time,
    run,
    is_yielding,
});

crate::snapshot_struct!(PilotPool {
    slots,
    brains_enabled,
    cur_event_id,
    cur_event_trigger,
    num_warriors_in_combat,
});

#[cfg(test)]
mod save_tests {
    use super::*;
    use crate::command::OrderCode;
    use crate::save::{Reader, Writer};

    #[test]
    fn pilot_pool_round_trips_bit_exact() {
        let mut pool = PilotPool::default();
        for slot in [3u16, 4] {
            let mut p = Pilot {
                slot,
                mover: WatchId(u32::from(slot) * 10),
                wounds: 1.5,
                timers: [0.2, 1.0 + f32::from(slot), f32::from_bits(0x7fc0_0001)],
                memory: vec![0; 60],
                order_state: OrderState::Alarm,
                last_target: (WatchId(9), 3.25),
                brain: Some(mc2_abl::InstId(slot as u32)),
                status: WarriorStatus::Ejected,
                ..Pilot::default()
            };
            p.queue.push(QueuedOrder {
                data: [i32::from(slot) as u32, OrderCode::MoveToPoint as u32],
                point: Vec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                tactic: Tactic::FlankLeft,
            });
            p.queue.push(QueuedOrder {
                data: [0, OrderCode::Guard as u32],
                point: Vec3 {
                    x: -4.0,
                    y: 5.0,
                    z: 6.0,
                },
                tactic: Tactic::StopAndFire,
            });
            p.alarms[2].push(11);
            p.alarms[18] = vec![-1, 4];
            pool.slots.push(p);
        }
        pool.brains_enabled[1] = true;

        let mut w = Writer::new();
        pool.save(&mut w);
        let bytes = w.into_bytes();

        let mut fresh = PilotPool::default();
        fresh.load_into(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(fresh.slots.len(), 2);
        assert_eq!(fresh.slots[1].queue.len(), 2);
        assert_eq!(fresh.slots[1].alarms[18], vec![-1, 4]);
        assert_eq!(fresh.slots[0].status, WarriorStatus::Ejected);

        let mut w2 = Writer::new();
        fresh.save(&mut w2);
        assert_eq!(w2.into_bytes(), bytes);
    }
}
