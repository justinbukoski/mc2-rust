// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `TerrainObject`, `Building`, `Turret` (non-weapon part) and `Gate` state, init, update and collision reactions
//! (terrobj.cpp, bldng.cpp, turret.cpp, gate.cpp). See DESIGN.md Lane A for the quirks (shared type mutation, turret double
//! update, building extent from screen-space bounds, ...). `handleWeaponHit` of structures is lane C
//! ([`crate::combat::handle_weapon_hit`]); `Gate::destroyGate` is here because it is also called by the gate itself.

use std::rc::Rc;

use mc2_sim_data::objtype::building::BuildingSubType;
use mc2_sim_data::objtype::{ObjectTable, ObjectType, ObjectTypeData, TerrainSubType};

use super::{flags, hooks, terr, Kind, ObjectClass, ObjectStatus};
use crate::combat::{AttackSource, WeaponShotInfo};
use crate::ids::{ObjectHandle, WatchId, MAX_TEAMS};
use crate::math::{Vec3, Vec3Ext};
use crate::World;

/// `MAX_SPECIAL_SUB_AREAS` (mclib/move.h:1076).
pub const MAX_SPECIAL_SUB_AREAS: usize = 25;
/// `TREE_FALL_RATE` (terrobj.cpp:62).
pub const TREE_FALL_RATE: f32 = 15.0;
/// `TREE_FALL_ACCEL` (terrobj.cpp:63).
pub const TREE_FALL_ACCEL: f32 = 5.0;
/// `MAX_TURRET_WEAPONS`.
pub const MAX_TURRET_WEAPONS: usize = 4;
/// `MAX_TURRET_PITCH`.
pub const MAX_TURRET_PITCH: f32 = -45.0;
/// `GENERIC_DESTRUCTIBLE_RESOURCE_BUILDING_OBJNUM` (code/dbldng.h:17).
pub const GENERIC_DESTRUCTIBLE_RESOURCE_BUILDING_OBJNUM: i32 = 357;
/// `BRIDGE_TYPE` building number (bldng.cpp:122).
pub const BRIDGE_BUILDING_TYPE: i32 = 607;
/// `Turret::turretsEnabled[team]` (all true; ABL can clear them in the original; no native does in the shipped scripts).
pub const TURRETS_ENABLED_DEFAULT: bool = true;

/// Fields of `TerrainObject` (terrobj.h) shared by buildings, turrets, gates and trees.
#[derive(Clone, Debug)]
pub struct Structure {
    /// `damage`.
    pub damage: f32,
    /// `vertexNumber` (file-derived: `tileRow * 20 + tileRow`, see [`super::store::ObjDataLoader`]).
    pub vertex_number: i32,
    /// `blockNumber`.
    pub block_number: i32,
    /// `pitchAngle` (tree fall).
    pub pitch_angle: f32,
    /// `fallRate`.
    pub fall_rate: f32,
    /// `powerSupply`: watch id of the linked power generator.
    pub power_supply: WatchId,
    /// `cellFootprint` (4 cells). Computed from the shape; never read by the simulation (dead code), left at -1.
    pub cell_footprint: [i16; 4],
    /// `subAreas0/1`: global-map area ids this object opens/closes (ground map, hover map).
    pub sub_areas: [Vec<i16>; 2],
    /// `cellsCovered`: cells to mark in the move map (row, col).
    pub cells_covered: Vec<(i16, i16)>,
    /// `listID`.
    pub list_id: u8,
    /// Updates whose geometry step was skipped because the object was in view and the frame supplied no structure LOD
    /// (`hooks::recalc_bounds_with_lod` returned `None`). Not a C++ field: the record of the explicit "unavailable" result.
    pub lod_unavailable_updates: u32,
}

impl Default for Structure {
    /// `TerrainObject::init(bool)` (terrobj.h:131).
    fn default() -> Self {
        Structure {
            damage: 0.0,
            vertex_number: 0,
            block_number: 0,
            pitch_angle: 0.0,
            fall_rate: 0.0,
            power_supply: WatchId::NONE,
            cell_footprint: [-1; 4],
            sub_areas: [Vec::new(), Vec::new()],
            cells_covered: Vec::new(),
            list_id: 255,
            lod_unavailable_updates: 0,
        }
    }
}

/// `recalcBounds` for a caller that goes on to use appearance geometry. `None` = the object is in view but the frame supplied no
/// structure LOD (bdactor.cpp:1297-1331 selects it in the in-view branch): the caller skips every geometry-dependent step and
/// the skipped update is counted in [`Structure::lod_unavailable_updates`].
fn recalc_bounds_for_geometry(world: &mut World, h: ObjectHandle) -> Option<bool> {
    let bounds = hooks::recalc_bounds_with_lod(world, h);
    if bounds.is_none() {
        if let Some(s) = world.objects.get_mut(h).and_then(|o| o.structure_mut()) {
            s.lod_unavailable_updates += 1;
        }
    }
    bounds
}

/// `recalcBounds` then the appearance `update` (the destruction paths). With the structure LOD unavailable the update is
/// skipped (it would transform the retained shape) and counted in [`Structure::lod_unavailable_updates`].
fn recalc_bounds_and_update(world: &mut World, h: ObjectHandle) {
    if recalc_bounds_for_geometry(world, h).is_some() {
        hooks::structure_update(world, h);
    }
}

/// `Building` (bldng.h).
#[derive(Clone, Debug)]
pub struct BuildingState {
    /// Terrain-object part.
    pub s: Structure,
    /// `teamId` (-1 none).
    pub team_id: i8,
    /// `commanderId` (last capturer).
    pub commander_id: i8,
    /// `baseTileId`.
    pub base_tile_id: u8,
    /// `refitBuddyWID`.
    pub refit_buddy: WatchId,
    /// `parentId` (file) and `parent` (resolved).
    pub parent_id: u32,
    /// Resolved parent.
    pub parent: WatchId,
    /// `listID`.
    pub list_id: u8,
    /// `captureTime`.
    pub capture_time: f32,
    /// `scoreTime`.
    pub score_time: f32,
    /// `moverInProximity`.
    pub mover_in_proximity: bool,
    /// `proximityTimer`.
    pub proximity_timer: f32,
    /// `updatedTurn` (guards double update).
    pub updated_turn: i64,
}

impl BuildingState {
    /// `Building::init(bool)` after `TerrainObject::init(bool)` (bldng.h:117).
    pub fn new() -> BuildingState {
        BuildingState {
            s: Structure::default(),
            team_id: -1,
            commander_id: -1,
            base_tile_id: 0,
            refit_buddy: WatchId::NONE,
            parent_id: 0xffff_ffff,
            parent: WatchId::NONE,
            list_id: 255,
            capture_time: 0.0,
            score_time: 0.0,
            mover_in_proximity: false,
            proximity_timer: 0.0,
            updated_turn: -1,
        }
    }
}

impl Default for BuildingState {
    fn default() -> Self {
        BuildingState::new()
    }
}

/// `Turret` (turret.h) without fire control.
#[derive(Clone, Debug)]
pub struct TurretState {
    /// Terrain-object part.
    pub s: Structure,
    /// `teamId`.
    pub team_id: i8,
    /// `targetWID`.
    pub target: WatchId,
    /// `readyTime[MAX_TURRET_WEAPONS]`.
    pub ready_time: [f32; 4],
    /// `lastFireTime` (uninitialised in the C++; zero here like the oracle's `calloc`).
    pub last_fire_time: [f32; 4],
    /// `turretRotation`.
    pub turret_rotation: f32,
    /// `didReveal`.
    pub did_reveal: bool,
    /// `minRange`, `maxRange`, `numFunctionalWeapons` (`calcFireRanges`).
    pub min_range: f32,
    /// See `min_range`.
    pub max_range: f32,
    /// See `min_range`.
    pub num_functional_weapons: i32,
    /// `idleWait` (drawn in the constructor, i.e. once per turret slot during `setNumObjects`).
    pub idle_wait: f32,
    /// `idlePosition`.
    pub idle_position: Vec3,
    /// `oldPosition` (spotlight tracking).
    pub old_position: Vec3,
    /// `parentId` (file) and `parent` (resolved watch id of the control building).
    pub parent_id: u32,
    /// Resolved parent.
    pub parent: WatchId,
    /// `currentWeaponNode`.
    pub current_weapon_node: i32,
}

impl TurretState {
    /// `Turret::init(bool)` (turret.cpp:328): the constructor state with the `idleWait` draw already made.
    pub fn new(idle_wait: f32) -> TurretState {
        TurretState {
            s: Structure::default(),
            team_id: 0,
            target: WatchId::NONE,
            ready_time: [0.0; 4],
            last_fire_time: [0.0; 4],
            turret_rotation: 0.0,
            did_reveal: false,
            min_range: 0.0,
            max_range: 0.0,
            num_functional_weapons: 0,
            idle_wait,
            idle_position: Vec3::zero(),
            old_position: Vec3::zero(),
            parent_id: 0xffff_ffff,
            parent: WatchId::NONE,
            current_weapon_node: 0,
        }
    }
}

/// `Gate` (gate.h).
#[derive(Clone, Debug)]
pub struct GateState {
    /// Terrain-object part.
    pub s: Structure,
    /// `lockedOpen`.
    pub locked_open: bool,
    /// `lockedClose`.
    pub locked_close: bool,
    /// `reasonToOpen`.
    pub reason_to_open: bool,
    /// `opened`.
    pub opened: bool,
    /// `opening`.
    pub opening: bool,
    /// `closed`.
    pub closed: bool,
    /// `closing`.
    pub closing: bool,
    /// `justDestroyed`.
    pub just_destroyed: bool,
    /// `lastMarkedOpen`.
    pub last_marked_open: bool,
    /// `teamId` (uninitialised in C++ until set; zero like the oracle's `calloc`).
    pub team_id: i8,
    /// `closestObject` (last mover seen, not the closest: C++ bug kept).
    pub closest_object: ObjectHandle,
    /// `parentId`.
    pub parent_id: u32,
    /// `parent`.
    pub parent: WatchId,
    /// `updatedTurn`.
    pub updated_turn: i64,
}

impl GateState {
    /// `Gate::Gate()` (gate.h:109).
    pub fn new() -> GateState {
        GateState {
            s: Structure::default(),
            locked_open: false,
            locked_close: false,
            reason_to_open: true,
            opened: false,
            opening: false,
            closed: true,
            closing: false,
            just_destroyed: false,
            last_marked_open: false,
            team_id: 0,
            closest_object: ObjectHandle::NONE,
            parent_id: 0xffff_ffff,
            parent: WatchId::NONE,
            updated_turn: -1,
        }
    }
}

impl Default for GateState {
    fn default() -> Self {
        GateState::new()
    }
}

// ------------------------------------------------------------------------------------------------------------------
// Small accessors
// ------------------------------------------------------------------------------------------------------------------

fn table(world: &World) -> Rc<ObjectTable> {
    world.objects.types.clone().unwrap_or_default()
}

fn type_num(world: &World, h: ObjectHandle) -> i32 {
    world.objects.get(h).map(|o| o.c.type_handle).unwrap_or(0)
}

/// A snapshot of the `BuildingType` fields the simulation reads.
#[derive(Clone, Copy, Debug)]
pub struct BuildingInfo {
    /// `damageLevel`.
    pub damage_level: f32,
    /// `perimeterAlarmRange`.
    pub perimeter_alarm_range: f32,
    /// `perimeterAlarmTimer`.
    pub perimeter_alarm_timer: f32,
    /// `lookoutTowerRange`.
    pub lookout_tower_range: f32,
    /// `sensorRange`.
    pub sensor_range: f32,
    /// `resourcePoints`.
    pub resource_points: i32,
    /// `marksImpassableWhenDestroyed`.
    pub marks_impassable_when_destroyed: bool,
    /// `capturable`.
    pub capturable: bool,
    /// `powerSource`.
    pub power_source: bool,
    /// `canRefit`.
    pub can_refit: bool,
    /// `mechBay`.
    pub mech_bay: bool,
    /// `baseTonnage`.
    pub base_tonnage: f32,
    /// `explDmg`.
    pub expl_dmg: f32,
    /// `explRad`.
    pub expl_rad: f32,
    /// `startBR`.
    pub start_br: i32,
    /// `BuildingType::teamId` as the type file declares it (`TeamID`, bldng.cpp:236; the member of bldng.h:58 that shadows
    /// `ObjectType::teamId`).
    pub declared_team_id: i32,
    /// Wall / land bridge sub type (`BUILDING_SUBTYPE_*`).
    pub sub_type: mc2_sim_data::objtype::building::BuildingSubType,
}

/// `BuildingType::activityEffectId` of a building (`None` for `0xffffffff`).
fn activity_effect(world: &World, h: ObjectHandle) -> Option<i32> {
    let num = type_num(world, h);
    match &world.objects.type_by_num(num)?.data {
        ObjectTypeData::Building(b) if b.activity_effect_id != 0xffff_ffff => {
            Some(b.activity_effect_id as i32)
        }
        _ => None,
    }
}

