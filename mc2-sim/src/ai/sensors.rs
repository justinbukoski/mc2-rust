// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Sensors and contacts (`SensorSystemManager`, `TeamSensorSystem`, `SensorSystem`, `ContactInfo`; contact.cpp).
//! Updates ONE team per frame (round robin); contact info lags up to `numTeams - 1` frames. `updateScan` is gated by
//! `turn >= 2`. Contact status: 0 none, 1..4 sensor quality, 5 visual.
//!
//! BLOCKED (moving-contact transition/removal parity): the mc2_02 moving oracle first powers a sensor down at frame 61 and
//! moves units beyond frame 60 through pilot orders (`mainDecisionTree`, warrior.cpp:4899-5200), which stop at
//! `ai/mod.rs:355` (`AiUnavailable::DecisionTree`). Until that pilot behaviour is ported, contact removal and quality
//! transitions of moving units are not compared; `tests/sensors_oracle.rs` replays the moving fixture to frame 60 only.

use crate::ids::{ObjectHandle, MAX_TEAMS};
use std::collections::BTreeMap;

type ContactList = Vec<(ObjectHandle, u8)>;
type ContactChange = (ContactList, Option<ObjectHandle>, Option<ObjectHandle>);

/// `CONTACT_VISUAL` (`code/dcontact.h`): the status of a contact seen by line of sight.
pub const CONTACT_VISUAL: u8 = 5;

/// `MAX_CONTACTS_PER_SENSOR` (`code/dcontact.h:16`): `SensorSystem::addContact` silently drops a contact past this many
/// (`contact.cpp:394`), so a 201st detectable mover is never published privately or to the team.
pub const MAX_CONTACTS_PER_SENSOR: usize = 200;

/// Original `MAX_SENSORS` pool size (`mclib/dcontact.h:14`).
pub const MAX_SENSORS: usize = 150;
/// One sensor system owned by a mover.
#[derive(Clone, Debug, Default)]
pub struct Sensor {
    /// Pool id. The C++ allocates the whole pool up front, then hands slots out in order.
    pub id: usize,
    /// Owning mover.
    pub owner: ObjectHandle,
    /// Team sensor-system index; -1 until the mover joins a team.
    pub team_index: i32,
    /// Component sensor range.
    pub range: f32,
    /// `broken` / `notShutdown` / LOS capability.
    pub broken: bool,
    /// Sensor inventory slot (`Mover::sensor`), used to suppress broken systems.
    pub inventory_index: usize,
    /// `hasLOSCapability` (true for standard mover sensors).
    pub has_los_capability: bool,
    /// Sensor is on (not shut down).
    pub not_shutdown: bool,
    /// Whether this sensor's private contacts are currently published to its team.
    /// Mirrors `SensorSystem::setShutdown`'s add/remove transitions.
    pub team_contacts_enabled: bool,
    /// `nextScanUpdate` / `lastScanUpdate`.
    pub next_scan_update: f32,
    /// See above.
    pub last_scan_update: f32,
    /// Insertion ordered contacts and their current status (1..4 sensor quality, 5 visual).
    pub contacts: Vec<(ObjectHandle, u8)>,
}

/// Mission scoped port of `SensorSystemManager` (`contact.cpp:1153-1200`).
#[derive(Clone, Debug)]
pub struct SensorManager {
    /// Fixed sensor pool; ids are stable for the mission.
    pub pool: Vec<Sensor>,
    /// Per-team sensor ids, in registration order.
    pub teams: [Vec<usize>; MAX_TEAMS],
    /// `teamToUpdate`.
    pub team_to_update: usize,
    /// `enemyInLOS`.
    pub enemy_in_los: bool,
    /// `ContactInfo::contactStatus[team]`, aggregated across each team's sensors.
    pub team_contact_status: BTreeMap<ObjectHandle, [u8; MAX_TEAMS]>,
    /// `TeamSensorSystem::contacts`, in first-detection order with swap-remove semantics.
    pub team_contact_order: [Vec<ObjectHandle>; MAX_TEAMS],
}

/// Team whose sensor system backs a mover's `sensorSystem` pointer.
pub(crate) fn mover_sensor_team(w: &crate::World, mover: ObjectHandle) -> Option<usize> {
    let sensor = w
        .sensors
        .pool
        .iter()
        .find(|sensor| sensor.owner == mover && sensor.team_index >= 0)?;
    w.sensors
        .teams
        .iter()
        .position(|team_sensors| team_sensors.contains(&sensor.id))
        .filter(|team| *team < MAX_TEAMS)
}

impl Default for SensorManager {
    fn default() -> Self {
        Self {
            pool: Vec::new(),
            teams: std::array::from_fn(|_| Vec::new()),
            team_to_update: 0,
            enemy_in_los: true,
            team_contact_status: BTreeMap::new(),
            team_contact_order: std::array::from_fn(|_| Vec::new()),
        }
    }
}

// Saved in full (DESIGN.md section 9): the contacts and the round-robin team index decide what the next frames see.
crate::snapshot_struct!(Sensor {
    id,
    owner,
    team_index,
    range,
    broken,
    inventory_index,
    has_los_capability,
    not_shutdown,
    team_contacts_enabled,
    next_scan_update,
    last_scan_update,
    contacts,
});

crate::snapshot_struct!(SensorManager {
    pool,
    teams,
    team_to_update,
    enemy_in_los,
    team_contact_status,
    team_contact_order,
});

/// `SensorSystemManager::update` (contact.cpp:1329).
pub fn update(w: &mut crate::World) {
    let num_teams = w.teams.teams.iter().filter(|t| t.is_some()).count();
    // `setShutdown` publishes or withdraws contacts immediately, before the
    // manager's one-team scan. Apply changed power state for every team here.
    for id in 0..w.sensors.pool.len() {
        let (team, was_enabled, is_enabled) = {
            let sensor = &w.sensors.pool[id];
            let team = w
                .sensors
                .teams
                .iter()
                .position(|team_sensors| team_sensors.contains(&id));
            (team, sensor.team_contacts_enabled, sensor.not_shutdown)
        };
        if was_enabled == is_enabled {
            continue;
        }
        w.sensors.pool[id].team_contacts_enabled = is_enabled;
        if is_enabled {
            if let Some(team) = team {
                let visual_contacts = w.sensors.pool[id]
                    .contacts
                    .iter()
                    .filter(|(_, status)| *status == 5)
                    .map(|(target, _)| *target)
                    .collect::<Vec<_>>();
                for target in visual_contacts {
                    sync_team_contact_order(w, team, &[], &[target]);
                }
            }
        } else {
            // SensorSystem::setShutdown removes nonvisual entries from its
            // private list with swap-remove, while retaining visual entries
            // and withdrawing each from the team list (contact.cpp:169-196).
            let mut index = 0;
            while index < w.sensors.pool[id].contacts.len() {
                let (target, status) = w.sensors.pool[id].contacts[index];
                if status == 5 {
                    if let Some(team) = team {
                        sync_team_contact_order(w, team, &[target], &[]);
                    }
                    index += 1;
                } else {
                    w.sensors.pool[id].contacts.swap_remove(index);
                    if let Some(team) = team {
                        sync_team_contact_order(w, team, &[target], &[]);
                    }
                }
            }
        }
        if let Some(team) = team {
            let ids = w.sensors.teams[team].clone();
            rebuild_team_contact_status(w, team, &ids);
        }
    }
    let team = w.sensors.team_to_update;
    if team >= num_teams || team >= MAX_TEAMS {
        w.sensors.team_to_update = 0;
        return;
    }
    if team == w.teams.home {
        w.sensors.enemy_in_los = false;
    }
    // `SensorSystemManager::updateSensors` advances the team every frame. The
    // turn gate lives in `SensorSystem::updateScan`, so turn 1 consumes team 0
    // without scanning and turn 2 starts with team 1 (contact.cpp:510-514,
    // 1329-1343).
    if w.clock.turn < 2 {
        advance_team(w, num_teams);
        return;
    }
    scan_team(w, team, false);
    advance_team(w, num_teams);
}

