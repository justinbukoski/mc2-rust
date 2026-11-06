// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `CollisionSystem` (collsn.cpp): 24x24 grid of 200-unit cells, LIFO cell lists, giants list, fixed neighbour pattern
//! (x+1,y), (x+1,y+1), (x,y+1). Deterministic and RNG free; pair order is part of the behaviour.
//!
//! The grid is rebuilt every frame from the collidable snapshot ([`ObjectStore::collidable_list`]): objects are added one by
//! one and each object's `handleStaticCollision` runs right after its `add`, then `createGrid` walks the cells and tests every
//! object against the older objects of its own cell, the giants, and three of the eight neighbour cells.
//!
//! Debug: with [`CollisionSystem::log_enabled`] set, every pair that reaches `checkExtents` is appended to
//! [`CollisionSystem::log`] (cleared at the start of each `update_collisions`); the oracle's `--collision-log` writes the same
//! pairs.

use super::{hooks, structure, terr, Kind, ObjectClass, ObjectStore};
use crate::ids::ObjectHandle;
use crate::World;
use mc2_sim_data::objtype::ObjectTypeData;

/// Grid width and height in cells (`CollisionSystem::xGridSize`/`yGridSize`).
pub const GRID_CELLS: usize = 24;
/// World units per grid cell (`gridRadius`).
pub const GRID_CELL_SIZE: u32 = 200;
/// Capacity assertion of the C++ (`maxObjects`): grid nodes available per frame.
pub const GRID_MAX_OBJECTS: usize = 300;
/// `CollisionSystem::warningDist`.
pub const WARNING_DIST: f32 = 250.0;
/// `CollisionSystem::alertTime`.
pub const ALERT_TIME: f32 = 2.5;

/// `CollisionAlertRecord`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionAlert {
    /// `object1`.
    pub object1: ObjectHandle,
    /// `object2`.
    pub object2: ObjectHandle,
    /// `currentDistance`.
    pub current_distance: f32,
    /// `timeToImpact`.
    pub time_to_impact: f32,
}

/// `GlobalCollisionAlert` (collsn.cpp:63): the per-frame list crash avoidance reads (20 records, reset every frame).
pub const MAX_ALERTS: usize = 20;

/// The grid and the per-frame alert list.
#[derive(Clone, Debug)]
pub struct CollisionSystem {
    /// `grid[x + y * 24]`: handles in insertion order; the C++ list head is the *last* element (LIFO).
    grid: Vec<Vec<ObjectHandle>>,
    /// `giantObjects` (head = last element).
    giants: Vec<ObjectHandle>,
    /// `nextAvailableNode`.
    nodes_used: usize,
    /// `globalCollisionAlert` records.
    pub alerts: Vec<CollisionAlert>,
    /// Record every pair that reaches `checkExtents` (debug).
    pub log_enabled: bool,
    /// The pairs of the last `update_collisions`, in order.
    pub log: Vec<(u32, u32)>,
}

impl Default for CollisionSystem {
    fn default() -> Self {
        CollisionSystem {
            grid: vec![Vec::new(); GRID_CELLS * GRID_CELLS],
            giants: Vec::new(),
            nodes_used: 0,
            alerts: Vec::new(),
            log_enabled: false,
            log: Vec::new(),
        }
    }
}

impl CollisionSystem {
    /// `GlobalCollisionAlert::addRecord`: false when the 20-record list is full.
    pub fn add_alert(&mut self, a: CollisionAlert) -> bool {
        if self.alerts.len() < MAX_ALERTS {
            self.alerts.push(a);
            true
        } else {
            false
        }
    }

    /// `GlobalCollisionAlert::findAlert(object, startRecord)`: the first record at or after `start` involving `object`.
    pub fn find_alert(&self, object: ObjectHandle, start: Option<usize>) -> Option<usize> {
        let from = start.map(|s| s + 1).unwrap_or(0);
        (from..self.alerts.len())
            .find(|&i| self.alerts[i].object1 == object || self.alerts[i].object2 == object)
    }

    fn reset_grid(&mut self) {
        for c in &mut self.grid {
            c.clear();
        }
        self.giants.clear();
        self.nodes_used = 0;
    }
}

