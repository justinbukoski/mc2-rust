// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Everything lane A needs from the other lanes, in one file.
//!
//! Each function stands for a C++ call that reaches into another lane (appearance: lane F; teams, sensors and pilots: lane D;
//! mission state and logistics: lane E; weapon bolts: lane C; mover release: lanes B/E). Appearance calls delegate to the
//! real headless models in `appearance`; unrelated lane interfaces remain local hooks. Nothing here draws random numbers.

use crate::ids::{ObjectHandle, WatchId};
use crate::math::{Vec3, Vec3Ext};
use crate::World;

use super::{Kind, ObjectClass, ObjectStatus};

/// Names available to `gosFX::EffectLibrary::Find`, read from the common class/name prefix of effect specifications.
#[derive(Clone, Debug, Default)]
pub struct EffectLibraryNames {
    bytes: Vec<u8>,
}

impl EffectLibraryNames {
    /// Wrap the bytes of `data/effects/mc2.fx`.
    pub fn new(bytes: Vec<u8>) -> EffectLibraryNames {
        EffectLibraryNames { bytes }
    }

    /// `EffectLibrary::Find(name)` (`S_stricmp`).
    pub fn contains(&self, name: &str) -> bool {
        let nb = name.as_bytes();
        if nb.is_empty() {
            return false;
        }
        let mut pat = (nb.len() as u32).to_le_bytes().to_vec();
        pat.extend_from_slice(nb);
        pat.push(0);
        self.bytes
            .windows(pat.len() + 4)
            .any(|w| w[3] == 0 && w[4..].eq_ignore_ascii_case(&pat))
    }
}

// ------------------------------------------------------------------------------------------------------------------
// Appearance (lane F)
// ------------------------------------------------------------------------------------------------------------------

/// `BldgAppearance::init` / `TreeAppearance::init` using lane F's loaded shape and animation types.
pub fn init_structure_appearance(world: &mut World, h: ObjectHandle, type_num: i32) {
    let Some((class, name)) = world.objects.type_by_num(type_num).map(|t| {
        (
            super::store::type_object_class(t),
            t.base
                .appearance_name
                .clone()
                .unwrap_or_else(|| "TESTBLDG".to_string()),
        )
    }) else {
        return;
    };
    if class == ObjectClass::Tree {
        if let Some(i) = world
            .appearance
            .tree_types
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(&name))
        {
            world.appearance.create_tree(h, i);
        }
    } else if let Some(i) = world
        .appearance
        .building_types
        .iter()
        .position(|t| t.name.eq_ignore_ascii_case(&name))
    {
        world.appearance.create_building(h, i);
    }
}

/// `appearance->getCurrentGestureId()` (bdAnimationState; -1 until the first gesture).
pub fn structure_gesture(world: &mut World, h: ObjectHandle) -> i32 {
    world.appearance.current_gesture_id(h)
}

/// `appearance->getInTransition()` (`!canTransition`: the current gesture's animation has not finished).
pub fn structure_in_transition(world: &mut World, h: ObjectHandle) -> bool {
    world.appearance.in_transition(h)
}

/// `appearance->setGesture(id)` (bdactor.cpp:982).
pub fn structure_set_gesture(world: &mut World, h: ObjectHandle, gesture: u32) {
    crate::appearance::set_gesture(world, h, gesture);
}

/// `recalcBounds` for a TURRET. Turret behaviour (gesture state machine, `canFire`, targeting, rotation limits, weapon fire) does
/// not read which LOD shape the camera picked, so when the frame supplies no structure LOD (headless or model-less runs) the
/// simulation uses the appearance's own current LOD (the base shape, LOD 0, until a view supplies another) instead of reporting
/// the update unavailable. A supplied LOD is used as before. Returns `inView`.
pub fn recalc_bounds_for_turret(world: &mut World, h: ObjectHandle) -> bool {
    let v = world.visibility.structure_in_view(h);
    let input = world.visibility.structure_lod(h);
    world.appearance.set_structure_lod_input(h, input);
    let lod = input.unwrap_or_else(|| current_lod(world, h));
    world.appearance.recalc_bounds(h, v, lod)
}

/// `appearance->update()` for a turret: as [`structure_update`] but a missing structure LOD input is not a gap (see
/// [`recalc_bounds_for_turret`]); the retained shape is transformed at its current LOD.
pub fn turret_structure_update(world: &mut World, h: ObjectHandle) {
    crate::appearance::update(world, h, true);
}