/// Force each team sensor to rescan, as `execSetRevealed` does (contact.cpp:952).
pub fn force_scan_all(w: &mut crate::World) {
    for team in 0..MAX_TEAMS {
        if w.teams.teams[team].is_some() {
            scan_team(w, team, true);
        }
    }
}

fn scan_team(w: &mut crate::World, team: usize, force: bool) {
    let ids = w.sensors.teams[team].clone();
    let targets = w.objects.movers.clone();
    for &id in &ids {
        let Some(sensor) = w.sensors.pool.get(id) else {
            continue;
        };
        // `SensorSystem::updateScan` returns before enabled checks, contact
        // mutation, or timer advancement when range is negative
        // (`contact.cpp:510-514`).
        if sensor.range < 0.0 && !force {
            continue;
        }
        let owner = sensor.owner;
        let inventory_index = sensor.inventory_index;
        let Some(owner_team) = w.objects.get(owner).map(|o| o.team_id()) else {
            continue;
        };
        let enabled = w.objects.get(owner).is_some_and(|o| {
            // `SensorSystem::enabled` checks exists/awake, disabled, and the
            // sensor inventory slot. Shutdown status alone does not disable
            // scans while the AWAKE flag remains set (contact.cpp:203-218).
            let component_enabled = match &o.kind {
                crate::object::Kind::Mech(mech) => mech
                    .loadout
                    .inventory
                    .get(inventory_index)
                    .is_some_and(|item| !item.disabled),
                crate::object::Kind::Vehicle(vehicle) => vehicle
                    .loadout
                    .inventory
                    .get(inventory_index)
                    .is_some_and(|item| !item.disabled),
                _ => true,
            };
            o.exists()
                && o.c.flags & crate::object::flags::AWAKE != 0
                && !o.c.is_disabled()
                && component_enabled
        });
        if !enabled {
            let removed = w.sensors.pool[id]
                .contacts
                .iter()
                .map(|(target, _)| *target)
                .collect::<Vec<_>>();
            w.sensors.pool[id].contacts.clear();
            sync_team_contact_order(w, team, &removed, &[]);
            continue;
        }
        // scanBattlefield visits movers in ObjectManager mover-list order and applies
        // each update/removal immediately (`contact.cpp:591-638`). Swap-removal is
        // observable, so collecting old contacts first would change the order when
        // multiple movers lose contact in one scan.
        let mut scan_results = Vec::new();
        if w.sensors.pool[id].range < 0.0 {
            w.sensors.pool[id].last_scan_update = w.clock.scenario_time;
            continue;
        }
        for target in targets.iter().copied() {
            if target == owner
                || !w.objects.get(target).is_some_and(|o| o.exists())
                || w.objects
                    .get(target)
                    .is_none_or(|o| o.team_id() == owner_team)
            {
                continue;
            }
            let status = contact_status(w, id, owner, target).unwrap_or(0);
            if status != 0
                && can_raise_enemy_alert(w, id, owner)
                && team == w.teams.home
                && target_is_enemy(w, owner_team, target)
            {
                w.sensors.enemy_in_los = true;
            }
            scan_results.push((target, status));
        }
        let (scanned, changes) =
            reconcile_contacts_in_mover_order(&w.sensors.pool[id].contacts, &scan_results);
        for (contacts, removed, added) in changes {
            w.sensors.pool[id].contacts = contacts;
            if let Some(removed) = removed {
                sync_team_contact_order(w, team, &[removed], &[]);
            }
            if let Some(added) = added {
                sync_team_contact_order(w, team, &[], &[added]);
            }
        }
        w.sensors.pool[id].contacts = scanned;
        w.sensors.pool[id].last_scan_update = w.clock.scenario_time;
        if !force {
            w.sensors.pool[id].next_scan_update += 0.5;
        }
    }
    rebuild_team_contact_status(w, team, &ids);
}

/// Apply one `scanBattlefield` pass in mover-list order, preserving immediate
/// swap-removal effects (`contact.cpp:591-638`).
fn reconcile_contacts_in_mover_order(
    contacts: &[(ObjectHandle, u8)],
    scan_results: &[(ObjectHandle, u8)],
) -> (ContactList, Vec<ContactChange>) {
    let mut scanned = contacts.to_vec();
    let mut changes = Vec::new();
    for &(target, status) in scan_results {
        if let Some(index) = scanned.iter().position(|(contact, _)| *contact == target) {
            if status == 0 {
                scanned.swap_remove(index);
                changes.push((scanned.clone(), Some(target), None));
            } else {
                scanned[index].1 = status;
            }
        } else if status != 0 && scanned.len() < MAX_CONTACTS_PER_SENSOR {
            // `SensorSystem::addContact` (contact.cpp:394): a full list drops the new contact.
            scanned.push((target, status));
            changes.push((scanned.clone(), None, Some(target)));
        }
    }
    (scanned, changes)
}

fn rebuild_team_contact_status(w: &mut crate::World, team: usize, ids: &[usize]) {
    for statuses in w.sensors.team_contact_status.values_mut() {
        statuses[team] = 0;
    }
    for id in ids {
        if let Some(sensor) = w.sensors.pool.get(*id).filter(|sensor| sensor.not_shutdown) {
            for (target, status) in &sensor.contacts {
                let entry = w
                    .sensors
                    .team_contact_status
                    .entry(*target)
                    .or_insert([0; MAX_TEAMS]);
                entry[team] = entry[team].max(*status);
            }
        }
    }
    w.sensors
        .team_contact_status
        .retain(|_, statuses| statuses.iter().any(|status| *status != 0));
}

/// Keep the aggregate contact list's insertion and swap-removal behavior while one sensor scans
/// (`contact.cpp:991-1005, 1065-1088`).
fn sync_team_contact_order(
    w: &mut crate::World,
    team: usize,
    removed: &[ObjectHandle],
    added: &[ObjectHandle],
) {
    for target in removed {
        let still_detected = w.sensors.teams[team].iter().any(|id| {
            w.sensors.pool.get(*id).is_some_and(|sensor| {
                sensor.not_shutdown && sensor.contacts.iter().any(|(contact, _)| contact == target)
            })
        });
        if !still_detected {
            if let Some(index) = w.sensors.team_contact_order[team]
                .iter()
                .position(|contact| contact == target)
            {
                w.sensors.team_contact_order[team].swap_remove(index);
            }
        }
    }
    for target in added {
        let already_listed = w.sensors.team_contact_order[team].contains(target);
        let detected = w.sensors.teams[team].iter().any(|id| {
            w.sensors.pool.get(*id).is_some_and(|sensor| {
                sensor.not_shutdown && sensor.contacts.iter().any(|(contact, _)| contact == target)
            })
        });
        if detected && !already_listed {
            w.sensors.team_contact_order[team].push(*target);
        }
    }
}

