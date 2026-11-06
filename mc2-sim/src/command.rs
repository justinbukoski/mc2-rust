// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Player commands: the event/command interface by which the UI (or a test script, or a replay) orders units.
//!
//! In the original the GUI (`MissionInterfaceManager`, `code/missiongui.cpp`) fills a `TacticalOrder`
//! (`code/tacordr.h`) and calls `Mover::handleTacticalOrder` for each selected mover (`handleOrders`, missiongui.cpp
//! ~2470; `doMove` ~3140; `forceShot` ~3210; `doJump`, `doGuard`, `doCapture`, `doRepair`, `doEject`...). The Rust sim has
//! no GUI: the application translates input into [`PlayerCommand`]s and passes them to [`Sim::step`](crate::Sim::step);
//! lane E applies them at the point where `missionInterface->update()` ran in `Mission::update` (before the path manager and
//! the object update), through lane D's order handlers.
//!
//! [`TacticalOrder`] mirrors the C++ struct field for field (network-only fields `pointLocalMoverId`, `groupFlags`,
//! `data[]` are dropped: single player only).

use crate::ids::{PartId, WatchId};
use crate::math::Vec3;

/// `TacticalOrderCode`. Numeric values match the C++ enum (they appear in the digest as `o<code>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(i32)]
#[allow(missing_docs)]
pub enum OrderCode {
    #[default]
    None = 0,
    Wait,
    MoveToPoint,
    MoveToObject,
    JumpToPoint,
    JumpToObject,
    TraversePath,
    PatrolPath,
    Escort,
    Follow,
    Guard,
    Stop,
    PowerUp,
    PowerDown,
    WaypointsDone,
    Eject,
    AttackObject,
    AttackPoint,
    HoldFire,
    Withdraw,
    Scramble,
    Capture,
    Refit,
    GetFixed,
    LoadIntoCarrier,
    DeployElementals,
    Recover,
}

/// `OrderOriginType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(missing_docs)]
pub enum OrderOrigin {
    #[default]
    Player,
    Commander,
    SelfOrder,
}

/// `TravelModeType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(missing_docs)]
pub enum TravelMode {
    #[default]
    Slow,
    Fast,
    Jump,
}

/// `AttackMethod`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(missing_docs)]
pub enum AttackMethod {
    #[default]
    Ranged,
    Dfa,
    Ramming,
}

/// `TacticType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(missing_docs)]
pub enum Tactic {
    #[default]
    None,
    FlankLeft,
    FlankRight,
    FlankRear,
    StopAndFire,
    Turret,
    Joust,
}

/// `FireRangeType` values used by `TacOrderAttackParams::range` (negative values are special ranges, non-negative
/// values are a distance in cells... see `code/tacordr.h`).
pub type FireRange = i32;
/// `FIRERANGE_DEFAULT`.
pub const FIRERANGE_DEFAULT: FireRange = -4;
/// `FIRERANGE_RAMMING`.
pub const FIRERANGE_RAMMING: FireRange = -3;
/// `FIRERANGE_LONGEST`.
pub const FIRERANGE_LONGEST: FireRange = -2;
/// `FIRERANGE_OPTIMAL` (enum order in tacordr.h:76-86: DEFAULT -4, RAMMING, LONGEST, OPTIMAL = -1, SHORT 0, MEDIUM, LONG, CURRENT).
pub const FIRERANGE_OPTIMAL: FireRange = -1;
/// `FIRERANGE_SHORT`.
pub const FIRERANGE_SHORT: FireRange = 0;
/// `FIRERANGE_MEDIUM`.
pub const FIRERANGE_MEDIUM: FireRange = 1;
/// `FIRERANGE_LONG`.
pub const FIRERANGE_LONG: FireRange = 2;
/// `FIRERANGE_CURRENT`.
pub const FIRERANGE_CURRENT: FireRange = 3;

/// Maximum waypoints in a way path (`MAX_WAYPTS`, dwarrior.h:14 = 15).
pub const MAX_WAYPTS: usize = 15;

/// `WayPath`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WayPath {
    /// Points (`x`, `y`, `z`) in world units.
    pub points: Vec<Vec3>,
    /// Travel mode per point.
    pub mode: Vec<TravelMode>,
    /// `curPoint`.
    pub cur_point: usize,
    /// `numPoints` (appended): the count the C++ keeps apart from the point array. `TacticalOrder::init` sets 0 and only `unpack`
    /// (1) and `initWayPath` / `addWayPoint` change it, so an order built by `orderMoveToPoint` (which writes `points[0..2]` only) has
    /// 0 here, and `equals` then compares no coordinates.
    pub num_points: usize,
}