/// `appearance->update()`: the animation clock advances by one frame (lights, flashes and effects are presentation).
///
/// The single entry every structure appearance update goes through. The update transforms the retained LOD shape, so with the
/// object in view and no structure LOD supplied by the frame (`Visibility::structure_lod`, the case where
/// [`recalc_bounds_with_lod`] is `None`) it is UNAVAILABLE: nothing is transformed, `Structure::lod_unavailable_updates` is
/// incremented, and `false` is returned. With the LOD supplied (or out of view) the update runs as in C++ and `true` is
/// returned; callers keep the C++ update-before-recalcBounds order.
pub fn structure_update(world: &mut World, h: ObjectHandle) -> bool {
    if world.visibility.structure_in_view(h) && world.visibility.structure_lod(h).is_none() {
        if let Some(s) = world.objects.get_mut(h).and_then(|o| o.structure_mut()) {
            s.lod_unavailable_updates += 1;
        }
        return false;
    }
    crate::appearance::update(world, h, true);
    true
}

/// `appearance->setObjStatus(status)`.
pub fn set_obj_status(world: &mut World, h: ObjectHandle, status: ObjectStatus) {
    world.appearance.set_obj_status(h, status as i32);
}

/// `appearance->recalcBounds()`: whether the object is in the camera's view this frame. The result comes from the frame's
/// [`Visibility`](crate::visibility::Visibility) input and is stored in the appearance (`inView`).
///
/// The LOD `recalcBounds` selects from the camera (bdactor.cpp:1305-1325, inside the in-view branch of :1297) is a declared
/// upstream input (`Visibility::structure_lod`). This is the in-view-only form for callers whose use of the result does not
/// depend on the LOD shape; an in-view frame without the LOD input leaves the appearance's shape untouched. Callers that use
/// LOD-dependent geometry call [`recalc_bounds_with_lod`] and handle its `None`.
pub fn recalc_bounds(world: &mut World, h: ObjectHandle) -> bool {
    let v = world.visibility.structure_in_view(h);
    recalc_bounds_with_lod(world, h).unwrap_or(v)
}

/// `recalcBounds` for a caller that goes on to use LOD-dependent geometry (`markMoveMap` reads `bldgShape[currentLOD]`,
/// bdactor.cpp:2801-2805). `None` = UNAVAILABLE: the object is in view but the frame supplied no LOD
/// (`Visibility::structure_lod`). Then only `inView` is stored; the appearance's LOD and shape are not touched and no LOD 0
/// is substituted, so the caller must not use the geometry as a camera result.
pub fn recalc_bounds_with_lod(world: &mut World, h: ObjectHandle) -> Option<bool> {
    let v = world.visibility.structure_in_view(h);
    let input = world.visibility.structure_lod(h);
    world.appearance.set_structure_lod_input(h, input);
    match input {
        Some(lod) => Some(world.appearance.recalc_bounds(h, v, lod)),
        None if v => {
            world.appearance.set_in_view(h, v);
            None
        }
        // Out of view the LOD is not selected (bdactor.cpp:1297): the result does not need it.
        None => {
            let lod = current_lod(world, h);
            Some(world.appearance.recalc_bounds(h, v, lod))
        }
    }
}

/// The LOD input of the structure's last [`recalc_bounds`]: `None` when the frame did not supply it (unavailable).
pub fn structure_lod_input(world: &World, h: ObjectHandle) -> Option<usize> {
    world.appearance.structure_lod_input(h)
}

/// `currentLOD` of a structure's appearance (0 before any appearance or LOD exists).
fn current_lod(world: &mut World, h: ObjectHandle) -> usize {
    if let Some((b, _)) = world.appearance.building_mut(h) {
        return b.current_lod;
    }
    match world.appearance.tree_mut(h) {
        Some((t, _)) => t.current_lod,
        None => 0,
    }
}

/// `appearance->setInView(v)`.
pub fn set_in_view(world: &mut World, h: ObjectHandle, v: bool) {
    world.appearance.set_in_view(h, v);
}

/// `appearance->inView` (the digest's `iv` token).
pub fn appearance_in_view(world: &World, h: ObjectHandle) -> bool {
    world.appearance.in_view(h)
}

/// `appearance->flashBuilding(duration, flashDuration, color)`: damage/capture feedback, presentation only.
pub fn flash_building(world: &mut World, h: ObjectHandle, argb: u32) {
    world
        .events
        .push(crate::SimEvent::Flash { object: h, argb });
}

/// `appearance->getTopZ()` (`BuildingAppearance::high_z`).
pub fn top_z(_world: &World, _h: ObjectHandle) -> f32 {
    _world.appearance.top_z(_h)
}