fn advance_team(w: &mut crate::World, num_teams: usize) {
    w.sensors.team_to_update += 1;
    if w.sensors.team_to_update >= num_teams {
        w.sensors.team_to_update = 0;
    }
}

fn target_is_enemy(w: &crate::World, owner_team: i32, target: ObjectHandle) -> bool {
    w.objects.get(target).is_some_and(|obj| {
        crate::object::hooks::team_relation(w, owner_team, obj.team_id())
            == Some(crate::ai::Relation::Enemy)
    })
}

/// `calcContactStatus` returns early before its enemy alert assignment for a
/// shut-down sensor, zero range, invalid team/range, broken system, or missing
/// mover sensor (`contact.cpp:273-290, 343-348`).
fn can_raise_enemy_alert(w: &crate::World, sensor_id: usize, owner: ObjectHandle) -> bool {
    let Some(sensor) = w.sensors.pool.get(sensor_id) else {
        return false;
    };
    if !sensor.not_shutdown
        || (sensor.range == 0.0 && !sensor.broken)
        || sensor.team_index < 0
        || sensor.range < 0.0
        || sensor.broken
    {
        return false;
    }
    w.objects.get(owner).is_some_and(|obj| match &obj.kind {
        crate::object::Kind::Mech(mech) => mech
            .loadout
            .inventory
            .get(sensor.inventory_index)
            .is_some_and(|item| !item.disabled),
        crate::object::Kind::Vehicle(vehicle) => vehicle
            .loadout
            .inventory
            .get(sensor.inventory_index)
            .is_some_and(|item| !item.disabled),
        _ => true,
    })
}

fn has_assigned_ecm(loadout: &crate::combat::Loadout) -> bool {
    loadout
        .critical
        .get(crate::mission::loadout::crit::ECM)
        .is_some_and(|index| *index != crate::mission::loadout::NO_COMPONENT)
}

fn contact_status(
    w: &mut crate::World,
    sensor_id: usize,
    owner: ObjectHandle,
    target: ObjectHandle,
) -> Option<u8> {
    let (not_shutdown, range, broken, team_index, inventory_index, has_los) = {
        let sensor = w.sensors.pool.get(sensor_id)?;
        (
            sensor.not_shutdown,
            sensor.range,
            sensor.broken,
            sensor.team_index,
            sensor.inventory_index,
            sensor.has_los_capability,
        )
    };
    let (owner_pos, target_pos, target_flags, target_disabled, target_shutdown, owner_is_mover) = {
        let owner_obj = w.objects.get(owner)?;
        let target_obj = w.objects.get(target)?;
        (
            owner_obj.c.position,
            target_obj.c.position,
            target_obj.c.flags,
            target_obj.c.is_disabled(),
            target_obj.c.status == crate::object::ObjectStatus::ShutDown,
            owner_obj.is_mover(),
        )
    };
    if !w.terrain.is_game_select_position(&target_pos)
        || target_flags & crate::object::flags::REMOVED != 0
    {
        return None;
    }
    if !not_shutdown || (range == 0.0 && !broken) {
        // `owner->lineOfSight(mover) && !mover->isDisabled()` (contact.cpp:269-273).
        let visual = crate::object::line_of_sight(w, owner, target, 0.0, false) && !target_disabled;
        return Some(if visual { CONTACT_VISUAL } else { 0 });
    }
    let owner_sensor_broken = owner_is_mover
        && w.objects.get(owner).is_some_and(|obj| match &obj.kind {
            crate::object::Kind::Mech(m) => m
                .loadout
                .inventory
                .get(inventory_index)
                .is_none_or(|item| item.disabled),
            crate::object::Kind::Vehicle(v) => v
                .loadout
                .inventory
                .get(inventory_index)
                .is_none_or(|item| item.disabled),
            _ => true,
        });
    if team_index < 0 || range < 0.0 || broken || owner_sensor_broken {
        return Some(0);
    }
    let dx = owner_pos.x - target_pos.x;
    let dy = owner_pos.y - target_pos.y;
    let distance = (dx * dx + dy * dy).sqrt() * crate::terrain::METERS_PER_WORLD_UNIT;
    let sensor_target = target_flags & crate::object::flags::SENSOR != 0;
    let target_has_ecm = w.objects.get(target).is_some_and(|obj| match &obj.kind {
        crate::object::Kind::Mech(m) => has_assigned_ecm(&m.loadout),
        crate::object::Kind::Vehicle(v) => has_assigned_ecm(&v.loadout),
        _ => false,
    });
    let target_null_signature = w.objects.get(target).is_some_and(|obj| match &obj.kind {
        crate::object::Kind::Vehicle(_) => w
            .data
            .vehicle_objects
            .get(&(obj.c.type_handle as u32))
            .is_some_and(|vehicle| !vehicle.is_sensor_contact),
        _ => false,
    });
    let hidden = target_shutdown || target_has_ecm || target_null_signature;
    if !sensor_target || target_disabled {
        return Some(0);
    }
    let status = if !hidden && distance <= range {
        sensor_quality(w, owner)
    } else {
        0
    };
    if has_los {
        // `hasLOSCapability && owner->lineOfSight(mover, startRadius)` (contact.cpp:309-333) runs in all three sub-branches (in sensor
        // range, out of range, target hidden by ECM / shutdown / null signature) and promotes the result to visual.
        // `startRadius` is the owner's appearance radius unless the owner is a mover.
        let start_radius = if owner_is_mover {
            0.0
        } else {
            crate::object::hooks::appear_radius(w, owner)
        };
        if crate::object::line_of_sight(w, owner, target, start_radius, false) {
            return Some(CONTACT_VISUAL);
        }
    }
    Some(status)
}

pub fn sensor_quality(w: &crate::World, owner: ObjectHandle) -> u8 {
    let slot = w.objects.get(owner).and_then(|obj| match &obj.kind {
        crate::object::Kind::Mech(m) => Some(m.m.pilot_slot),
        crate::object::Kind::Vehicle(v) => Some(v.m.pilot_slot),
        _ => None,
    });
    let Some(slot) = slot else { return 1 };
    if !w.pilots.get(slot).is_some_and(|pilot| pilot.specialty[6]) {
        return 1;
    }
    let rank = w
        .mission
        .pilot_loads
        .get(slot as usize)
        .and_then(|load| load.rank)
        .map_or(0, |rank| match rank {
            mc2_sim_data::pilot::WarriorRank::Green => 0,
            mc2_sim_data::pilot::WarriorRank::Regular => 1,
            mc2_sim_data::pilot::WarriorRank::Veteran => 2,
            mc2_sim_data::pilot::WarriorRank::Elite => 3,
            mc2_sim_data::pilot::WarriorRank::Ace => 4,
        });
    (rank + 1).min(4) as u8
}

