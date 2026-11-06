// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The plain per-frame data the mission HUD is built from, read off the live simulation.
//!
//! [`HudSnapshot`] holds only `mc2-ui` input types and numbers (no `mc2-sim` types), so the HUD
//! runtime ([`crate::hud`]) and the generic [`crate::game::GameCore`] never see the simulation.
//! [`snapshot`] is the one place that reads `mc2_sim::World`; it is read-only and runs once per
//! game frame (`ControlGui::update`'s reads of `ObjectManager`, `Team::home`, `Mover` and
//! `Objective`).
//!
//! What the simulation cannot give yet is left at the value the C++ has when nothing happened and
//! is named in [`UNAVAILABLE`]; nothing is invented.

use crate::orders::{AimFacts, SupportTarget};
use mc2_sim::combat::{loadout_of, HOME_COMMANDER};
use mc2_sim::object::{Kind, Obj, ObjectStatus};
use mc2_sim::World;
use mc2_sim_data::component::MasterComponentList;
use mc2_ui::control::{FireRange, UnitCapabilities};
use mc2_ui::hud_info::info::{PilotInfo, UnitInfo, WeaponInfo};
use mc2_ui::mission_keys::ObjectClass;
use mc2_ui::objectives::{ObjectiveStatus, ObjectiveText, ObjectiveView};
use mc2_ui::tacmap::{TacTeam, TacUnit};
use mc2_ui::unitbar::{Armor, Loc, MechArmor, Pilot, UnitBarUnit, VehicleArmor};

/// HUD inputs the simulation does not provide yet, reported once by the runtime. The control panel's support buttons and the
/// selection's capability flags are real (`caps`, `HudRuntime`'s `SupportEnabled`); the multiplayer `allUnitsDestroyed` disable
/// has no single-player counterpart.
pub const UNAVAILABLE: &[&str] = &[];

/// A string that is either literal or a string-table id (`cLoadString`), resolved by the HUD
/// runtime, which owns the string table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HudText {
    Literal(String),
    Resource(u32),
}

impl HudText {
    pub fn resolve(&self, strings: &dyn Fn(u32) -> String) -> String {
        match self {
            HudText::Literal(s) => s.clone(),
            HudText::Resource(id) => strings(*id),
        }
    }
}

/// One player unit with everything the unit bar, the info window and the control panel read.
#[derive(Clone, Debug, PartialEq)]
pub struct HudUnit {
    /// `partID`: the stable id of the unit (the unit bar id).
    pub part_id: i32,
    /// The pilot pool slot; radio playback events address this id.
    pub pilot_slot: u32,
    /// World position (x, y) and z.
    pub position: (f32, f32, f32),
    pub bar: UnitBarUnit,
    pub pilot_name: HudText,
    pub name: HudText,
    pub info: UnitInfo,
    pub caps: UnitCapabilities,
    pub sensor_range_m: f32,
    /// The mover's current `attackRange` (`FIRERANGE_*`), which `doAttack` reads per mover.
    pub attack_range: i32,
    /// `Mover::hasNonAreaWeapon()` for called-shot eligibility (`missiongui.cpp:2447-2458`).
    pub has_non_area_weapon: bool,
    /// `BattleMech::getJumpRange()` in world units (`mech.cpp:6744`); 0 when the unit cannot jump.
    pub jump_range_wu: f32,
    /// Origin `canJumpToWPos` measures from (`missiongui.cpp:3524-3538`): the last queued
    /// tac-order waypoint when the pilot's queue is not empty, else the unit's current (x, y).
    pub last_jump_from: (f32, f32),
    /// Projected selection box `[left, top, right, bottom]` in camera viewport pixels
    /// (`upperLeft`/`lowerRight` from the last `recalcBounds` pass).
    pub screen_box: Option<[f32; 4]>,
    /// `recalcBounds` marked the mover inside the camera view volume.
    pub in_view: bool,
    /// `Mover::needsRefit()` (`canRepairBay`, missiongui.cpp:4540).
    pub needs_refit: bool,
}

/// An object of another commander a world click can order an attack on (`getObjectFromScreenPos`
/// for an object the home team has in contact).
#[derive(Clone, Debug, PartialEq)]
pub struct PickTarget {
    /// The object's `watchID`; the order's target.
    pub watch_id: u32,
    /// The picked mover's stable part id. Structures have no player-eject path.
    pub part_id: Option<i32>,
    pub position: (f32, f32, f32),
    /// The object's projected selection box `[left, top, right, bottom]` in camera viewport pixels
    /// (the appearance's `upperLeft`/`lowerRight`, which `findObjectByMouse` tests the mouse against).
    pub screen_box: [f32; 4],
    /// The visible faces (`PerPolySelect`, `mclib/tgl.cpp:2763`): a structure's click must land on one.
    pub triangles: ScreenTriangles,
    /// A mover: `findObjectByMouse` takes it on the box alone ("Movers are NOT per poly", objmgr.cpp:2253).
    pub mover: bool,
    pub team: TacTeam,
    /// `isDisabled`: `doAttack` is not called on a disabled target (`missiongui.cpp:1454`).
    pub disabled: bool,
    /// `isCaptureable(home)` (bldng.cpp:1046): `doAttack` takes the capture branch, not the attack branch.
    pub captureable: bool,
    /// `getCaptureBlocker(selectedMover)` from `doAttack` (missiongui.cpp:1848), by home mover part id.
    pub capture_blockers: std::collections::BTreeMap<i32, bool>,
    /// `getObjectClass()` as `choose_cursor` branches on it (`missiongui.cpp:3851-3915`).
    pub class: ObjectClass,
    /// `getObjectType()->getSubType() == BUILDING_SUBTYPE_LANDBRIDGE` (`missiongui.cpp:3858`).
    pub landbridge: bool,
    /// What `canRepair`, `canSalvage` and the repair bay test read off the object.
    pub support: SupportTarget,
    /// Target body facts for `aimLeg/aimArm/aimHead` (`missiongui.cpp:2117-2270`).
    pub aim: Option<AimFacts>,
    /// Unit facts for showing a picked mover in the Info tab.
    pub info_unit: Option<HudUnit>,
}

/// A mission message line (`ControlGui::setChatText`).
#[derive(Clone, Debug, PartialEq)]
pub enum HudMessage {
    Text(String),
    Resource(u32),
}

/// The projected selection boxes the last `recalcBounds` pass produced (`[left, top, right, bottom]`
/// in viewport pixels): movers by part id, structures by object handle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickBoxes {
    pub movers: std::collections::BTreeMap<i32, [f32; 4]>,
    pub structures: std::collections::BTreeMap<u32, [f32; 4]>,
    /// The front-facing screen triangles of the shape drawn now (`PerPolySelect`'s visible faces), as
    /// `[x0, y0, x1, y1, x2, y2]`; same keys as the boxes.
    pub mover_triangles: std::collections::BTreeMap<i32, ScreenTriangles>,
    pub structure_triangles: std::collections::BTreeMap<u32, ScreenTriangles>,
    /// The last turn each mover's `recalcBounds` found it in view: its `windowsVisible` (`mech.cpp:6055`,
    /// `objmgr.cpp:2820`).  The sim does not keep that field for running movers, so the app records it
    /// from the same `inView` result.
    pub mover_drawn_turn: std::collections::BTreeMap<i32, i32>,
}

/// Screen triangles of one object's visible faces.
pub type ScreenTriangles = std::sync::Arc<[[f32; 6]]>;