/// `appearance->getHitNode()`.
pub fn hit_node(world: &mut World, h: ObjectHandle) -> Vec3 {
    if world.appearance.get(h).is_some() {
        world.appearance.hit_node(h)
    } else {
        world
            .objects
            .get(h)
            .map(|o| o.c.position)
            .unwrap_or_else(Vec3::zero)
    }
}

/// `appearance->getWeaponNodePosition(node)`. The object position only where the object has no appearance, which the C++
/// never has: every turret of the shipped missions has one (`structure_geometry::every_turret_of_the_shipped_missions_has_an_appearance`).
pub fn weapon_node_position(world: &mut World, h: ObjectHandle, node: i32) -> Vec3 {
    if world.appearance.get(h).is_some() {
        return world.appearance.weapon_node_position(h, node);
    }
    world
        .objects
        .get(h)
        .map(|o| o.c.position)
        .unwrap_or_else(Vec3::zero)
}

/// `getAppearRadius()` (`appearance->getRadius()`).
pub fn appear_radius(world: &World, h: ObjectHandle) -> f32 {
    world.appearance.radius(h)
}

/// The terrain's movement cells (`GameMap->getCell(row, col)`) as the appearance's [`MapCells`](crate::appearance::MapCells).
struct TerrainCells<'a>(&'a mut crate::terrain::Terrain);

impl crate::appearance::MapCells for TerrainCells<'_> {
    fn world_to_cell(&self, pos: Vec3) -> (i32, i32) {
        self.0.world_to_cell(&pos)
    }
    fn cells_per_side(&self) -> i32 {
        self.0.cells_per_side()
    }
    fn set_passable(&mut self, row: i32, col: i32, passable: bool) {
        self.0.set_passable(row, col, passable);
    }
    fn local_height(&self, row: i32, col: i32) -> u32 {
        self.0.get_local_height(row, col)
    }
    fn set_local_height(&mut self, row: i32, col: i32, height: u32) {
        self.0.set_local_height(row, col, height);
    }
}

/// `appearance->markLOS(clearIt)` over the terrain's mutable movement cells.
pub fn mark_los(world: &mut World, h: ObjectHandle, clear: bool) {
    world
        .appearance
        .mark_los(h, clear, &mut TerrainCells(&mut world.terrain));
}

/// `appearance->setObjectParameters(position, rotation, ...)` from the object's current position and rotation
/// (turret.cpp:573 and 801). Buildings pass the appearance's own rotation: see [`set_building_parameters`].
pub fn set_object_parameters(world: &mut World, h: ObjectHandle) {
    let Some(o) = world.objects.get(h) else {
        return;
    };
    let (pos, rot) = (o.c.position, o.c.rotation);
    world.appearance.set_object_parameters(h, pos, rot);
}

/// `appearance->setObjectParameters(position, ((ObjectAppearance*)appearance)->rotation, ...)` for a building (bldng.cpp:789).
/// The rotation is the appearance's own: a SpinMe building advances it in the appearance update (bdactor.cpp:1955), and the
/// object's rotation would reset that every frame. The object's rotation is used only when there is no appearance.
pub fn set_building_parameters(world: &mut World, h: ObjectHandle) {
    let Some(o) = world.objects.get(h) else {
        return;
    };
    let (pos, object_rot) = (o.c.position, o.c.rotation);
    let rot = match world.appearance.get(h) {
        Some(crate::appearance::Appearance::Building(b)) => b.rotation,
        _ => object_rot,
    };
    world.appearance.set_object_parameters(h, pos, rot);
}

/// `appearance->setMoverParameters(turretRotation, turretPitch, 0.0f)` (turret.cpp:802): the turret yaw and pitch that
/// the weapon nodes are placed with.
pub fn set_turret_parameters(world: &mut World, h: ObjectHandle, yaw: f32, pitch: f32) {
    world
        .appearance
        .set_mover_parameters(h, yaw, pitch, 0.0, false);
}

// ------------------------------------------------------------------------------------------------------------------
// Teams (lane D)
// ------------------------------------------------------------------------------------------------------------------

/// `Team::relations[a][b]` as the raw enum value (0 friendly, 1 neutral, 2 enemy); `None` when either id is not a team.
pub fn team_relation(world: &World, a: i32, b: i32) -> Option<crate::ai::Relation> {
    if a < 0 || b < 0 || a as usize >= crate::ids::MAX_TEAMS || b as usize >= crate::ids::MAX_TEAMS
    {
        return None;
    }
    Some(world.teams.relations[a as usize][b as usize])
}

/// `Team::teams[id] != NULL`.
pub fn team_exists(world: &World, id: i32) -> bool {
    id >= 0
        && world
            .teams
            .teams
            .get(id as usize)
            .is_some_and(|t| t.is_some())
}