/// `TacOrderMoveParams`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoveParams {
    /// Waypoints.
    pub way_path: WayPath,
    /// `faceObject`.
    pub face_object: bool,
    /// `wait`.
    pub wait: bool,
    /// `mode == MOVE_MODE_MINELAYING`.
    pub mine_laying: bool,
    /// `escapeTile`.
    pub escape_tile: bool,
    /// `jump`.
    pub jump: bool,
    /// `fromArea`.
    pub from_area: i32,
    /// `keepMoving`.
    pub keep_moving: bool,
}

/// `TacOrderAttackParams`.
#[derive(Clone, Debug, PartialEq)]
pub struct AttackParams {
    /// `method`.
    pub method: AttackMethod,
    /// `range` (`FireRangeType`).
    pub range: FireRange,
    /// `tactic`.
    pub tactic: Tactic,
    /// `aimLocation` (-1 = none; called shots use a body location).
    pub aim_location: i32,
    /// `pursue`.
    pub pursue: bool,
    /// `obliterate`.
    pub obliterate: bool,
    /// `targetPoint`.
    pub target_point: Vec3,
    /// `type` (`ATTACK_TO_DESTROY` by default; C++ comments say no longer used).
    pub attack_type: i32,
}

impl Default for AttackParams {
    fn default() -> Self {
        AttackParams {
            method: AttackMethod::Ranged,
            range: FIRERANGE_DEFAULT,
            tactic: Tactic::None,
            aim_location: -1,
            pursue: false,
            obliterate: false,
            target_point: Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            attack_type: 1,
        }
    }
}

/// `TacticalOrder` (code/tacordr.h).
#[derive(Clone, Debug, PartialEq)]
pub struct TacticalOrder {
    /// `id` (assigned when the pilot accepts the order).
    pub id: i32,
    /// `time`: scenario time the order was issued.
    pub time: f32,
    /// `delayedTime`: do not start before this scenario time (selection stagger).
    pub delayed_time: f32,
    /// `lastTime`.
    pub last_time: f32,
    /// `unitOrder`: applies to the whole selected group.
    pub unit_order: bool,
    /// `subOrder`.
    pub sub_order: bool,
    /// `origin`.
    pub origin: OrderOrigin,
    /// `code`.
    pub code: OrderCode,
    /// Move parameters.
    pub move_params: MoveParams,
    /// Attack parameters.
    pub attack_params: AttackParams,
    /// `targetWID`.
    pub target: WatchId,
    /// `targetObjectClass`.
    pub target_object_class: i32,
    /// `selectionIndex`.
    pub selection_index: i32,
    /// `stage`.
    pub stage: i8,
    /// `statusCode`.
    pub status_code: i8,
    /// `groupFlags` (single player order grouping bits; appended to preserve the shared scaffold's field order).
    pub group_flags: u16,
}

impl TacticalOrder {
    /// `TacticalOrder::init(origin, code, unitOrder)` (`tacordr.cpp:169-222`).
    pub fn new(origin: OrderOrigin, code: OrderCode, unit_order: bool, scenario_time: f32) -> Self {
        Self {
            time: scenario_time,
            delayed_time: -1.0,
            last_time: -1.0,
            unit_order,
            sub_order: false,
            origin,
            code,
            move_params: MoveParams {
                way_path: WayPath {
                    points: vec![Vec3::default(); 1],
                    mode: vec![TravelMode::Slow],
                    cur_point: 0,
                    num_points: 0,
                },
                face_object: true,
                wait: false,
                mine_laying: false,
                escape_tile: false,
                jump: false,
                from_area: -1,
                keep_moving: true,
            },
            attack_params: AttackParams {
                attack_type: 1,
                method: AttackMethod::Ranged,
                range: FIRERANGE_OPTIMAL,
                tactic: Tactic::None,
                aim_location: -1,
                pursue: true,
                obliterate: false,
                target_point: Vec3::default(),
            },
            id: 0,
            target: WatchId(0),
            target_object_class: 0,
            selection_index: -1,
            stage: 1,
            status_code: 0,
            group_flags: 0,
        }
    }