/// Everything the HUD reads from the simulation in one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudSnapshot {
    /// The home commander's movers on the GUI, in object-manager order.
    pub units: Vec<HudUnit>,
    /// Movers of other commanders in contact with the home team.
    pub contacts: Vec<TacUnit>,
    /// The movers, buildings and turrets of other commanders in contact: what a click can pick.
    pub picks: Vec<PickTarget>,
    /// Live facts of every mover the Info window may keep showing, by part id: a mover of the home
    /// team, a disabled one, or one the home team sees by line of sight (`InfoWindow::update`,
    /// `code/infowindow.cpp:483-489`).  Absent means the window clears.
    pub info_units: std::collections::BTreeMap<i32, HudUnit>,
    pub objectives: Vec<ObjectiveView>,
    /// The numerals the minimap draws (`CObjective::RenderMarkers`, objective.cpp:2256-2303), in objective order.
    pub tac_objectives: Vec<mc2_ui::tacmap::TacObjective>,
    /// How many objectives `Team::home` holds, drawn or not: the flash of `animateTacMap` indexes that list.
    pub tac_objective_total: usize,
    /// The first still-undetermined NAV objective (`gotoNextNavMarker`, missiongui.cpp:4712-4746),
    /// at terrain elevation. `None` means there is no currently valid marker.
    pub next_nav_marker: Option<mc2_sim::math::Vec3>,
    /// `mission->actualTime`.
    pub mission_time: f32,
    /// `m_timeLimit`, the mission's time limit in seconds (-1 for none). The timer and its Betty warnings
    /// read it (`code/controlgui.cpp:375-400`).
    pub time_limit: f32,
    /// `realVerticesMapSide`.
    pub vertices_per_side: i32,
    pub resource_points: i32,
    /// `paintingMyVtol`: the home commander's support VTOL is in flight (`MissionInterfaceManager::paintingVtol`).
    pub vtol_in_flight: bool,
    /// `LogisticsData::gotPilotsLeft()`: the recovery craft can be bought.
    pub pilots_remaining: bool,
    /// The mission's termination result when it is final (`scenarioResult`), else 0.
    pub result: u32,
    /// `terminationCounterStarted`: the mission is ending, so the in-mission results (`renderStatusInfo`,
    /// `controlgui.cpp:431-436`) are on screen.
    pub results_started: bool,
    /// `neverEndingStory`: the results do not fade to black (controlgui.cpp:589).
    pub never_ending_story: bool,
    /// `Team::home->objectives.Status()`: `true` only when it is `OS_SUCCESSFUL`, which is what
    /// `ControlGui::renderResults` writes over the status frame (controlgui.cpp:565-569). An
    /// undetermined list is `MISSION FAILED` there, as is a failed one.
    pub home_successful: bool,
    /// Global `invulnerableON`, set by the `setInvulnerable` ABL native. The order bridge uses it
    /// to refuse home-commander ejection (`missiongui.cpp:2313`).
    pub invulnerable: bool,
}

/// `pilotWoundTable` (mech.cpp:9068).
const PILOT_WOUND_TABLE: [f32; 7] = [1.0, 0.95, 0.85, 0.75, 0.50, 0.30, 0.00];

fn ratio(cur: f32, max: f32) -> f32 {
    // The C++ divides unguarded; a location without armour (a vehicle's absent rear) is NaN
    // there and counts as undamaged here.
    if max == 0.0 {
        1.0
    } else {
        cur / max
    }
}

/// `MECH_ARMOR_LOCATION_*` order of `Loadout::armor`.
mod mech_loc {
    pub const HEAD: usize = 0;
    pub const CTORSO: usize = 1;
    pub const LTORSO: usize = 2;
    pub const RTORSO: usize = 3;
    pub const LARM: usize = 4;
    pub const RARM: usize = 5;
    pub const LLEG: usize = 6;
    pub const RLEG: usize = 7;
    pub const RCTORSO: usize = 8;
    pub const RLTORSO: usize = 9;
    pub const RRTORSO: usize = 10;
}

/// `GROUNDVEHICLE_LOCATION_*` order of `Loadout::armor`.
mod gv_loc {
    pub const FRONT: usize = 0;
    pub const LEFT: usize = 1;
    pub const RIGHT: usize = 2;
    pub const REAR: usize = 3;
    pub const TURRET: usize = 4;
}

/// `ARMOR_TO_BODY_LOCATION`: the body location whose `damageState` marks an armour location
/// destroyed (`MechArmorToBodyLocation`, mech.cpp:427).
const ARMOR_TO_BODY: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 1, 2, 3];

fn loc(l: &mc2_sim::combat::Loadout, armor_index: usize, mech: bool) -> Loc {
    let (cur, max) = l.armor.get(armor_index).copied().unwrap_or((0.0, 0));
    let destroyed = mech
        && l.body
            .get(ARMOR_TO_BODY[armor_index.min(10)])
            .is_some_and(|b| b.damage_state == mc2_sim::combat::stats::IS_DAMAGE_DESTROYED);
    Loc {
        cur,
        max: f32::from(max),
        destroyed,
    }
}

fn mech_armor(l: &mc2_sim::combat::Loadout) -> MechArmor {
    use mech_loc::*;
    MechArmor {
        head: loc(l, HEAD, true),
        ctorso: loc(l, CTORSO, true),
        rctorso: loc(l, RCTORSO, true),
        ltorso: loc(l, LTORSO, true),
        rltorso: loc(l, RLTORSO, true),
        rtorso: loc(l, RTORSO, true),
        rrtorso: loc(l, RRTORSO, true),
        larm: loc(l, LARM, true),
        rarm: loc(l, RARM, true),
        lleg: loc(l, LLEG, true),
        rleg: loc(l, RLEG, true),
    }
}

fn vehicle_armor(l: &mc2_sim::combat::Loadout) -> VehicleArmor {
    use gv_loc::*;
    VehicleArmor {
        front: loc(l, FRONT, false),
        rear: loc(l, REAR, false),
        left: loc(l, LEFT, false),
        right: loc(l, RIGHT, false),
        turret: loc(l, TURRET, false),
    }
}

/// `BattleMech::getTotalEffectiveness` / `GroundVehicle::getTotalEffectiveness`: the unit bar's
/// health bar fill (`barStatus`) of `obj`.  See [`bar_fill`].
pub fn bar_status(
    obj: &Obj,
    l: &mc2_sim::combat::Loadout,
    wounds: f32,
    disabled_or_destroyed: bool,
    refit_or_mines: bool,
) -> f32 {
    bar_fill(
        matches!(obj.kind, Kind::Mech(_)),
        &l.armor,
        (
            l.stats.weapon_effectiveness,
            l.stats.max_weapon_effectiveness,
        ),
        wounds,
        disabled_or_destroyed,
        refit_or_mines,
    )
}

/// The health bar fill from the unit's armour `(cur, max)` per location (mech or vehicle order),
/// `(weaponEffectiveness, maxWeaponEffectiveness)`, the pilot's wounds and whether the unit is
/// disabled or destroyed.
///
/// Intentional difference (rust/docs/DIFFERENCES.md D-48): the mech's leg term in the C++ reads the
/// ARM armour (`armorLeg = curArm / maxArm * 0.4 + 0.6`, mech.cpp:9057, a copy-paste slip); the
/// leg armour it just summed is used here.
pub fn bar_fill(
    mech: bool,
    armor: &[(f32, u8)],
    (weapon_eff, max_weapon_eff): (f32, f32),
    wounds: f32,
    disabled_or_destroyed: bool,
    refit_or_mines: bool,
) -> f32 {
    let weapon = if max_weapon_eff != 0.0 {
        weapon_eff.min(max_weapon_eff) / max_weapon_eff
    } else if !mech {
        1.0
    } else {
        // `maxWeaponEffectiveness == 0` asserts and is forced to 1 (mech.cpp:9004-9006).
        weapon_eff.min(1.0)
    };
    let pilot = PILOT_WOUND_TABLE[(wounds.round().max(0.0) as usize).min(6)];
    let armor_effect = if disabled_or_destroyed {
        0.0
    } else {
        let a = |i: usize| armor.get(i).copied().unwrap_or((0.0, 0));
        let r = |cur: f32, max: f32| ratio(cur, max);
        if mech {
            use mech_loc::*;
            let head = r(a(HEAD).0, f32::from(a(HEAD).1)) * 0.6 + 0.4;
            let (mut cc, mut cm) = (a(CTORSO).0, f32::from(a(CTORSO).1));
            if cc > a(RCTORSO).0 {
                cc = a(RCTORSO).0;
                cm = f32::from(a(RCTORSO).1);
            }
            let ctorso = r(cc, cm) * 0.5 + 0.5;
            let torso_cur = a(RLTORSO).0 + a(LTORSO).0 + a(RRTORSO).0 + a(RTORSO).0;
            let torso_max = f32::from(a(RLTORSO).1)
                + f32::from(a(LTORSO).1)
                + f32::from(a(RRTORSO).1)
                + f32::from(a(RTORSO).1);
            let torso = r(torso_cur, torso_max) * 0.25 + 0.75;
            let arm = r(
                a(LARM).0 + a(RARM).0,
                f32::from(a(LARM).1) + f32::from(a(RARM).1),
            ) * 0.25
                + 0.75;
            let leg = r(
                a(LLEG).0 + a(RLEG).0,
                f32::from(a(LLEG).1) + f32::from(a(RLEG).1),
            ) * 0.4
                + 0.6;
            head * ctorso * torso * arm * leg
        } else {
            use gv_loc::*;
            let side = |i: usize| r(a(i).0, f32::from(a(i).1)) * 0.6 + 0.4;
            let turret = if a(TURRET).1 != 0 { side(TURRET) } else { 1.0 };
            // `extraEffect` (gvehicl.cpp:5404): a refit vehicle or a mine layer also scales by its
            // turret points (refit/mine points), the plain ratio.
            let extra = if refit_or_mines && a(TURRET).1 != 0 {
                r(a(TURRET).0, f32::from(a(TURRET).1))
            } else {
                1.0
            };
            side(FRONT) * side(REAR) * side(LEFT) * side(RIGHT) * turret * extra
        }
    };
    weapon * armor_effect * pilot
}