/// `Team::home->getId()`.
pub fn home_team(world: &World) -> i32 {
    world.teams.home as i32
}

/// `GameObject::isFriendly(obj)` / `isEnemy` / `isNeutral` (gameobj.cpp:2104-2134): both objects need a team; the relation is
/// `relations[myTeam][objTeam]`.
pub fn is_friendly(world: &World, a: ObjectHandle, b: ObjectHandle) -> bool {
    relation_between(world, a, b) == Some(crate::ai::Relation::Friendly)
}

/// See [`is_friendly`].
pub fn is_enemy(world: &World, a: ObjectHandle, b: ObjectHandle) -> bool {
    relation_between(world, a, b) == Some(crate::ai::Relation::Enemy)
}

/// See [`is_friendly`] (`GameObject::isNeutral` returns false when either has no team).
pub fn is_neutral(world: &World, a: ObjectHandle, b: ObjectHandle) -> bool {
    relation_between(world, a, b) == Some(crate::ai::Relation::Neutral)
}

fn relation_between(
    world: &World,
    a: ObjectHandle,
    b: ObjectHandle,
) -> Option<crate::ai::Relation> {
    let ta = world.objects.get(a)?.team_id();
    let tb = world.objects.get(b)?.team_id();
    if !team_exists(world, ta) || !team_exists(world, tb) {
        return None;
    }
    team_relation(world, ta, tb)
}

// ------------------------------------------------------------------------------------------------------------------
// Mission state and logistics (lane E)
// ------------------------------------------------------------------------------------------------------------------

/// `GeneralAlarm` (perimeter alarms set it; only ABL resets it).
pub fn general_alarm(world: &World) -> bool {
    world.mission.general_alarm
}

/// Sets `GeneralAlarm`.
pub fn set_general_alarm(world: &mut World, on: bool) {
    world.mission.general_alarm = on;
}

/// `LogisticsData::addResourcePoints(n)` (captured resource buildings and vehicles): update the in-mission balance and keep
/// the gain ledger used by campaign/results reporting.
pub fn add_resource_points(world: &mut World, n: i32) {
    world.mission.resource_points += n;
    world.objects.resource_points_gained += n;
}

/// `MineSplashDamage` and `MineSplashRange` (gamesys.fit, lane C data): stored in the object store by the gamesys loader.
pub fn mine_splash(world: &World) -> (f32, f32) {
    world.objects.mine_splash
}

/// `Terrain::userMin` / `userMax` (the `[Terrain]` block of the mission file) and `MapData::waterDepth`:
/// used for the line-of-sight altitude table.
pub fn altitude_range(world: &World) -> (f32, f32, f32) {
    (
        world.terrain.user_min,
        world.terrain.user_max,
        world.terrain.water_depth_const,
    )
}

// ------------------------------------------------------------------------------------------------------------------
// Movers and bolts (lanes B, C, E)
// ------------------------------------------------------------------------------------------------------------------

/// `Mover::release()` (mover.cpp:2380): clears the team, leaves the group, frees the pilot. Lane D/E own those; the stand-in
/// does nothing.
pub fn release_mover(_world: &mut World, _h: ObjectHandle) {}

/// `WeaponBolt::finishNow()` for the pool victim (lane C delivers the pending damage).
pub fn finish_bolt_now(world: &mut World, h: ObjectHandle) {
    crate::combat::bolt_impact::finish_now(world, h);
}

// ------------------------------------------------------------------------------------------------------------------
// Positions used by line of sight
// ------------------------------------------------------------------------------------------------------------------

/// `getLOSPosition()` (virtual): terrain objects and gates use the position raised by half the shape height; buildings
/// with a lookout range or sensor use `position.z + 75`; turrets use the position at the height of weapon node 0.
/// Movers are lane B/F ([`crate::mover`]).
pub fn los_position(world: &mut World, h: ObjectHandle) -> Vec3 {
    let Some(o) = world.objects.get(h) else {
        return Vec3::zero();
    };
    let position = o.c.position;
    let is_mover = o.is_mover();
    if matches!(o.kind, Kind::Turret(_)) {
        // Turret::getLOSPosition (turret.cpp:1239): the position at the height of weapon node 0 (yaw applied by the appearance).
        let node = weapon_node_position(world, h, 0);
        return Vec3 {
            x: position.x,
            y: position.y,
            z: node.z,
        };
    }
    if is_mover {
        return crate::mover::mover_los_position_for_candidate(world, h, position);
    }
    let mut p = position;
    match &o.kind {
        Kind::Terrain(_) | Kind::Gate(_) => {
            p.z += top_z(world, h) * 0.5;
            p
        }
        Kind::Building(_) => {
            p.z += top_z(world, h) * 0.5;
            if let Some(t) = world.objects.type_of(h) {
                if let mc2_sim_data::objtype::ObjectTypeData::Building(b) = &t.data {
                    if b.lookout_tower_range > 0.0 || b.sensor_range > 0.0 {
                        p.z = position.z + 75.0;
                    }
                }
            }
            p
        }
        // Movers add their own offsets (lane B); everything else uses the plain position.
        _ => p,
    }
}