/// `GameObject::lineOfSight` target visibility cutoff + `Team::lineOfSight` ray march (team.cpp:1071; forest cells count
/// against `MaxTreeLOSCellBlock`; the fog-of-war `checkVisibleBits` branch is `#if 0` and is not ported).
pub fn line_of_sight_ray(
    w: &crate::World,
    from: crate::math::Vec3,
    to: crate::math::Vec3,
    team_id: i32,
    mut ext_rad: f32,
    start_ext_rad: f32,
) -> bool {
    use crate::math::approximate_length;
    const ACCURACY_ADJUST: f32 = 1.5;
    const HALF_CELL_DIST: f32 = 128.0 / 6.0;

    if !(0..MAX_TEAMS as i32).contains(&team_id) {
        return true;
    }
    let terrain = &w.terrain;
    let (to_row, to_col) = terrain.world_to_cell(&to);
    let (from_row, from_col) = terrain.world_to_cell(&from);
    let mut end = terrain.get_cell_pos(to_row, to_col);
    let mut start = terrain.get_cell_pos(from_row, from_col);
    start.z += from.z - terrain.elevation(&from);
    end.z += to.z - terrain.elevation(&to);

    let mut delta = crate::math::Vec3 {
        x: (to_col - from_col) as f32,
        y: (to_row - from_row) as f32,
        z: 0.0,
    };
    let length = approximate_length(&delta) * ACCURACY_ADJUST;
    if length <= crate::time::STUFF_SMALL {
        return true;
    }
    let col_step = (end.x - start.x) / length;
    let row_step = (end.y - start.y) / length;
    let height_step = (end.z - start.z) / (length + ACCURACY_ADJUST);
    let mut current = start;
    current.z = terrain.elevation(&current);
    let max_iter = (length - 0.5) as i32;
    let mut max_trees = 0i32;
    let check_extent = ext_rad > crate::time::STUFF_SMALL;
    let check_start = start_ext_rad > crate::time::STUFF_SMALL;
    ext_rad += HALF_CELL_DIST;
    let mut start_height = start.z;
    for _ in 0..max_iter.max(0) {
        let outside_start = if check_start {
            delta.x = current.x - start.x;
            delta.y = current.y - start.y;
            delta.z = 0.0;
            approximate_length(&delta) > start_ext_rad
        } else {
            true
        };
        start_height += height_step;
        let (row, col) = terrain.world_to_cell(&current);
        let local =
            crate::terrain::WORLD_UNITS_PER_METER * 4.0 * terrain.get_local_height(row, col) as f32;
        let this_height = current.z + local;
        if check_extent {
            delta.x = end.x - current.x;
            delta.y = end.y - current.y;
            delta.z = end.z - current.z;
            if approximate_length(&delta) <= ext_rad {
                break;
            }
        }
        if outside_start && start_height < this_height {
            if !terrain.get_forest(row, col) {
                return false;
            }
            max_trees += 1;
            if max_trees as f32 >= w.data.gamesys.ranges[2] {
                return false;
            }
        }
        current.x += col_step;
        current.y += row_step;
        current.z = terrain.elevation(&current);
    }
    true
}

/// `mover->sensorSystem->disable()` and, for a destroyed ground vehicle, `sensorSystem->broken = true` (gvehicl.cpp:559-563,
/// mech.cpp `handleDestruction`/`destroyBodyLocation`). `disable` is `clearContacts` (contact.cpp:162, 455): `removeContact(0)` until
/// empty, each swap-filling slot 0 with the last contact, and (while not shut down) withdrawing the contact from the team list
/// (`TeamSensorSystem::removeContact`, contact.cpp:1065). A mover has one `sensorSystem`: the last sensor component's pool slot.
pub fn disable(w: &mut crate::World, owner: ObjectHandle, broken: bool) {
    let Some(id) = w
        .sensors
        .pool
        .iter()
        .rev()
        .find(|sensor| sensor.owner == owner)
        .map(|sensor| sensor.id)
    else {
        return;
    };
    clear_contacts(w, id);
    if broken {
        w.sensors.pool[id].broken = true;
    }
}

/// `SensorSystem::clearContacts` (contact.cpp:455): `removeContact(0)` until the private list is empty, each swap-filling slot 0
/// with the last contact and, while the sensor is not shut down, withdrawing the contact from its team (contact.cpp:440).
fn clear_contacts(w: &mut crate::World, id: usize) {
    let team = w
        .sensors
        .teams
        .iter()
        .position(|team_sensors| team_sensors.contains(&id));
    let not_shutdown = w.sensors.pool[id].not_shutdown;
    while !w.sensors.pool[id].contacts.is_empty() {
        let (target, _) = w.sensors.pool[id].contacts.swap_remove(0);
        if let (true, Some(team)) = (not_shutdown, team) {
            sync_team_contact_order(w, team, &[target], &[]);
        }
    }
    if let (true, Some(team)) = (not_shutdown, team) {
        let ids = w.sensors.teams[team].clone();
        rebuild_team_contact_status(w, team, &ids);
    }
}

/// `SensorSystemManager::newSensor` (contact.cpp:1221): the first sensor of the free list, i.e. the lowest-id unused pool slot
/// (`sensor->id` is its pool index). The C++ `Fatal`s when the pool is exhausted; Rust returns `None` and the caller reports it.
/// The fresh state of the slot is `SensorSystem::init` (contact.cpp:98-119): `owner = NULL`, `range = -1`, `masterIndex = -1`,
/// `broken = false`, `notShutdown = true`, `hasLOSCapability = true`, `nextScanUpdate = 0.5`, `lastScanUpdate = 0`, no contacts.
/// `setOwner` and `setRange` follow at the call sites.
pub fn new_sensor(w: &mut crate::World) -> Option<usize> {
    let id = w
        .sensors
        .pool
        .iter()
        .position(|sensor| sensor.owner == ObjectHandle::NONE)?;
    w.sensors.pool[id] = Sensor {
        id,
        owner: ObjectHandle::NONE,
        team_index: -1,
        range: -1.0,
        broken: false,
        inventory_index: usize::MAX,
        has_los_capability: true,
        not_shutdown: true,
        team_contacts_enabled: true,
        next_scan_update: 0.5,
        last_scan_update: 0.0,
        contacts: Vec::new(),
    };
    Some(id)
}

/// The slot [`new_sensor`] would hand out (the lowest-id free slot), without taking it. `None` when the pool is full.
pub fn free_sensor_slot(w: &crate::World) -> Option<usize> {
    w.sensors
        .pool
        .iter()
        .position(|sensor| sensor.owner == ObjectHandle::NONE)
}

/// `SensorSystemManager::freeSensor` (contact.cpp:1247): the sensor returns to the free list. Rust marks the slot free by clearing
/// its owner (the same test [`new_sensor`] uses); every consumer of a free slot goes through [`new_sensor`], which re-initialises
/// the whole slot, so the fields the C++ leaves set are not observable here.
pub fn free_sensor(w: &mut crate::World, id: usize) {
    if let Some(sensor) = w.sensors.pool.get_mut(id) {
        sensor.owner = ObjectHandle::NONE;
    }
}

/// `SensorSystemManager::addTeamSensor(teamId, sensor)` (contact.cpp:1291) -> `TeamSensorSystem::addSensor` (contact.cpp:722): the
/// sensor takes the next master index of the team; a team that does not exist (`teamId >= Team::numTeams`) ignores it.
pub(crate) fn add_team_sensor(w: &mut crate::World, team: i32, id: usize) {
    let num_teams = w.teams.teams.iter().filter(|t| t.is_some()).count();
    let Ok(team) = usize::try_from(team) else {
        return;
    };
    if team >= num_teams || team >= MAX_TEAMS {
        return;
    }
    w.sensors.pool[id].team_index = w.sensors.teams[team].len() as i32;
    w.sensors.teams[team].push(id);
}