fn weapons(l: &mc2_sim::combat::Loadout, comps: Option<&MasterComponentList>) -> Vec<WeaponInfo> {
    let Some(comps) = comps else {
        return Vec::new();
    };
    (l.num_other()..l.num_other() + l.num_weapons())
        .filter_map(|i| {
            let it = l.inventory.get(i)?;
            let c = comps.get(i32::from(it.master_id));
            let range = c
                .and_then(|c| c.weapon())
                .and_then(|w| w.range)
                .map_or(0, |r| r.index() as u8);
            let unlimited = c
                .and_then(|c| c.weapon())
                .is_none_or(|w| w.ammo_type == mc2_sim_data::component::WeaponAmmoType::Unlimited);
            Some(WeaponInfo {
                master_id: it.master_id,
                range,
                unlimited_ammo: unlimited,
                shots: l.weapon_shots(comps, i as i64),
                disabled: it.disabled,
                health: f32::from(it.health),
            })
        })
        .collect()
}

fn equipment(l: &mc2_sim::combat::Loadout) -> [Option<u8>; 4] {
    // `Loadout::critical`: cockpit, engine, lifeSupport, sensor, ecm, probe, jumpJets.
    let id = |slot: usize| {
        l.critical
            .get(slot)
            .copied()
            .filter(|&i| i != 255)
            .and_then(|i| l.inventory.get(usize::from(i)))
            .map(|it| it.master_id)
    };
    [id(4), id(5), id(6), id(3)]
}

fn relation(w: &World, team: i32) -> TacTeam {
    use mc2_sim::ai::team::Relation;
    let home = w.teams.home;
    match usize::try_from(team)
        .ok()
        .filter(|t| *t < w.teams.relations.len())
        .map(|t| w.teams.relations[home][t])
    {
        Some(Relation::Friendly) => TacTeam::PlayerAlt,
        Some(Relation::Neutral) => TacTeam::Neutral,
        _ => TacTeam::Enemy,
    }
}

/// `getObjectClass()` as `makeTargetCursor` matches it (`missiongui.cpp:3818-3915`).
fn cursor_class(class: mc2_sim::object::ObjectClass) -> ObjectClass {
    use mc2_sim::object::ObjectClass as S;
    match class {
        S::BattleMech => ObjectClass::Mech,
        S::GroundVehicle => ObjectClass::GroundVehicle,
        S::Elemental => ObjectClass::Elemental,
        S::Building => ObjectClass::Building,
        S::TreeBuilding => ObjectClass::TreeBuilding,
        S::Turret => ObjectClass::Turret,
        S::Gate => ObjectClass::Gate,
        S::Tree => ObjectClass::Tree,
        S::TerrainObject => ObjectClass::TerrainObject,
        S::Bridge => ObjectClass::Bridge,
        S::Debris => ObjectClass::Debris,
        _ => ObjectClass::Other,
    }
}

fn is_landbridge(w: &World, obj: &Obj) -> bool {
    obj.c.type_handle == mc2_sim_data::objtype::building::LAND_BRIDGE_TYPE_INDEX as i32
        || matches!(
            mc2_sim::object::structure::building_info(w, obj.c.type_handle),
            Some(b)
                if b.sub_type == mc2_sim_data::objtype::building::BuildingSubType::LandBridge
        )
}

fn sensor_range(w: &World, h: mc2_sim::ObjectHandle) -> f32 {
    w.sensors
        .pool
        .iter()
        .find(|s| s.owner == h && s.team_index >= 0)
        .map_or(0.0, |s| if s.broken { 0.0 } else { s.range })
}

/// `Mover::getEcmRange() > 0` (gametacmap.cpp:393): the mission loader stores an assigned ECM
/// as the critical's inventory index.  A disabled component still counts here, as it does in the
/// C++ tactical map filter.
fn has_ecm(obj: &Obj) -> bool {
    loadout_of(obj).is_some_and(|loadout| {
        loadout
            .critical
            .get(mc2_sim::mission::loadout::crit::ECM)
            .is_some_and(|index| *index != mc2_sim::mission::loadout::NO_COMPONENT)
    })
}

fn is_null_signature(w: &World, obj: &Obj) -> bool {
    matches!(obj.kind, Kind::Vehicle(_))
        && w.data
            .vehicle_objects
            .get(&(obj.c.type_handle as u32))
            .is_some_and(|vehicle| !vehicle.is_sensor_contact)
}

fn unit_names(w: &World, obj: &Obj) -> (HudText, i32) {
    let table = w.data.object_types.as_deref();
    match &obj.kind {
        Kind::Mech(m) => {
            let name = if m.variant_name.is_empty() {
                table
                    .and_then(|t| t.get(obj.c.type_handle as u32))
                    .and_then(|t| t.base.appearance_name.clone())
                    .unwrap_or_default()
            } else {
                m.variant_name.clone()
            };
            let icon = table
                .and_then(|t| t.get(obj.c.type_handle as u32))
                .and_then(|t| t.base.appearance_name.as_ref())
                .and_then(|n| w.data.chassis.get(&n.to_ascii_lowercase()))
                .and_then(|c| c.icon_picture_index)
                .unwrap_or(0);
            (HudText::Literal(name), icon)
        }
        Kind::Vehicle(_) => {
            let v = w.data.vehicle_objects.get(&(obj.c.type_handle as u32));
            let name = match v {
                Some(v) if v.desc_id >= 0 => HudText::Resource(v.desc_id as u32),
                _ => HudText::Literal("VEHICLE".into()),
            };
            (name, v.map_or(0, |v| v.icon_index))
        }
        _ => (HudText::Literal(String::new()), 0),
    }
}

/// `isRefit() || mineLayer` of a ground vehicle: `canRefit = refitPoints > 0` (gvehicl.cpp:931) or
/// mines still left (`mineLayer` is the live count, decremented per mine laid, damage.rs:1162).
pub fn refit_or_mines(w: &World, obj: &Obj) -> bool {
    let mines = loadout_of(obj).is_some_and(|l| l.stats.mine_layer > 0);
    matches!(obj.kind, Kind::Vehicle(_))
        && (mines
            || w.data
                .vehicle_objects
                .get(&(obj.c.type_handle as u32))
                .is_some_and(|v| v.refit_points > 0))
}