    /// `TacticalOrder::equals` (`tacordr.cpp:1919`): compares common identity, then only fields used by move-point,
    /// attack-object and attack-point orders. Order id and timestamps are intentionally ignored.
    pub fn equals(&self, other: &Self) -> bool {
        if self.code != other.code
            || self.target != other.target
            || self.stage != other.stage
            || self.unit_order != other.unit_order
            || self.origin != other.origin
            || self.group_flags != other.group_flags
        {
            return false;
        }
        match self.code {
            OrderCode::MoveToPoint => {
                // tacordr.cpp:1938-1955: the loop runs over `numPoints * 3` flat coordinates (and compares `mode[i]` for the same
                // indices, so the unused entries of the 15-entry mode array count as `TRAVEL_MODE_SLOW`); with `numPoints == 0`
                // (every order made by `orderMoveToPoint`) no coordinate is compared at all.
                self.move_params.face_object == other.move_params.face_object
                    && self.move_params.wait == other.move_params.wait
                    && self.move_params.mine_laying == other.move_params.mine_laying
                    && self.move_params.escape_tile == other.move_params.escape_tile
                    && self.move_params.way_path.num_points == other.move_params.way_path.num_points
                    && (0..self.move_params.way_path.num_points * 3).all(|i| {
                        let coord = |p: &WayPath| {
                            p.points.get(i / 3).map_or(0.0, |v| match i % 3 {
                                0 => v.x,
                                1 => v.y,
                                _ => v.z,
                            })
                        };
                        let mode = |p: &WayPath| p.mode.get(i).copied().unwrap_or(TravelMode::Slow);
                        coord(&self.move_params.way_path) == coord(&other.move_params.way_path)
                            && mode(&self.move_params.way_path) == mode(&other.move_params.way_path)
                    })
            }
            OrderCode::AttackObject => {
                self.attack_params.method == other.attack_params.method
                    && self.attack_params.range == other.attack_params.range
                    && self.attack_params.aim_location == other.attack_params.aim_location
                    && self.attack_params.pursue == other.attack_params.pursue
                    && self.attack_params.obliterate == other.attack_params.obliterate
                    && self.attack_params.tactic == other.attack_params.tactic
            }
            OrderCode::AttackPoint => {
                self.attack_params.method == other.attack_params.method
                    && self.attack_params.range == other.attack_params.range
                    && self.attack_params.aim_location == other.attack_params.aim_location
                    && self.attack_params.pursue == other.attack_params.pursue
                    && self.attack_params.obliterate == other.attack_params.obliterate
                    && self.attack_params.target_point == other.attack_params.target_point
                    && self.attack_params.tactic == other.attack_params.tactic
            }
            _ => true,
        }
    }
}

impl Default for TacticalOrder {
    fn default() -> Self {
        Self::new(OrderOrigin::Commander, OrderCode::None, false, -1.0)
    }
}

impl TacticalOrder {
    /// `TacticalOrder::isCombatOrder` (tacordr.cpp:306).
    pub fn is_combat_order(&self) -> bool {
        matches!(self.code, OrderCode::AttackObject | OrderCode::AttackPoint)
    }

    /// `TacticalOrder::init()` (tacordr.cpp:169-206): the state of the three order slots, `curTacOrder` and `lastTacOrder` of a freshly
    /// initialised pilot (`MechWarrior::init(bool)` / `clear()`, warrior.cpp:948-952, 1460-1464). Differs from `Default` in the
    /// times (-1), origin (commander), `faceObject`, `keepMoving`, `fromArea`, `pursue`, range (`FIRERANGE_OPTIMAL`), `selectionIndex`
    /// and `stage` (1: this is the `g1` of every frame-0 `M`/`V` digest line).
    pub fn init() -> TacticalOrder {
        TacticalOrder::new(OrderOrigin::Commander, OrderCode::None, false, -1.0)
    }
}