/// `SensorSystemManager::removeTeamSensor(teamId, sensor)` (contact.cpp:1302) -> `TeamSensorSystem::removeSensor` (contact.cpp:733):
/// the sensor loses its contacts, and the team's last sensor takes its master index (swap removal, which changes scan order).
pub(crate) fn remove_team_sensor(w: &mut crate::World, team: i32, id: usize) {
    let Ok(team) = usize::try_from(team) else {
        return;
    };
    if team >= MAX_TEAMS || !w.sensors.teams[team].contains(&id) {
        return;
    }
    clear_contacts(w, id);
    let index = w.sensors.pool[id].team_index;
    w.sensors.pool[id].team_index = -1;
    if let Ok(index) = usize::try_from(index) {
        if index < w.sensors.teams[team].len() {
            w.sensors.teams[team].swap_remove(index);
            if let Some(&moved) = w.sensors.teams[team].get(index) {
                w.sensors.pool[moved].team_index = index as i32;
            }
        }
    }
}

/// `Building::setSensorData(team, range, setTeam)` (bldng.cpp:1166-1186): the building takes a pool sensor (`newSensor`, the first
/// free slot) with its range; a building that is not a lookout tower has no line-of-sight capability; `setTeam` adds the sensor to
/// the building's team.
pub fn set_building_sensor_data(
    w: &mut crate::World,
    building: ObjectHandle,
    range: f32,
    set_team: bool,
) {
    if range <= -1.0 {
        return;
    }
    let lookout = w
        .objects
        .get(building)
        .and_then(|o| crate::object::structure::building_info(w, o.c.type_handle))
        .is_some_and(|info| info.lookout_tower_range > 0.0);
    let existing = w.sensors.pool.iter().position(|s| s.owner == building);
    let Some(id) = existing.or_else(|| {
        w.sensors
            .pool
            .iter()
            .position(|s| s.owner == ObjectHandle::NONE)
    }) else {
        return; // "No More Free Sensors" is fatal in C++
    };
    let sensor = &mut w.sensors.pool[id];
    sensor.owner = building;
    sensor.range = range;
    sensor.team_index = -1;
    sensor.inventory_index = usize::MAX;
    if !lookout {
        sensor.has_los_capability = false;
    }
    if set_team {
        let team = w.objects.get(building).map_or(-1, |o| o.team_id());
        add_team_sensor(w, team, id);
    }
}

/// The sensor lines of `Building::setTeamId` (bldng.cpp:923-951): the building's sensor leaves its old team before the team changes
/// and joins the new one after it (when the new team exists).
pub fn building_sensor_leave_team(w: &mut crate::World, building: ObjectHandle, old_team: i32) {
    if let Some(id) = w.sensors.pool.iter().position(|s| s.owner == building) {
        remove_team_sensor(w, old_team, id);
    }
}

/// See [`building_sensor_leave_team`].
pub fn building_sensor_join_team(w: &mut crate::World, building: ObjectHandle, new_team: i32) {
    if new_team <= -1 {
        return;
    }
    if let Some(id) = w.sensors.pool.iter().position(|s| s.owner == building) {
        add_team_sensor(w, new_team, id);
    }
}

/// CROSS-LANE (lane E -> D): `SensorManager = new SensorSystemManager; SensorManager->init(true)` (mission.cpp:2094-2098,
/// contact.cpp:1153): the sensor pool and one team sensor system per created team (`Team::numTeams`). Called after the teams
/// exist and before any mover is created (movers add their sensor system to the team in `Mover::setTeamId`).
pub fn init(_w: &mut crate::World) {
    _w.sensors = SensorManager {
        pool: (0..MAX_SENSORS)
            .map(|id| Sensor {
                id,
                not_shutdown: true,
                team_contacts_enabled: true,
                inventory_index: usize::MAX,
                has_los_capability: true,
                next_scan_update: 0.5,
                ..Sensor::default()
            })
            .collect(),
        teams: std::array::from_fn(|_| Vec::new()),
        team_to_update: 0,
        enemy_in_los: true,
        team_contact_status: BTreeMap::new(),
        team_contact_order: std::array::from_fn(|_| Vec::new()),
    };
}

/// CROSS-LANE (lane E -> D): `sensorSystem = SensorManager->newSensor(); setOwner; setRange(sensor component range)` for a mover with a
/// sensor component (mech.cpp:1767, gvehicl.cpp:1075) and `SensorManager->addTeamSensor(teamId, sensorSystem)` (`Mover::setTeamId`,
/// mover.cpp:2678). Called by `createPartObject`/`addMover` right after the mover's team is set.
pub fn register_mover(w: &mut crate::World, mover: ObjectHandle) {
    let (team, sensor_components) = {
        let Some(obj) = w.objects.get(mover) else {
            return;
        };
        let team = obj.team_id();
        let inventory = match &obj.kind {
            crate::object::Kind::Mech(m) => Some(&m.loadout.inventory),
            crate::object::Kind::Vehicle(v) => Some(&v.loadout.inventory),
            _ => None,
        };
        let ids = inventory
            .into_iter()
            .flat_map(|items| items.iter().enumerate())
            .filter(|(_, item)| {
                w.data
                    .components
                    .as_ref()
                    .and_then(|list| list.get(i32::from(item.master_id)))
                    .is_some_and(|component| {
                        component.form == Some(mc2_sim_data::component::ComponentForm::Sensor)
                    })
            })
            .map(|(index, item)| (index, item.master_id))
            .collect::<Vec<_>>();
        (team, ids)
    };
    if sensor_components.is_empty() {
        return;
    }
    let mut active_sensor = None;
    for (inventory_index, master_id) in sensor_components {
        let range = w
            .data
            .components
            .as_ref()
            .and_then(|list| list.get(i32::from(master_id)))
            .and_then(|component| match &component.stats {
                mc2_sim_data::component::ComponentStats::Sensor { range } => Some(*range),
                _ => None,
            })
            .unwrap_or(-1.0);
        let Some(sensor) = w
            .sensors
            .pool
            .iter_mut()
            .find(|sensor| sensor.owner == ObjectHandle::NONE)
        else {
            return;
        };
        sensor.owner = mover;
        sensor.range = range;
        sensor.inventory_index = inventory_index;
        active_sensor = Some(sensor.id);
    }
    // The C++ mover has one `sensorSystem` pointer. Each sensor component consumes a pool slot
    // during construction and overwrites that pointer; only the final component is registered.
    if let Some(sensor_id) = active_sensor {
        if let Some(ids) = usize::try_from(team)
            .ok()
            .and_then(|team| w.sensors.teams.get_mut(team))
        {
            let sensor = &mut w.sensors.pool[sensor_id];
            sensor.team_index = ids.len() as i32;
            ids.push(sensor_id);
        }
    }
}

/// `Mover::setTeamId(team, false)` removes the mover's old team sensor and roster membership.
pub fn mover_leave_team(w: &mut crate::World, mover: ObjectHandle, old_team: i32) {
    let sensor = w
        .sensors
        .pool
        .iter()
        .find(|s| s.owner == mover && s.team_index >= 0)
        .map(|s| s.id);
    if let Some(id) = sensor {
        remove_team_sensor(w, old_team, id);
    }
}