/// `BuildingType` of a type number.
pub fn building_info(world: &World, num: i32) -> Option<BuildingInfo> {
    match &world.objects.type_by_num(num)?.data {
        ObjectTypeData::Building(b) => Some(BuildingInfo {
            damage_level: b.damage_level,
            perimeter_alarm_range: b.perimeter_alarm_range,
            perimeter_alarm_timer: b.perimeter_alarm_timer,
            lookout_tower_range: b.lookout_tower_range,
            sensor_range: b.sensor_range,
            resource_points: b.resource_points,
            marks_impassable_when_destroyed: b.marks_impassable_when_destroyed,
            capturable: b.capturable,
            power_source: b.power_source,
            can_refit: b.can_refit,
            mech_bay: b.mech_bay,
            base_tonnage: b.base_tonnage,
            expl_dmg: b.explosion_damage,
            expl_rad: b.explosion_radius,
            start_br: b.start_battle_rating,
            declared_team_id: b.declared_team_id,
            sub_type: b.sub_type,
        }),
        _ => None,
    }
}

impl BuildingInfo {
    /// `Building::isSpecialBuilding()`: perimeter alarms, lookout towers and sensor buildings.
    pub fn is_special(&self) -> bool {
        (self.perimeter_alarm_range > 0.0 && self.perimeter_alarm_timer > 0.0)
            || self.lookout_tower_range > 0.0
            || self.sensor_range > 0.0
    }
}

/// `Building::isSpecialBuilding()` for an object (false for anything that is not a building).
pub fn is_special_building(world: &World, h: ObjectHandle) -> bool {
    match world.objects.get(h) {
        Some(o) if matches!(o.kind, Kind::Building(_)) => {
            building_info(world, o.c.type_handle).is_some_and(|b| b.is_special())
        }
        _ => false,
    }
}

/// `Building::isPowerSource()`.
pub fn is_power_source(world: &World, h: ObjectHandle) -> bool {
    match world.objects.get(h) {
        Some(o) if matches!(o.kind, Kind::Building(_)) => {
            building_info(world, o.c.type_handle).is_some_and(|b| b.power_source)
        }
        _ => false,
    }
}

/// `Building::canBeCaptured()` (and `GameObject::canBeCaptured()` false for everything else).
pub fn can_be_captured(world: &World, h: ObjectHandle) -> bool {
    match world.objects.get(h) {
        Some(o) if matches!(o.kind, Kind::Building(_)) => {
            building_info(world, o.c.type_handle).is_some_and(|b| b.capturable)
        }
        _ => false,
    }
}

fn turret_type(table: &ObjectTable, num: i32) -> Option<&mc2_sim_data::objtype::TurretType> {
    match &table.get(num as u32)?.data {
        ObjectTypeData::Turret(t) => Some(t),
        _ => None,
    }
}

fn gate_type(table: &ObjectTable, num: i32) -> Option<&mc2_sim_data::objtype::GateType> {
    match &table.get(num as u32)?.data {
        ObjectTypeData::Gate(t) => Some(t),
        _ => None,
    }
}

fn terrain_type(
    table: &ObjectTable,
    num: i32,
) -> Option<&mc2_sim_data::objtype::TerrainObjectType> {
    match &table.get(num as u32)?.data {
        ObjectTypeData::TerrainObject(t) => Some(t),
        _ => None,
    }
}

/// `Gate::getLittleExtent()` / `Turret::getLittleExtent()`: the type's `littleExtent`.
pub fn little_extent(world: &World, h: ObjectHandle) -> f32 {
    let t = table(world);
    let num = type_num(world, h);
    match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Gate(_)) => gate_type(&t, num).map(|g| g.little_extent).unwrap_or(20.0),
        Some(Kind::Turret(_)) => turret_type(&t, num)
            .map(|g| g.little_extent)
            .unwrap_or(20.0),
        _ => 20.0,
    }
}