/// One input event from the player (or a script).
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerCommand {
    /// `ControlGui::setRange` applies the selected fire range to selected home movers
    /// (`code/controlgui.cpp:1404-1417`). `update_current_order` is true for the hold-position
    /// toggle, which also reissues each non-empty current tactical order (`:2949-2955`).
    SetAttackRange {
        /// Selected home movers in roster order.
        movers: Vec<PartId>,
        /// A `FireRangeType` (`FIRERANGE_*`).
        range: i32,
        /// Whether to update and reissue an existing current order as hold-position does.
        update_current_order: bool,
        /// `ControlGui::getFireFromCurrentPos()` for the hold-position current-order update.
        fire_from_current_position: bool,
    },
    /// `MissionInterfaceManager::makeForceGroup`: replace one Ctrl+digit group's membership.
    ///
    /// The app sends the current selection.  The simulation owns the per-mover state so a
    /// mission save restores the group, as `Mover::copyTo`/`copyFrom` do in
    /// `code/mover.cpp:7318,7478`.
    SetForceGroup {
        /// Digit group, 0 through 9.
        group: u8,
        /// Selected home-commander's movers that join the group.
        members: Vec<PartId>,
    },
    /// `ForceGroupIcon::drawDeathEffect`: a member's death animation finished, so it leaves
    /// its group (`code/mechicon.cpp:1493-1500`).
    ClearForceGroup {
        /// Mover that leaves its group.
        mover: PartId,
    },
    /// Order for a set of movers (identified by part id, like the oracle's order script). `queue` is the waypoint-key
    /// behaviour: append to the pilot's order queue instead of replacing the current order.
    Order {
        /// Movers that receive the order, in selection order (selection index staggers `delayed_time`).
        movers: Vec<PartId>,
        /// The order.
        order: TacticalOrder,
        /// Queue instead of replace.
        queue: bool,
    },
    /// `Mover::suppressionFire` set by the GUI: the guard tower button (`ControlGui::handleClick`, controlgui.cpp:1629-1640) and
    /// `MissionInterfaceManager::doGuardTower` (missiongui.cpp:5495).
    SetSuppressionFire {
        /// The movers whose flag is set.
        movers: Vec<PartId>,
        /// The new value.
        on: bool,
    },
    /// `MissionInterfaceManager::handleOrders` with the mine toggle on (missiongui.cpp:2518): the order is queued and the queue is
    /// then released (`addQueuedTacOrder`, `setExecutingQueue(TRUE)`), so it starts when the pilot has no current order.
    OrderQueueExecute {
        /// Movers that receive the order, in selection order.
        movers: Vec<PartId>,
        /// The order.
        order: TacticalOrder,
    },
    /// Call in a support strike (artillery/air strike/sensor probe) at a point (`IfaceCallStrike`).
    CallStrike {
        /// Strike type id.
        strike: i32,
        /// Target point.
        at: Vec3,
    },
    /// `MissionInterfaceManager::beginVtol` after the click chain's `canAddVehicle` / `canRecover` (missiongui.cpp:1324): fly in a support
    /// vehicle (repair truck 182, scout 393, mine layer 120, guard tower 415) or the recovery craft (147, over `target`).
    BeginVtol {
        /// `Commander::home->getId()`.
        commander: i32,
        /// `vehicleID`.
        vehicle_id: i32,
        /// The clicked point (`wPos`).
        at: Vec3,
        /// The clicked mech, for the recovery craft.
        target: Option<crate::ids::WatchId>,
    },
    /// `LogisticsData::addResourcePoints` / `decrementResourcePoints`: the control panel buys a vehicle (negative) or takes it back
    /// (controlgui.cpp:1971-2008).
    AddResourcePoints(i32),
    /// Pause or unpause (freezes `scenarioTime`, sensors, collisions and object updates in single player).
    SetPaused(bool),
    /// A debug/script order as the oracle's `--orders` file issues it: one order built exactly like `issueOrder`
    /// (`oracle/oracle_main.cpp`) and handed straight to `Mover::handleTacticalOrder` of that mover (no GUI selection, group goal
    /// assignment or `attackRange` stamping for a move). Appended by lane D for the populated oracle comparison.
    Scripted(ScriptedOrder),
    /// Debug/script input as the oracle's `--hit-script` option applies it at the mission order point.
    ScriptedHit {
        /// Mission part id of the object to hit.
        part: PartId,
        /// Damage passed to `WeaponShotInfo::init`.
        damage: f32,
    },
    /// The player pressed ESC (`MissionInterfaceManager::update`, missiongui.cpp:443-446). In movie mode it asks the movie to end
    /// (`ForceMovieEnd` then reads 1); outside movie mode it does nothing.
    ForceMovieEnd,
    /// The mouse profile the player set (`useLeftRightMouseProfile`, read by `GUIIsAOEStyle`): true is the AOE profile.
    SetAoeStyle(bool),
    /// The GUI's cursor query of `MissionInterfaceManager::update` (missiongui.cpp:742-754): `eye->inverseProject` of the mouse gives
    /// `at`, and on game-select terrain the GUI asks `Team::home->teamLineOfSight(at, 0.0f)`. The answer only feeds the cursor, but the
    /// query is a sim effect: every home mover in visual range of `at` runs `getLOSPosition`, which (re)caches its lowest weapon node in
    /// turns < 6 (mover.cpp:3471-3490). `at` is camera + mouse, a declared upstream input (the sweep replays the oracle's
    /// `--cursor-trace`); the sim runs it after the frame's orders and the callout step, where the GUI update does.
    CursorLineOfSight {
        /// The terrain point under the mouse (`wPos`).
        at: Vec3,
    },
}