/// `GameObject::lineOfSight(target, startExtRad, checkVisibleBits)` (gameobj.cpp:1846).
pub fn line_of_sight(
    world: &mut World,
    from: ObjectHandle,
    to: ObjectHandle,
    start_ext_rad: f32,
) -> bool {
    let (Some(a), Some(b)) = (world.objects.get(from), world.objects.get(to)) else {
        return false;
    };
    let dx = b.c.position.x - a.c.position.x;
    let dy = b.c.position.y - a.c.position.y;
    let dz = b.c.position.z - a.c.position.z;
    let dist = crate::math::approximate_length(&Vec3 {
        x: dx,
        y: dy,
        z: dz,
    });

    let Some(range) = visual_range(world, from) else {
        return false;
    };
    let to_is_mover = b.is_mover();
    let from_is_mover = a.is_mover();
    let from_handle = a.c.handle.0 as usize;
    let to_handle = b.c.handle.0 as usize;
    if dist > range {
        return false;
    }
    let team = world.objects.get(from).map(|o| o.team_id()).unwrap_or(-1);
    let los_from = los_position(world, from);
    let los_to = target_los_position(world, to);
    let ext = appear_radius(world, to);

    if to_is_mover && from_is_mover && world.objects.use_mover_los_table {
        let index = from_handle * world.objects.max_movers + to_handle;
        if index < world.objects.mover_los_table.len() {
            let status = world.objects.mover_los_table[index];
            if status == -1 {
                let los = crate::ai::sensors::line_of_sight_ray(
                    world,
                    los_from,
                    los_to,
                    team,
                    ext,
                    start_ext_rad,
                );
                world.objects.mover_los_table[index] = i8::from(los);
            }
            return world.objects.mover_los_table[index] != 0;
        }
    }
    let _ = team;
    crate::ai::sensors::line_of_sight_ray(world, los_from, los_to, team, ext, start_ext_rad)
}

/// The visual range of `observer` in world units, as `GameObject::lineOfSight` and `Team::teamLineOfSight` compute it
/// (gameobj.cpp:1860-1893): the `visualRangeTable` entry for the observer's altitude above `max(waterDepth, userMin)` (scaled by
/// `userMax`), plus 20% for a scout pilot (`Warrior::isScout`, warrior.h:1210) and times the mover type's `LOSFactor`
/// (`getLOSFactor`, mech.h:715 / gvehicl.h:527), in meters times `worldUnitsPerMeter * 25`. `None` when the observer does not exist.
pub fn visual_range(world: &World, observer: ObjectHandle) -> Option<f32> {
    let a = world.objects.get(observer)?;
    // Figure out altitude above minimum terrain altitude and look up in table.
    let (user_min, user_max, water_depth) = altitude_range(world);
    let mut base_elevation = water_depth;
    if water_depth < user_min {
        base_elevation = user_min;
    }
    let altitude = a.c.position.z - base_elevation;
    let altitude_integer_range = (user_max - base_elevation) * 0.003_906_25f32;
    let mut alt_level: i64 = 0;
    if altitude_integer_range > crate::time::STUFF_SMALL {
        alt_level = (altitude / altitude_integer_range) as i64;
    }
    alt_level = alt_level.clamp(0, 255);
    let table = &world.data.gamesys.visual_range_table;
    let mut radius = table.get(alt_level as usize).copied().unwrap_or(0) as f32;
    if a.is_mover() {
        radius += radius * scout_bonus(world, observer);
        radius *= mover_los_factor(world, observer);
    }
    Some(radius * 25.0 * crate::terrain::WORLD_UNITS_PER_METER)
}