/// `getDamageLevel()` of structures (the type's damage level).
pub fn damage_level(world: &World, h: ObjectHandle) -> f32 {
    let t = table(world);
    let num = type_num(world, h);
    let Some(o) = world.objects.get(h) else {
        return 0.0;
    };
    match &o.kind {
        Kind::Terrain(_) => terrain_type(&t, num).map(|x| x.damage_level).unwrap_or(0.0),
        Kind::Building(_) => building_info(world, num)
            .map(|b| b.damage_level)
            .unwrap_or(0.0),
        Kind::Turret(_) => turret_type(&t, num).map(|x| x.damage_level).unwrap_or(0.0),
        Kind::Gate(_) => gate_type(&t, num)
            .map(|x| x.dmg_level as f32)
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

// ------------------------------------------------------------------------------------------------------------------
// Sub areas and the move map (terrobj.cpp:1000-1135)
// ------------------------------------------------------------------------------------------------------------------

/// `TerrainObject::calcSubAreas(numCells, cells)`: records the cells and the unique global-map areas under them (ground and
/// hover maps) and marks this object as the owner of each area. `getWatchID()` is first called here for walls, bridges and
/// gates.
pub fn calc_sub_areas(world: &mut World, h: ObjectHandle, cells: &[[i16; 2]]) {
    if cells.is_empty() {
        if let Some(s) = world.objects.get_mut(h).and_then(|o| o.structure_mut()) {
            s.cells_covered.clear();
        }
        return;
    }
    let mut areas: [Vec<i16>; 2] = [Vec::new(), Vec::new()];
    for (level, list) in areas.iter_mut().enumerate() {
        for c in cells {
            let area = terr::global_map(&world.terrain, level)
                .calc_area(i32::from(c[0]), i32::from(c[1])) as i16;
            if !list.contains(&area) {
                list.push(area);
            }
        }
    }
    {
        let Some(s) = world.objects.get_mut(h).and_then(|o| o.structure_mut()) else {
            return;
        };
        s.cells_covered = cells.iter().map(|c| (c[0], c[1])).collect();
        s.sub_areas = areas.clone();
    }
    for (level, list) in areas.iter().enumerate() {
        for &a in list {
            let wid = world.objects.watch_id(h);
            terr::global_map_mut(&mut world.terrain, level)
                .set_area_owner_wid(i32::from(a), wid.0 as i32);
        }
    }
}

/// `TerrainObject::markMoveMap(passable)`: sets the passable bit of every covered cell; opening also clears the local height.
pub fn mark_move_map(world: &mut World, h: ObjectHandle, passable: bool) {
    let cells: Vec<(i16, i16)> = match world.objects.get(h).and_then(|o| o.structure()) {
        Some(s) => s.cells_covered.clone(),
        None => return,
    };
    for (r, c) in cells {
        terr::set_passable(&mut world.terrain, i32::from(r), i32::from(c), passable);
        if passable {
            terr::set_local_height(&mut world.terrain, i32::from(r), i32::from(c), 0);
        }
    }
}

fn sub_areas_of(world: &World, h: ObjectHandle) -> [Vec<i16>; 2] {
    world
        .objects
        .get(h)
        .and_then(|o| o.structure())
        .map(|s| s.sub_areas.clone())
        .unwrap_or_default()
}

/// `TerrainObject::openSubAreas()`.
pub fn open_sub_areas(world: &mut World, h: ObjectHandle) {
    mark_move_map(world, h, true);
    let areas = sub_areas_of(world, h);
    for (level, list) in areas.iter().enumerate() {
        for &a in list {
            terr::global_map_mut(&mut world.terrain, level).open_area(i32::from(a));
        }
    }
}

/// `TerrainObject::closeSubAreas()`.
pub fn close_sub_areas(world: &mut World, h: ObjectHandle) {
    mark_move_map(world, h, false);
    let areas = sub_areas_of(world, h);
    for (level, list) in areas.iter().enumerate() {
        for &a in list {
            terr::global_map_mut(&mut world.terrain, level).close_area(i32::from(a));
        }
    }
}

/// `TerrainObject::setSubAreasTeamId(id)`.
pub fn set_sub_areas_team_id(world: &mut World, h: ObjectHandle, id: i32) {
    let areas = sub_areas_of(world, h);
    for (level, list) in areas.iter().enumerate() {
        for &a in list {
            terr::global_map_mut(&mut world.terrain, level)
                .set_area_team_id(i32::from(a), id as i8);
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------
// Init (the `init(true, type)` calls of `addObject`)
// ------------------------------------------------------------------------------------------------------------------

/// `TerrainObject::init(true, objType)` (terrobj.cpp:556).
pub fn init_terrain_object(world: &mut World, h: ObjectHandle, type_num_: i32) {
    let t = table(world);
    let (sub_type, damage_level) = match terrain_type(&t, type_num_) {
        Some(x) => (x.sub_type, x.damage_level),
        None => (TerrainSubType::None, 0.0),
    };
    let extent = world.objects.type_extent_radius(type_num_);
    hooks::init_structure_appearance(world, h, type_num_);
    let Some(o) = world.objects.get_mut(h) else {
        return;
    };
    // GameObject::init(true, type)
    o.c.type_handle = type_num_;
    o.c.class = ObjectClass::GameObject;
    o.c.set_flag(flags::JUSTCREATED, true);
    if extent > 0.0 {
        o.c.set_tangible(true);
    }
    o.c.class = ObjectClass::TerrainObject;
    match sub_type {
        TerrainSubType::None => {
            if damage_level == 0.0 {
                // Already destroyed. Used for extraction markers.
                o.c.set_tangible(false);
                o.c.set_status(ObjectStatus::Destroyed, false);
            }
        }
        TerrainSubType::Tree => {
            o.c.class = ObjectClass::Tree;
            o.c.set_flag(flags::DAMAGED, false);
        }
        TerrainSubType::Bridge
        | TerrainSubType::Forest
        | TerrainSubType::WallHeavy
        | TerrainSubType::WallMedium
        | TerrainSubType::WallLight => {
            o.c.set_tangible(false);
            o.c.class = ObjectClass::Bridge;
        }
    }
}

/// `TerrainObject::setDamage(long)` (terrobj.cpp:530): the damage is stored as an integer value.
pub fn set_terrain_damage(world: &mut World, h: ObjectHandle, new_damage: i64) {
    let level = damage_level(world, h);
    let mut destroyed = false;
    if let Some(s) = world.objects.get_mut(h).and_then(|o| o.structure_mut()) {
        s.damage = new_damage as f32;
        destroyed = s.damage >= level;
    }
    if destroyed {
        if let Some(o) = world.objects.get_mut(h) {
            o.c.set_status(ObjectStatus::Destroyed, false);
        }
        hooks::set_obj_status(world, h, ObjectStatus::Destroyed);
    }
}

/// `Building::init(true, objType)` (bldng.cpp:1186).
pub fn init_building(world: &mut World, h: ObjectHandle, type_num_: i32) {
    let Some(info) = building_info(world, type_num_) else {
        return;
    };
    let extent = world.objects.type_extent_radius(type_num_);
    hooks::init_structure_appearance(world, h, type_num_);
    {
        let Some(o) = world.objects.get_mut(h) else {
            return;
        };
        o.c.type_handle = type_num_;
        o.c.class = ObjectClass::GameObject;
        o.set_exists(true);
        o.c.set_flag(flags::JUSTCREATED, true);
        if let Kind::Building(b) = &mut o.kind {
            b.capture_time = -1000.0;
        }
        o.c.class = ObjectClass::Building;
        o.c.set_flag(flags::DAMAGED, true);
        if extent > 0.0 {
            o.c.set_tangible(true);
        }
        o.c.tonnage = info.base_tonnage;
        o.c.expl_damage = info.expl_dmg;
        o.c.expl_radius = info.expl_rad;
        o.c.max_cv = info.start_br as i16;
        o.c.cur_cv = info.start_br as i16;
        o.c.set_flag(flags::CANREFIT, info.can_refit);
        o.c.set_flag(flags::MECHBAY, info.mech_bay);
    }
    // setTeamId(type->teamId, true): `Building::init` reads `BuildingType::teamId` (bldng.h:58), the `TeamID` the type file declares;
    // it shadows `ObjectType::teamId`, which `ObjectType::init` forces to -1 (objtype.cpp:168) without touching it.
    set_building_team(world, h, info.declared_team_id, true);
    if info.sensor_range > -1.0 {
        // `getTeam()`: a building on an existing team gets a sensor system (bldng.cpp:1266)
        let team = world.objects.get(h).map_or(-1, |o| o.team_id());
        let exists = usize::try_from(team)
            .ok()
            .is_some_and(|t| world.teams.teams.get(t).is_some_and(|team| team.is_some()));
        if exists {
            crate::ai::sensors::set_building_sensor_data(world, h, info.sensor_range, true);
        }
    }
    {
        let Some(o) = world.objects.get_mut(h) else {
            return;
        };
        o.c.set_flag(flags::CAPTURABLE, false);
        if info.capturable {
            o.c.set_flag(flags::CAPTURABLE, true);
        }
        if let Kind::Building(b) = &mut o.kind {
            b.refit_buddy = WatchId::NONE;
        }
        if info.damage_level == 0.0 {
            // Already destroyed. Used for extraction markers.
            o.c.set_tangible(false);
            o.c.set_status(ObjectStatus::Destroyed, false);
            o.c.set_flag(flags::DAMAGED, true);
        }
    }
}

/// `Building::setDamage(float)` (bldng.cpp:1005).
pub fn set_building_damage(world: &mut World, h: ObjectHandle, new_damage: f32) {
    let level = damage_level(world, h);
    if let Some(Kind::Building(b)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        b.s.damage = new_damage;
    }
    if new_damage >= level {
        if let Some(o) = world.objects.get_mut(h) {
            o.c.set_status(ObjectStatus::Destroyed, false);
        }
        hooks::set_obj_status(world, h, ObjectStatus::Destroyed);
    }
}

/// `Turret::init(true, type)` (turret.cpp:2106).
pub fn init_turret(world: &mut World, h: ObjectHandle, type_num_: i32) {
    let t = table(world);
    let Some(tt) = turret_type(&t, type_num_) else {
        return;
    };
    // type->setExtentRadius(engageRadius) mutates the shared type.
    if tt.engage_radius != 0.0 {
        world
            .objects
            .set_type_extent_radius(type_num_, tt.engage_radius);
    }
    let extent = world.objects.type_extent_radius(type_num_);
    hooks::init_structure_appearance(world, h, type_num_);
    let Some(o) = world.objects.get_mut(h) else {
        return;
    };
    o.c.type_handle = type_num_;
    o.c.class = ObjectClass::GameObject;
    o.c.set_flag(flags::JUSTCREATED, true);
    o.c.class = ObjectClass::Turret;
    o.c.set_flag(flags::NEVER_REVEALED, true);
    if let Kind::Turret(ts) = &mut o.kind {
        ts.team_id = -1;
        ts.ready_time = [0.0; 4];
    }
    if extent > 0.0 {
        o.c.set_flag(flags::TANGIBLE, true);
    }
    o.c.tonnage = tt.base_tonnage;
    o.c.expl_damage = tt.explosion_damage;
    o.c.expl_radius = tt.explosion_radius;
    o.c.set_flag(flags::CAPTURABLE, false);
    o.c.set_flag(flags::POP_NEUTRALS, false);
}

/// `Turret::setDamage(float)` (turret.cpp:431). Note it sets `OBJECT_STATUS_DESTROYED` but not `OBJECT_FLAG_DESTROYED`:
/// a turret created pre-damaged keeps updating and firing.
pub fn set_turret_damage(world: &mut World, h: ObjectHandle, new_damage: f32) {
    let level = damage_level(world, h);
    if let Some(Kind::Turret(t)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        t.s.damage = new_damage;
    }
    if new_damage >= level {
        if let Some(o) = world.objects.get_mut(h) {
            o.c.set_status(ObjectStatus::Destroyed, false);
        }
        hooks::set_obj_status(world, h, ObjectStatus::Destroyed);
    }
}

/// `Gate::init(true, type)` (gate.cpp:~560).
pub fn init_gate(world: &mut World, h: ObjectHandle, type_num_: i32) {
    let t = table(world);
    let Some(gt) = gate_type(&t, type_num_) else {
        return;
    };
    if gt.open_radius != 0.0 {
        world
            .objects
            .set_type_extent_radius(type_num_, gt.open_radius);
    }
    let extent = world.objects.type_extent_radius(type_num_);
    hooks::init_structure_appearance(world, h, type_num_);
    let Some(o) = world.objects.get_mut(h) else {
        return;
    };
    o.c.type_handle = type_num_;
    o.c.class = ObjectClass::GameObject;
    o.c.set_flag(flags::JUSTCREATED, true);
    o.c.class = ObjectClass::Gate;
    if let Kind::Gate(g) = &mut o.kind {
        g.reason_to_open = false;
        g.last_marked_open = false;
    }
    if extent > 0.0 {
        o.c.set_flag(flags::TANGIBLE, true);
    }
    o.c.expl_damage = gt.explosion_damage;
    o.c.expl_radius = gt.explosion_radius;
}

/// `Gate::setDamage(float)` (gate.cpp:~590).
pub fn set_gate_damage(world: &mut World, h: ObjectHandle, new_damage: f32) {
    let level = damage_level(world, h);
    if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        g.s.damage = new_damage;
    }
    if new_damage >= level {
        if let Some(o) = world.objects.get_mut(h) {
            o.c.set_status(ObjectStatus::Destroyed, false);
        }
        hooks::set_obj_status(world, h, ObjectStatus::Destroyed);
    }
}

// ------------------------------------------------------------------------------------------------------------------
// Team handling
// ------------------------------------------------------------------------------------------------------------------

/// The team remapping all structures use outside multiplayer: ally teams (2) fold into the player's, everything above
/// into team 1.
fn fold_team(t: i32) -> i32 {
    if t == 2 {
        0
    } else if t > 2 {
        1
    } else {
        t
    }
}

/// `Building::setTeamId(teamId, setup)` (bldng.cpp:~830).
pub fn set_building_team(world: &mut World, h: ObjectHandle, new_team: i32, setup: bool) {
    let turn = world.clock.turn;
    let type_n = type_num(world, h);
    let resource_points = building_info(world, type_n)
        .map(|b| b.resource_points)
        .unwrap_or(0);
    let team = fold_team(new_team);
    // `if (sensorSystem) SensorManager->removeTeamSensor(teamId, sensorSystem)` (bldng.cpp:923) with the OLD team id
    let old_team = world.objects.get(h).map_or(-1, |o| o.team_id());
    crate::ai::sensors::building_sensor_leave_team(world, h, old_team);
    let (parent, commander) = {
        let Some(o) = world.objects.get_mut(h) else {
            return;
        };
        let Kind::Building(b) = &mut o.kind else {
            return;
        };
        b.team_id = team as i8;
        (b.parent, b.commander_id)
    };
    let _ = commander;
    if !setup && old_team != team {
        let (sensor, lookout) = building_info(world, type_n)
            .map(|b| (b.sensor_range > -1.0, b.lookout_tower_range > 0.0))
            .unwrap_or((false, false));
        if sensor && team > -1 && turn > 5 && !lookout {
            world.events.push(crate::SimEvent::Betty(
                crate::events::betty::SENSOR_CAPTURED,
            ));
        }
        if resource_points != 0 && turn > 10 {
            world
                .events
                .push(crate::SimEvent::Betty(crate::events::betty::RESOURCES));
        }
    }
    // `if ((teamId > -1) && sensorSystem) SensorManager->addTeamSensor(teamId, sensorSystem)` (bldng.cpp:948)
    crate::ai::sensors::building_sensor_join_team(world, h, team);
    if turn > 10 {
        // highLight[Team::relations[teamId][home]] = {green, blue, red}
        const HIGHLIGHT: [u32; 3] = [0x0000_7f00, 0x0000_007f, 0x007f_0000];
        let rel = hooks::team_relation(world, team, hooks::home_team(world))
            .map(|r| r as usize)
            .unwrap_or(0);
        hooks::flash_building(world, h, HIGHLIGHT[rel.min(2)]);
    }
    if turn > 10 && resource_points != 0 {
        hooks::add_resource_points(world, resource_points);
    }
    // Tell the parent (turret control) to follow.
    if !parent.is_none() {
        if let Some(ph) = world.objects.by_watch_id(parent) {
            let parent_team = world.objects.get(ph).map(|p| p.team_id()).unwrap_or(-1);
            if parent_team != team {
                set_team_id(world, ph, team, false);
            }
        }
    }
}

/// `Turret::setTeamId(teamId, setup)` (turret.cpp:366): also drops the current target.
pub fn set_turret_team(world: &mut World, h: ObjectHandle, new_team: i32, setup: bool) {
    let _ = setup;
    let turn = world.clock.turn;
    let team = fold_team(new_team);
    {
        let Some(o) = world.objects.get_mut(h) else {
            return;
        };
        let Kind::Turret(t) = &mut o.kind else {
            return;
        };
        t.team_id = team as i8;
        t.target = WatchId::NONE;
    }
    if turn > 10 && (0..8).contains(&team) {
        const HIGHLIGHT: [u32; 8] = [
            0x0000_7f00,
            0x007f_0000,
            0x0000_007f,
            0x0000_007f,
            0x0000_007f,
            0x0000_007f,
            0x0000_007f,
            0x0000_007f,
        ];
        hooks::flash_building(world, h, HIGHLIGHT[team as usize]);
    }
}

/// `Gate::setTeamId(teamId, setup)` (gate.cpp:~530): note the sub-area team ids get the *unmapped* argument.
pub fn set_gate_team(world: &mut World, h: ObjectHandle, new_team: i32, setup: bool) {
    let _ = setup;
    let turn = world.clock.turn;
    let team = fold_team(new_team);
    {
        let Some(o) = world.objects.get_mut(h) else {
            return;
        };
        let Kind::Gate(g) = &mut o.kind else {
            return;
        };
        g.team_id = team as i8;
    }
    if turn > 10 && (0..8).contains(&team) {
        const HIGHLIGHT: [u32; 8] = [
            0x0000_7f00,
            0x007f_0000,
            0x0000_007f,
            0x0000_007f,
            0x0000_007f,
            0x0000_007f,
            0x0000_007f,
            0x0000_007f,
        ];
        hooks::flash_building(world, h, HIGHLIGHT[team as usize]);
    }
    set_sub_areas_team_id(world, h, new_team);
}

/// `setTeamId(teamId, setup)` on any structure (terrain objects have no team).
pub fn set_team_id(world: &mut World, h: ObjectHandle, team: i32, setup: bool) {
    match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Building(_)) => set_building_team(world, h, team, setup),
        Some(Kind::Turret(_)) => set_turret_team(world, h, team, setup),
        Some(Kind::Gate(_)) => set_gate_team(world, h, team, setup),
        _ => {}
    }
}

/// `setParentId(pId)` (buildings, turrets and gates remember the control-building cell; others ignore it).
pub fn set_parent_id(world: &mut World, h: ObjectHandle, parent_id: u32) {
    if let Some(o) = world.objects.get_mut(h) {
        match &mut o.kind {
            Kind::Building(b) => b.parent_id = parent_id,
            Kind::Turret(t) => t.parent_id = parent_id,
            Kind::Gate(g) => g.parent_id = parent_id,
            _ => {}
        }
    }
}

/// `TerrainObject::setRotation(rot)`.
pub fn set_rotation(world: &mut World, h: ObjectHandle, rot: f32) {
    let position = if let Some(o) = world.objects.get_mut(h) {
        o.c.rotation = rot;
        o.c.position
    } else {
        return;
    };
    world.appearance.set_object_parameters(h, position, rot);
}

/// `setTerrainPosition(position, numbers)` (terrobj.cpp:604): the position plus the file-derived vertex/block numbers.
pub fn set_terrain_position(
    world: &mut World,
    h: ObjectHandle,
    pos: Vec3,
    vertex: i32,
    block: i32,
) {
    super::set_position(world, h, pos, true);
    let rotation = world.objects.get(h).map(|o| o.c.rotation).unwrap_or(0.0);
    world.appearance.set_object_parameters(h, pos, rotation);
    if let Some(s) = world.objects.get_mut(h).and_then(|o| o.structure_mut()) {
        s.vertex_number = vertex;
        s.block_number = block;
    }
}

/// `setPowerSupply(generator)`: remembers the generator's watch id (assigning one on first use).
pub fn set_power_supply(world: &mut World, h: ObjectHandle, generator: ObjectHandle) {
    let wid = world.objects.watch_id(generator);
    if let Some(s) = world.objects.get_mut(h).and_then(|o| o.structure_mut()) {
        s.power_supply = wid;
    }
}

// ------------------------------------------------------------------------------------------------------------------
// Updates
// ------------------------------------------------------------------------------------------------------------------

fn parent_state(world: &World, parent: WatchId) -> Option<(ObjectHandle, bool, bool, bool, i32)> {
    let ph = world.objects.by_watch_id(parent)?;
    let p = world.objects.get(ph)?;
    Some((
        ph,
        p.c.is_disabled(),
        p.c.is_destroyed(),
        p.c.awake(),
        p.team_id(),
    ))
}

/// `TerrainObject::update()` (terrobj.cpp:445): first-update flag fixups by sub type, tree fall.
pub fn update_terrain_object(world: &mut World, h: ObjectHandle) -> bool {
    let turn = world.clock.turn;
    let frame_length = world.clock.frame_length;
    let t = table(world);
    let (just, num, cell, power) = {
        let Some(o) = world.objects.get(h) else {
            return true;
        };
        let Kind::Terrain(s) = &o.kind else {
            return true;
        };
        (
            o.c.flag(flags::JUSTCREATED),
            o.c.type_handle,
            o.c.cell,
            s.power_supply,
        )
    };
    let _ = power;
    if just {
        let (sub_type, level) = match terrain_type(&t, num) {
            Some(x) => (x.sub_type, x.damage_level),
            None => (TerrainSubType::None, 0.0),
        };
        let passable = terr::get_passable(&world.terrain, i32::from(cell.0), i32::from(cell.1));
        if let Some(o) = world.objects.get_mut(h) {
            o.c.set_flag(flags::JUSTCREATED, false);
            o.c.set_flag(flags::TILECHANGED, false);
            match sub_type {
                TerrainSubType::None | TerrainSubType::Tree => o.c.set_tangible(true),
                TerrainSubType::Bridge => {
                    if !passable {
                        if let Kind::Terrain(s) = &mut o.kind {
                            s.damage = level;
                        }
                        o.c.set_status(ObjectStatus::Destroyed, false);
                    }
                }
                TerrainSubType::Forest
                | TerrainSubType::WallHeavy
                | TerrainSubType::WallMedium
                | TerrainSubType::WallLight => {
                    if passable {
                        if let Kind::Terrain(s) = &mut o.kind {
                            s.damage = level;
                        }
                        o.c.set_status(ObjectStatus::Destroyed, false);
                    }
                }
            }
        }
    }
    // power out: appearance->setLightsOut (presentation)
    let falling = world
        .objects
        .get(h)
        .is_some_and(|o| o.c.flag(flags::FALLING));
    if falling {
        let just_started = matches!(world.objects.get(h).map(|o| &o.kind), Some(Kind::Terrain(s)) if s.fall_rate == 0.0);
        if let Some(o) = world.objects.get_mut(h) {
            if let Kind::Terrain(s) = &mut o.kind {
                if s.fall_rate == 0.0 {
                    s.fall_rate = TREE_FALL_RATE;
                } else {
                    s.fall_rate += TREE_FALL_ACCEL;
                }
                s.pitch_angle -= frame_length * s.fall_rate;
                if s.pitch_angle < -85.0 {
                    o.c.set_flag(flags::FALLEN, true);
                    o.c.set_flag(flags::FALLING, false);
                }
            }
        }
        if just_started {
            crate::events::emit_sound(world, h, crate::events::sfx::TREEFALL, true);
        }
    }
    let Some(in_view) = recalc_bounds_for_geometry(world, h) else {
        return true; // LOD unavailable: no geometry update
    };
    if in_view {
        if let Some(o) = world.objects.get_mut(h) {
            o.c.windows_visible = turn as i32;
        }
        hooks::structure_update(world, h);
    }
    true
}

/// `Building::updateAnimations()` (bldng.cpp:~400): gesture selection from the building's role.
fn building_update_animations(world: &mut World, h: ObjectHandle) {
    let t = table(world);
    let num = type_num(world, h);
    let (resource_points, mech_bay, team, refit_buddy, destroyed_or_disabled) = {
        let Some(o) = world.objects.get(h) else {
            return;
        };
        let Kind::Building(b) = &o.kind else {
            return;
        };
        let info = building_info(world, num);
        (
            info.map(|i| i.resource_points).unwrap_or(0),
            info.map(|i| i.mech_bay).unwrap_or(false),
            b.team_id,
            b.refit_buddy,
            o.c.is_disabled() || o.c.is_destroyed(),
        )
    };
    let _ = &t;
    if resource_points != 0 {
        // A resource-points building opens when it has been captured (team 0).
        let anim = hooks::structure_gesture(world, h);
        let captured = team == 0;
        let in_t = hooks::structure_in_transition(world, h);
        if captured {
            match anim {
                -1 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 0);
                    }
                }
                2 => {}
                0 => hooks::structure_set_gesture(world, h, 1),
                1 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 2);
                    }
                }
                3 if !in_t => {
                    hooks::structure_set_gesture(world, h, 1);
                }
                _ => {}
            }
        } else {
            match anim {
                -1 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 0);
                    }
                }
                2 => {}
                0 => {}
                1 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 3);
                    }
                }
                3 if !in_t => {
                    hooks::structure_set_gesture(world, h, 0);
                }
                _ => {}
            }
        }
    } else if mech_bay {
        let anim = hooks::structure_gesture(world, h);
        let in_t = hooks::structure_in_transition(world, h);
        if refit_buddy.is_none() {
            // Repairing no one.
            match anim {
                -1 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 0);
                    }
                }
                1 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 5);
                    }
                }
                2 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 6);
                    }
                }
                3 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 7);
                    }
                }
                4 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 8);
                    }
                }
                5..=8 if !in_t => {
                    hooks::structure_set_gesture(world, h, 0);
                }
                _ => {}
            }
        } else {
            match anim {
                -1 => {
                    if !in_t {
                        hooks::structure_set_gesture(world, h, 0);
                    }
                }
                1..=4 => {
                    // In position: the bay's activity effect runs while the mech being refitted is shut down (bldng.cpp:623).
                    let shut_down = world
                        .objects
                        .by_watch_id(refit_buddy)
                        .and_then(|m| world.objects.get(m))
                        .is_some_and(|m| m.c.status == ObjectStatus::ShutDown);
                    if shut_down {
                        if let Some(id) = activity_effect(world, h) {
                            crate::appearance::fx::set_activity(world, h, Some(id), true);
                        }
                    } else {
                        crate::appearance::fx::set_activity(world, h, None, false);
                    }
                }
                0 => {
                    if !in_t {
                        if let Some(mh) = world.objects.by_watch_id(refit_buddy) {
                            let (mpos, tons) = world
                                .objects
                                .get(mh)
                                .map(|m| (m.c.position, m.c.tonnage))
                                .unwrap_or((Vec3::zero(), 0.0));
                            let pos = world
                                .objects
                                .get(h)
                                .map(|o| o.c.position)
                                .unwrap_or_else(Vec3::zero);
                            let d = Vec3 {
                                x: mpos.x - pos.x,
                                y: mpos.y - pos.y,
                                z: mpos.z - pos.z,
                            };
                            let dist = crate::math::approximate_length(&d);
                            if dist <= 128.0 {
                                if tons <= 40.0 {
                                    hooks::structure_set_gesture(world, h, 1);
                                } else if tons <= 60.0 {
                                    hooks::structure_set_gesture(world, h, 2);
                                } else if tons <= 80.0 {
                                    hooks::structure_set_gesture(world, h, 3);
                                } else if tons <= 100.0 {
                                    hooks::structure_set_gesture(world, h, 4);
                                }
                            }
                        }
                    }
                }
                5..=8 if !in_t => {
                    hooks::structure_set_gesture(world, h, 0);
                }
                _ => {}
            }
        }
    } else if !destroyed_or_disabled && !hooks::structure_in_transition(world, h) {
        // Just about everything else animates here.
        hooks::structure_set_gesture(world, h, 0);
    }
}