/// `CollisionGrid::add(object)`: put a tangible object in its cell (or the giants list).
fn grid_add(world: &mut World, h: ObjectHandle) {
    let (tangible, pos) = match world.objects.get(h) {
        Some(o) => (o.c.tangible(), o.c.position),
        None => return,
    };
    if !tangible {
        return;
    }
    let radius = world.objects.extent_radius(h);
    let max_radius = GRID_CELL_SIZE as f32;
    let cs = &mut world.objects.collision;
    cs.nodes_used += 1;
    if radius > max_radius {
        cs.giants.push(h);
        return;
    }
    let x_width = GRID_CELLS as f32;
    let grid_x_offset = ((GRID_CELLS as u32 + 1) * GRID_CELL_SIZE / 2) as f32;
    let grid_x_check = x_width * max_radius;
    let mut gx = pos.x;
    gx += grid_x_offset;
    if gx < 0.0 {
        gx = 0.0;
    }
    if gx >= grid_x_check {
        gx = grid_x_check - 1.0;
    }
    gx /= max_radius;
    let mut gy = pos.y;
    gy += grid_x_offset;
    if gy < 0.0 {
        gy = 0.0;
    }
    if gy >= grid_x_check {
        gy = grid_x_check - 1.0;
    }
    gy /= max_radius;
    // float2long truncates (mathfunc.h:115)
    let index = ((gx - 0.5f32) as i64 + (gy - 0.5f32) as i64 * GRID_CELLS as i64) as u64 as usize;
    if index < cs.grid.len() {
        cs.grid[index].push(h);
    }
}

/// `GameObjectManager::updateCollisions` -> `CollisionSystem::checkObjects` (collsn.cpp:210-290).
pub fn update_collisions(world: &mut World) {
    if !world.objects.collidables_built {
        world.objects.build_collidable_list();
    }
    check_objects(world);
}

fn check_objects(world: &mut World) {
    world.objects.collision.reset_grid();
    world.objects.collision.alerts.clear(); // globalCollisionAlert->purgeRecords()
    world.objects.collision.log.clear();

    let list = world.objects.collidable_list();
    for h in list {
        let live = world
            .objects
            .get(h)
            .is_some_and(|o| o.exists() && o.c.tangible());
        if live {
            grid_add(world, h);
            handle_static_collision(world, h);
        }
    }
    create_grid(world);
}

/// `handleStaticCollision()` (virtual): movers (lane B) and artillery (lane C) scan the objects around them, explosions are
/// here; everything else does nothing.
fn handle_static_collision(world: &mut World, h: ObjectHandle) {
    enum K {
        Mover,
        Artillery,
        Carnage,
        Other,
    }
    let k = match world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(_)) | Some(Kind::Vehicle(_)) => K::Mover,
        Some(Kind::Artillery(_)) => K::Artillery,
        Some(Kind::Carnage(_)) => K::Carnage,
        _ => K::Other,
    };
    match k {
        K::Mover => hooks::mover_handle_static_collision(world, h),
        K::Artillery => hooks::artillery_handle_static_collision(world, h),
        K::Carnage => super::carnage::handle_static_collision(world, h),
        K::Other => {}
    }
}

/// `CollisionGrid::createGrid` (collsn.cpp:213).
fn create_grid(world: &mut World) {
    // This block of code is only necessary if we collide a giant object against a giant object.
    let giants: Vec<ObjectHandle> = world
        .objects
        .collision
        .giants
        .iter()
        .rev()
        .copied()
        .collect();
    for (i, &g) in giants.iter().enumerate() {
        if i + 1 < giants.len() {
            check_grid(world, g, &giants[i + 1..]);
        }
    }
    let giants_head_first = giants;
    for y in 0..GRID_CELLS {
        for x in 0..GRID_CELLS {
            let index = x + y * GRID_CELLS;
            let cell: Vec<ObjectHandle> = world.objects.collision.grid[index]
                .iter()
                .rev()
                .copied()
                .collect();
            for (i, &obj) in cell.iter().enumerate() {
                // Check against the big things.
                if !giants_head_first.is_empty() {
                    check_grid(world, obj, &giants_head_first);
                }
                // Same grid as the object (the older objects of the cell).
                if i + 1 < cell.len() {
                    check_grid(world, obj, &cell[i + 1..]);
                }
                if x < GRID_CELLS - 1 {
                    // Grid at x+1, y
                    let area: Vec<ObjectHandle> = world.objects.collision.grid[index + 1]
                        .iter()
                        .rev()
                        .copied()
                        .collect();
                    if !area.is_empty() {
                        check_grid(world, obj, &area);
                    }
                    if y < GRID_CELLS - 1 {
                        // Grid at x+1, y+1
                        let area: Vec<ObjectHandle> = world.objects.collision.grid
                            [index + 1 + GRID_CELLS]
                            .iter()
                            .rev()
                            .copied()
                            .collect();
                        if !area.is_empty() {
                            check_grid(world, obj, &area);
                        }
                    }
                }
                if y < GRID_CELLS - 1 {
                    // Grid at x, y+1
                    let area: Vec<ObjectHandle> = world.objects.collision.grid[index + GRID_CELLS]
                        .iter()
                        .rev()
                        .copied()
                        .collect();
                    if !area.is_empty() {
                        check_grid(world, obj, &area);
                    }
                }
            }
        }
    }
}