/// `Team::teamLineOfSight(tPos, extRad)` (team.cpp:762) for the team `team_id`: does any live member of the team, or one of its
/// lookout towers, see the point `t_pos`?
pub fn team_line_of_sight(world: &mut World, team_id: i32, t_pos: Vec3, ext_rad: f32) -> bool {
    let roster: Vec<crate::ids::WatchId> = usize::try_from(team_id)
        .ok()
        .and_then(|t| world.teams.teams.get(t))
        .and_then(|t| t.as_ref())
        .map(|t| t.roster.clone())
        .unwrap_or_default();
    for wid in roster {
        let Some(obj) = world.objects.by_watch_id(wid) else {
            continue;
        };
        let Some(o) = world.objects.get(obj) else {
            continue;
        };
        if o.c.is_disabled()
            || o.c.is_destroyed()
            || o.c.status == crate::object::ObjectStatus::ShutDown
        {
            continue;
        }
        let dist = crate::math::approximate_length(&Vec3 {
            x: t_pos.x - o.c.position.x,
            y: t_pos.y - o.c.position.y,
            z: t_pos.z - o.c.position.z,
        });
        let Some(range) = visual_range(world, obj) else {
            continue;
        };
        if dist <= range {
            let from = los_position(world, obj);
            if crate::ai::sensors::line_of_sight_ray(world, from, t_pos, team_id, ext_rad, 0.0) {
                return true;
            }
        }
    }
    // Check the lookout towers now. You can find them in special Buildings!!
    for tower in world.objects.special_buildings.clone() {
        let Some(o) = world.objects.get(tower) else {
            continue;
        };
        let is_lookout = matches!(
            world.objects.type_of(tower).map(|t| &t.data),
            Some(mc2_sim_data::objtype::ObjectTypeData::Building(b)) if b.lookout_tower_range > 0.0
        );
        if !(o.exists() && is_lookout && o.team_id() == team_id) {
            continue;
        }
        if o.c.is_disabled()
            || o.c.is_destroyed()
            || o.c.status == crate::object::ObjectStatus::ShutDown
        {
            continue;
        }
        let dist = crate::math::approximate_length(&Vec3 {
            x: t_pos.x - o.c.position.x,
            y: t_pos.y - o.c.position.y,
            z: t_pos.z - o.c.position.z,
        });
        let Some(range) = visual_range(world, tower) else {
            continue;
        };
        if dist <= range {
            let from = los_position(world, tower);
            let start_ext = appear_radius(world, tower);
            if crate::ai::sensors::line_of_sight_ray(world, from, t_pos, team_id, 0.0, start_ext) {
                return true;
            }
        }
    }
    false
}

/// `Team::home->teamLineOfSight(position, getAppearRadius())` for the object `h` (the shut-down check of `BattleMech::update`).
pub fn home_team_sees(world: &mut World, h: ObjectHandle) -> bool {
    let Some(o) = world.objects.get(h) else {
        return false;
    };
    let position = o.c.position;
    let ext = appear_radius(world, h);
    let home = home_team(world);
    team_line_of_sight(world, home, position, ext)
}

fn target_los_position(world: &mut World, h: ObjectHandle) -> Vec3 {
    los_position(world, h)
}

/// Extra visual range for scouts: `radius += radius * 0.2` when the pilot is a scout (warrior.h:1210); 0 otherwise.
fn scout_bonus(world: &World, mover: ObjectHandle) -> f32 {
    let slot = world.objects.get(mover).and_then(|obj| match &obj.kind {
        Kind::Mech(mech) => Some(mech.m.pilot_slot),
        Kind::Vehicle(vehicle) => Some(vehicle.m.pilot_slot),
        _ => None,
    });
    if slot
        .and_then(|slot| world.pilots.get(slot))
        .is_some_and(|pilot| pilot.specialty[12])
    {
        0.2
    } else {
        0.0
    }
}

/// `Mover::getLOSFactor()` (the mover type's `LOSFactor`), defaulting to 1.0 when unavailable.
fn mover_los_factor(world: &World, mover: ObjectHandle) -> f32 {
    let Some(type_handle) = world.objects.get(mover).map(|obj| obj.c.type_handle) else {
        return 1.0;
    };
    match world.objects.type_by_num(type_handle).map(|ty| &ty.data) {
        Some(mc2_sim_data::objtype::ObjectTypeData::BattleMech(mech)) => mech.los_factor,
        Some(mc2_sim_data::objtype::ObjectTypeData::GroundVehicle(vehicle)) => vehicle.los_factor,
        _ => 1.0,
    }
}

/// Whether an owner watch id still resolves to a live object.
pub fn watch_alive(world: &World, id: WatchId) -> bool {
    world.objects.by_watch_id(id).is_some()
}

// ------------------------------------------------------------------------------------------------------------------
// More stand-ins
// ------------------------------------------------------------------------------------------------------------------