/// `Building::update()` (bldng.cpp:735).
pub fn update_building(world: &mut World, h: ObjectHandle) -> bool {
    let turn = world.clock.turn;
    let frame_length = world.clock.frame_length;
    let num = type_num(world, h);
    let Some(info) = building_info(world, num) else {
        return true;
    };

    let (just, parent_id) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Building(b) => (o.c.flag(flags::JUSTCREATED), b.parent_id),
            _ => return true,
        },
        None => return true,
    };
    if just {
        if let Some(o) = world.objects.get_mut(h) {
            o.c.set_flag(flags::JUSTCREATED, false);
        }
        // Use the sprite bounds to calculate the extent radius. The appearance bounds are still at their initial values
        // (-999) because nothing has called recalcBounds yet, so the diagonal is zero.
        if world.objects.type_extent_radius(num) < 0.0 {
            let mut actual_size = hooks::appearance_diagonal(world, h);
            actual_size /= crate::terrain::WORLD_UNITS_PER_METER;
            actual_size = (f64::from(actual_size) * 1.25) as f32;
            world.objects.set_type_extent_radius(num, actual_size);
        }
        if world.objects.type_extent_radius(num) != 0.0 {
            if let Some(o) = world.objects.get_mut(h) {
                o.c.set_tangible(true);
            }
        }
        // Resolve the parent control building.
        if parent_id != 0xffff_ffff && parent_id != 0 {
            let row = (parent_id >> 16) as i32;
            let col = (parent_id & 0xffff) as i32;
            if let Some(ph) = world.objects.find_by_cell_position(row, col) {
                let wid = world.objects.watch_id(ph);
                if let Some(Kind::Building(b)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
                    b.parent = wid;
                }
                if can_be_captured(world, ph) {
                    if let Some(p) = world.objects.get_mut(ph) {
                        p.c.set_flag(flags::CAPTURABLE, true);
                    }
                }
            }
        }
    }

    // refitBuddyWID && !getByWatchID(...)
    {
        let rb = match world.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Building(b)) => b.refit_buddy,
            _ => WatchId::NONE,
        };
        if !rb.is_none() && world.objects.by_watch_id(rb).is_none() {
            if let Some(Kind::Building(b)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
                b.refit_buddy = WatchId::NONE;
            }
        }
    }

    // We can be updated more than once a frame (special buildings update first, then again from their block).
    let updated_turn = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Building(b)) => b.updated_turn,
        _ => return true,
    };
    if turn != updated_turn {
        // Perimeter alarms: was there a mover near here last check? (set in the type's handleCollision)
        let mut set_alarm = false;
        {
            let Some(o) = world.objects.get_mut(h) else {
                return true;
            };
            let Kind::Building(b) = &mut o.kind else {
                return true;
            };
            if b.mover_in_proximity {
                b.proximity_timer += frame_length;
            } else {
                b.proximity_timer = 0.0;
            }
            b.mover_in_proximity = false;
            if !world.mission.general_alarm && b.proximity_timer > 0.0 {
                // soundSystem->playDigitalSample(PING_SFX): presentation
                if b.proximity_timer > info.perimeter_alarm_timer {
                    set_alarm = true;
                }
            }
            b.updated_turn = turn;
        }
        if set_alarm {
            // It can NEVER go false unless ABL sets it back.
            hooks::set_general_alarm(world, true);
        }

        building_update_animations(world, h);
        // appearance->setObjectParameters(position, appearance rotation, ...) (bldng.cpp:789), then recalcBounds.
        hooks::set_building_parameters(world, h);
        let bounds = hooks::recalc_bounds_with_lod(world, h);
        let in_view = bounds.unwrap_or_else(|| hooks::appearance_in_view(world, h));
        if num == GENERIC_DESTRUCTIBLE_RESOURCE_BUILDING_OBJNUM {
            // We are a random resource building: mark the terrain under the shape impassable (bldng.cpp:793-797).
            // markMoveMap reads the shape at the current LOD (bdactor.cpp:2801-2805), so an in-view frame whose LOD input
            // is unavailable (`bounds` is None) does not mark: no footprint is invented from a substituted LOD.
            if bounds.is_some() {
                hooks::mark_move_map_from_shape(world, h, false);
            }
        }
        if bounds.is_none() {
            // LOD unavailable: no geometry update.
            if let Some(Kind::Building(b)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
                b.s.lod_unavailable_updates += 1;
            }
        } else {
            hooks::structure_update(world, h);
        }
        if in_view && bounds.is_some() {
            let pos = {
                let o = world.objects.get_mut(h).expect("building exists");
                o.c.windows_visible = turn as i32;
                o.c.position
            };
            let z = terr::elevation(&world.terrain, &pos);
            let mut p = pos;
            p.z = z;
            super::set_position(world, h, p, true);
            // A building with a gosFX "activity" runs it while it stands and is not a mech bay (bldng.cpp:811-818).
            let live = world
                .objects
                .get(h)
                .is_some_and(|o| !o.c.is_destroyed() && !o.c.is_disabled());
            let bay = building_info(world, num).is_some_and(|i| i.mech_bay);
            if live && !bay {
                if let Some(id) = activity_effect(world, h) {
                    crate::appearance::fx::set_activity(world, h, Some(id), true);
                }
            }
        }

        // A destroyed bridge changes the overlay under it; the rest of the destruction is done once.
        let destroyed_once = match world.objects.get(h) {
            Some(o) => match &o.kind {
                Kind::Building(b) => b.base_tile_id != 177 && o.c.status == ObjectStatus::Destroyed,
                _ => false,
            },
            None => false,
        };
        if destroyed_once {
            if let Some(Kind::Building(b)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
                b.base_tile_id = 177;
            }
            if num == BRIDGE_BUILDING_TYPE {
                // Terrain::mapData->setOverlay(DAMAGED_BRIDGE): tile art only (lane T).
                hooks::damage_bridge_overlay(world, h);
            }
            if !info.marks_impassable_when_destroyed {
                if info.sub_type == mc2_sim_data::objtype::building::BuildingSubType::LandBridge {
                    close_sub_areas(world, h);
                } else {
                    open_sub_areas(world, h);
                }
                hooks::clear_path_exists_tables(world);
            }
        }
    }

    // Handle lookout tower: Team::markSeen is a no-op in the shipped build (Terrain::markSeen returns first thing).
    // Handle sensor building: buildings never get a sensorSystem (setSensorData is unreachable), nothing to do.

    // Handle building captured.
    let (parent, my_team) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Building(b) => (b.parent, i32::from(b.team_id)),
            _ => return true,
        },
        None => return true,
    };
    if !parent.is_none() {
        if let Some((_ph, p_disabled, p_destroyed, p_awake, p_team)) = parent_state(world, parent) {
            if !p_disabled && !p_destroyed && p_team != my_team {
                if p_team != hooks::home_team(world) && turn > 5 && my_team != -1 {
                    world.events.push(crate::SimEvent::Betty(
                        crate::events::betty::BUILDING_RECAPTURED,
                    ));
                }
                set_building_team(world, h, p_team, false);
            }
            // Handle parent disabled or destroyed or asleep: put the child to sleep.
            if p_disabled || p_destroyed || !p_awake {
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.set_awake(false);
                }
            }
        }
    }
    true
}