/// `CollisionGrid::checkGrid(obj1, area)`: test `obj1` against every object of the list (head first).
fn check_grid(world: &mut World, obj1: ObjectHandle, area: &[ObjectHandle]) {
    for &obj2 in area {
        let (c1, c2) = match (world.objects.get(obj1), world.objects.get(obj2)) {
            (Some(a), Some(b)) => (a.c.class, b.c.class),
            _ => continue,
        };
        // Cull collisions between things which can never collide here.
        use ObjectClass::{Explosion, Gate, Tree, Turret};
        let culled = (c1 == Turret && c2 == Turret)
            || (c1 == Gate && c2 == Gate)
            || (c1 == Gate && c2 == Turret)
            || (c1 == Turret && c2 == Gate)
            || (c1 == Turret && c2 == Tree)
            || (c1 == Tree && c2 == Turret)
            || (c1 == Explosion && c2 == Explosion);
        if !culled {
            detect_collision(world, obj1, obj2);
        }
    }
}

/// `CollisionSystem::detectCollision` (collsn.cpp:340): movers and terrain objects collide by cell, everything else by
/// extent radius.
fn detect_collision(world: &mut World, obj1: ObjectHandle, obj2: ObjectHandle) {
    let time_of_closest = 0.0f32;
    let (c1, c2, cell1, cell2, p1, p2) = match (world.objects.get(obj1), world.objects.get(obj2)) {
        (Some(a), Some(b)) => (
            a.c.class as i32,
            b.c.class as i32,
            a.c.cell,
            b.c.cell,
            a.c.position,
            b.c.position,
        ),
        _ => return,
    };
    let explosion = ObjectClass::Explosion as i32;
    if c1 < explosion && c2 < explosion {
        // Do a cell based collision, ignoring radii.
        let (b1, v1) = super::carnage::block_and_vertex_number(world, obj1);
        let (b2, v2) = super::carnage::block_and_vertex_number(world, obj2);
        if b1 == b2 && v1 == v2 && cell1 == cell2 {
            check_extents(world, obj1, obj2, time_of_closest);
        }
    } else {
        // First check whether we are already colliding.
        let pos_x = p2.x - p1.x;
        let pos_y = p2.y - p1.y;
        let dist_mag = pos_x * pos_x + pos_y * pos_y; // elevation ignored
        let dist0 = world.objects.extent_radius(obj1);
        let dist1 = world.objects.extent_radius(obj2);
        let mut max_dist = dist1 + dist0;
        max_dist *= max_dist;
        if dist_mag < max_dist {
            check_extents(world, obj1, obj2, time_of_closest);
        }
    }
}

/// `CollisionSystem::detectStaticCollision(obj1, obj2)` (collsn.cpp:420): a mover only hits a static object when it stands on
/// impassable ground (except trees, terrain objects, special buildings and bridges).
pub fn detect_static_collision(world: &mut World, obj1: ObjectHandle, obj2: ObjectHandle) {
    let time_of_closest = 0.0f32;
    let (is_mover, cell1, p1, c2, p2) = match (world.objects.get(obj1), world.objects.get(obj2)) {
        (Some(a), Some(b)) => (
            a.is_mover(),
            a.c.cell,
            a.c.position,
            b.c.class,
            b.c.position,
        ),
        _ => return,
    };
    let pos_x = p2.x - p1.x;
    let pos_y = p2.y - p1.y;
    let dist_mag = pos_x * pos_x + pos_y * pos_y;
    let dist0 = world.objects.extent_radius(obj1);
    let dist1 = world.objects.extent_radius(obj2);
    let mut max_dist = dist1 + dist0;
    max_dist *= max_dist;
    if dist_mag < max_dist {
        // If we are inside the extent radius AND standing on impassable terrain, we hit the static object. ONLY if we are a
        // mover. We hit them if we are not a mover and we are an artillery or turret or gate.
        if is_mover && c2 != ObjectClass::Tree && c2 != ObjectClass::TerrainObject {
            let special = structure::is_special_building(world, obj2);
            if !special
                && c2 != ObjectClass::Bridge
                && terr::get_passable(&world.terrain, i32::from(cell1.0), i32::from(cell1.1))
            {
                return;
            }
        }
        check_extents(world, obj1, obj2, time_of_closest);
    }
}