/// CROSS-LANE (lane E -> D): `curMover->sensorSystem->scanMover(newMover)` (contact.cpp `SensorSystem::scanMover`), one call per
/// existing not-disabled mover when `Mission::addMover` finishes (mission.cpp:1349-1356).
pub fn scan_mover(w: &mut crate::World, looker: ObjectHandle, target: ObjectHandle) {
    let Some(sensor_index) = w
        .sensors
        .pool
        .iter()
        .position(|sensor| sensor.owner == looker && sensor.team_index >= 0)
    else {
        return;
    };
    let inventory_index = w.sensors.pool[sensor_index].inventory_index;
    let enabled = w.objects.get(looker).is_some_and(|owner| {
        let inventory_enabled = match &owner.kind {
            crate::object::Kind::Mech(mech) => mech
                .loadout
                .inventory
                .get(inventory_index)
                .is_some_and(|item| !item.disabled),
            crate::object::Kind::Vehicle(vehicle) => vehicle
                .loadout
                .inventory
                .get(inventory_index)
                .is_some_and(|item| !item.disabled),
            _ => true,
        };
        owner.exists()
            && owner.c.flags & crate::object::flags::AWAKE != 0
            && !owner.c.is_disabled()
            && inventory_enabled
    });
    // `scanMover` returns before changing private or team contacts when
    // `enabled()` rejects the disabled sensor inventory item (contact.cpp:627,
    // 203-218).
    if !enabled {
        return;
    }
    let Some(owner_team) = w.objects.get(looker).map(|owner| owner.team_id()) else {
        return;
    };
    // SensorSystem::scanMover only records a mover on another team. This is
    // checked before contactStatus in contact.cpp:627, so a friendly never
    // enters either the per-sensor list or the aggregate K digest.
    if target == looker
        || !w
            .objects
            .get(target)
            .is_some_and(|obj| obj.exists() && obj.team_id() != owner_team)
    {
        return;
    }
    let status = contact_status(w, sensor_index, looker, target).unwrap_or(0);
    let team = usize::try_from(owner_team)
        .ok()
        .filter(|team| *team < MAX_TEAMS);
    if status != 0
        && can_raise_enemy_alert(w, sensor_index, looker)
        && owner_team == w.teams.home as i32
        && w.objects.get(target).is_some_and(|obj| {
            crate::object::hooks::team_relation(w, owner_team, obj.team_id())
                == Some(crate::ai::Relation::Enemy)
        })
    {
        w.sensors.enemy_in_los = true;
    }
    let sensor = &mut w.sensors.pool[sensor_index];
    let mut removed = Vec::new();
    let mut added = Vec::new();
    if status == 0 {
        if sensor.contacts.iter().any(|(handle, _)| *handle == target) {
            removed.push(target);
        }
        sensor.contacts.retain(|(handle, _)| *handle != target);
    } else if let Some((_, old_status)) = sensor
        .contacts
        .iter_mut()
        .find(|(handle, _)| *handle == target)
    {
        *old_status = status;
    } else if sensor.contacts.len() < MAX_CONTACTS_PER_SENSOR {
        // `SensorSystem::addContact` (contact.cpp:394): a full list drops the new contact.
        sensor.contacts.push((target, status));
        added.push(target);
    }
    if let Some(team) = team {
        sync_team_contact_order(w, team, &removed, &added);
    }
    if let Some(team) = team {
        let mut best = 0;
        for id in w.sensors.teams[team].clone() {
            if let Some(s) = w.sensors.pool.get(id).filter(|sensor| sensor.not_shutdown) {
                for (contact, contact_status) in &s.contacts {
                    if *contact == target {
                        best = best.max(*contact_status);
                    }
                }
            }
        }
        let statuses = w
            .sensors
            .team_contact_status
            .entry(target)
            .or_insert([0; MAX_TEAMS]);
        statuses[team] = best;
        if statuses.iter().all(|s| *s == 0) {
            w.sensors.team_contact_status.remove(&target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_round_robin_advances_before_the_turn_two_scan_gate() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        world.teams.teams[0] = Some(crate::ai::team::Team::default());
        world.teams.teams[1] = Some(crate::ai::team::Team::default());
        world.clock.turn = 1;

        update(&mut world);
        assert_eq!(world.sensors.team_to_update, 1);

        world.clock.turn = 2;
        update(&mut world);
        assert_eq!(world.sensors.team_to_update, 0);
    }

    #[test]
    fn sensor_manager_init_builds_the_fixed_pool() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        init(&mut world);
        assert_eq!(world.sensors.pool.len(), MAX_SENSORS);
        assert_eq!(world.sensors.pool[0].id, 0);
        assert_eq!(world.sensors.pool[MAX_SENSORS - 1].id, MAX_SENSORS - 1);
        assert_eq!(world.sensors.pool[0].next_scan_update, 0.5);
    }

    #[test]
    fn visual_contacts_from_inactive_sensors_do_not_raise_enemy_alert() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        let owner = ObjectHandle(1);
        let mut owner_common =
            crate::object::Common::new(crate::object::ObjectClass::BattleMech, owner);
        owner_common.flags |= crate::object::flags::EXISTS | crate::object::flags::AWAKE;
        let mut owner_mech = crate::mover::MechState::default();
        owner_mech.m.team = crate::ids::TeamId(0);
        owner_mech
            .loadout
            .inventory
            .push(crate::combat::InventoryItem::default());
        world.objects.list.resize(3, None);
        world.objects.list[1] = Some(crate::object::Obj {
            c: owner_common,
            kind: crate::object::Kind::Mech(Box::new(owner_mech)),
        });
        world.sensors.pool = vec![
            Sensor {
                id: 0,
                owner,
                team_index: 0,
                inventory_index: 0,
                not_shutdown: false,
                range: 100.0,
                ..Sensor::default()
            },
            Sensor {
                id: 1,
                owner,
                team_index: 0,
                inventory_index: 0,
                not_shutdown: true,
                range: 0.0,
                ..Sensor::default()
            },
        ];
        assert!(!can_raise_enemy_alert(&world, 0, owner)); // contact.cpp:274-278
        assert!(!can_raise_enemy_alert(&world, 1, owner)); // contact.cpp:274-278
    }

    #[test]
    fn damaged_or_disabled_assigned_ecm_still_hides_sensor_contacts() {
        let mut loadout = crate::combat::Loadout::default();
        loadout.critical = vec![crate::mission::loadout::NO_COMPONENT; 7];
        loadout.critical[crate::mission::loadout::crit::ECM] = 3;
        loadout.inventory.push(crate::combat::InventoryItem {
            health: 0,
            disabled: true,
            ..crate::combat::InventoryItem::default()
        });
        assert!(has_assigned_ecm(&loadout)); // mover.cpp:3142-3147 checks slot assignment only
        loadout.critical[crate::mission::loadout::crit::ECM] =
            crate::mission::loadout::NO_COMPONENT;
        assert!(!has_assigned_ecm(&loadout));
    }

    #[test]
    fn mover_without_registered_sensor_has_no_team_contact_interface() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        let looker = ObjectHandle(1);
        world.sensors.pool.push(Sensor {
            id: 0,
            owner: ObjectHandle(2),
            team_index: 0,
            ..Sensor::default()
        });
        assert_eq!(mover_sensor_team(&world, looker), None);

        world.sensors.pool.push(Sensor {
            id: 1,
            owner: looker,
            team_index: -1,
            ..Sensor::default()
        });
        assert_eq!(mover_sensor_team(&world, looker), None);

        world.sensors.pool[1].team_index = 0;
        world.sensors.pool[1].id = 1;
        world.sensors.teams[0].push(1);
        assert_eq!(mover_sensor_team(&world, looker), Some(0));
        world.sensors.teams[0].clear();
        world.sensors.teams[1].push(1);
        world.sensors.pool[1].team_index = 0; // first sensor within team 1
        assert_eq!(mover_sensor_team(&world, looker), Some(1)); // contact.cpp:722-729
    }

    #[test]
    fn immediate_mover_scan_ignores_disabled_sensor_inventory() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        let looker = ObjectHandle(1);
        let target = ObjectHandle(2);
        world.objects.list.resize(3, None);

        let mut owner_common =
            crate::object::Common::new(crate::object::ObjectClass::BattleMech, looker);
        owner_common.flags |= crate::object::flags::EXISTS | crate::object::flags::AWAKE;
        let mut owner_mech = crate::mover::MechState::default();
        owner_mech.m.team = crate::ids::TeamId(0);
        owner_mech
            .loadout
            .inventory
            .push(crate::combat::InventoryItem {
                disabled: true,
                ..crate::combat::InventoryItem::default()
            });
        world.objects.list[1] = Some(crate::object::Obj {
            c: owner_common,
            kind: crate::object::Kind::Mech(Box::new(owner_mech)),
        });

        let mut target_common =
            crate::object::Common::new(crate::object::ObjectClass::BattleMech, target);
        target_common.flags |= crate::object::flags::EXISTS | crate::object::flags::AWAKE;
        let mut target_mech = crate::mover::MechState::default();
        target_mech.m.team = crate::ids::TeamId(1);
        world.objects.list[2] = Some(crate::object::Obj {
            c: target_common,
            kind: crate::object::Kind::Mech(Box::new(target_mech)),
        });

        world.sensors.pool.push(Sensor {
            id: 0,
            owner: looker,
            team_index: 0,
            inventory_index: 0,
            contacts: vec![(target, 1)],
            ..Sensor::default()
        });
        world.sensors.teams[0].push(0);
        world.sensors.team_contact_order[0].push(target);
        world
            .sensors
            .team_contact_status
            .insert(target, [1; MAX_TEAMS]);

        scan_mover(&mut world, looker, target);

        assert_eq!(world.sensors.pool[0].contacts, [(target, 1)]); // contact.cpp:627; disabled sensor leaves state untouched
        assert_eq!(world.sensors.team_contact_order[0], [target]);
        assert_eq!(world.sensors.team_contact_status[&target][0], 1);
    }

    #[test]
    fn aggregate_team_contacts_keep_detection_order_and_swap_remove() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        let first = ObjectHandle(30);
        let second = ObjectHandle(20);
        world.sensors.pool = vec![
            Sensor {
                id: 0,
                team_index: 0,
                not_shutdown: true,
                contacts: vec![(second, 1)],
                ..Sensor::default()
            },
            Sensor {
                id: 1,
                team_index: 1,
                not_shutdown: true,
                contacts: vec![(first, 1)],
                ..Sensor::default()
            },
        ];
        world.sensors.teams[0] = vec![0, 1];
        sync_team_contact_order(&mut world, 0, &[], &[first]);
        sync_team_contact_order(&mut world, 0, &[], &[second]);
        assert_eq!(world.sensors.team_contact_order[0], [first, second]);

        world.sensors.pool[0].contacts.clear();
        sync_team_contact_order(&mut world, 0, &[second], &[]);
        assert_eq!(world.sensors.team_contact_order[0], [first]);
    }

    #[test]
    fn simultaneous_sensor_losses_follow_mover_list_swap_remove_order() {
        let contacts = [
            (ObjectHandle(1), 1),
            (ObjectHandle(2), 1),
            (ObjectHandle(3), 1),
            (ObjectHandle(4), 1),
        ];
        let mover_order = [
            (ObjectHandle(1), 0),
            (ObjectHandle(2), 0),
            (ObjectHandle(3), 1),
            (ObjectHandle(4), 1),
        ];

        let (remaining, changes) = reconcile_contacts_in_mover_order(&contacts, &mover_order);

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].1, Some(ObjectHandle(1)));
        assert_eq!(changes[1].1, Some(ObjectHandle(2)));
        assert_eq!(remaining, [(ObjectHandle(4), 1), (ObjectHandle(3), 1)]);
    }

    #[test]
    fn interleaved_sensor_addition_keeps_team_swap_remove_order() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        let (a, b, c, d, e) = (
            ObjectHandle(1),
            ObjectHandle(2),
            ObjectHandle(3),
            ObjectHandle(4),
            ObjectHandle(5),
        );
        world.sensors.pool.push(Sensor {
            id: 0,
            team_index: 0,
            not_shutdown: true,
            team_contacts_enabled: true,
            contacts: vec![(a, 1), (c, 1), (d, 1), (e, 1)],
            ..Sensor::default()
        });
        world.sensors.teams[0].push(0);
        world.sensors.team_contact_order[0] = vec![a, c, d, e];

        let (remaining, changes) = reconcile_contacts_in_mover_order(
            &world.sensors.pool[0].contacts,
            &[(a, 0), (b, 1), (c, 0), (d, 1), (e, 1)],
        );
        for (contacts, removed, added) in changes {
            world.sensors.pool[0].contacts = contacts;
            if let Some(removed) = removed {
                sync_team_contact_order(&mut world, 0, &[removed], &[]);
            }
            if let Some(added) = added {
                sync_team_contact_order(&mut world, 0, &[], &[added]);
            }
        }

        assert_eq!(remaining, [(e, 1), (b, 1), (d, 1)]);
        assert_eq!(world.sensors.team_contact_order[0], [e, b, d]); // contact.cpp:591-638, 991-1005, 1065-1088
    }

    #[test]
    fn shut_down_sensor_private_contacts_are_not_published_to_team() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        let target = ObjectHandle(8);
        let sensor_only = ObjectHandle(9);
        world.teams.teams[0] = Some(crate::ai::team::Team::default());
        world.sensors.pool.push(Sensor {
            id: 0,
            team_index: 0,
            not_shutdown: false,
            team_contacts_enabled: true,
            contacts: vec![(target, 5), (sensor_only, 2)],
            ..Sensor::default()
        });
        world.sensors.teams[0].push(0);
        world.sensors.team_contact_order[0] = vec![target, sensor_only];
        let mut statuses = [0; MAX_TEAMS];
        statuses[0] = 5;
        world.sensors.team_contact_status.insert(target, statuses);
        statuses[0] = 2;
        world
            .sensors
            .team_contact_status
            .insert(sensor_only, statuses);

        update(&mut world);

        assert!(!world.sensors.team_contact_status.contains_key(&target)); // contact.cpp:169-196, 397-447
        assert!(!world.sensors.team_contact_status.contains_key(&sensor_only));
        assert!(world.sensors.team_contact_order[0].is_empty());
        assert_eq!(world.sensors.pool[0].contacts, [(target, 5)]); // only visual private contacts survive shutdown

        world.sensors.pool[0].not_shutdown = true;
        update(&mut world);

        assert_eq!(world.sensors.pool[0].contacts, [(target, 5)]);
        assert_eq!(world.sensors.team_contact_order[0], [target]);
        assert_eq!(world.sensors.team_contact_status[&target][0], 5);
        assert!(!world.sensors.team_contact_status.contains_key(&sensor_only));
    }

    #[test]
    fn shutdown_withdraws_team_contacts_before_that_teams_next_scan() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        let target = ObjectHandle(8);
        world.teams.teams[0] = Some(crate::ai::team::Team::default());
        world.teams.teams[1] = Some(crate::ai::team::Team::default());
        world.sensors.pool.push(Sensor {
            id: 0,
            team_index: 0,
            not_shutdown: false,
            team_contacts_enabled: true,
            contacts: vec![(target, 5)],
            ..Sensor::default()
        });
        world.sensors.teams[1].push(0);
        world.sensors.team_contact_order[1].push(target);
        let mut statuses = [0; MAX_TEAMS];
        statuses[1] = 5;
        world.sensors.team_contact_status.insert(target, statuses);
        world.clock.turn = 1;
        world.sensors.team_to_update = 0;

        update(&mut world);

        assert_eq!(world.sensors.team_to_update, 1);
        assert!(world.sensors.team_contact_order[1].is_empty());
        assert!(!world.sensors.team_contact_status.contains_key(&target));
        assert_eq!(world.sensors.pool[0].contacts, [(target, 5)]);
    }

    #[test]
    fn negative_range_skips_contact_and_scan_timer_changes() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        world.teams.teams[0] = Some(crate::ai::team::Team::default());
        world.clock.turn = 2;
        let target = ObjectHandle(8);
        world.sensors.pool.push(Sensor {
            id: 0,
            team_index: 0,
            range: -1.0,
            next_scan_update: 9.0,
            last_scan_update: 4.0,
            contacts: vec![(target, 2)],
            ..Sensor::default()
        });
        world.sensors.teams[0].push(0);

        update(&mut world);

        let sensor = &world.sensors.pool[0];
        assert_eq!(sensor.contacts, [(target, 2)]); // contact.cpp:510-514
        assert_eq!(sensor.next_scan_update, 9.0);
        assert_eq!(sensor.last_scan_update, 4.0);
    }

    #[test]
    fn invalid_team_los_does_not_block_visibility() {
        let world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        assert!(line_of_sight_ray(
            &world,
            crate::math::Vec3::default(),
            crate::math::Vec3::default(),
            -1,
            0.0,
            0.0,
        ));
    }

    /// Two enemy mechs 100 world units apart, the owner with one registered sensor of `range` metres.
    fn pair(range: f32, has_los: bool) -> (crate::World, ObjectHandle, ObjectHandle) {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
        world.terrain.world_units_map_side = 20_000.0;
        let (owner, target) = (ObjectHandle(1), ObjectHandle(2));
        world.objects.list.resize(3, None);
        for (handle, team, x, extra) in [
            (owner, 0, 0.0, 0),
            (target, 1, 100.0, crate::object::flags::SENSOR),
        ] {
            let mut common =
                crate::object::Common::new(crate::object::ObjectClass::BattleMech, handle);
            common.flags |= crate::object::flags::EXISTS | crate::object::flags::AWAKE | extra;
            common.position = crate::math::Vec3 { x, y: 0.0, z: 0.0 };
            let mut mech = crate::mover::MechState::default();
            mech.m.team = crate::ids::TeamId(team);
            mech.loadout
                .inventory
                .push(crate::combat::InventoryItem::default());
            world.objects.list[handle.0 as usize] = Some(crate::object::Obj {
                c: common,
                kind: crate::object::Kind::Mech(Box::new(mech)),
            });
        }
        world.sensors.pool = vec![Sensor {
            id: 0,
            owner,
            team_index: 0,
            inventory_index: 0,
            range,
            has_los_capability: has_los,
            not_shutdown: true,
            team_contacts_enabled: true,
            ..Sensor::default()
        }];
        world.sensors.teams[0].push(0);
        (world, owner, target)
    }

    /// Without LOS capability the status is the sensor quality alone, in sensor range only; a target without the SENSOR flag is no
    /// contact and a disabled target is never one (contact.cpp:292-333). The branches that consult `lineOfSight` are covered
    /// against the shipped mission in `tests/visual_los.rs`.
    #[test]
    fn contact_status_without_los_capability_is_the_sensor_quality() {
        for range in [1_000.0, 1.0] {
            let (mut world, owner, target) = pair(range, false);
            let expected = if range > 100.0 { 1 } else { 0 };
            assert_eq!(
                contact_status(&mut world, 0, owner, target),
                Some(expected),
                "range {range}"
            );
        }
        let (mut world, owner, target) = pair(1_000.0, false);
        if let Some(obj) = world.objects.get_mut(target) {
            obj.c.flags &= !crate::object::flags::SENSOR;
        }
        assert_eq!(contact_status(&mut world, 0, owner, target), Some(0));
        let (mut world, owner, target) = pair(1_000.0, false);
        if let Some(obj) = world.objects.get_mut(target) {
            obj.c.status = crate::object::ObjectStatus::Disabled;
        }
        assert_eq!(contact_status(&mut world, 0, owner, target), Some(0));
    }

    /// `SensorSystem::addContact` drops a contact when the list holds `MAX_CONTACTS_PER_SENSOR` (contact.cpp:394); removals still
    /// work at capacity, and the freed slot takes the next new contact in the same pass (`scanBattlefield`, contact.cpp:591-638).
    #[test]
    fn a_full_contact_list_drops_new_contacts_but_keeps_removals() {
        let existing: ContactList = (0..MAX_CONTACTS_PER_SENSOR as u32)
            .map(|i| (ObjectHandle(100 + i), 1))
            .collect();
        let extra = ObjectHandle(1000);
        let (list, changes) = reconcile_contacts_in_mover_order(&existing, &[(extra, 1)]);
        assert_eq!(list, existing);
        assert!(changes.is_empty());
        // the first contact disappears, then the new one is seen: swap-remove fills slot 0 with the last, the new one is appended
        let (list, changes) =
            reconcile_contacts_in_mover_order(&existing, &[(ObjectHandle(100), 0), (extra, 1)]);
        assert_eq!(list.len(), MAX_CONTACTS_PER_SENSOR);
        assert_eq!(list[0], (ObjectHandle(100 + 199), 1));
        assert_eq!(list.last(), Some(&(extra, 1)));
        assert_eq!(changes.len(), 2);
        // an existing contact keeps updating at capacity
        let (list, _) = reconcile_contacts_in_mover_order(&existing, &[(ObjectHandle(150), 3)]);
        assert_eq!(list[50], (ObjectHandle(150), 3));
    }

    /// The immediate scan of a new mover (`SensorSystem::scanMover` -> `addContact`) obeys the same capacity, for the private list and
    /// for the team list (`TeamSensorSystem::addContact` is only reached from `addContact`, contact.cpp:394-398).
    #[test]
    fn scan_mover_does_not_publish_past_the_sensor_capacity() {
        // a sensor that cannot see (no LOS capability, no quality) is enough: use a known status through a non-LOS sensor
        let (mut world, owner, target) = pair(1_000.0, false);
        world.sensors.pool[0].contacts = (0..MAX_CONTACTS_PER_SENSOR as u32)
            .map(|i| (ObjectHandle(100 + i), 1))
            .collect();
        world.sensors.team_contact_order[0] = world.sensors.pool[0]
            .contacts
            .iter()
            .map(|(handle, _)| *handle)
            .collect();
        scan_mover(&mut world, owner, target);
        assert_eq!(
            world.sensors.pool[0].contacts.len(),
            MAX_CONTACTS_PER_SENSOR
        );
        assert!(!world.sensors.pool[0]
            .contacts
            .iter()
            .any(|(h, _)| *h == target));
        assert!(!world.sensors.team_contact_order[0].contains(&target));
        // below capacity the same scan publishes it
        let (mut world, owner, target) = pair(1_000.0, false);
        scan_mover(&mut world, owner, target);
        assert_eq!(world.sensors.pool[0].contacts, [(target, 1)]);
        assert_eq!(world.sensors.team_contact_order[0], [target]);
    }
}