/// `Stuff::Vector4D::GetLength()` of `appearance->upperLeft - appearance->lowerRight` for a building that has never had
/// `recalcBounds()` called: both corners are still at their initial -999, so the diagonal is zero (bldng.cpp:743-753).
pub fn appearance_diagonal(_world: &World, _h: ObjectHandle) -> f32 {
    0.0
}

/// `appearance->markMoveMap(passable, NULL)` for the shape-derived cell set (only the resource building type 357 does this
/// every update, bldng.cpp:796). The cells are the shape vertices above 1 wu (`markMoveMap`, bdactor.cpp:2790).
pub fn mark_move_map_from_shape(world: &mut World, h: ObjectHandle, passable: bool) {
    world
        .appearance
        .mark_move_map(h, passable, false, &mut TerrainCells(&mut world.terrain));
}

/// `Terrain::mapData->setOverlay(DAMAGED_BRIDGE)` when the land bridge building is destroyed: terrain art only (lane T).
pub fn damage_bridge_overlay(_world: &mut World, _h: ObjectHandle) {}

/// `GlobalMoveMap[0..1]->clearPathExistsTable()`: lane B adds the table to `GlobalMap`. Stand-in: nothing.
pub fn clear_path_exists_tables(_world: &mut World) {}

/// `(WeaponRanges[range][0], WeaponRanges[range][1])` of a master component's weapon range class (turret.cpp:2231): the table is
/// [`ObjectStore::weapon_ranges`](super::ObjectStore::weapon_ranges), the class comes from the component list.
pub fn weapon_range_limits(world: &World, master_id: i32) -> (f32, f32) {
    let class = world
        .data
        .components
        .as_ref()
        .and_then(|l| l.get(master_id))
        .and_then(|c| c.weapon())
        .and_then(|w| w.range)
        .map(|r| r.index());
    match class {
        Some(i) => {
            let r = world.objects.weapon_ranges[i];
            (r[0], r[1])
        }
        None => (0.0, 0.0),
    }
}

/// `TurretType::punch`: the summed battle value of the turret's weapons (turret.cpp:189-210).
pub fn turret_punch(world: &World, weapons: &[i32; 4]) -> f32 {
    let Some(list) = world.data.components.as_ref() else {
        return 0.0;
    };
    let mut punch = 0.0f32;
    for &w in weapons {
        if w != -1 {
            if let Some(c) = list.get(w) {
                punch += c.battle_rating;
            }
        }
    }
    punch
}

/// `Mover::isJumping()` (mechs only).
pub fn is_jumping(world: &World, h: ObjectHandle) -> bool {
    match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.in_jump,
        _ => false,
    }
}

/// `BattleMech::handleStaticCollision` / `GroundVehicle::handleStaticCollision` (mech.cpp:1098, gvehicl.cpp:783): a tangible
/// mover (a ground vehicle also needs `pathLocks`) tests itself against the collidable objects of its own block that are
/// trees, terrain objects, buildings and tangible. Ported here because the collision pair log needs it; lane B owns the
/// movers and may take it over.
pub fn mover_handle_static_collision(world: &mut World, h: ObjectHandle) {
    let ok = match world.objects.get(h) {
        Some(o) => match &o.kind {
            Kind::Mech(_) => o.c.tangible(),
            Kind::Vehicle(v) => o.c.tangible() && v.m.path_locks_enabled,
            _ => false,
        },
        None => false,
    };
    if !ok {
        return;
    }
    let (block, _vertex) = super::carnage::block_and_vertex_number(world, h);
    let Some(blk) = usize::try_from(block)
        .ok()
        .and_then(|b| world.objects.blocks.get(b))
        .cloned()
    else {
        return;
    };
    for i in 0..blk.num_collidable {
        let oh = ObjectHandle(blk.first_handle + i);
        let tangible = match world.objects.get(oh) {
            Some(o) => match o.c.class {
                super::ObjectClass::Tree
                | super::ObjectClass::TreeBuilding
                | super::ObjectClass::TerrainObject
                | super::ObjectClass::Building => o.c.tangible(),
                _ => false,
            },
            None => false,
        };
        if tangible {
            super::collision::detect_static_collision(world, h, oh);
        }
    }
}

/// `Artillery::handleStaticCollision` (artlry.cpp:670): lane C. Both the mine clearance and the static-object scan are
/// [`crate::combat::artillery_static`].
pub fn artillery_handle_static_collision(world: &mut World, h: ObjectHandle) {
    crate::combat::artillery_static::handle_static_collision(world, h);
}

/// `BattleMechType::handleCollision` / `GroundVehicleType::handleCollision` (mech.cpp:744, gvehicl.cpp:416): see
/// [`crate::mover::type_handle_collision`].
pub fn mover_type_handle_collision(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    crate::mover::type_handle_collision(world, collidee, collider)
}