fn caps(w: &World, h: mc2_sim::ObjectHandle, obj: &Obj) -> UnitCapabilities {
    let (jump, range, mover) = match &obj.kind {
        Kind::Mech(m) => (m.num_jump_jets > 0, m.m.attack_range, &m.m),
        Kind::Vehicle(v) => (false, v.m.attack_range, &v.m),
        _ => return UnitCapabilities::default(),
    };
    let h = Some(h);
    let vehicle = matches!(obj.kind, Kind::Vehicle(_));
    // `ControlGui::update` (controlgui.cpp:1033-1039): `isMineLayer()`, `isRecover()`, `isRefit() && getRefitPoints()`, and a
    // guard tower is a mover that cannot move (`maxMoveSpeed == 0`) whose `suppressionFire` is its pressed state
    let tower = mover.max_move_speed == 0.0;
    UnitCapabilities {
        can_jump: jump,
        can_lay_mines: loadout_of(obj).is_some_and(|l| l.stats.mine_layer != 0),
        can_salvage: h.is_some_and(|h| mc2_sim::mover::refit::can_recover(w, h)),
        can_repair: vehicle
            && h.is_some_and(|h| {
                mc2_sim::mover::refit::can_refit(w, h)
                    && mc2_sim::mover::refit::vehicle_refit_points(w, h) > 0.0
            }),
        is_guard_tower: tower,
        guard_tower_suppression: tower && mover.suppression_fire != 0,
        has_queued_orders: w
            .pilots
            .get(mover.pilot_slot)
            .is_some_and(|p| !p.queue.is_empty()),
        attack_range: Some(match range {
            r if r == mc2_sim::command::FIRERANGE_OPTIMAL => FireRange::Optimal,
            r if r == mc2_sim::command::FIRERANGE_CURRENT => FireRange::Current,
            0 => FireRange::Short,
            1 => FireRange::Medium,
            2 => FireRange::Long,
            _ => FireRange::Optimal,
        }),
    }
}

fn unit(w: &World, h: mc2_sim::ObjectHandle, obj: &Obj) -> Option<HudUnit> {
    let l = loadout_of(obj)?;
    let (mover, mech) = match &obj.kind {
        Kind::Mech(m) => (&m.m, true),
        Kind::Vehicle(v) => (&v.m, false),
        _ => return None,
    };
    let pilot = w.pilots.get(mover.pilot_slot);
    let load = w.mission.pilot_loads.get(usize::from(mover.pilot_slot));
    let wounds = pilot.map_or(0.0, |p| p.wounds);
    let dead = obj.c.is_disabled();
    let (name, icon) = unit_names(w, obj);
    let pilot_name = load
        .map(|l| l.ids[0])
        .filter(|id| *id >= 0)
        .map_or(HudText::Literal(String::new()), |id| {
            HudText::Resource(id as u32)
        });
    let photo = load.map_or(0, |l| l.ids[2].max(0) as u32);
    let active =
        pilot.is_some_and(|p| p.status == mc2_sim::mission::outcome::WarriorStatus::Normal);
    let comps = w.data.components.as_deref();
    let bar = UnitBarUnit {
        id: obj.c.part_id.0 as u32,
        icon_picture: icon.max(0) as u32,
        tonnage: obj.c.tonnage.round().max(0.0) as u32,
        pilot: Pilot {
            name: String::new(),
            photo,
            message_playing: false,
        },
        armor: if mech {
            Armor::Mech(mech_armor(l))
        } else {
            Armor::Vehicle(vehicle_armor(l))
        },
        bar_status: bar_status(obj, l, wounds, dead, refit_or_mines(w, obj)),
        selected: false,
        groups: w
            .mission
            .unit_groups
            .get(&obj.c.part_id)
            .copied()
            .filter(|group| *group < 10)
            .map_or(0, |group| 1_u16 << group),
        shutdown: obj.c.status == ObjectStatus::ShutDown,
        destroyed: dead,
        recover_pending: !mover.recover_buddy.is_none(),
        power_used_up: l.stats.cause_of_death == mc2_sim::combat::fx::death::POWER_USED_UP as i32,
        can_jump: matches!(&obj.kind, Kind::Mech(m) if m.num_jump_jets > 0),
    };
    let info = UnitInfo {
        id: obj.c.part_id.0 as u32,
        name: String::new(),
        is_mech: mech,
        team_is_home: obj.team_id() == w.teams.home as i32,
        contact_visual: true,
        has_team: obj.team_id() >= 0,
        team_not_enemy: relation(w, obj.team_id()) != TacTeam::Enemy,
        disabled: obj.c.is_disabled(),
        destroyed: obj.c.is_destroyed(),
        health: bar.bar_status,
        weapons: weapons(l, comps),
        equipment: equipment(l),
        pilot: PilotInfo {
            active,
            name: String::new(),
            rank: load.and_then(|l| l.rank).map_or(0, |r| r as u8),
            skills: pilot.map_or([0; 3], |p| {
                [
                    p.skills[mc2_sim_data::pilot::Skill::Piloting.index()].clamp(0, 255) as u8,
                    p.skills[mc2_sim_data::pilot::Skill::Sensors.index()].clamp(0, 255) as u8,
                    p.skills[mc2_sim_data::pilot::Skill::Gunnery.index()].clamp(0, 255) as u8,
                ]
            }),
            specialties: pilot.map_or_else(Vec::new, |p| p.specialty.to_vec()),
        },
    };
    Some(HudUnit {
        part_id: obj.c.part_id.0,
        pilot_slot: u32::from(mover.pilot_slot),
        position: (obj.c.position.x, obj.c.position.y, obj.c.position.z),
        bar,
        pilot_name,
        name,
        info,
        caps: caps(w, h, obj),
        sensor_range_m: sensor_range(w, h),
        attack_range: mover.attack_range,
        has_non_area_weapon: has_non_area_weapon(w, l),
        jump_range_wu: mc2_sim::ai::mover_ops::jump_range(w, h),
        last_jump_from: last_jump_from(w, mover.pilot_slot, obj.c.position),
        screen_box: None,
        in_view: false,
        needs_refit: mc2_sim::mover::refit::needs_refit(w, h),
    })
}

fn has_non_area_weapon(w: &World, l: &mc2_sim::combat::Loadout) -> bool {
    let Some(tables) = w.data.gamesys.tables.as_ref() else {
        return false;
    };
    (l.num_other()..l.num_other() + l.num_weapons()).any(|i| {
        l.inventory.get(i).is_some_and(|item| {
            !tables
                .mover_general
                .area_effect_weapons
                .contains(&i32::from(item.master_id))
        })
    })
}

fn aim_facts(obj: &Obj) -> Option<AimFacts> {
    let (ground_mech, loadout) = match &obj.kind {
        Kind::Mech(mech) => (mech.m.move_level == 0, loadout_of(obj)),
        _ => (false, None),
    };
    let Some(loadout) = loadout else {
        return Some(AimFacts {
            ground_mech: false,
            ..AimFacts::default()
        });
    };
    let destroyed = |index: usize| {
        loadout
            .body
            .get(index)
            .is_some_and(|body| body.damage_state == mc2_sim::combat::stats::IS_DAMAGE_DESTROYED)
    };
    Some(AimFacts {
        ground_mech,
        any_selected_non_area_weapon: false,
        left_leg_destroyed: destroyed(mc2_sim::combat::stats::body::LLEG),
        right_leg_destroyed: destroyed(mc2_sim::combat::stats::body::RLEG),
        left_arm_destroyed: destroyed(mc2_sim::combat::stats::body::LARM),
        right_arm_destroyed: destroyed(mc2_sim::combat::stats::body::RARM),
        head_destroyed: destroyed(mc2_sim::combat::stats::body::HEAD),
    })
}

fn last_jump_from(w: &World, pilot_slot: u16, position: mc2_sim::math::Vec3) -> (f32, f32) {
    match w.pilots.get(pilot_slot).and_then(|p| p.queue.last()) {
        Some(q) => (q.point.x, q.point.y),
        None => (position.x, position.y),
    }
}

fn objectives(w: &World) -> Vec<ObjectiveView> {
    let Some(set) = w.mission.objectives.iter().find(|s| s.team == w.teams.home) else {
        return Vec::new();
    };
    set.entries
        .iter()
        .map(|e| {
            let text = |literal: &str, use_res: bool, id: u32| {
                if use_res {
                    ObjectiveText::Resource(id)
                } else {
                    ObjectiveText::Literal(literal.to_string())
                }
            };
            ObjectiveView {
                title: text(
                    &e.def.title,
                    e.def.title_use_resource_string,
                    e.def.title_resource_string_id,
                ),
                description: text(
                    &e.def.description,
                    e.def.description_use_resource_string,
                    e.def.description_resource_string_id,
                ),
                status: match e.runtime.status {
                    mc2_sim::mission::objective::ObjectiveStatus::Successful => {
                        ObjectiveStatus::Successful
                    }
                    mc2_sim::mission::objective::ObjectiveStatus::Failed => ObjectiveStatus::Failed,
                    mc2_sim::mission::objective::ObjectiveStatus::Undetermined => {
                        ObjectiveStatus::Undetermined
                    }
                },
                priority: e.def.priority,
                resource_points: e.def.resource_points,
                is_active: e.runtime.active,
                is_hidden_trigger: e.def.is_hidden_trigger,
                activate_on_flag: e.def.activate_on_flag,
                activation_time: f64::from(e.def.activation_time),
            }
        })
        .collect()
}