/// `Turret::updateAnimations()` (turret.cpp:444): the pop-up gesture state machine. Returns `canFire`.
fn turret_update_animations(world: &mut World, h: ObjectHandle) -> bool {
    let (awake, target) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Turret(t) => (o.c.awake(), t.target),
            _ => return false,
        },
        None => return false,
    };
    let anim = hooks::structure_gesture(world, h);
    let in_t = hooks::structure_in_transition(world, h);
    let mut can_fire = false;
    if awake && !target.is_none() {
        match anim {
            // NOT UPDATED YET: switch to zero
            -1 => {
                if !in_t {
                    hooks::structure_set_gesture(world, h, 0);
                }
            }
            // Just animating: do nothing
            2 => can_fire = true,
            // Not triggered yet: switch to 1
            0 => hooks::structure_set_gesture(world, h, 1),
            // Triggered: when fully open switch to 2
            1 => {
                if !in_t {
                    hooks::structure_set_gesture(world, h, 2);
                    can_fire = true;
                }
            }
            // Closing: wait until closed and then switch to 1
            3 if !in_t => {
                hooks::structure_set_gesture(world, h, 1);
            }
            _ => {}
        }
    } else {
        match anim {
            -1 => {
                if !in_t {
                    hooks::structure_set_gesture(world, h, 0);
                }
            }
            // Wait until one loop is done, then trigger closing
            2 => hooks::structure_set_gesture(world, h, 3),
            0 => {}
            1 => {
                if !in_t {
                    hooks::structure_set_gesture(world, h, 3);
                }
            }
            3 if !in_t => {
                hooks::structure_set_gesture(world, h, 0);
            }
            _ => {}
        }
    }
    can_fire
}

/// `Turret::calcFireRanges()` (turret.cpp:2219).
fn calc_fire_ranges(world: &mut World, h: ObjectHandle) {
    let t = table(world);
    let num = type_num(world, h);
    let Some(tt) = turret_type(&t, num) else {
        return;
    };
    let mut max_range = 0.0f32;
    let mut min_range = 1_000_000.0f32;
    let mut functional = 0;
    for &w in &tt.weapon_master_id {
        if w != -1 {
            let (min_w, max_w) = hooks::weapon_range_limits(world, w);
            if max_w > max_range {
                max_range = max_w;
            }
            if min_w < min_range {
                min_range = min_w;
            }
            functional += 1;
        }
    }
    if let Some(Kind::Turret(ts)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        ts.max_range = max_range;
        ts.min_range = min_range;
        ts.num_functional_weapons = functional;
    }
}

/// `Turret::relFacingTo(goal)` (turret.cpp:940): like `GameObject::relFacingTo` but relative to the turret rotation.
pub fn turret_rel_facing_to(position: &Vec3, turret_rotation: f32, goal: &Vec3) -> f32 {
    crate::math::rel_facing_to(position, turret_rotation, goal)
}

/// `Turret::update()` (turret.cpp:525). Runs twice per frame for turrets whose vertex is active (block loop and the turret
/// list); there is no `updatedTurn` guard.
pub fn update_turret(world: &mut World, h: ObjectHandle) -> bool {
    let turn = world.clock.turn;
    let frame_length = world.clock.frame_length;
    let t = table(world);
    let num = type_num(world, h);
    let Some(tt) = turret_type(&t, num) else {
        return true;
    };
    let yaw_rate = tt.turret_yaw_rate;
    let weapon0 = tt.weapon_master_id[0];
    let weapon_ids = tt.weapon_master_id;

    let just = world
        .objects
        .get(h)
        .is_some_and(|o| o.c.flag(flags::JUSTCREATED));
    if just {
        let (pos, parent_id) = {
            let o = world.objects.get_mut(h).expect("turret exists");
            o.c.set_flag(flags::JUSTCREATED, false);
            let Kind::Turret(ts) = &o.kind else {
                return true;
            };
            (o.c.position, ts.parent_id)
        };
        // Since we are idling, pick a new random position to point to.
        let mut idle = Vec3::zero();
        {
            let mut rng = world.rng.0.borrow_mut();
            if rng.sim.roll_dice(50) {
                idle.x = pos.x + rng.sim.random_number(500) as f32;
            } else {
                idle.x = pos.x - rng.sim.random_number(500) as f32;
            }
            if rng.sim.roll_dice(50) {
                idle.y = pos.y + rng.sim.random_number(500) as f32;
            } else {
                idle.y = pos.y - rng.sim.random_number(500) as f32;
            }
        }
        idle.z = pos.z;
        let mut parent = WatchId::NONE;
        if parent_id != 0xffff_ffff && parent_id != 0 {
            let row = (parent_id >> 16) as i32;
            let col = (parent_id & 0xffff) as i32;
            if let Some(ph) = world.objects.find_by_cell_position(row, col) {
                parent = world.objects.watch_id(ph);
                if let Some(p) = world.objects.get_mut(ph) {
                    p.c.set_flag(flags::CAPTURABLE, true);
                }
            }
        }
        let punch = hooks::turret_punch(world, &weapon_ids);
        if let Some(o) = world.objects.get_mut(h) {
            if let Kind::Turret(ts) = &mut o.kind {
                ts.idle_position = idle;
                ts.old_position = idle;
                ts.parent = parent;
            }
            o.c.threat_rating = punch as i16;
        }
        calc_fire_ranges(world, h);
    }

    // Turret is dead: react appropriately, do not call any of the fire routines.
    if world
        .objects
        .get(h)
        .is_some_and(|o| o.c.flag(flags::DESTROYED))
    {
        let did_reveal = match world.objects.get_mut(h) {
            Some(o) => {
                o.c.set_flag(flags::TANGIBLE, false);
                match &o.kind {
                    Kind::Turret(ts) => ts.did_reveal,
                    _ => return true,
                }
            }
            None => return true,
        };
        // setObjectParameters, then setMoverParameters(turretRotation, 0, 0) (turret.cpp:573-574): a dead turret keeps its yaw.
        let yaw = match world.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Turret(ts)) => ts.turret_rotation,
            _ => 0.0,
        };
        hooks::set_object_parameters(world, h);
        hooks::set_turret_parameters(world, h, yaw, 0.0);
        let in_view = hooks::recalc_bounds_for_turret(world, h);
        if in_view || !did_reveal || turn < 4 {
            hooks::set_in_view(world, h, true);
            hooks::turret_structure_update(world, h);
            if let Some(o) = world.objects.get_mut(h) {
                o.c.windows_visible = turn as i32;
                if let Kind::Turret(ts) = &mut o.kind {
                    ts.did_reveal = true;
                }
            }
        }
        return true; // NEVER RETURN ANYTHING BUT TRUE
    }

    // turretsEnabled[team] is true unless the team id is bad.
    let team = world.objects.get(h).map(|o| o.team_id()).unwrap_or(-1);
    let mut target = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Turret(ts)) => ts.target,
        _ => return true,
    };
    if team >= MAX_TEAMS as i32 || team < 0 || !TURRETS_ENABLED_DEFAULT {
        target = WatchId::NONE;
    }
    // In case the target has been purged...
    if !target.is_none() && world.objects.by_watch_id(target).is_none() {
        target = WatchId::NONE;
    }
    let our_extent_radius = world.objects.extent_radius(h);
    let pos = world
        .objects
        .get(h)
        .map(|o| o.c.position)
        .unwrap_or_else(Vec3::zero);
    if !target.is_none() {
        match world.objects.by_watch_id(target) {
            Some(th) => {
                let (tpos, t_disabled, t_destroyed) = {
                    let to = world.objects.get(th).expect("target exists");
                    (to.c.position, to.c.is_disabled(), to.c.is_destroyed())
                };
                let dx = tpos.x - pos.x;
                let dy = tpos.y - pos.y;
                let target_range = dx * dx + dy * dy;
                if t_disabled
                    || t_destroyed
                    || hooks::is_friendly(world, h, th)
                    || target_range > (our_extent_radius * our_extent_radius)
                {
                    target = WatchId::NONE;
                }
            }
            None => target = WatchId::NONE,
        }
    }
    if let Some(Kind::Turret(ts)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        ts.target = target;
    }

    let mut active = world.objects.get(h).is_some_and(|o| o.c.awake());
    let parent = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Turret(ts)) => ts.parent,
        _ => return true,
    };
    // Old linkage code set awake in ABL; both are supported.
    if active && !parent.is_none() {
        if let Some((_ph, p_disabled, p_destroyed, p_awake, _pt)) = parent_state(world, parent) {
            if p_destroyed || p_disabled || !p_awake {
                world.events.push(crate::SimEvent::Betty(
                    crate::events::betty::GENERATOR_DESTROYED,
                ));
                active = false;
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.set_awake(false);
                }
                // `appearance->startActivity(TURRET_POWER_DOWN_EFFECT, false)` (turret.cpp:638)
                crate::appearance::fx::set_activity(
                    world,
                    h,
                    Some(crate::events::fx_id::TURRET_POWER_DOWN_EFFECT),
                    false,
                );
            }
        }
    }
    // Must also check that the parent is alive and on the same side.
    if active && !parent.is_none() {
        if let Some((_ph, p_disabled, p_destroyed, _p_awake, p_team)) = parent_state(world, parent)
        {
            if !p_destroyed && !p_disabled && p_team != team {
                if p_team != hooks::home_team(world) && turn > 5 && team != -1 {
                    world.events.push(crate::SimEvent::Betty(
                        crate::events::betty::BUILDING_RECAPTURED,
                    ));
                }
                set_turret_team(world, h, p_team, false);
            }
        }
    }
    // Must reveal every frame for real LOS: Team::markSeen is a no-op in the shipped build.

    let team = world.objects.get(h).map(|o| o.team_id()).unwrap_or(-1);
    let _ = team;
    let (turret_rotation0, idle_position, idle_wait0, target) =
        match world.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Turret(ts)) => (
                ts.turret_rotation,
                ts.idle_position,
                ts.idle_wait,
                ts.target,
            ),
            _ => return true,
        };
    let mut turret_rotation = turret_rotation0;
    let mut idle_wait = idle_wait0;
    let mut idle_pos = idle_position;
    let mut turret_facing = 0.0f32;
    let mut turret_turn_rate = 0.0f32;
    let mut facing_target: Option<bool> = None;

    if active && !target.is_none() {
        // Rotate turret to point at target.
        if let Some(th) = world.objects.by_watch_id(target) {
            let tpos = world
                .objects
                .get(th)
                .map(|o| o.c.position)
                .unwrap_or_else(Vec3::zero);
            turret_facing = turret_rel_facing_to(&pos, turret_rotation, &tpos);
        }
        if !(-10.0..=10.0).contains(&turret_facing) {
            // We can and will shift facing to destination...
            let max_rate = yaw_rate;
            if turret_facing < 0.0 {
                turret_turn_rate = max_rate * frame_length;
                if turret_turn_rate > turret_facing.abs() {
                    turret_turn_rate = turret_facing.abs();
                }
            } else {
                turret_turn_rate = -max_rate * frame_length;
                if turret_turn_rate < -turret_facing {
                    turret_turn_rate = -turret_facing;
                }
            }
            facing_target = Some(false);
        } else {
            facing_target = Some(true);
        }
        turret_rotation += turret_turn_rate;
    } else if active && target.is_none() {
        // Idle: slowly rotate towards a random spot.
        turret_facing = turret_rel_facing_to(&pos, turret_rotation, &idle_pos);
        if !(-10.0..=10.0).contains(&turret_facing) {
            let max_rate = yaw_rate;
            if turret_facing < 0.0 {
                turret_turn_rate = max_rate * frame_length;
                if turret_turn_rate > turret_facing.abs() {
                    turret_turn_rate = turret_facing.abs();
                }
            } else {
                turret_turn_rate = -max_rate * frame_length;
                if turret_turn_rate < -turret_facing {
                    turret_turn_rate = -turret_facing;
                }
            }
            facing_target = Some(false);
        } else {
            facing_target = Some(true);
            idle_wait += frame_length;
            if idle_wait > 10.0 {
                // Since we are idling, pick a new random position to point to.
                let ext = our_extent_radius as i32;
                let mut rng = world.rng.0.borrow_mut();
                if rng.sim.roll_dice(50) {
                    idle_pos.x = pos.x + rng.sim.random_number(ext) as f32;
                } else {
                    idle_pos.x = pos.x - rng.sim.random_number(ext) as f32;
                }
                if rng.sim.roll_dice(50) {
                    idle_pos.y = pos.y + rng.sim.random_number(ext) as f32;
                } else {
                    idle_pos.y = pos.y - rng.sim.random_number(ext) as f32;
                }
                idle_pos.z = pos.z;
                idle_wait = 0.0;
            }
        }
        turret_rotation += turret_turn_rate;
    }

    if turret_rotation > 360.0 {
        turret_rotation -= 360.0;
    }
    if turret_rotation < -360.0 {
        turret_rotation += 360.0;
    }
    {
        let Some(o) = world.objects.get_mut(h) else {
            return true;
        };
        if let Some(f) = facing_target {
            o.c.set_flag(flags::FACING_TARGET, f);
        }
        if let Kind::Turret(ts) = &mut o.kind {
            ts.turret_rotation = turret_rotation;
            ts.idle_wait = idle_wait;
            ts.idle_position = idle_pos;
        }
    }

    // turret.cpp:777-792: the spotlight pitch (weapon-less turrets) from the distance the light has moved, and -35 when the
    // parent is down. Pitch only feeds the appearance.
    let old_position = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Turret(ts)) => ts.old_position,
        _ => return true,
    };
    let spot_distance = crate::math::approximate_length(&Vec3 {
        x: old_position.x - pos.x,
        y: old_position.y - pos.y,
        z: old_position.z - pos.z,
    });
    let mut turret_pitch = 0.0f32;
    if weapon0 == -1 {
        turret_pitch = (1.0 - (spot_distance / our_extent_radius)) * MAX_TURRET_PITCH;
        if turret_pitch > 0.0 {
            turret_pitch = 0.0;
        }
    }
    if !active {
        turret_pitch = -35.0;
    }

    // turret.cpp:801-805: setObjectParameters, setMoverParameters, recalcBounds, then updateAnimations.
    hooks::set_object_parameters(world, h);
    hooks::set_turret_parameters(world, h, turret_rotation, turret_pitch);
    // The camera's LOD choice only selects which shape's nodes are transformed; turret targeting, gesture and fire do not depend
    // on it, so without a view-supplied LOD the appearance's own (base) LOD is used.
    let in_view = hooks::recalc_bounds_for_turret(world, h);
    let can_fire = turret_update_animations(world, h);
    let did_reveal = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Turret(ts)) => ts.did_reveal,
        _ => return true,
    };
    if in_view || !did_reveal || turn < 4 {
        hooks::set_in_view(world, h, true);
        hooks::turret_structure_update(world, h);
        if let Some(o) = world.objects.get_mut(h) {
            o.c.windows_visible = turn as i32;
            if let Kind::Turret(ts) = &mut o.kind {
                ts.did_reveal = true;
            }
        }
    }

    // Spotlights (weapon type -1) create a night light: presentation only.
    let _ = weapon0;

    if let Some(o) = world.objects.get_mut(h) {
        o.c.set_flag(flags::CAN_FIRE, true);
    }
    for i in 0..MAX_TURRET_WEAPONS {
        let (destroyed_flag, target) = match world.objects.get(h) {
            Some(o) => (
                o.c.flag(flags::DESTROYED),
                match &o.kind {
                    Kind::Turret(ts) => ts.target,
                    _ => return true,
                },
            ),
            None => return true,
        };
        if !destroyed_flag
            && !target.is_none()
            && turret_weapon_ready(world, h, i)
            && active
            && can_fire
        {
            if let Some(th) = world.objects.by_watch_id(target) {
                crate::combat::turret::fire(world, h, th, i);
            }
        }
    }
    true
}