/// `CollisionSystem::checkExtents(obj1, obj2, time)` (collsn.cpp:462): both types react to the collision; a type that returns
/// `true` then gets `handleDestruction`, and a `true` there removes the object.
fn check_extents(world: &mut World, obj1: ObjectHandle, obj2: ObjectHandle, _time: f32) {
    if world.objects.collision.log_enabled {
        world.objects.collision.log.push((obj1.0, obj2.0));
    }
    let (e1, e2) = match (world.objects.get(obj1), world.objects.get(obj2)) {
        (Some(a), Some(b)) => (a.exists(), b.exists()),
        _ => return,
    };
    if !e1 || !e2 {
        return;
    }
    let (l1, l2, k1, k2) = {
        let a = world.objects.get(obj1).expect("obj1");
        let b = world.objects.get(obj2).expect("obj2");
        (a.move_level(), b.move_level(), a.c.class, b.c.class)
    };
    if l1 == 2 || l2 == 2 {
        use ObjectClass::{Artillery, Explosion, Turret};
        if l1 != l2
            && k1 != Turret
            && k2 != Turret
            && k1 != Artillery
            && k2 != Artillery
            && k1 != Explosion
            && k2 != Explosion
        {
            return;
        }
    }

    let obj1_result = type_handle_collision(world, obj1, obj2);
    let obj2_result = type_handle_collision(world, obj2, obj1);

    // If we came back TRUE, whack the object.
    if obj1_result {
        let remove = type_handle_destruction(world, obj1, obj2);
        // Pull the object from the list if this flag is set; otherwise it is around a little longer.
        if remove {
            if let Some(o) = world.objects.get_mut(obj1) {
                o.set_exists(false);
            }
        }
    }
    if obj2_result {
        let remove = type_handle_destruction(world, obj2, obj1);
        if remove {
            if let Some(o) = world.objects.get_mut(obj2) {
                o.set_exists(false);
            }
        }
    }
}

/// `getObjectType()->handleCollision(collidee, collider)`, dispatched on the object's type.
fn type_handle_collision(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    enum T {
        Terrain,
        Building,
        Turret,
        Gate,
        Explosion,
        Fire,
        Mover,
        Artillery,
        Bolt,
        Default,
    }
    let t = {
        let Some(o) = world.objects.get(collidee) else {
            return false;
        };
        match world.objects.type_by_num(o.c.type_handle).map(|t| &t.data) {
            Some(ObjectTypeData::TerrainObject(_)) => T::Terrain,
            Some(ObjectTypeData::Building(_)) => T::Building,
            Some(ObjectTypeData::Turret(_)) => T::Turret,
            Some(ObjectTypeData::Gate(_)) => T::Gate,
            Some(ObjectTypeData::Explosion(_)) => T::Explosion,
            Some(ObjectTypeData::Fire(_)) => T::Fire,
            Some(ObjectTypeData::BattleMech(_)) | Some(ObjectTypeData::GroundVehicle(_)) => {
                T::Mover
            }
            Some(ObjectTypeData::Artillery(_)) => T::Artillery,
            Some(ObjectTypeData::WeaponBolt(_)) => T::Bolt,
            _ => T::Default,
        }
    };
    match t {
        T::Terrain => structure::terrain_handle_collision(world, collidee, collider),
        T::Building => structure::building_handle_collision(world, collidee, collider),
        T::Turret => structure::turret_handle_collision(world, collidee, collider),
        T::Gate => structure::gate_handle_collision(world, collidee, collider),
        T::Explosion => super::carnage::explosion_handle_collision(world, collidee, collider),
        // FireType::handleCollision only lights trees on fire (RollDice(10) is dead: no fires exist) and returns false.
        T::Fire => false,
        T::Mover => hooks::mover_type_handle_collision(world, collidee, collider),
        T::Artillery => hooks::artillery_type_handle_collision(world, collidee, collider),
        // WeaponBoltType::handleCollision always returns false.
        T::Bolt => false,
        // ObjectType::handleCollision: the default reaction of any object is to explode (return true).
        T::Default => true,
    }
}