/// Whether `ControlGui::render` starts its in-mission results presentation
/// (`controlgui.cpp:430-436`).  The status panel is deliberately independent of mission
/// termination: the original also opens it as soon as the home objectives have decided, even in
/// a `neverEndingStory` mission.
fn results_started(w: &World) -> bool {
    if w.mission.term.counter_started {
        return true;
    }
    // The start decision is `CObjectives::Status()` itself (controlgui.cpp:430), which honours
    // reset flags and activation; a cached per-objective status would disagree with it.
    let has_entries = w
        .mission
        .objectives
        .iter()
        .find(|s| s.team == w.teams.home)
        .is_some_and(|set| !set.entries.is_empty());
    has_entries
        && home_status(w) != Some(mc2_sim::mission::objective::ObjectiveStatus::Undetermined)
}

/// `Team::home->objectives.Status() == OS_SUCCESSFUL`, the condition for `IDS_MISSION_SUCCESS`
/// on the results status frame (`controlgui.cpp:565-569`). `ControlGui::render` evaluates the
/// home objectives every frame, including before the results begin (`controlgui.cpp:426-427`),
/// but the snapshot is read-only: the evaluation runs through `ObjectiveWorld` on a borrowed copy
/// whose sounds and objective actions are dropped, so nothing is committed twice and the sim's own
/// evaluation (which the mission's termination follows) stays the only one.
fn home_successful(w: &World) -> bool {
    // No objective set is `CObjectives::Status() == OS_SUCCESSFUL`.
    home_status(w).is_none_or(|s| s == mc2_sim::mission::objective::ObjectiveStatus::Successful)
}

/// `Team::home->objectives.Status()` on the read-only snapshot described at [`home_successful`];
/// `None` when the home team has no objective set.
fn home_status(w: &World) -> Option<mc2_sim::mission::objective::ObjectiveStatus> {
    use mc2_sim::mission::objective_eval::{self, ObjectiveWorld};
    use mc2_sim_data::mission::{Action, ObjectRef, TeamObjectives};

    struct ReadOnly<'a>(&'a World);
    impl ObjectiveWorld for ReadOnly<'_> {
        fn num_movers(&self) -> usize {
            self.0.num_movers()
        }
        fn mover(&self, index: usize) -> objective_eval::MoverView {
            self.0.mover(index)
        }
        fn mover_by_ref(&self, reference: &ObjectRef) -> Option<objective_eval::MoverView> {
            self.0.mover_by_ref(reference)
        }
        fn building_by_ref(&self, reference: &ObjectRef) -> Option<objective_eval::BuildingView> {
            self.0.building_by_ref(reference)
        }
        fn home_commander_id(&self) -> i32 {
            self.0.home_commander_id()
        }
        fn actual_time(&self) -> f32 {
            self.0.actual_time()
        }
        fn time_limit(&self) -> f32 {
            self.0.time_limit()
        }
        fn max_extract_unit_distance(&self) -> f32 {
            self.0.max_extract_unit_distance()
        }
        fn play_betty(&mut self, _sample: objective_eval::Betty) {}
        fn show_objectives(&mut self) {}
        fn execute_action(&mut self, _team: i32, _action: &Action) {}
    }

    let set = w.mission.objectives.get(w.teams.home)?;
    let mut model = TeamObjectives {
        team: set.team as u32,
        boolean_flags: set.boolean_flags.clone(),
        objectives: mc2_sim::mission::objective::team_model_objectives(&set.entries),
    };
    Some(objective_eval::team_status(&mut ReadOnly(w), &mut model))
}

/// Reads the HUD's inputs off `w`; `boxes` are the selection boxes of the last `recalcBounds` pass.
pub fn snapshot(w: &World, boxes: &PickBoxes) -> HudSnapshot {
    let mut units = Vec::new();
    let mut contacts = Vec::new();
    let home_successful = home_successful(w);
    let mut info_units = std::collections::BTreeMap::new();
    for &h in &w.objects.movers {
        let Some(obj) = w.objects.get(h) else {
            continue;
        };
        if !obj.exists() {
            continue;
        }
        let (mover_gui, path_locks, commander) = match &obj.kind {
            Kind::Mech(m) => (m.m.flags.on_gui, m.m.path_locks_enabled, m.m.commander.0),
            Kind::Vehicle(v) => (v.m.flags.on_gui, v.m.path_locks_enabled, v.m.commander.0),
            _ => continue,
        };
        if i32::from(commander) == HOME_COMMANDER && mover_gui {
            if let Some(mut u) = unit(w, h, obj) {
                u.screen_box = boxes.movers.get(&u.part_id).copied();
                // Current visibility only: the entry must be this frame's `recalcBounds` turn, not a stale one.
                u.in_view = boxes
                    .mover_drawn_turn
                    .get(&u.part_id)
                    .is_some_and(|&t| t >= w.clock.turn as i32 - 1);
                units.push(u);
            }
            continue;
        }
        if obj.c.is_destroyed() {
            continue;
        }
        let own_team = obj.team_id() == w.teams.home as i32;
        if own_team
            || obj.c.is_disabled()
            || mc2_sim::ai::targeting::contact_status(w, h, w.teams.home as i32)
                == mc2_sim::ai::sensors::CONTACT_VISUAL
        {
            if let Some(u) = unit(w, h, obj) {
                info_units.insert(u.part_id, u);
            }
        }
        let in_contact = w
            .sensors
            .team_contact_status
            .get(&h)
            .is_some_and(|s| s.get(w.teams.home).copied().unwrap_or(0) > 0);
        // `GameTacMap::render` (gametacmap.cpp:366-401).  The port has no debug `ShowMovers`
        // mode, so foreign movers require the same contact, shutdown, null-signature and ECM
        // gates the shipped player HUD uses.
        let shown = if own_team {
            if i32::from(commander) == HOME_COMMANDER {
                mover_gui
            } else {
                mover_gui && path_locks && w.terrain.is_game_select_position(&obj.c.position)
            }
        } else {
            in_contact
                && obj.c.status != ObjectStatus::ShutDown
                && !is_null_signature(w, obj)
                && !has_ecm(obj)
        };
        if shown {
            contacts.push(TacUnit {
                team: if i32::from(commander) == HOME_COMMANDER {
                    TacTeam::Ours
                } else {
                    relation(w, obj.team_id())
                },
                artillery: false,
                sensor_entry: false,
                pos: (obj.c.position.x, obj.c.position.y),
                selected: false,
                sensor_range_m: if obj.c.status == ObjectStatus::ShutDown {
                    0.0
                } else {
                    sensor_range(w, h)
                },
            });
        }
    }
    // Non-mover sensor owners are a separate first pass in `GameTacMap::render`
    // (gametacmap.cpp:282-350).  They are not contacts: their own active sensor supplies the
    // blip and ring, before movers are drawn.
    for sensor in w
        .sensors
        .teams
        .iter()
        .flatten()
        .filter_map(|&id| w.sensors.pool.get(id))
    {
        let Some(owner) = w.objects.get(sensor.owner) else {
            continue;
        };
        if owner.is_mover()
            || !owner.exists()
            || owner.c.is_disabled()
            || owner.c.is_destroyed()
            || owner.c.status == ObjectStatus::ShutDown
            || sensor.range < 1.1
            || sensor.broken
            || owner.team_id() < 0
        {
            continue;
        }
        contacts.push(TacUnit {
            team: relation(w, owner.team_id()),
            artillery: owner.c.class == mc2_sim::object::ObjectClass::Artillery,
            sensor_entry: true,
            pos: (owner.c.position.x, owner.c.position.y),
            selected: false,
            sensor_range_m: sensor.range,
        });
    }
    HudSnapshot {
        units,
        contacts,
        picks: picks(w, boxes),
        info_units,
        objectives: objectives(w),
        tac_objectives: tac_objectives(w),
        tac_objective_total: w
            .mission
            .objectives
            .iter()
            .find(|s| s.team == w.teams.home)
            .map_or(0, |s| s.entries.len()),
        next_nav_marker: next_nav_marker(w),
        mission_time: w.mission.actual_time,
        time_limit: w.mission.time_limit,
        vertices_per_side: w.terrain.vertices_per_side,
        resource_points: w.mission.resource_points,
        vtol_in_flight: w.vtol.in_flight(mc2_sim::combat::HOME_COMMANDER),
        pilots_remaining: w
            .recovery_pilots
            .as_ref()
            .is_some_and(|r| r.has_pilot(&|file| mc2_sim::mission::trade::warrior_in_use(w, file))),
        result: w.mission.term.scenario_result,
        results_started: results_started(w),
        never_ending_story: w.mission.term.never_ending_story,
        home_successful,
        invulnerable: w.mission.invulnerable,
    }
}