/// `Turret::isWeaponReady(weaponId)` (turret.cpp:964).
pub fn turret_weapon_ready(world: &World, h: ObjectHandle, weapon_id: usize) -> bool {
    let t = table(world);
    let num = type_num(world, h);
    let Some(tt) = turret_type(&t, num) else {
        return false;
    };
    if tt.weapon_master_id[weapon_id] == -1 {
        return false;
    }
    let Some(o) = world.objects.get(h) else {
        return false;
    };
    let Kind::Turret(ts) = &o.kind else {
        return false;
    };
    if ts.ready_time[weapon_id] > world.clock.scenario_time
        || !o.c.flag(flags::CAN_FIRE)
        || !o.c.flag(flags::FACING_TARGET)
    {
        return false;
    }
    true
}

/// `Gate::update()` (gate.cpp:233).
pub fn update_gate(world: &mut World, h: ObjectHandle) -> bool {
    let turn = world.clock.turn;
    let (just, parent_id, team) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Gate(g) => (
                o.c.flag(flags::JUSTCREATED),
                g.parent_id,
                i32::from(g.team_id),
            ),
            _ => return true,
        },
        None => return true,
    };
    let _ = team;
    if just {
        if let Some(o) = world.objects.get_mut(h) {
            o.c.set_flag(flags::JUSTCREATED, false);
        }
        // Figure out what the box should be: appearance update + recalcBounds + highlight (presentation)
        hooks::structure_update(world, h);
        hooks::recalc_bounds(world, h);
        if parent_id != 0xffff_ffff && parent_id != 0 {
            let row = (parent_id >> 16) as i32;
            let col = (parent_id & 0xffff) as i32;
            if let Some(ph) = world.objects.find_by_cell_position(row, col) {
                let wid = world.objects.watch_id(ph);
                if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
                    g.parent = wid;
                }
                if let Some(p) = world.objects.get_mut(ph) {
                    p.c.set_flag(flags::CAPTURABLE, true);
                }
            }
        }
    }

    // Keep the global map areas pointing at this gate (index i of the hover list reads the same i as the ground list).
    {
        let (sub0, sub1, destroyed, team) = {
            let Some(o) = world.objects.get(h) else {
                return true;
            };
            let Kind::Gate(g) = &o.kind else {
                return true;
            };
            (
                g.s.sub_areas[0].clone(),
                g.s.sub_areas[1].clone(),
                o.c.status == ObjectStatus::Destroyed,
                g.team_id,
            )
        };
        let wid = world.objects.watch_id(h);
        for (i, &a0) in sub0.iter().enumerate() {
            let a1 = sub1.get(i).copied().unwrap_or(0);
            terr::global_map_mut(&mut world.terrain, 0)
                .set_area_owner_wid(i32::from(a0), wid.0 as i32);
            terr::global_map_mut(&mut world.terrain, 1)
                .set_area_owner_wid(i32::from(a1), wid.0 as i32);
            if destroyed {
                terr::global_map_mut(&mut world.terrain, 0).set_area_team_id(i32::from(a0), -1);
                terr::global_map_mut(&mut world.terrain, 1).set_area_team_id(i32::from(a1), -1);
            } else {
                terr::global_map_mut(&mut world.terrain, 0).set_area_team_id(i32::from(a0), team);
                terr::global_map_mut(&mut world.terrain, 1).set_area_team_id(i32::from(a1), team);
            }
        }
    }

    // We can be called more than once a frame (gates always update, and again from their block).
    let updated_turn = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Gate(g)) => g.updated_turn,
        _ => return true,
    };
    if turn != updated_turn {
        if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
            g.updated_turn = turn;
        }
        let parent = match world.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Gate(g)) => g.parent,
            _ => return true,
        };
        if !parent.is_none() {
            if let Some((_ph, p_disabled, p_destroyed, p_awake, p_team)) =
                parent_state(world, parent)
            {
                let my_team = world.objects.get(h).map(|o| o.team_id()).unwrap_or(-1);
                // Handle building captured.
                if !p_disabled && !p_destroyed && p_team != my_team {
                    set_gate_team(world, h, p_team, false);
                }
                // Lock the gate closed if its parent is dead, disabled or asleep.
                if p_disabled || p_destroyed || !p_awake {
                    if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
                        g.locked_open = false;
                        g.locked_close = true;
                    }
                }
            }
        }

        open_gate(world, h); // actually updates the gate states
        if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
            g.closest_object = ObjectHandle::NONE; // reset every time openGate completes
        }

        // MUST update the appearance last: if the gate state changed, the LOD is no longer valid.
        let Some(in_view) = recalc_bounds_for_geometry(world, h) else {
            return true; // LOD unavailable: no geometry update
        };
        hooks::set_in_view(world, h, true);
        hooks::structure_update(world, h);
        hooks::set_in_view(world, h, in_view);
        if in_view {
            let pos = {
                let o = world.objects.get_mut(h).expect("gate exists");
                o.c.windows_visible = turn as i32;
                o.c.position
            };
            let mut p = pos;
            p.z = terr::elevation(&world.terrain, &pos);
            super::set_position(world, h, p, true);
        }
    }
    true
}

/// `Gate::openGate()` (gate.cpp:~330): drive the gate state from the "electric eye".
pub fn open_gate(world: &mut World, h: ObjectHandle) {
    let turn = world.clock.turn;
    let (destroyed, locked_close, locked_open, reason) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Gate(g) => (
                o.c.is_destroyed(),
                g.locked_close,
                g.locked_open,
                g.reason_to_open,
            ),
            _ => return,
        },
        None => return,
    };
    if destroyed {
        return;
    }
    let set_state =
        |world: &mut World, opened: bool, opening: bool, closed: bool, closing: bool| {
            if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
                g.opened = opened;
                g.opening = opening;
                g.closed = closed;
                g.closing = closing;
            }
        };
    if locked_close || !reason {
        let anim = hooks::structure_gesture(world, h);
        if anim == -1 || anim == 0 {
            // Never updated / already closed.
            hooks::structure_set_gesture(world, h, 0);
            set_state(world, false, false, true, false);
        } else if anim == 1 {
            // Gate is opening.
            if !hooks::structure_in_transition(world, h) {
                // Done opening: switch to closing.
                hooks::structure_set_gesture(world, h, 3);
                set_state(world, false, false, false, true);
                crate::events::emit_sound(world, h, crate::events::sfx::GATE_CLOSE, true);
            } else {
                set_state(world, false, true, false, false);
            }
        } else if anim == 2 {
            // Gate is open: start it closing.
            hooks::structure_set_gesture(world, h, 3);
            crate::events::emit_sound(world, h, crate::events::sfx::GATE_CLOSE, true);
            set_state(world, false, false, false, true);
        } else if anim == 3 {
            // Gate is closing.
            if !hooks::structure_in_transition(world, h) {
                hooks::structure_set_gesture(world, h, 0);
                set_state(world, false, false, true, false);
                // Check if something is "inside" the gate; if so, blow it and the gate.
                blow_any_offending_object(world, h);
            } else {
                set_state(world, false, false, false, true);
            }
        }
    } else if locked_open || reason {
        let anim = hooks::structure_gesture(world, h);
        if anim == -1 {
            // Never updated: closed by default. (No else-if in the C++: the next test sees the same animState.)
            hooks::structure_set_gesture(world, h, 1);
            crate::events::emit_sound(world, h, crate::events::sfx::GATE_OPEN, true);
            set_state(world, false, true, false, false);
        }
        if anim == 0 {
            hooks::structure_set_gesture(world, h, 1);
            crate::events::emit_sound(world, h, crate::events::sfx::GATE_OPEN, true);
            set_state(world, false, true, false, false);
        } else if anim == 1 {
            if !hooks::structure_in_transition(world, h) {
                hooks::structure_set_gesture(world, h, 2);
                set_state(world, true, false, false, false);
            } else {
                set_state(world, false, true, false, false);
            }
        } else if anim == 2 {
            hooks::structure_set_gesture(world, h, 2);
            set_state(world, true, false, false, false);
        } else if anim == 3 {
            if !hooks::structure_in_transition(world, h) {
                hooks::structure_set_gesture(world, h, 1);
                crate::events::emit_sound(world, h, crate::events::sfx::GATE_OPEN, true);
                set_state(world, false, true, false, false);
            } else {
                set_state(world, false, false, false, true);
            }
        }
    }

    let opened = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Gate(g)) => g.opened,
        _ => return,
    };
    if opened && turn > 3 {
        // Mark passable when open so the side pieces of the gate stay impassable.
        open_sub_areas(world, h);
        if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
            g.last_marked_open = true;
        }
    } else if !opened && turn > 3 {
        // Any other state but opened is closed.
        close_sub_areas(world, h);
        hooks::mark_los(world, h, false);
        if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
            g.last_marked_open = false;
        }
    }
    if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        g.reason_to_open = false; // always reset the electric eye
    }
}