/// `BattleMechType::handleDestruction` / `GroundVehicleType::handleDestruction` (mech.cpp:994, gvehicl.cpp:521): lane C
/// ([`crate::combat::damage::handle_destruction`]). CROSS-LANE: takes the collider, which the C++ passes on (its watch id goes to the
/// pilot's `PILOT_ALARM_VEHICLE_DESTROYED`).
pub fn mover_type_handle_destruction(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    let wid = world
        .objects
        .get(collider)
        .is_some()
        .then(|| world.objects.watch_id(collider));
    crate::combat::damage::handle_destruction(world, collidee, wid)
}

/// `ArtilleryType::handleCollision` (artlry.cpp:532): lane C ([`crate::combat::artillery::handle_collision`]).
pub fn artillery_type_handle_collision(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    crate::combat::artillery::handle_collision(world, collidee, collider)
}

/// `handleWeaponHit(shot)` on any object: structures are handled here ([`super::structure::handle_weapon_hit`]), movers
/// go to lane C ([`crate::combat::handle_weapon_hit`]) unless a [`HitSink`] is installed (tests that replay movers).
pub fn weapon_hit(
    world: &mut World,
    target: ObjectHandle,
    shot: &crate::combat::WeaponShotInfo,
) -> i32 {
    let is_structure = world
        .objects
        .get(target)
        .is_some_and(|o| o.structure().is_some());
    let is_mover = world.objects.get(target).is_some_and(|o| o.is_mover());
    if is_structure {
        super::structure::handle_weapon_hit(world, target, shot)
    } else if !is_mover {
        // `GameObject::handleWeaponHit` (gameobj.h:461) is an empty virtual: carnage, bolts and artillery shrug hits off.
        0
    } else if let Some(sink) = world.objects.hit_sink.as_mut() {
        sink.push(HitRecord {
            turn: world.clock.turn,
            target,
            damage: shot.damage,
            attacker: shot.attacker,
        });
        0
    } else {
        crate::combat::handle_weapon_hit(world, target, shot)
    }
}

/// `target->calcHitLocation(attacker, weaponIndex, source, type)` (lane C; draws from the sim stream). With a [`HitSink`]
/// installed the location is 0 and nothing is drawn: tests that replay the movers line the RNG stream up themselves.
pub fn calc_hit_location(
    world: &mut World,
    target: ObjectHandle,
    attacker: ObjectHandle,
    weapon_index: i32,
    source: crate::combat::AttackSource,
    attack_type: i32,
) -> i32 {
    if world.objects.hit_sink.is_some() {
        0
    } else {
        crate::combat::calc_hit_location(world, target, attacker, weapon_index, source, attack_type)
    }
}

/// One hit that reached a non-structure and was diverted into the [`HitSink`].
#[derive(Clone, Debug, PartialEq)]
pub struct HitRecord {
    /// `turn` when the hit happened.
    pub turn: i64,
    /// The mover that was hit.
    pub target: ObjectHandle,
    /// `WeaponShotInfo::damage`.
    pub damage: f32,
    /// `WeaponShotInfo::attackerWID`.
    pub attacker: WatchId,
}

/// Debug/test facility: when `ObjectStore::hit_sink` is `Some`, hits on movers are recorded here instead of calling lane C.
pub type HitSink = Vec<HitRecord>;

/// `((MoverPtr)attacker)->getPilot()->triggerAlarm(PILOT_ALARM_KILLED_TARGET, target->getWatchID())` when a structure is
/// destroyed by a mover (lane D owns alarms). Stand-in: nothing.
///
/// TODO(lanes C/D): live kill credit is not wired. Lane C owns the shot that destroys the target (its damage path); lane D owns
/// the alarm, through `mission::pilots::credit_kill`. Until this body calls it, kill counters stay zero in every live run. The
/// unit tests of `credit_kill` call it directly, so they do not exercise this live path.
pub fn attacker_killed_target(_world: &mut World, _attacker: WatchId, _target: ObjectHandle) {}

/// `appearance->playDestruction()` (bdactor.cpp:1449): whether the building type has a gosFX destruction effect
/// (`[TGLDestructEffect]` in its appearance `.ini`); the effect itself is presentation. True makes the building create
/// `EMPTY_EXPLOSION_ID` (splash damage, no drawn explosion) instead of the generic building explosion.
pub fn play_destruction(world: &mut World, h: ObjectHandle) -> bool {
    crate::appearance::fx::play_destruction(world, h)
}