/// The objectives `RenderMarkers` draws a numeral for: neither resolved nor hidden (`m_resolved || !m_displayMarker ||
/// !m_isActive` draws nothing), at `(m_markerX, m_markerY)` with the text `CObjective::Read` formatted.
fn tac_objectives(w: &World) -> Vec<mc2_ui::tacmap::TacObjective> {
    let Some(set) = w.mission.objectives.iter().find(|s| s.team == w.teams.home) else {
        return Vec::new();
    };
    set.entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.def.display_marker && !e.runtime.resolved && e.runtime.active)
        .filter(|(_, e)| !e.def.marker_text.is_empty())
        .map(|(index, e)| mc2_ui::tacmap::TacObjective {
            index,
            pos: (e.def.marker_x, e.def.marker_y),
            text: e.def.marker_text.clone(),
        })
        .collect()
}

fn next_nav_marker(w: &World) -> Option<mc2_sim::math::Vec3> {
    let home = w.teams.home;
    let set = w.mission.objectives.iter().find(|set| set.team == home)?;
    set.entries.iter().find_map(|entry| {
        if !entry.def.display_marker
            || entry.runtime.status != mc2_sim::mission::objective::ObjectiveStatus::Undetermined
            || (entry.def.marker_x == 0.0 && entry.def.marker_y == 0.0)
        {
            return None;
        }
        let mut p = mc2_sim::math::Vec3 {
            x: entry.def.marker_x,
            y: entry.def.marker_y,
            z: 0.0,
        };
        p.z = w.terrain.elevation(&p);
        Some(p)
    })
}