/// `Gate::blowAnyOffendingObject()` (gate.cpp:~290): a gate that closes on a mover destroys both.
fn blow_any_offending_object(world: &mut World, h: ObjectHandle) {
    let closest = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Gate(g)) => g.closest_object,
        _ => return,
    };
    if closest.is_none() {
        return;
    }
    let (gpos, mpos, mtype) = {
        let Some(g) = world.objects.get(h) else {
            return;
        };
        let Some(m) = world.objects.get(closest) else {
            return;
        };
        (g.c.position, m.c.position, m.c.type_handle)
    };
    let dx = gpos.x - mpos.x;
    let dy = gpos.y - mpos.y;
    let object_range = dx * dx + dy * dy;
    let mut little_radius = little_extent(world, h) + world.objects.type_extent_radius(mtype);
    little_radius *= little_radius;
    if object_range < little_radius {
        // Put about a million points of damage into the closest object.
        let mut shot = WeaponShotInfo {
            attacker: WatchId::NONE,
            master_id: -3,
            damage: 250.0,
            hit_location: 0,
            entry_angle: 0.0,
        };
        for _ in 0..10 {
            shot.hit_location = hooks::calc_hit_location(
                world,
                closest,
                ObjectHandle::NONE,
                -1,
                AttackSource::Artillery,
                0,
            );
            hooks::weapon_hit(world, closest, &shot);
        }
        // The gate must die too.
        let level = damage_level(world, h);
        let shot = WeaponShotInfo {
            attacker: WatchId::NONE,
            master_id: -3,
            damage: level + 5.0,
            hit_location: 0,
            entry_angle: 0.0,
        };
        hooks::weapon_hit(world, h, &shot);
    }
}

/// `Gate::destroyGate()` (gate.cpp:~620): called by the gate's `handleWeaponHit` (lane C) once the damage level is reached.
pub fn destroy_gate(world: &mut World, h: ObjectHandle) {
    let just_created = world
        .objects
        .get(h)
        .is_some_and(|o| o.c.flag(flags::JUSTCREATED));
    if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        g.just_destroyed = false;
    }
    if !just_created {
        // First mark every occupied cell passable for the original shape; the destroyed shape marks impassable again.
        mark_move_map(world, h, true);
        hooks::clear_path_exists_tables(world);
        // Unmark these cells as gate cells, so they are passable.
        let cells: Vec<(i16, i16)> = world
            .objects
            .get(h)
            .and_then(|o| o.structure())
            .map(|s| s.cells_covered.clone())
            .unwrap_or_default();
        for (r, c) in cells {
            terr::set_gate(&mut world.terrain, i32::from(r), i32::from(c), false);
        }
    }
    hooks::mark_los(world, h, true);
    hooks::set_obj_status(world, h, ObjectStatus::Destroyed);
    recalc_bounds_and_update(world, h);
    if !just_created {
        open_sub_areas(world, h);
        hooks::mark_los(world, h, false);
    }
    let pos = {
        let Some(o) = world.objects.get_mut(h) else {
            return;
        };
        if let Kind::Gate(g) = &mut o.kind {
            g.opened = true;
            g.closed = false;
            g.closing = false;
            g.opening = false;
            g.locked_open = true;
            g.just_destroyed = false;
        }
        o.c.set_flag(flags::TANGIBLE, false);
        // Gate is dead: you may no longer collide with it.
        o.c.set_status(ObjectStatus::Destroyed, false);
        o.c.position
    };
    let _ = pos;
    if !just_created {
        let (dmg, rad) = world
            .objects
            .get(h)
            .map(|o| (o.c.expl_damage, o.c.expl_radius))
            .unwrap_or((0.0, 0.0));
        let at = world
            .objects
            .get(h)
            .map(|o| o.c.position)
            .unwrap_or_else(Vec3::zero);
        super::ObjectStore::create_explosion(world, BUILDING_EXPLOSION_ID, Some(h), at, dmg, rad);
    }
}

/// `BUILDING_EXPLOSION_ID` (mclib/weaponfx.h:12).
pub const BUILDING_EXPLOSION_ID: i32 = 45;
/// `TURRET_EXPLOSION_ID`.
pub const TURRET_EXPLOSION_ID: i32 = 44;

// ------------------------------------------------------------------------------------------------------------------
// Type-level collision reactions (`ObjectType::handleCollision(collidee, collider)`)
// ------------------------------------------------------------------------------------------------------------------

fn shot(attacker: WatchId, master_id: i32, damage: f32) -> WeaponShotInfo {
    WeaponShotInfo {
        attacker,
        master_id,
        damage,
        hit_location: 0,
        entry_angle: 0.0,
    }
}

/// `TerrainObjectType::handleCollision` (terrobj.cpp:240). Returns the C++ result (always `true`).
pub fn terrain_handle_collision(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    let t = table(world);
    let num = type_num(world, collidee);
    let Some(tt) = terrain_type(&t, num) else {
        return true;
    };
    let (class, is_mover, path_locks, kpos) = match world.objects.get(collider) {
        Some(o) => (
            o.c.class,
            o.is_mover(),
            match &o.kind {
                Kind::Mech(m) => m.m.path_locks_enabled,
                Kind::Vehicle(v) => v.m.path_locks_enabled,
                _ => false,
            },
            o.c.position,
        ),
        None => return true,
    };
    let steppable = !is_mover || path_locks;
    match tt.sub_type {
        TerrainSubType::None => {
            if matches!(
                class,
                ObjectClass::Explosion | ObjectClass::BattleMech | ObjectClass::GroundVehicle
            ) && steppable
            {
                let dmg = damage_level(world, collidee);
                let s = shot(WatchId::NONE, -1, dmg);
                hooks::weapon_hit(world, collidee, &s);
            }
        }
        TerrainSubType::Tree => {
            if matches!(
                class,
                ObjectClass::Explosion | ObjectClass::BattleMech | ObjectClass::GroundVehicle
            ) && steppable
            {
                let (falling, fallen, pos, rot) = {
                    let o = world.objects.get(collidee).expect("collidee exists");
                    (
                        o.c.flag(flags::FALLING),
                        o.c.flag(flags::FALLEN),
                        o.c.position,
                        o.c.rotation,
                    )
                };
                if !fallen && !falling {
                    let fall_angle = crate::math::rel_facing_to(&pos, rot, &kpos);
                    if let Some(o) = world.objects.get_mut(collidee) {
                        o.c.set_flag(flags::FALLING, true);
                        // collidee->rotate(fallAngle, 0.0f): rotation = yaw, pitchAngle = pitch
                        o.c.rotation = fall_angle;
                        if let Kind::Terrain(s) = &mut o.kind {
                            s.pitch_angle = 0.0;
                        }
                        // Tree has fallen. You may no longer collide with it.
                        o.c.set_tangible(false);
                    }
                }
            }
        }
        TerrainSubType::Bridge
        | TerrainSubType::Forest
        | TerrainSubType::WallHeavy
        | TerrainSubType::WallMedium => {}
        TerrainSubType::WallLight => {
            if matches!(class, ObjectClass::BattleMech | ObjectClass::GroundVehicle) {
                let wid = world.objects.watch_id(collider);
                let s = shot(wid, -1, 250.0);
                hooks::weapon_hit(world, collidee, &s);
            }
        }
    }
    true
}

/// `TerrainObjectType::handleDestruction` (terrobj.cpp:323): forests open their sub areas. Returns false.
pub fn terrain_handle_destruction(world: &mut World, collidee: ObjectHandle) -> bool {
    let t = table(world);
    let num = type_num(world, collidee);
    if terrain_type(&t, num).is_some_and(|x| x.sub_type == TerrainSubType::Forest) {
        open_sub_areas(world, collidee);
    }
    false
}

/// `BuildingType::handleCollision` (bldng.cpp:320). Returns the C++ result (`true`).
pub fn building_handle_collision(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    let num = type_num(world, collidee);
    let Some(info) = building_info(world, num) else {
        return true;
    };
    let (class, k_team, k_free_time) = match world.objects.get(collider) {
        Some(o) => (o.c.class, o.team_id(), o.c.collision_free_time),
        None => return true,
    };
    let c_team = world
        .objects
        .get(collidee)
        .map(|o| o.team_id())
        .unwrap_or(-1);
    let mut apply = false;
    match class {
        ObjectClass::BattleMech | ObjectClass::GroundVehicle | ObjectClass::Elemental => {
            if info.perimeter_alarm_range > 0.0
                && info.perimeter_alarm_timer > 0.0
                && k_team != c_team
            {
                if let Some(Kind::Building(b)) =
                    world.objects.get_mut(collidee).map(|o| &mut o.kind)
                {
                    b.mover_in_proximity = true;
                }
                return true; // don't blow the perimeter alarm up
            }
            apply = true;
        }
        ObjectClass::TerrN | ObjectClass::Explosion | ObjectClass::Fire => apply = true,
        _ => {}
    }
    if apply {
        // `if (collider->getCollisionFreeTime() < scenarioTime) return(true);`: only a collider that was recently in a
        // collision (free time not yet passed) damages the building.
        if k_free_time < world.clock.scenario_time {
            return true;
        }
        let s = shot(WatchId::NONE, -1, 10.0);
        hooks::weapon_hit(world, collidee, &s);
    }
    true
}

/// `TurretType::handleCollision` (turret.cpp:238): pick the closest enemy mover as the new target. Returns `true`.
pub fn turret_handle_collision(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    let (tpos, target, pop_neutrals) = match world.objects.get(collidee) {
        Some(o) => match &o.kind {
            Kind::Turret(t) => (o.c.position, t.target, o.c.flag(flags::POP_NEUTRALS)),
            _ => return true,
        },
        None => return true,
    };
    let mut target_range = 10_000_000.0f32;
    if let Some(th) = world.objects.by_watch_id(target) {
        if let Some(to) = world.objects.get(th) {
            let dx = tpos.x - to.c.position.x;
            let dy = tpos.y - to.c.position.y;
            target_range = dx * dx + dy * dy;
        }
    }
    // Turrets can check for neutral.
    if (pop_neutrals && hooks::is_neutral(world, collidee, collider))
        || hooks::is_enemy(world, collidee, collider)
    {
        let (class, kpos, dead) = match world.objects.get(collider) {
            Some(o) => (
                o.c.class,
                o.c.position,
                o.c.is_disabled() || o.c.is_destroyed(),
            ),
            None => return true,
        };
        match class {
            ObjectClass::BattleMech | ObjectClass::GroundVehicle | ObjectClass::Elemental => {
                if !dead {
                    let dx = tpos.x - kpos.x;
                    let dy = tpos.y - kpos.y;
                    let collider_range = dx * dx + dy * dy;
                    if collider_range < target_range {
                        let wid = world.objects.watch_id(collider);
                        if let Some(Kind::Turret(t)) =
                            world.objects.get_mut(collidee).map(|o| &mut o.kind)
                        {
                            t.target = wid;
                        }
                    }
                }
            }
            ObjectClass::CameraDrone
                if !world
                    .objects
                    .get(collider)
                    .is_some_and(|o| o.c.is_destroyed()) =>
            {
                let dx = tpos.x - kpos.x;
                let dy = tpos.y - kpos.y;
                let collider_range = dx * dx + dy * dy;
                if collider_range < target_range {
                    let wid = world.objects.watch_id(collider);
                    if let Some(Kind::Turret(t)) =
                        world.objects.get_mut(collidee).map(|o| &mut o.kind)
                    {
                        t.target = wid;
                    }
                }
            }
            _ => {}
        }
    }
    true
}

/// `GateType::handleCollision` (gate.cpp:166): opens the gate for movers on its side and remembers the last mover seen.
pub fn gate_handle_collision(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    let t = table(world);
    let num = type_num(world, collidee);
    let Some(gt) = gate_type(&t, num) else {
        return true;
    };
    let open_radius = gt.open_radius;
    let (class, kpos, k_team, dead) = match world.objects.get(collider) {
        Some(o) => (
            o.c.class,
            o.c.position,
            o.team_id(),
            o.c.is_disabled() || o.c.is_destroyed(),
        ),
        None => return true,
    };
    let (gpos, gate_team) = match world.objects.get(collidee) {
        Some(o) => (o.c.position, o.team_id()),
        None => return true,
    };
    if matches!(
        class,
        ObjectClass::BattleMech | ObjectClass::GroundVehicle | ObjectClass::Elemental
    ) {
        let dx = gpos.x - kpos.x;
        let dy = gpos.y - kpos.y;
        let collider_range = dx * dx + dy * dy;
        if (k_team == gate_team || gate_team == -1)
            && !dead
            && collider_range < open_radius * open_radius
        {
            if let Some(Kind::Gate(g)) = world.objects.get_mut(collidee).map(|o| &mut o.kind) {
                g.reason_to_open = true;
            }
        }
        // Regardless of alignment, if I'm closest, mark me (closestRange is a local initialised per call, so the last mover
        // seen always wins).
        let closest_range = 119_999_999.99f32;
        if collider_range < closest_range && !dead && !hooks::is_jumping(world, collider) {
            if let Some(Kind::Gate(g)) = world.objects.get_mut(collidee).map(|o| &mut o.kind) {
                g.closest_object = collider;
            }
        }
    }
    true
}