/// One line of an oracle order script (`<frame> move|attack|jump|stop <part> ...`), without the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScriptedOrder {
    /// `move <part> <x> <y> run|walk`: `MOVETO_POINT` to the clicked point (z = terrain elevation there).
    Move {
        /// Mover.
        part: PartId,
        /// World x.
        x: f32,
        /// World y.
        y: f32,
        /// `run` (`TRAVEL_MODE_FAST`) or `walk` (`TRAVEL_MODE_SLOW`).
        run: bool,
    },
    /// `attack <part> <target part>`: `ATTACK_OBJECT` / `ATTACK_TO_DESTROY`, ranged, range = the mover's `attackRange`, pursue.
    Attack {
        /// Attacker.
        part: PartId,
        /// Target.
        target: PartId,
    },
    /// `jump <part> <x> <y>`: `JUMPTO_POINT` (`MissionInterfaceManager::doJump`).
    Jump {
        /// Mover.
        part: PartId,
        /// World x.
        x: f32,
        /// World y.
        y: f32,
    },
    /// `stop <part>`.
    Stop {
        /// Mover.
        part: PartId,
    },
}

// ---- save/load (lane D: order state; the mover/pilot code saves TacticalOrder by value) ----

/// Enum `Snapshot` with explicit `u8` tags; an unknown tag is a load error. Shared by lane D files via `pub(crate) use`.
macro_rules! snapshot_enum {
    ($ty:path { $($v:ident = $t:literal),* $(,)? }) => {
        impl $crate::save::Snapshot for $ty {
            fn save(&self, w: &mut $crate::save::Writer) {
                let t: u8 = match self {
                    $(Self::$v => $t,)*
                };
                w.u8(t);
            }
            fn load(r: &mut $crate::save::Reader<'_>) -> $crate::save::SaveResult<Self> {
                Ok(match r.u8()? {
                    $($t => Self::$v,)*
                    other => {
                        return Err($crate::save::SaveError(format!(
                            "save: bad {} tag {other}",
                            stringify!($ty)
                        )))
                    }
                })
            }
        }
    };
}
pub(crate) use snapshot_enum;

snapshot_enum!(OrderCode {
    None = 0, Wait = 1, MoveToPoint = 2, MoveToObject = 3, JumpToPoint = 4, JumpToObject = 5,
    TraversePath = 6, PatrolPath = 7, Escort = 8, Follow = 9, Guard = 10, Stop = 11, PowerUp = 12,
    PowerDown = 13, WaypointsDone = 14, Eject = 15, AttackObject = 16, AttackPoint = 17,
    HoldFire = 18, Withdraw = 19, Scramble = 20, Capture = 21, Refit = 22, GetFixed = 23,
    LoadIntoCarrier = 24, DeployElementals = 25, Recover = 26,
});
snapshot_enum!(OrderOrigin { Player = 0, Commander = 1, SelfOrder = 2 });
snapshot_enum!(TravelMode { Slow = 0, Fast = 1, Jump = 2 });
snapshot_enum!(AttackMethod { Ranged = 0, Dfa = 1, Ramming = 2 });
snapshot_enum!(Tactic {
    None = 0, FlankLeft = 1, FlankRight = 2, FlankRear = 3, StopAndFire = 4, Turret = 5, Joust = 6,
});