/// The text of the mission messages (`DisplayTextMessage` and its resource form) in `events`. Radio
/// events are audio (`Radio::playMessage`, `radio.cpp:120-289`), not chat: the C++ chat is written only
/// by these two actions and the multiplayer/tutorial callers (`controlgui.cpp:3017`).
pub fn messages(events: &[mc2_sim::SimEvent]) -> Vec<HudMessage> {
    use mc2_sim_data::mission::Action;
    events
        .iter()
        .filter_map(|e| match e {
            mc2_sim::SimEvent::ObjectiveAction { action, .. } => match action {
                Action::DisplayTextMessage { message } => Some(HudMessage::Text(message.clone())),
                Action::DisplayResourceTextMessage { resource_string_id } => {
                    Some(HudMessage::Resource(*resource_string_id))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Whether the home team has `h` in contact (`SensorManager`'s team contact status).
fn home_contact(w: &World, h: mc2_sim::ObjectHandle) -> bool {
    w.sensors
        .team_contact_status
        .get(&h)
        .is_some_and(|s| s.get(w.teams.home).copied().unwrap_or(0) > 0)
}

/// What `findObjectByMouse` may return for a click (`missiongui.cpp:1215-1250`): a mover of another
/// commander only when the home team has it in sensor contact, a building or turret of a team
/// whatever the sensors say; nothing destroyed or (for a structure) disabled.  In order movers,
/// buildings, turrets, then every other object (gates, terrain objects, special buildings) by
/// handle, trees excluded (objmgr.cpp:2263).  An object without a watch id is left out; [`ensure_pick_watch_ids`] hands them out.
fn picks(w: &World, boxes: &PickBoxes) -> Vec<PickTarget> {
    // `findObjectByMouse` walks `objList` in handle order for everything that is not a mover.
    let mut others: Vec<mc2_sim::ObjectHandle> = [
        &w.objects.terrain_objects,
        &w.objects.buildings,
        &w.objects.turrets,
        &w.objects.gates,
        &w.objects.special_buildings,
    ]
    .into_iter()
    .flatten()
    .copied()
    .collect();
    others.sort_by_key(|h| h.0);
    others.dedup();
    let mut out: Vec<(u8, PickTarget)> = Vec::new();
    for &h in w.objects.movers.iter().chain(&others) {
        let Some(obj) = w.objects.get(h) else {
            continue;
        };
        let commander = match &obj.kind {
            Kind::Mech(m) => i32::from(m.m.commander.0),
            Kind::Vehicle(v) => i32::from(v.m.commander.0),
            Kind::Building(_) | Kind::Turret(_) | Kind::Gate(_) | Kind::Terrain(_) => -1,
            _ => continue,
        };
        // `obj->getObjectClass() != TREE` (objmgr.cpp:2263).
        if obj.c.class == mc2_sim::object::ObjectClass::Tree {
            continue;
        }
        let mover = commander >= 0;
        // Rock clumps carry damage level 36000000 (objmgr.cpp:2199); a mover must be on the GUI and
        // on selectable terrain (objmgr.cpp:2263, `Terrain::IsGameSelectTerrainPosition`).
        if !mover && mc2_sim::object::structure::damage_level(w, h) == 36_000_000.0 {
            continue;
        }
        if mover {
            let on_gui = match &obj.kind {
                Kind::Mech(m) => m.m.flags.on_gui,
                Kind::Vehicle(v) => v.m.flags.on_gui,
                _ => false,
            };
            let limit =
                w.terrain.vertices_per_side as f32 * mc2_sim::terrain::WORLD_UNITS_PER_VERTEX / 2.0
                    - mc2_sim::terrain::WORLD_UNITS_PER_VERTEX * 2.0;
            let p = obj.c.position;
            if !on_gui || !(p.x > -limit && p.x < limit && p.y > -limit && p.y < limit) {
                continue;
            }
        }
        if !obj.exists() || obj.c.is_destroyed() || (!mover && obj.c.is_disabled()) {
            continue;
        }
        // `findObjectByMouse` (objmgr.cpp:2178): only an object drawn last turn (`windowsVisible ==
        // turn - VISIBLE_THRESHOLD`; this snapshot is read after the turn advanced, so this turn also counts).
        let drawn = if mover {
            boxes.mover_drawn_turn.get(&obj.c.part_id.0).copied()
        } else {
            Some(obj.c.windows_visible)
        };
        if drawn.is_none_or(|t| t < w.clock.turn as i32 - 1) {
            continue;
        }
        // `objAppearance->canBeSeen()` (objmgr.cpp:2172): the appearance is in view now.
        if !w.appearance.in_view(h) {
            continue;
        }
        // `!target->isSelectable()` clears a live non-mover click target (missiongui.cpp:1245-1248);
        // a building is selectable unless its type is `SpinMe` (bdactor.h:346-349).
        if !mover {
            if let Some(mc2_sim::appearance::Appearance::Building(b)) = w.appearance.get(h) {
                if w.appearance
                    .building_types
                    .get(b.type_index)
                    .is_some_and(|t| t.spin_me)
                {
                    continue;
                }
            }
        }
        // `upperLeft`/`lowerRight` (objmgr.cpp:2178-2183): no box means `recalcBounds` never projected it.
        let (screen_box, triangles) = if mover {
            (
                boxes.movers.get(&obj.c.part_id.0),
                boxes.mover_triangles.get(&obj.c.part_id.0),
            )
        } else {
            (
                boxes.structures.get(&h.0),
                boxes.structure_triangles.get(&h.0),
            )
        };
        // No triangles: the shape was not transformed last turn, so `PerPolySelect` is false.
        let Some(&screen_box) = screen_box else {
            continue;
        };
        // A structure needs its faces; a mover is picked on its box alone (objmgr.cpp:2253).
        let triangles = match triangles {
            Some(t) => t.clone(),
            None if mover => ScreenTriangles::default(),
            None => continue,
        };
        let watch = w.objects.watch_id_peek(h);
        // A disabled mover needs no contact (`!target->isDisabled() && conStat < CONTACT_SENSOR_QUALITY_1`, missiongui.cpp:1223-1227):
        // a shot-down 'Mech stays clickable for the recovery craft.
        if watch.is_none()
            || (mover && commander != HOME_COMMANDER && !obj.c.is_disabled() && !home_contact(w, h))
        {
            continue;
        }
        // `findObjectByMouse` (objmgr.cpp:2347-2360) searches the movers whose commander is NOT home
        // first (`findMoverByMouse` skips `commanderId`, objmgr.cpp:2278; live before disabled), then
        // all movers (live, then disabled), then buildings and turrets.
        let rank = match (mover, commander == HOME_COMMANDER, obj.c.is_disabled()) {
            (true, false, false) => 0,
            (true, false, true) => 1,
            (true, _, false) => 2,
            (true, _, true) => 3,
            (false, ..) => 4,
        };
        out.push((
            rank,
            PickTarget {
                watch_id: watch.0,
                part_id: mover.then_some(obj.c.part_id.0),
                position: (obj.c.position.x, obj.c.position.y, obj.c.position.z),
                screen_box,
                triangles,
                mover,
                team: relation(w, obj.team_id()),
                disabled: obj.c.is_disabled(),
                capture_blockers: std::collections::BTreeMap::new(),
                // `isCaptureable(Team::home)` per class: buildings, ground vehicles (salvage-less,
                // missiongui.cpp:1810 `doAttack`) and the BattleMech override.
                captureable: mc2_sim::ai::capture::is_captureable(w, h, w.teams.home as i32),
                class: cursor_class(obj.c.class),
                landbridge: is_landbridge(w, obj),
                support: support_target(w, h, obj, mover),
                aim: aim_facts(obj),
                info_unit: mover.then(|| unit(w, h, obj)).flatten(),
            },
        ));
    }
    out.sort_by_key(|(rank, _)| *rank);
    out.into_iter().map(|(_, p)| p).collect()
}

/// The support facts of a pickable object (`canRepair`, `canSalvage`, the repair bay branch: missiongui.cpp:4461-4570).
fn support_target(w: &World, h: mc2_sim::ObjectHandle, obj: &Obj, mover: bool) -> SupportTarget {
    let team = relation(w, obj.team_id());
    let home_commanded = is_home_mover(w, h);
    let buddy = match &obj.kind {
        Kind::Mech(m) => m.m.refit_buddy,
        Kind::Vehicle(v) => v.m.refit_buddy,
        _ => mc2_sim::ids::WatchId::NONE,
    };
    // `Team::teamLineOfSight` needs the mutable world; a home mover is seen, anything else by a visual contact
    let visible = home_commanded
        || w.sensors.team_contact_status.get(&h).is_some_and(|s| {
            s.get(w.teams.home).copied().unwrap_or(0) == mc2_sim::ai::sensors::CONTACT_VISUAL
        });
    SupportTarget {
        mover,
        battle_mech: matches!(obj.kind, Kind::Mech(_)),
        enemy: team == TacTeam::Enemy,
        disabled: obj.c.is_disabled(),
        destroyed: obj.c.is_destroyed(),
        needs_refit: mover && mc2_sim::mover::refit::needs_refit(w, h),
        refit_buddy: w
            .objects
            .by_watch_id(buddy)
            .and_then(|b| w.objects.get(b))
            .map(|b| b.c.part_id),
        visible,
        mech_bay: matches!(obj.kind, Kind::Building(_))
            && matches!(team, TacTeam::PlayerAlt | TacTeam::Ours)
            && obj.c.flag(mc2_sim::object::flags::CANREFIT)
            && obj.c.flag(mc2_sim::object::flags::MECHBAY)
            && mc2_sim::mover::refit::building_refit_points(w, h) > 0.0,
    }
}

fn is_home_mover(w: &World, h: mc2_sim::ObjectHandle) -> bool {
    match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => i32::from(m.m.commander.0) == HOME_COMMANDER,
        Some(Kind::Vehicle(v)) => i32::from(v.m.commander.0) == HOME_COMMANDER,
        _ => false,
    }
}

/// Gives every object [`picks`] may list its `watchID` (`GameObject::getWatchID` hands them out on
/// first request in the C++, which the GUI's pick does).  Identity only: no digest reads them.
pub fn ensure_pick_watch_ids(w: &mut World) {
    let handles: Vec<_> = w
        .objects
        .movers
        .iter()
        .copied()
        .filter(|&h| home_contact(w, h) || is_home_mover(w, h))
        .chain(
            w.objects
                .buildings
                .iter()
                .chain(&w.objects.turrets)
                .chain(&w.objects.gates)
                .chain(&w.objects.special_buildings)
                .chain(&w.objects.terrain_objects)
                .copied()
                .filter(|&h| {
                    w.objects
                        .get(h)
                        .is_some_and(|o| o.c.class != mc2_sim::object::ObjectClass::Tree)
                }),
        )
        .collect();
    for h in handles {
        w.objects.watch_id(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full armour of a mech: head 9, centre torso 20 (rear 10), side torsos 15 (rear 5), arms 10, legs 12.
    fn full() -> Vec<(f32, u8)> {
        vec![
            (9.0, 9),
            (20.0, 20),
            (15.0, 15),
            (15.0, 15),
            (10.0, 10),
            (10.0, 10),
            (12.0, 12),
            (12.0, 12),
            (10.0, 10),
            (5.0, 5),
            (5.0, 5),
        ]
    }

    #[test]
    fn an_undamaged_unit_fills_the_bar() {
        assert_eq!(bar_fill(true, &full(), (3.0, 3.0), 0.0, false, false), 1.0);
        assert_eq!(bar_fill(true, &full(), (3.0, 3.0), 0.0, true, false), 0.0);
    }

    #[test]
    fn leg_damage_lowers_the_bar_not_arm_damage_twice() {
        let base = bar_fill(true, &full(), (1.0, 1.0), 0.0, false, false);
        let mut legs = full();
        legs[6].0 = 0.0;
        legs[7].0 = 0.0;
        let mut arms = full();
        arms[4].0 = 0.0;
        arms[5].0 = 0.0;
        let leg_fill = bar_fill(true, &legs, (1.0, 1.0), 0.0, false, false);
        let arm_fill = bar_fill(true, &arms, (1.0, 1.0), 0.0, false, false);
        // both legs gone: the leg term is 0.6; both arms gone: the arm term is 0.75
        assert!((leg_fill - base * 0.6).abs() < 1e-6, "{leg_fill}");
        assert!((arm_fill - base * 0.75).abs() < 1e-6, "{arm_fill}");
    }

    #[test]
    fn wounds_and_weapon_loss_scale_the_bar() {
        let f = |wounds, weapon| bar_fill(true, &full(), (weapon, 4.0), wounds, false, false);
        assert_eq!(f(0.0, 4.0), 1.0);
        assert_eq!(f(1.0, 4.0), 0.95);
        assert_eq!(f(6.0, 4.0), 0.0);
        assert_eq!(f(0.0, 2.0), 0.5);
    }

    #[test]
    fn a_vehicle_without_turret_armour_is_not_nan() {
        let armor = vec![(10.0, 10), (10.0, 10), (10.0, 10), (0.0, 0), (0.0, 0)];
        let fill = bar_fill(false, &armor, (0.0, 0.0), 0.0, false, false);
        assert!(fill.is_finite() && fill > 0.0, "{fill}");
    }

    #[test]
    fn a_refit_or_mine_layer_vehicle_also_scales_by_its_turret_points() {
        // turret points at half; C++ (gvehicl.cpp:5404-5455): 0.5 (extraEffect) * (0.5*0.6+0.4)
        let armor = vec![(10.0, 10), (10.0, 10), (10.0, 10), (10.0, 10), (5.0, 10)];
        let plain = bar_fill(false, &armor, (0.0, 0.0), 0.0, false, false);
        let refit = bar_fill(false, &armor, (0.0, 0.0), 0.0, false, true);
        assert!((plain - 0.7).abs() < 1e-6, "{plain}");
        assert!((refit - 0.35).abs() < 1e-6, "{refit}");
        // no turret points at all: the term is 1
        let none = vec![(10.0, 10), (10.0, 10), (10.0, 10), (10.0, 10), (0.0, 0)];
        assert_eq!(bar_fill(false, &none, (0.0, 0.0), 0.0, false, true), 1.0);
    }

    /// One world with one active, successful primary objective for the home team (team 0).
    fn world_with_one_primary() -> World {
        use mc2_sim::mission::objective::{
            ObjectiveEntry, ObjectiveRuntime, ObjectiveStatus as SimObjectiveStatus,
            TeamObjectiveSet,
        };
        use mc2_sim_data::mission::{Objective, ObjectiveModel, ObjectiveOrigin};

        let mut world = World::new(mc2_sim::rng::RngMode::LegacyMsvc, 1);
        world.teams.home = 0;
        let entry = ObjectiveEntry {
            def: Objective {
                origin: ObjectiveOrigin::Block { index: 0 },
                title: String::new(),
                title_use_resource_string: false,
                title_resource_string_id: 0,
                description: String::new(),
                description_use_resource_string: false,
                marker_text: String::new(),
                description_resource_string_id: 0,
                priority: 1,
                resource_points: 0,
                previous_primary_objective_must_be_complete: false,
                all_previous_primary_objectives_must_be_complete: false,
                display_marker: false,
                marker_x: 0.0,
                marker_y: 0.0,
                is_hidden_trigger: false,
                activate_on_flag: false,
                activate_flag_id: String::new(),
                reset_status_on_flag: false,
                reset_status_flag_id: String::new(),
                model: ObjectiveModel::default(),
                is_active: true,
                activation_time: -1.0,
                resolved: false,
                resolved_status: 0,
                changed_status: false,
                conditions: Vec::new(),
                actions: Vec::new(),
                failure_conditions: Vec::new(),
                failure_actions: Vec::new(),
            },
            conditions: Vec::new(),
            actions: Vec::new(),
            failure_conditions: Vec::new(),
            failure_actions: Vec::new(),
            runtime: ObjectiveRuntime {
                active: true,
                resolved: true,
                status: SimObjectiveStatus::Successful,
                changed: false,
            },
        };
        world.mission.objectives.push(TeamObjectiveSet {
            team: 0,
            boolean_flags: Vec::new(),
            entries: vec![entry],
            num_primary: 1,
        });
        world
    }

    /// `CObjective::RenderMarkers` (objective.cpp:2259-2263): a resolved, hidden or inactive
    /// objective, or one without `DisplayMarker`, draws no numeral.
    #[test]
    fn tac_objectives_list_only_unresolved_active_shown_markers() {
        let mut world = world_with_one_primary();
        let e = &mut world.mission.objectives[0].entries[0];
        e.def.display_marker = true;
        e.def.marker_text = "1".to_string();
        e.def.marker_x = 12.0;
        e.def.marker_y = -34.0;
        assert!(tac_objectives(&world).is_empty(), "resolved");
        world.mission.objectives[0].entries[0].runtime.resolved = false;
        let list = tac_objectives(&world);
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].pos, list[0].text.as_str()), ((12.0, -34.0), "1"));
        assert_eq!(list[0].index, 0);
        world.mission.objectives[0].entries[0].runtime.active = false;
        assert!(tac_objectives(&world).is_empty(), "inactive");
        world.mission.objectives[0].entries[0].runtime.active = true;
        world.mission.objectives[0].entries[0].def.display_marker = false;
        assert!(tac_objectives(&world).is_empty(), "no DisplayMarker");
    }

    #[test]
    fn decided_objectives_start_results_without_the_termination_counter() {
        use mc2_sim::mission::objective::ObjectiveStatus as SimObjectiveStatus;

        let mut world = world_with_one_primary();
        assert!(results_started(&world));

        world.mission.objectives[0].entries[0].runtime.status = SimObjectiveStatus::Undetermined;
        assert!(!results_started(&world));
        world.mission.objectives[0].entries[0].runtime.active = false;
        assert!(results_started(&world));
        world.mission.objectives[0].entries[0].def.activate_on_flag = true;
        assert!(!results_started(&world));
        world.mission.objectives[0].entries[0].runtime.active = true;
        world.mission.objectives[0].entries[0].def.activate_on_flag = false;
        let mut failed = world.mission.objectives[0].entries[0].clone();
        failed.runtime.status = SimObjectiveStatus::Failed;
        world.mission.objectives[0].entries.push(failed);
        world.mission.objectives[0].num_primary = 2;
        assert!(results_started(&world));
        world.mission.term.counter_started = true;
        assert!(results_started(&world));
    }

    /// `renderResults` prints `IDS_MISSION_SUCCESS` only for `objectives.Status() ==
    /// OS_SUCCESSFUL` and `IDS_MISSION_FAILED` for everything else, undetermined included
    /// (`controlgui.cpp:565-569`);
    /// `CObjectives::Status` is successful for no objectives, for every active primary successful,
    /// failed for a failed primary and undetermined while an activate-on-flag primary waits
    /// (`objective.cpp:2165-2255`).
    #[test]
    fn the_status_frame_text_follows_the_objective_list_status_only() {
        use mc2_sim::mission::objective::ObjectiveStatus as SimObjectiveStatus;

        let no_objectives = World::new(mc2_sim::rng::RngMode::LegacyMsvc, 1);
        assert!(home_successful(&no_objectives), "no objectives: successful");

        let mut world = world_with_one_primary();
        assert!(home_successful(&world));

        // a second primary still waiting for its flag keeps the list undetermined: the C++ says
        // MISSION FAILED although every drawn objective is successful
        let mut waiting = world.mission.objectives[0].entries[0].clone();
        waiting.runtime.active = false;
        waiting.def.activate_on_flag = true;
        waiting.def.activate_flag_id = "later".into();
        world.mission.objectives[0].entries[0].runtime.active = false;
        world.mission.objectives[0].entries.push(waiting);
        world.mission.objectives[0].num_primary = 2;
        assert!(
            !home_successful(&world),
            "a waiting primary is undetermined"
        );

        // the same list with every primary decided is a success once more
        world.mission.objectives[0].entries[0].runtime.active = true;
        world.mission.objectives[0].entries.remove(1);
        assert!(home_successful(&world));

        world.mission.objectives[0].entries[0].runtime.status = SimObjectiveStatus::Failed;
        assert!(
            !home_successful(&world),
            "a failed primary is MISSION FAILED"
        );

        world.mission.objectives[0].entries[0].runtime.status = SimObjectiveStatus::Successful;
        world.mission.term.counter_started = true;
        assert!(
            home_successful(&world),
            "termination does not change the text"
        );
    }

    /// The mission chat is `ControlGui::setChatText`, whose only gameplay callers are
    /// `CDisplayTextMessage::Execute` and `CDisplayResourceTextMessage::Execute`
    /// (objective.cpp:1233, `1269`). A pilot's radio message is `Radio::playMessage`, which only
    /// queues audio (`radio.cpp:120-289`) and never writes the chat: no shipped mission script
    /// turns a radio event into a chat line, so none is invented here.
    #[test]
    fn only_display_text_messages_reach_the_mission_chat() {
        let messages = messages(&[
            mc2_sim::SimEvent::Radio {
                pilot: 7,
                message: 16,
                under_home_command: true,
            },
            mc2_sim::SimEvent::ObjectiveAction {
                team: 0,
                action: mc2_sim_data::mission::Action::DisplayTextMessage {
                    message: "Move to the ridge".into(),
                },
            },
            mc2_sim::SimEvent::ObjectiveAction {
                team: 0,
                action: mc2_sim_data::mission::Action::DisplayResourceTextMessage {
                    resource_string_id: 44001,
                },
            },
        ]);
        assert_eq!(
            messages,
            vec![
                HudMessage::Text("Move to the ridge".into()),
                HudMessage::Resource(44001),
            ],
            "the radio sound event is audio, not chat text"
        );
    }
}