/// `ObjectType::handleDestruction` for the structure types: building, turret and gate return false (the object is not
/// removed).
pub fn structure_handle_destruction(world: &mut World, collidee: ObjectHandle) -> bool {
    match world.objects.get(collidee).map(|o| &o.kind) {
        Some(Kind::Terrain(_)) => terrain_handle_destruction(world, collidee),
        _ => false,
    }
}

/// Used by `Object` lookups of the type for a handle: `ObjectType` for structures.
pub fn object_type_of(table: &ObjectTable, num: i32) -> Option<&ObjectType> {
    table.get(num as u32)
}

// ------------------------------------------------------------------------------------------------------------------
// handleWeaponHit of the structures (terrobj.cpp:804, bldng.cpp:1421, turret.cpp:2179, gate.cpp:~600)
// ------------------------------------------------------------------------------------------------------------------

/// `EMPTY_EXPLOSION_ID` (mclib/weaponfx.h:25).
pub const EMPTY_EXPLOSION_ID: i32 = 54;

/// `ObjectType::createExplosion(position, dmg, rad)` (objtype.cpp:177) for the explosion object of the object's type: the
/// effect id goes through `GetEffectObjNum` twice (once here, once in `GameObjectManager::createExplosion`).
fn type_create_explosion(world: &mut World, h: ObjectHandle, position: Vec3, dmg: f32, rad: f32) {
    let num = type_num(world, h);
    let explosion_object = world
        .objects
        .type_by_num(num)
        .map(|t| t.base.explosion_object)
        .unwrap_or(-1);
    let effect_id = world.objects.effect_object_number(explosion_object);
    if explosion_object >= 0 {
        let rad = if rad == 0.0 {
            world.objects.type_extent_radius(num)
        } else {
            rad
        };
        super::ObjectStore::create_explosion(world, effect_id, None, position, dmg, rad);
    }
}

/// `handleWeaponHit(shotInfo)` for terrain objects, buildings, turrets and gates. Returns `NO_ERR` (0).
pub fn handle_weapon_hit(world: &mut World, h: ObjectHandle, shot: &WeaponShotInfo) -> i32 {
    match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Terrain(_)) => terrain_weapon_hit(world, h, shot),
        Some(Kind::Building(_)) => building_weapon_hit(world, h, shot),
        Some(Kind::Turret(_)) => turret_weapon_hit(world, h, shot),
        Some(Kind::Gate(_)) => gate_weapon_hit(world, h, shot),
        _ => {}
    }
    0
}

/// `TerrainObject::handleWeaponHit` (terrobj.cpp:804).
fn terrain_weapon_hit(world: &mut World, h: ObjectHandle, shot: &WeaponShotInfo) {
    let t = table(world);
    let num = type_num(world, h);
    let Some(tt) = terrain_type(&t, num) else {
        return;
    };
    let level = tt.damage_level;
    let (damaged, cur_damage0, pos) = match world.objects.get(h) {
        Some(o) => (
            o.c.flag(flags::DAMAGED),
            o.structure().map(|s| s.damage).unwrap_or(0.0),
            o.c.position,
        ),
        None => return,
    };
    if damaged {
        return;
    }
    let mut cur_damage = cur_damage0;
    match tt.sub_type {
        TerrainSubType::None => {
            cur_damage += shot.damage;
            if cur_damage > level {
                cur_damage = level;
            }
            set_terrain_damage(world, h, cur_damage as i64);
            if cur_damage >= level {
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.set_flag(flags::DAMAGED, true);
                    o.c.set_status(ObjectStatus::Destroyed, false);
                    o.c.set_tangible(false);
                }
                // appearance markLOS/setObjStatus/setLightsOut/recalcBounds/update: presentation (+ LOS heights, lane F)
                hooks::mark_los(world, h, true);
                recalc_bounds_and_update(world, h);
                hooks::mark_los(world, h, false);
                // A dust poof plays when somebody stepped on it (presentation).
            }
        }
        TerrainSubType::Tree => {
            set_terrain_damage(world, h, (cur_damage + 1.0) as i64);
            if cur_damage > level {
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.set_flag(flags::DAMAGED, true);
                    o.c.set_status(ObjectStatus::Destroyed, false);
                }
                hooks::mark_los(world, h, true);
                hooks::set_obj_status(world, h, ObjectStatus::Destroyed);
                recalc_bounds_and_update(world, h);
                hooks::mark_los(world, h, false);
            }
        }
        TerrainSubType::Forest => {
            cur_damage += shot.damage;
            if cur_damage >= level {
                type_create_explosion(world, h, pos, 0.0, 0.0);
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.set_status(ObjectStatus::Destroyed, false);
                }
                open_sub_areas(world, h);
            }
            set_terrain_damage(world, h, cur_damage as i64);
        }
        TerrainSubType::WallHeavy | TerrainSubType::WallMedium | TerrainSubType::WallLight => {
            cur_damage += shot.damage;
            if cur_damage >= level {
                type_create_explosion(world, h, pos, 0.0, 0.0);
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.set_status(ObjectStatus::Destroyed, false);
                }
                if matches!(
                    terrain_type(&table(world), type_num(world, h)).map(|t| t.sub_type),
                    Some(TerrainSubType::WallLight)
                ) {
                    crate::events::emit_sound(world, h, crate::events::sfx::BREAKINGFENCE, true);
                }
            }
            set_terrain_damage(world, h, cur_damage as i64);
        }
        TerrainSubType::Bridge => {}
    }
}

/// `Building::handleWeaponHit` (bldng.cpp:1421).
fn building_weapon_hit(world: &mut World, h: ObjectHandle, shot: &WeaponShotInfo) {
    let num = type_num(world, h);
    let Some(info) = building_info(world, num) else {
        return;
    };
    let (destroyed, cur_damage, animated, fallen, falling) = match world.objects.get(h) {
        Some(o) => (
            o.c.is_destroyed(),
            o.structure().map(|s| s.damage).unwrap_or(0.0),
            o.c.flag(flags::ANIMATED),
            o.c.flag(flags::FALLEN),
            o.c.flag(flags::FALLING),
        ),
        None => return,
    };
    if destroyed {
        return;
    }
    let mut dmg = cur_damage + shot.damage;
    if let Some(o) = world.objects.get_mut(h) {
        o.c.set_flag(flags::DAMAGED, true);
    }
    if dmg >= info.damage_level {
        dmg = info.damage_level;
        let mut blow_it_up = false;
        if animated {
            if !fallen && !falling {
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.set_flag(flags::FALLING, true);
                    o.c.set_tangible(false);
                    o.c.set_status(ObjectStatus::Destroyed, false);
                }
                // appearance->stopActivity(); sensorSystem->disable(): buildings have no sensor system
                hooks::attacker_killed_target(world, shot.attacker, h);
                blow_it_up = true;
            }
        } else {
            if let Some(o) = world.objects.get_mut(h) {
                o.c.set_status(ObjectStatus::Destroyed, false);
                o.c.set_tangible(false);
            }
            blow_it_up = true;
        }
        if blow_it_up {
            // Blow the building up using a generic building explosion, unless it has a special gosFX destruction.
            let hit_node_pos = hooks::hit_node(world, h);
            let (dmg_e, rad_e) = world
                .objects
                .get(h)
                .map(|o| (o.c.expl_damage, o.c.expl_radius))
                .unwrap_or((0.0, 0.0));
            let id = if hooks::play_destruction(world, h) {
                EMPTY_EXPLOSION_ID
            } else {
                BUILDING_EXPLOSION_ID
            };
            super::ObjectStore::create_explosion(world, id, None, hit_node_pos, dmg_e, rad_e);
            hooks::mark_los(world, h, true);
            hooks::set_obj_status(world, h, ObjectStatus::Destroyed);
            recalc_bounds_and_update(world, h);
            hooks::mark_los(world, h, false);
            if !info.marks_impassable_when_destroyed {
                if info.sub_type == BuildingSubType::LandBridge {
                    close_sub_areas(world, h);
                } else {
                    open_sub_areas(world, h);
                }
                hooks::clear_path_exists_tables(world);
            }
            if info.sub_type == BuildingSubType::LandBridge {
                // Blow every mover standing on a cell that just went impassable (they cannot be salvaged either).
                let cells: Vec<(i16, i16)> = world
                    .objects
                    .get(h)
                    .and_then(|o| o.structure())
                    .map(|s| s.cells_covered.clone())
                    .unwrap_or_default();
                for (r, c) in cells {
                    let movers = world.objects.movers.clone();
                    for m in movers {
                        let at_cell = world.objects.get(m).is_some_and(|o| {
                            o.exists() && {
                                let (cr, cc) = terr::world_to_cell(&world.terrain, &o.c.position);
                                cc == i32::from(c) && cr == i32::from(r)
                            }
                        });
                        if at_cell {
                            for _ in 0..50 {
                                let shot = WeaponShotInfo {
                                    attacker: WatchId::NONE,
                                    master_id: 160,
                                    damage: 50.0,
                                    hit_location: hooks::calc_hit_location(
                                        world,
                                        m,
                                        ObjectHandle::NONE,
                                        -1,
                                        AttackSource::WeaponFire,
                                        0,
                                    ),
                                    entry_angle: 0.0,
                                };
                                hooks::weapon_hit(world, m, &shot);
                            }
                        }
                    }
                }
            }
        }
    }
    set_building_damage(world, h, dmg);
}

/// `Turret::handleWeaponHit` (turret.cpp:2179).
fn turret_weapon_hit(world: &mut World, h: ObjectHandle, shot: &WeaponShotInfo) {
    let level = damage_level(world, h);
    let (destroyed, cur, pos, dmg_e, rad_e) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Turret(t) => (
                o.c.is_destroyed(),
                t.s.damage,
                o.c.position,
                o.c.expl_damage,
                o.c.expl_radius,
            ),
            _ => return,
        },
        None => return,
    };
    let damage = cur + shot.damage;
    if let Some(Kind::Turret(t)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        t.s.damage = damage;
    }
    if !destroyed && damage >= level {
        if let Some(o) = world.objects.get_mut(h) {
            o.c.set_flag(flags::DESTROYED, true);
            // Turret is dead: you may no longer collide with it.
            o.c.set_flag(flags::TANGIBLE, false);
            o.c.set_status(ObjectStatus::Destroyed, false);
        }
        hooks::set_obj_status(world, h, ObjectStatus::Destroyed);
        super::ObjectStore::create_explosion(world, TURRET_EXPLOSION_ID, None, pos, dmg_e, rad_e);
    }
}

/// `Gate::handleWeaponHit` (gate.cpp:~600).
fn gate_weapon_hit(world: &mut World, h: ObjectHandle, shot: &WeaponShotInfo) {
    let level = damage_level(world, h);
    let (destroyed, cur) = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Gate(g) => (o.c.is_destroyed(), g.s.damage),
            _ => return,
        },
        None => return,
    };
    let damage = cur + shot.damage;
    if let Some(Kind::Gate(g)) = world.objects.get_mut(h).map(|o| &mut o.kind) {
        g.s.damage = damage;
    }
    if damage >= level && !destroyed {
        destroy_gate(world, h);
    }
}

// ==================================================================================================================
// Save/load (DESIGN.md section 9). Appended by the lane A save work. Every field is dynamic except `cell_footprint`.
// ==================================================================================================================

crate::snapshot_struct!(Structure {
    damage,
    vertex_number,
    block_number,
    pitch_angle,
    fall_rate,
    power_supply,
    // Static: nothing writes `cellFootprint` (the C++ value is dead), so it stays at its init value [-1; 4].
    cell_footprint => [-1; 4],
    sub_areas,
    cells_covered,
    list_id,
    lod_unavailable_updates,
});

crate::snapshot_struct!(BuildingState {
    s,
    team_id,
    commander_id,
    base_tile_id,
    refit_buddy,
    parent_id,
    parent,
    list_id,
    capture_time,
    score_time,
    mover_in_proximity,
    proximity_timer,
    updated_turn,
});

crate::snapshot_struct!(TurretState {
    s,
    team_id,
    target,
    ready_time,
    last_fire_time,
    turret_rotation,
    did_reveal,
    min_range,
    max_range,
    num_functional_weapons,
    idle_wait,
    idle_position,
    old_position,
    parent_id,
    parent,
    current_weapon_node,
});

crate::snapshot_struct!(GateState {
    s,
    locked_open,
    locked_close,
    reason_to_open,
    opened,
    opening,
    closed,
    closing,
    just_destroyed,
    last_marked_open,
    team_id,
    closest_object,
    parent_id,
    parent,
    updated_turn,
});