crate::snapshot_struct!(WayPath {
    points,
    mode,
    cur_point,
    num_points,
});
crate::snapshot_struct!(MoveParams {
    way_path,
    face_object,
    wait,
    mine_laying,
    escape_tile,
    jump,
    from_area,
    keep_moving,
});
crate::snapshot_struct!(AttackParams {
    method,
    range,
    tactic,
    aim_location,
    pursue,
    obliterate,
    target_point,
    attack_type,
});
crate::snapshot_struct!(TacticalOrder {
    id,
    time,
    delayed_time,
    last_time,
    unit_order,
    sub_order,
    origin,
    code,
    move_params,
    attack_params,
    target,
    target_object_class,
    selection_index,
    stage,
    status_code,
    group_flags,
});
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tactical_order_init_matches_cpp_defaults() {
        let order = TacticalOrder::new(OrderOrigin::Player, OrderCode::MoveToPoint, true, 12.5);
        assert_eq!(order.time, 12.5);
        assert_eq!(order.delayed_time, -1.0);
        assert_eq!(order.last_time, -1.0);
        assert_eq!(order.selection_index, -1);
        assert_eq!(order.stage, 1);
        assert!(order.move_params.face_object);
        assert!(order.move_params.keep_moving);
        assert!(order.attack_params.pursue);
        assert_eq!(order.attack_params.range, FIRERANGE_OPTIMAL);
        assert_eq!(TacticalOrder::default().origin, OrderOrigin::Commander);
    }

    #[test]
    fn tactical_order_equals_ignores_id_and_timestamps_but_checks_order_payload() {
        let mut a = TacticalOrder::new(OrderOrigin::Player, OrderCode::MoveToPoint, false, 1.0);
        let mut b = a.clone();
        b.id = 42;
        b.time = 99.0;
        b.delayed_time = 10.0;
        assert!(a.equals(&b));

        // tacordr.cpp:1938-1955 compares `numPoints * 3` coordinates; an order built by orderMoveToPoint has numPoints == 0, so
        // two such orders to different places are equal (the quirk behind `curTacOrder.equals(&tacOrder)` in coreMoveTo)
        a.move_params.way_path.points[0] = Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        assert!(a.equals(&b));
        // after unpack (numPoints = 1) the coordinates and the first three mode entries count
        a.move_params.way_path.num_points = 1;
        b.move_params.way_path.num_points = 1;
        assert!(!a.equals(&b));
        b.move_params.way_path.points = a.move_params.way_path.points.clone();
        assert!(a.equals(&b));
        // mode[1], mode[2] are the unused entries of the 15-entry array: Slow
        a.move_params.way_path.mode = vec![TravelMode::Slow, TravelMode::Fast];
        assert!(!a.equals(&b));
        b.move_params.way_path.mode = vec![TravelMode::Slow, TravelMode::Fast];
        assert!(a.equals(&b));
        b.group_flags = 1;
        assert!(!a.equals(&b));
    }

    #[test]
    fn tactical_order_attack_equality_checks_cpp_attack_fields() {
        let a = TacticalOrder::new(OrderOrigin::Player, OrderCode::AttackObject, false, 0.0);
        let mut b = a.clone();
        assert!(a.equals(&b));
        b.attack_params.attack_type = 3;
        assert!(a.equals(&b));
        b.attack_params.tactic = Tactic::Joust;
        assert!(!a.equals(&b));
    }

    #[test]
    fn tactical_order_init_has_one_way_point_slot_and_no_points() {
        // tacordr.cpp:169-206: points[0..2] = 0, mode[0] = SLOW, numPoints = 0 (the arrays always exist in C++)
        let o = TacticalOrder::init();
        assert_eq!(o.move_params.way_path.points, vec![Vec3::default()]);
        assert_eq!(o.move_params.way_path.mode, vec![TravelMode::Slow]);
        assert_eq!(o.move_params.way_path.num_points, 0);
        assert_eq!(o.target_object_class, 0);
        assert_eq!(o.attack_params.attack_type, 1);
        assert!(o.move_params.keep_moving && o.move_params.face_object && o.attack_params.pursue);
    }

    #[test]
    fn max_waypts_matches_cpp() {
        // code/dwarrior.h:14 `#define MAX_WAYPTS 15` (tacordr.cpp:228 addWayPoint caps at it)
        assert_eq!(MAX_WAYPTS, 15);
    }
}