/// `getObjectType()->handleDestruction(collidee, collider)`.
fn type_handle_destruction(
    world: &mut World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    let Some(o) = world.objects.get(collidee) else {
        return false;
    };
    match world.objects.type_by_num(o.c.type_handle).map(|t| &t.data) {
        Some(ObjectTypeData::TerrainObject(_)) => {
            structure::terrain_handle_destruction(world, collidee)
        }
        Some(ObjectTypeData::BattleMech(_)) | Some(ObjectTypeData::GroundVehicle(_)) => {
            hooks::mover_type_handle_destruction(world, collidee, collider)
        }
        // Building, turret, gate, explosion, fire, artillery, bolt: all return false.
        Some(_) => false,
        // ObjectType::handleDestruction creates an explosion and returns true; no typed object lacks a type.
        None => false,
    }
}

/// `ObjectManager::getCollidableList()` for callers outside the grid (movers scanning their neighbours).
pub fn collidable_list(store: &mut ObjectStore) -> Vec<ObjectHandle> {
    store.collidable_list()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::RngMode;
    use crate::World;

    fn world_with_mechs(n: usize) -> World {
        let mut w = World::new(RngMode::LegacyMsvc, 1);
        let rng = w.rng.clone();
        w.objects.set_num_objects(&rng, n, 0);
        for _ in 0..n {
            w.objects.new_mover(ObjectClass::BattleMech).unwrap();
        }
        w
    }

    fn place(w: &mut World, h: ObjectHandle, x: f32, y: f32) {
        let o = w.objects.get_mut(h).unwrap();
        o.c.position.x = x;
        o.c.position.y = y;
        o.c.set_tangible(true);
    }

    fn cell_of(w: &World, h: ObjectHandle) -> Option<usize> {
        w.objects.collision.grid.iter().position(|c| c.contains(&h))
    }

    #[test]
    fn grid_cell_index_truncates_clamps_and_shifts_by_half_a_cell() {
        let mut w = world_with_mechs(4);
        let m = w.objects.mechs.clone();
        // the origin sits at (2500 / 200 - 0.5) = 12.0 -> cell (12, 12)
        place(&mut w, m[0], 0.0, 0.0);
        // just left of the origin: 12.4 - 0.5 truncates to 11
        place(&mut w, m[1], -60.0, 0.0);
        // far outside: clamped to the last cell
        place(&mut w, m[2], 1.0e9, 1.0e9);
        // far outside the other way: the lowest cell
        place(&mut w, m[3], -1.0e9, -1.0e9);
        for &h in &m[..4] {
            grid_add(&mut w, h);
        }
        let n = GRID_CELLS;
        assert_eq!(cell_of(&w, m[0]), Some(12 + 12 * n));
        assert_eq!(cell_of(&w, m[1]), Some(11 + 12 * n));
        assert_eq!(cell_of(&w, m[2]), Some(23 + 23 * n));
        assert_eq!(cell_of(&w, m[3]), Some(0));
        // intangible objects never enter the grid
        let mut w2 = world_with_mechs(1);
        let h = w2.objects.mechs[0];
        w2.objects.get_mut(h).unwrap().c.set_tangible(false);
        grid_add(&mut w2, h);
        assert_eq!(cell_of(&w2, h), None);
    }

    #[test]
    fn cells_keep_insertion_order_and_x_plus_one_is_the_next_cell() {
        let mut w = world_with_mechs(3);
        let m = w.objects.mechs.clone();
        // a and b in one cell, c in the cell to the right (x + 1)
        place(&mut w, m[0], 10.0, 10.0);
        place(&mut w, m[1], 20.0, 10.0);
        place(&mut w, m[2], 210.0, 10.0);
        for &h in &m[..3] {
            grid_add(&mut w, h);
        }
        w.objects.collision.log_enabled = true;
        create_grid(&mut w);
        // `log` is only filled by check_extents, which needs an equal cell; here we look at the candidate order through the
        // grid contents instead: LIFO means the cell is walked from the last added object.
        let cell = cell_of(&w, m[0]).unwrap();
        assert_eq!(w.objects.collision.grid[cell], vec![m[0], m[1]]);
        assert_eq!(w.objects.collision.grid[cell + 1], vec![m[2]]);
    }
}

// ==================================================================================================================
// Save/load (DESIGN.md section 9). Appended by the lane A save work.
// ==================================================================================================================

crate::snapshot_struct!(CollisionAlert {
    object1,
    object2,
    current_distance,
    time_to_impact,
});

crate::snapshot_struct!(CollisionSystem {
    // Derived: `check_objects` clears the grid, the giants list and the node count before any read (every update).
    grid => vec![Vec::new(); GRID_CELLS * GRID_CELLS],
    giants => Vec::new(),
    nodes_used => 0,
    alerts,
    // Debug: `log_enabled` is set by the oracle, the pair log is cleared at the start of every update.
    log_enabled => false,
    log => Vec::new(),
});
