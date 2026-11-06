// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `GameObjectManager` data: handle-indexed object storage with the original list structure.
//!
//! Handle layout (`setNumObjects`, objmgr.cpp:292-495): `objList[0]` is null, then mechs (`max_mechs` slots: mission mech
//! count + 128 reinforcement slots), vehicles (same), terrain-object blocks (`first_handle` per block, cumulative from
//! `max_mechs + max_vehicles + 1`; within a block collidables first in file order, then non-collidable buildings), weapon
//! bolts (100), carnage (50), artillery (130). Mech `i` has handle `i + 1`, vehicle `i` has `max_mechs + i + 1`.
//!
//! The mission load sequence (lane E drives it, see [`ObjectStore::build_for_mission`]):
//! `count_terrain_objects` -> `set_num_objects` -> create the movers (`mech(i)`/`vehicle(i)`) -> `load_terrain_objects` ->
//! `build_mover_lists`.

use std::collections::BTreeMap;
use std::rc::Rc;

use mc2_sim_data::objtype::{ObjectTable, ObjectType, ObjectTypeData, TerrainSubType};

use super::carnage::CarnageType;
use super::collision::CollisionSystem;
use super::structure::{Structure, TurretState};
use super::{
    flags, hooks, Common, Kind, Obj, ObjectClass, MAX_MAP_CELL_WIDTH, MAX_MOVERGROUPS,
    MAX_MOVERGROUP_COUNT_START, MAX_REINFORCEMENT_PART_ID, MIN_MOVER_PART_ID,
    MIN_REINFORCEMENT_PART_ID, MIN_TERRAIN_PART_ID,
};
use crate::ids::{ObjectHandle, PartId, WatchId, MAX_TEAMS};
use crate::math::Vec3;
use crate::rng::SharedSimRng;

/// `MAX_REINFORCEMENTS_PER_TEAM` (objmgr.h).
pub const MAX_REINFORCEMENTS_PER_TEAM: usize = 16;
/// Weapon bolt pool size (`setNumObjects(..., 100, ...)`, mission.cpp:2734).
pub const NUM_WEAPONS: usize = 100;
/// Carnage pool size.
pub const NUM_CARNAGE: usize = 50;
/// Artillery pool size.
pub const NUM_ARTILLERY: usize = 130;
/// `MAX_CAPTURES_PER_TEAM`.
pub const MAX_CAPTURES_PER_TEAM: usize = 30;
/// `MAX_REMOVED`.
pub const MAX_REMOVED: usize = 20;

/// `ObjBlockInfo` of one block of 20x20 vertices (the part the object manager fills in).
#[derive(Clone, Debug, Default)]
pub struct ObjBlock {
    /// First handle of the block's range.
    pub first_handle: u32,
    /// Objects in the range.
    pub num_objects: u32,
    /// Collidable objects (trees, terrain objects, turrets, gates, special buildings) - they come first.
    pub num_collidable: u32,
}

/// `ObjDataLoader` (objmgr.h): one object record of packet 1, with the derived block and vertex numbers.
#[derive(Clone, Debug, Default)]
pub struct ObjDataLoader {
    /// `objTypeNum`.
    pub obj_type_num: i32,
    /// `vector`.
    pub position: Vec3,
    /// `rotation`.
    pub rotation: f32,
    /// `damage`: low nibble nonzero means pre-destroyed, `>> 4` is the base tile id.
    pub damage: i32,
    /// `teamId`.
    pub team_id: i32,
    /// `parentId`.
    pub parent_id: i32,
    /// `tileCol`.
    pub tile_col: i32,
    /// `tileRow`.
    pub tile_row: i32,
    /// `blockNumber`.
    pub block_number: i32,
    /// `vertexNumber`: `tileRow * 20 + tileRow` (the C++ uses the row twice, objmgr.cpp:839; kept).
    pub vertex_number: i32,
}

pub use super::load::build_for_mission;

/// Which rotating pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pool {
    /// Weapon bolts (100 slots).
    Weapons,
    /// Explosions and fires (50 slots).
    Carnage,
    /// Artillery strikes (130 slots).
    Artillery,
}

/// What `modifyMoverLists` and `buildMoverLists` read of the teams: which exist and their relation to `Team::home`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TeamView {
    /// `Team::teams[t] != NULL`.
    pub exists: Vec<bool>,
    /// `Team::relations[t][Team::home]` (None when home is out of range).
    pub relation_to_home: Vec<Option<crate::ai::Relation>>,
}

impl TeamView {
    /// Snapshot of the teams.
    pub fn from_teams(teams: &crate::ai::Teams) -> TeamView {
        TeamView {
            exists: (0..MAX_TEAMS)
                .map(|t| teams.teams.get(t).is_some_and(|x| x.is_some()))
                .collect(),
            relation_to_home: (0..MAX_TEAMS)
                .map(|t| (teams.home < MAX_TEAMS).then(|| teams.relations[t][teams.home]))
                .collect(),
        }
    }
}

/// `MOVERLIST_*` actions of `modifyMoverLists`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoverListAction {
    /// `MOVERLIST_DELETE`.
    Delete,
    /// `MOVERLIST_ADD`.
    Add,
    /// `MOVERLIST_TRADE`.
    Trade,
}

/// One removed-mover record (`RemovedMoverRec`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemovedMover {
    /// Turn it was removed.
    pub turn: u32,
    /// Its part id.
    pub part_id: i32,
}

/// All objects and the index lists around them.
#[derive(Clone, Debug)]
pub struct ObjectStore {
    /// Object type table (shared with `World::data`; cloned in by [`ObjectStore::count_terrain_objects`]).
    pub types: Option<Rc<ObjectTable>>,
    /// Per-type `extentRadius` overrides. The C++ mutates the shared object type (`Turret::init`, `Gate::init`, building
    /// JUSTCREATED set the type's extent), so every instance sees the change; the table is immutable here, so the
    /// mutations live in this map (type number -> extent).
    pub type_extent: BTreeMap<i32, f32>,
    /// `objList`, index = handle, length `max_objects + 1`; `None` for the null slot.
    pub list: Vec<Option<Obj>>,
    /// `numMechs`.
    pub num_mechs: usize,
    /// `maxMechs`.
    pub max_mechs: usize,
    /// `numVehicles`.
    pub num_vehicles: usize,
    /// `maxVehicles`.
    pub max_vehicles: usize,
    /// `maxMovers` (`maxMechs + maxVehicles`; no elementals).
    pub max_movers: usize,
    /// `mechs[]`: handles in array order (free = `memmove` shift left, freed slot goes to the end). `max_mechs` entries.
    pub mechs: Vec<ObjectHandle>,
    /// `vehicles[]` (`max_vehicles` entries are iterated every frame, `num_vehicles` is the live count).
    pub vehicles: Vec<ObjectHandle>,
    /// `terrainObjects[]` (also includes the unused slot per bridge).
    pub terrain_objects: Vec<ObjectHandle>,
    /// `buildings[]`.
    pub buildings: Vec<ObjectHandle>,
    /// `turrets[]`.
    pub turrets: Vec<ObjectHandle>,
    /// `gates[]`.
    pub gates: Vec<ObjectHandle>,
    /// `specialBuildings[]` (perimeter alarms, lookout towers, sensors: updated first every frame).
    pub special_buildings: Vec<ObjectHandle>,
    /// `gateControls[]`.
    pub gate_controls: Vec<ObjectHandle>,
    /// `turretControls[]`.
    pub turret_controls: Vec<ObjectHandle>,
    /// `powerGenerators[]`.
    pub power_generators: Vec<ObjectHandle>,
    /// `weapons[]` pool.
    pub weapons: Vec<ObjectHandle>,
    /// `currentWeaponsIndex`.
    pub weapon_index: usize,
    /// `carnage[]` pool.
    pub carnage: Vec<ObjectHandle>,
    /// `currentCarnageIndex`.
    pub carnage_index: usize,
    /// `artillery[]` pool.
    pub artillery: Vec<ObjectHandle>,
    /// `currentArtilleryIndex`.
    pub artillery_index: usize,
    /// `Terrain::objBlockInfo[]` (object block ranges).
    pub blocks: Vec<ObjBlock>,
    /// `moverList` (mechs then vehicles; delete = swap-remove, so the order changes).
    pub movers: Vec<ObjectHandle>,
    /// `goodMoverList` (friendly to the home team).
    pub good_movers: Vec<ObjectHandle>,
    /// `badMoverList` (enemy of the home team).
    pub bad_movers: Vec<ObjectHandle>,
    /// The team facts `modifyMoverLists` reads (`Team::teams`, `Team::relations[team][home]`), as of the last call that had
    /// the teams.
    pub team_view: TeamView,
    /// `nextWatchID` (starts at 1).
    pub next_watch_id: u32,
    /// `watchList[id]` -> handle (0 = none). Grows on demand.
    pub watch_list: Vec<ObjectHandle>,
    /// `collidableList` snapshot (rebuilt only when `rebuild_collidables`, or while empty-and-null at the first check).
    pub collidables: Vec<ObjectHandle>,
    /// True once the snapshot was built (the C++ tests `collidableList == NULL`).
    pub collidables_built: bool,
    /// `rebuildCollidableList`.
    pub rebuild_collidables: bool,
    /// `nextReinforcementPartId` (2050..3550).
    pub next_reinforcement_part_id: i32,
    /// `moverLineOfSightTable`: `(max_movers + 1)^2` bytes, -1 = unknown; reset every frame.
    pub mover_los_table: Vec<i8>,
    /// `useMoverLineOfSightTable`.
    pub use_mover_los_table: bool,
    /// `numCaptures[team]` and `captureList[team][]` (rebuilt each frame by `updateCaptureList`).
    pub capture_list: [Vec<WatchId>; MAX_TEAMS],
    /// `moversRemoved[MAX_REMOVED]`.
    pub removed_movers: Vec<RemovedMover>,
    /// `objData` between `countTerrainObjects` and `loadTerrainObjects`.
    pub pending: Vec<ObjDataLoader>,
    /// `numTerrainObjects` counted from the mission (before `setNumObjects` allocates them).
    pub count_terrain_objects_total: usize,
    /// `numBuildings` counted from the mission.
    pub count_buildings_total: usize,
    /// `numTurrets` counted from the mission.
    pub count_turrets_total: usize,
    /// `numGates` counted from the mission.
    pub count_gates_total: usize,
    /// `WeaponEffects::effects[id].effectObjNum` (effects.csv, lane C data): -1 where absent. Filled by whoever loads the
    /// effects table.
    pub effect_objects: Vec<i32>,
    /// Debug/test sink for hits that reach movers (see [`hooks::HitSink`]); `None` in a real run.
    pub hit_sink: Option<hooks::HitSink>,
    /// `WeaponEffects::effects[id].effectName` (effects.csv column 2); "NONE" for an unknown id.
    pub effect_names: Vec<String>,
    /// The `useNonWeaponEffects` preference as the headless oracle sees it: false (its `gosFX` library is not loaded, so no bolt has a
    /// miss effect and no impact draws a crater).
    pub use_non_weapon_effects: bool,
    /// The whole `WeaponEffects` table (all four names and the object number per row).
    pub effect_rows: Vec<crate::combat::weapon_fx::WeaponEffectRow>,
    /// `MineSplashDamage` and `MineSplashRange` (gamesys.fit; set by the gamesys loader).
    pub mine_splash: (f32, f32),
    /// `WeaponRanges[short/medium/long] = [min, max]` (defaults of mover.cpp:310, replaced by gamesys.fit `[WeaponRanges]`).
    pub weapon_ranges: [[f32; 2]; 3],
    /// Resource points granted by captured buildings (`LogisticsData::addResourcePoints`); lane E adds them to the
    /// player's total.
    pub resource_points_gained: i32,
    /// The collision grid and its debug pair log (collsn.cpp).
    pub collision: CollisionSystem,
}

impl Default for ObjectStore {
    fn default() -> Self {
        ObjectStore {
            types: None,
            type_extent: BTreeMap::new(),
            list: Vec::new(),
            num_mechs: 0,
            max_mechs: 0,
            num_vehicles: 0,
            max_vehicles: 0,
            max_movers: 0,
            mechs: Vec::new(),
            vehicles: Vec::new(),
            terrain_objects: Vec::new(),
            buildings: Vec::new(),
            turrets: Vec::new(),
            gates: Vec::new(),
            special_buildings: Vec::new(),
            gate_controls: Vec::new(),
            turret_controls: Vec::new(),
            power_generators: Vec::new(),
            weapons: Vec::new(),
            weapon_index: 0,
            carnage: Vec::new(),
            carnage_index: 0,
            artillery: Vec::new(),
            artillery_index: 0,
            blocks: Vec::new(),
            movers: Vec::new(),
            good_movers: Vec::new(),
            bad_movers: Vec::new(),
            team_view: TeamView::default(),
            next_watch_id: 1,
            watch_list: vec![ObjectHandle::NONE],
            collidables: Vec::new(),
            collidables_built: false,
            rebuild_collidables: false,
            next_reinforcement_part_id: MIN_REINFORCEMENT_PART_ID,
            mover_los_table: Vec::new(),
            use_mover_los_table: true,
            capture_list: Default::default(),
            removed_movers: Vec::new(),
            pending: Vec::new(),
            count_terrain_objects_total: 0,
            count_buildings_total: 0,
            count_turrets_total: 0,
            count_gates_total: 0,
            effect_objects: Vec::new(),
            effect_names: Vec::new(),
            effect_rows: Vec::new(),
            use_non_weapon_effects: false,
            hit_sink: None,
            mine_splash: (0.0, 0.0),
            weapon_ranges: [[0.0, 100.0], [50.0, 150.0], [100.0, 225.0]],
            resource_points_gained: 0,
            collision: CollisionSystem::default(),
        }
    }
}

/// `getObjectClass()` of the instances a type creates (`ObjectType::objectClass`).
pub fn type_object_class(t: &ObjectType) -> ObjectClass {
    match &t.data {
        ObjectTypeData::BattleMech(_) => ObjectClass::BattleMech,
        ObjectTypeData::GroundVehicle(_) => ObjectClass::GroundVehicle,
        ObjectTypeData::Building(_) => ObjectClass::Building,
        ObjectTypeData::TerrainObject(tt) => {
            if tt.sub_type == TerrainSubType::Tree && !tt.misc_terrain {
                ObjectClass::Tree
            } else {
                ObjectClass::TerrainObject
            }
        }
        ObjectTypeData::Turret(_) => ObjectClass::Turret,
        ObjectTypeData::Gate(_) => ObjectClass::Gate,
        ObjectTypeData::Artillery(_) => ObjectClass::Artillery,
        ObjectTypeData::Explosion(_) => ObjectClass::Explosion,
        ObjectTypeData::Fire(_) => ObjectClass::Fire,
        ObjectTypeData::Light(_) => ObjectClass::KliegLight,
        ObjectTypeData::WeaponBolt(_) => ObjectClass::WeaponBolt,
        ObjectTypeData::Raw => ObjectClass::Invalid,
    }
}

impl ObjectStore {
    /// `getMaxObjects()`: the highest valid handle.
    pub fn max_objects(&self) -> usize {
        self.list.len().saturating_sub(1)
    }

    /// `get(handle)`.
    pub fn get(&self, h: ObjectHandle) -> Option<&Obj> {
        self.list.get(h.0 as usize).and_then(|o| o.as_ref())
    }

    /// Mutable access.
    pub fn get_mut(&mut self, h: ObjectHandle) -> Option<&mut Obj> {
        self.list.get_mut(h.0 as usize).and_then(|o| o.as_mut())
    }

    /// The object type with this number (`objTypeManager->get`).
    pub fn type_by_num(&self, num: i32) -> Option<&ObjectType> {
        if num < 0 {
            return None;
        }
        self.types.as_ref().and_then(|t| t.get(num as u32))
    }

    /// The type of an object (`getObjectType`).
    pub fn type_of(&self, h: ObjectHandle) -> Option<&ObjectType> {
        let o = self.get(h)?;
        self.type_by_num(o.c.type_handle)
    }

    /// `ObjectType::getExtentRadius()` including the C++'s mutations of the shared type.
    pub fn type_extent_radius(&self, num: i32) -> f32 {
        if let Some(v) = self.type_extent.get(&num) {
            return *v;
        }
        self.type_by_num(num)
            .map(|t| t.base.extent_radius)
            .unwrap_or(-1.0)
    }

    /// `ObjectType::setExtentRadius(r)` (mutates the shared type).
    pub fn set_type_extent_radius(&mut self, num: i32, r: f32) {
        self.type_extent.insert(num, r);
    }

    /// `GameObject::getExtentRadius()`: the type's extent, except carnage which returns its instance radius.
    pub fn extent_radius(&self, h: ObjectHandle) -> f32 {
        match self.get(h) {
            Some(o) => match &o.kind {
                Kind::Carnage(c) => c.extent_radius(),
                _ => self.type_extent_radius(o.c.type_handle),
            },
            None => -1.0,
        }
    }

    /// `getWatchID()`: lazily assign the next watch id to this object (call order decides ids).
    pub fn watch_id(&mut self, h: ObjectHandle) -> WatchId {
        let cur = match self.get(h) {
            Some(o) => o.c.watch_id,
            None => return WatchId::NONE,
        };
        if !cur.is_none() {
            return cur;
        }
        let id = self.next_watch_id;
        self.next_watch_id += 1;
        if self.watch_list.len() <= id as usize {
            self.watch_list.resize(id as usize + 1, ObjectHandle::NONE);
        }
        self.watch_list[id as usize] = h;
        if let Some(o) = self.get_mut(h) {
            o.c.watch_id = WatchId(id);
        }
        WatchId(id)
    }

    /// `getWatchID(false)`: the id if one was assigned, else 0.
    pub fn watch_id_peek(&self, h: ObjectHandle) -> WatchId {
        self.get(h).map(|o| o.c.watch_id).unwrap_or(WatchId::NONE)
    }

    /// `getByWatchID`: valid for `0 < id < next_watch_id`; `None` for a freed object.
    pub fn by_watch_id(&self, id: WatchId) -> Option<ObjectHandle> {
        if id.0 > 0 && id.0 < self.next_watch_id {
            let h = self.watch_list.get(id.0 as usize).copied()?;
            if !h.is_none() {
                return Some(h);
            }
        }
        None
    }

    /// `getByWatchID` as a reference.
    pub fn get_by_watch_id(&self, id: WatchId) -> Option<&Obj> {
        self.by_watch_id(id).and_then(|h| self.get(h))
    }

    /// `findByPartId`: first existing object in handle order with this part id (0 never matches).
    pub fn find_by_part_id(&self, part: PartId) -> Option<ObjectHandle> {
        if part.0 == 0 {
            return None;
        }
        self.list.iter().enumerate().skip(1).find_map(|(i, o)| {
            let o = o.as_ref()?;
            (o.exists() && o.c.part_id == part).then_some(ObjectHandle(i as u32))
        })
    }

    /// `findByCellPosition(row, col)`: first existing object (movers included, in handle order) standing on the cell.
    pub fn find_by_cell_position(&self, row: i32, col: i32) -> Option<ObjectHandle> {
        self.list.iter().enumerate().skip(1).find_map(|(i, o)| {
            let o = o.as_ref()?;
            (o.exists() && i32::from(o.c.cell.0) == row && i32::from(o.c.cell.1) == col)
                .then_some(ObjectHandle(i as u32))
        })
    }

    /// `findObjectByTypeHandle`.
    pub fn find_by_type_handle(&self, type_handle: i32) -> Option<ObjectHandle> {
        self.list.iter().enumerate().skip(1).find_map(|(i, o)| {
            let o = o.as_ref()?;
            (o.exists() && o.c.type_handle == type_handle).then_some(ObjectHandle(i as u32))
        })
    }

    /// `findByUnitInfo(commander, group, mate)`.
    pub fn find_by_unit_info(
        &mut self,
        commander: i32,
        group: i32,
        mate: i32,
    ) -> Option<ObjectHandle> {
        let part = self.calc_part_id(ObjectClass::Mover, commander, group, mate);
        self.find_by_part_id(PartId(part))
    }

    /// `findByBlockVertex(block, vertex)`.
    pub fn find_by_block_vertex(&mut self, block: i32, vertex: i32) -> Option<ObjectHandle> {
        let part = self.calc_part_id(ObjectClass::TerrainObject, block, vertex, 0);
        self.find_by_part_id(PartId(part))
    }

    /// `calcPartId(objectClass, p1, p2, p3)` (objmgr.cpp:2457). Movers with `p1 == -1` take the next reinforcement id
    /// (2050..=3550, then 0).
    pub fn calc_part_id(&mut self, class: ObjectClass, p1: i32, p2: i32, p3: i32) -> i32 {
        match class {
            ObjectClass::Mover
            | ObjectClass::BattleMech
            | ObjectClass::GroundVehicle
            | ObjectClass::Elemental => {
                if p1 == -1 {
                    if self.next_reinforcement_part_id == MAX_REINFORCEMENT_PART_ID + 1 {
                        return 0;
                    }
                    let id = self.next_reinforcement_part_id;
                    self.next_reinforcement_part_id += 1;
                    id
                } else {
                    MIN_MOVER_PART_ID
                        + p1 * MAX_MOVERGROUPS * MAX_MOVERGROUP_COUNT_START
                        + p2 * MAX_MOVERGROUP_COUNT_START
                        + p3
                }
            }
            ObjectClass::TerrainObject
            | ObjectClass::Tree
            | ObjectClass::Building
            | ObjectClass::TreeBuilding
            | ObjectClass::Turret
            | ObjectClass::Gate => MIN_TERRAIN_PART_ID + p1 * MAX_MAP_CELL_WIDTH + p2,
            _ => 0,
        }
    }

    /// `setPartId(obj, p1, p2, p3)`: all partIds are assigned through here.
    pub fn set_part_id(&mut self, h: ObjectHandle, p1: i32, p2: i32, p3: i32) {
        let class = match self.get(h) {
            Some(o) => o.c.class,
            None => return,
        };
        let id = self.calc_part_id(class, p1, p2, p3);
        if let Some(o) = self.get_mut(h) {
            o.c.part_id = PartId(id);
        }
    }

    /// The handle of mech slot `index` (`getMech`, only live slots `0..numMechs`).
    pub fn mech(&self, index: usize) -> Option<ObjectHandle> {
        if index >= self.num_mechs {
            return None;
        }
        self.mechs.get(index).copied()
    }

    /// The handle of vehicle slot `index` (`getVehicle`).
    pub fn vehicle(&self, index: usize) -> Option<ObjectHandle> {
        if index >= self.num_vehicles {
            return None;
        }
        self.vehicles.get(index).copied()
    }

    /// `getOpenVehicle()`: the first non-existing vehicle slot (raises `num_vehicles` to the slot index, as the C++ does).
    pub fn open_vehicle(&mut self) -> Option<ObjectHandle> {
        for i in 0..self.max_vehicles {
            let h = self.vehicles[i];
            if !self.get(h).map(|o| o.exists()).unwrap_or(false) {
                if i > self.num_vehicles {
                    self.num_vehicles = i;
                }
                return Some(h);
            }
        }
        None
    }

    /// `setNumObjects` (objmgr.cpp:292-495): allocate every object in the original order. Needs
    /// [`ObjectStore::count_terrain_objects`] to have run (it supplies the terrain/building/turret/gate counts). The only
    /// sim RNG draws are the turret constructors (`idleWait = RandomNumber(10)`, in turret index order).
    pub fn set_num_objects(&mut self, rng: &SharedSimRng, num_mechs: usize, num_vehicles: usize) {
        self.num_mechs = num_mechs;
        self.num_vehicles = num_vehicles;
        self.max_mechs = num_mechs + MAX_TEAMS * MAX_REINFORCEMENTS_PER_TEAM;
        self.max_vehicles = num_vehicles + MAX_TEAMS * MAX_REINFORCEMENTS_PER_TEAM;
        self.max_movers = self.max_mechs + self.max_vehicles;

        let n_terrain = self.count_terrain_objects_total;
        let n_buildings = self.count_buildings_total;
        let n_turrets = self.count_turrets_total;
        let n_gates = self.count_gates_total;
        let total = self.max_mechs
            + self.max_vehicles
            + n_terrain
            + n_buildings
            + n_turrets
            + n_gates
            + NUM_WEAPONS
            + NUM_CARNAGE
            + NUM_ARTILLERY;

        self.list = vec![None; total + 1];
        self.watch_list = vec![ObjectHandle::NONE; total + 1];
        self.next_watch_id = 1;
        self.weapon_index = 0;
        self.carnage_index = 0;
        self.artillery_index = 0;
        self.mechs.clear();
        self.vehicles.clear();
        self.terrain_objects.clear();
        self.buildings.clear();
        self.turrets.clear();
        self.gates.clear();
        self.weapons.clear();
        self.carnage.clear();
        self.artillery.clear();

        let mut cur = 1u32;
        fn put(list: &mut [Option<Obj>], cur: &mut u32, obj: Obj) -> ObjectHandle {
            let h = ObjectHandle(*cur);
            debug_assert_eq!(obj.c.handle, h);
            list[*cur as usize] = Some(obj);
            *cur += 1;
            h
        }
        let handle_for = |cur: u32| ObjectHandle(cur);

        for _ in 0..self.max_mechs {
            let obj = Obj {
                c: Common::new(ObjectClass::BattleMech, handle_for(cur)),
                kind: Kind::Mech(Box::default()),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.mechs.push(h);
        }
        for _ in 0..self.max_vehicles {
            let obj = Obj {
                c: Common::new(ObjectClass::GroundVehicle, handle_for(cur)),
                kind: Kind::Vehicle(Box::default()),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.vehicles.push(h);
        }
        for _ in 0..n_terrain {
            let obj = Obj {
                c: {
                    let mut c = Common::new(ObjectClass::TerrainObject, handle_for(cur));
                    c.set_flag(flags::JUSTCREATED, true);
                    c
                },
                kind: Kind::Terrain(Structure::default()),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.terrain_objects.push(h);
        }
        for _ in 0..n_buildings {
            let obj = Obj {
                c: {
                    let mut c = Common::new(ObjectClass::TerrainObject, handle_for(cur));
                    c.set_flag(flags::JUSTCREATED, true);
                    c
                },
                kind: Kind::Building(Box::default()),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.buildings.push(h);
        }
        for _ in 0..n_turrets {
            // Turret::init(bool) in the constructor: idleWait = RandomNumber(10) (turret.cpp:340).
            let idle_wait = rng.0.borrow_mut().sim.random_number(10) as f32;
            let obj = Obj {
                c: {
                    let mut c = Common::new(ObjectClass::TerrainObject, handle_for(cur));
                    c.set_flag(flags::JUSTCREATED, true);
                    c.set_flag(flags::CAN_FIRE, true);
                    c.set_flag(flags::ONFIRE, false);
                    c.set_flag(flags::CHECKED_ONSCREEN, false);
                    c
                },
                kind: Kind::Turret(Box::new(TurretState::new(idle_wait))),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.turrets.push(h);
        }
        for _ in 0..n_gates {
            let obj = Obj {
                c: {
                    let mut c = Common::new(ObjectClass::TerrainObject, handle_for(cur));
                    c.set_flag(flags::JUSTCREATED, true);
                    c
                },
                kind: Kind::Gate(Box::default()),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.gates.push(h);
        }
        for _ in 0..NUM_WEAPONS {
            let obj = Obj {
                // WeaponBolt::init never ran for the pool slot yet: objectClass is still GameObject.
                c: Common::new(ObjectClass::GameObject, handle_for(cur)),
                kind: Kind::WeaponBolt(Box::default()),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.weapons.push(h);
        }
        for _ in 0..NUM_CARNAGE {
            // Carnage instances keep objectClass == GameObject (Carnage::init never sets EXPLOSION).
            let obj = Obj {
                c: Common::new(ObjectClass::GameObject, handle_for(cur)),
                kind: Kind::Carnage(Box::default()),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.carnage.push(h);
        }
        for _ in 0..NUM_ARTILLERY {
            let obj = Obj {
                c: Common::new(ObjectClass::GameObject, handle_for(cur)),
                kind: Kind::Artillery(Box::default()),
            };
            let h = put(&mut self.list, &mut cur, obj);
            self.artillery.push(h);
        }
        debug_assert_eq!(cur as usize, total + 1);

        self.mover_los_table = vec![-1; (self.max_movers + 1) * (self.max_movers + 1)];
        self.use_mover_los_table = true;
        self.collidables.clear();
        self.collidables_built = false;
    }

    /// `newMech` / `newVehicle` (reinforcements): take the next free slot (`numX++`), re-init it, assign a reinforcement part
    /// id, clear the watch id and force a collidable list rebuild. The object is replaced by a fresh default of its class
    /// (`BattleMech::init(false)` / `GroundVehicle::init(false)`, lane B refines).
    pub fn new_mover(&mut self, class: ObjectClass) -> Option<ObjectHandle> {
        let is_mech = match class {
            ObjectClass::BattleMech => true,
            ObjectClass::GroundVehicle => false,
            _ => return None,
        };
        let (num, max) = if is_mech {
            (self.num_mechs, self.max_mechs)
        } else {
            (self.num_vehicles, self.max_vehicles)
        };
        if num >= max {
            return None;
        }
        let h = if is_mech {
            self.mechs[num]
        } else {
            self.vehicles[num]
        };
        let mut c = Common::new(class, h);
        c.set_flag(flags::EXISTS, true);
        let kind = if is_mech {
            Kind::Mech(Box::default())
        } else {
            Kind::Vehicle(Box::default())
        };
        self.list[h.0 as usize] = Some(Obj { c, kind });
        self.set_part_id(h, -1, -1, -1);
        self.rebuild_collidables = true;
        if is_mech {
            self.num_mechs += 1;
        } else {
            self.num_vehicles += 1;
        }
        Some(h)
    }

    /// `modifyMoverLists(mover, action)` (objmgr.cpp:1959). Returns whether the mover was found (delete only). The C++ reads
    /// the global `Team::home`, `Team::teams` and `Team::relations`; here they come from `world_teams`.
    pub fn modify_mover_lists(
        &mut self,
        world_teams: &crate::ai::Teams,
        h: ObjectHandle,
        action: MoverListAction,
    ) -> bool {
        self.team_view = TeamView::from_teams(world_teams);
        self.modify_mover_lists_view(h, action)
    }

    /// `modifyMoverLists(mover, ADD)` for callers that cannot lend the teams (the mission load holds `&mut World`): uses the
    /// team relations seen by the last [`ObjectStore::build_mover_lists`] / [`ObjectStore::modify_mover_lists`] call. Prefer
    /// `modify_mover_lists(&w.teams, h, MoverListAction::Add)` (disjoint field borrows of `w` allow it).
    pub fn modify_mover_lists_add(&mut self, h: ObjectHandle) -> bool {
        self.modify_mover_lists_view(h, MoverListAction::Add)
    }

    fn modify_mover_lists_view(&mut self, h: ObjectHandle, action: MoverListAction) -> bool {
        let (class, team) = match self.get(h) {
            Some(o) => (o.c.class, o.team_id()),
            None => return false,
        };
        let view = self.team_view.clone();
        let team_exists = team >= 0 && view.exists.get(team as usize).copied().unwrap_or(false);
        let relation = |t: i32| -> Option<crate::ai::Relation> {
            if t >= 0 && (t as usize) < MAX_TEAMS {
                view.relation_to_home[t as usize]
            } else {
                None
            }
        };
        match action {
            MoverListAction::Delete => {
                let mut found = false;
                if class == ObjectClass::BattleMech {
                    if let Some(i) = self.mechs[..self.num_mechs].iter().position(|&m| m == h) {
                        found = true;
                        // memmove(&mechs[i], &mechs[i + 1], ...); mechs[maxMechs - 1] = mech;
                        self.mechs[i..].rotate_left(1);
                        self.num_mechs -= 1;
                    }
                } else if class == ObjectClass::GroundVehicle {
                    if let Some(i) = self.vehicles[..self.num_vehicles]
                        .iter()
                        .position(|&m| m == h)
                    {
                        found = true;
                        self.vehicles[i..].rotate_left(1);
                        self.num_vehicles -= 1;
                    }
                }
                if let Some(i) = self.movers.iter().position(|&m| m == h) {
                    self.movers.swap_remove(i);
                }
                if found && team_exists {
                    match relation(team) {
                        Some(crate::ai::Relation::Friendly) => {
                            if let Some(i) = self.good_movers.iter().position(|&m| m == h) {
                                self.good_movers.swap_remove(i);
                            }
                        }
                        Some(crate::ai::Relation::Enemy) => {
                            if let Some(i) = self.bad_movers.iter().position(|&m| m == h) {
                                self.bad_movers.swap_remove(i);
                            }
                        }
                        _ => {}
                    }
                }
                found
            }
            MoverListAction::Add => {
                self.movers.push(h);
                if team_exists {
                    match relation(team) {
                        Some(crate::ai::Relation::Friendly) => self.good_movers.push(h),
                        Some(crate::ai::Relation::Enemy) => self.bad_movers.push(h),
                        _ => {}
                    }
                }
                true
            }
            MoverListAction::Trade => {
                if let Some(i) = self.good_movers.iter().position(|&m| m == h) {
                    self.good_movers.swap_remove(i);
                } else if let Some(i) = self.bad_movers.iter().position(|&m| m == h) {
                    self.bad_movers.swap_remove(i);
                }
                if team_exists {
                    match relation(team) {
                        Some(crate::ai::Relation::Friendly) => self.good_movers.push(h),
                        Some(crate::ai::Relation::Enemy) => self.bad_movers.push(h),
                        _ => {}
                    }
                }
                true
            }
        }
    }

    /// `buildMoverLists` (objmgr.cpp:1926): mechs then vehicles with a team, split into good and bad lists by their relation
    /// to the home team.
    pub fn build_mover_lists(&mut self, teams: &crate::ai::Teams) {
        self.team_view = TeamView::from_teams(teams);
        self.movers.clear();
        self.good_movers.clear();
        self.bad_movers.clear();
        let home = teams.home;
        let mut all: Vec<ObjectHandle> = Vec::new();
        all.extend_from_slice(&self.mechs[..self.num_mechs]);
        all.extend_from_slice(&self.vehicles[..self.num_vehicles]);
        for h in all {
            let team = self.get(h).map(|o| o.team_id()).unwrap_or(-1);
            let has_team = team >= 0 && teams.teams.get(team as usize).is_some_and(|t| t.is_some());
            if !has_team {
                continue;
            }
            self.movers.push(h);
            if home < MAX_TEAMS {
                match teams.relations[team as usize][home] {
                    crate::ai::Relation::Friendly => self.good_movers.push(h),
                    crate::ai::Relation::Enemy => self.bad_movers.push(h),
                    crate::ai::Relation::Neutral => {}
                }
            }
        }
    }

    /// `freeMover` (objmgr.cpp:597): take the mover out of the lists; if it was found, release it
    /// ([`hooks::release_mover`]), clear `exists`, set `REMOVED`, zero the part id and clear the watch id slot.
    pub fn free_mover(world: &mut crate::World, h: ObjectHandle) -> bool {
        let found = world
            .objects
            .modify_mover_lists(&world.teams, h, MoverListAction::Delete);
        if found {
            hooks::release_mover(world, h);
            let store = &mut world.objects;
            let wid = store.get(h).map(|o| o.c.watch_id).unwrap_or(WatchId::NONE);
            if let Some(o) = store.get_mut(h) {
                o.set_exists(false);
                o.c.set_flag(flags::REMOVED, true);
                o.c.part_id = PartId(0);
                o.c.watch_id = WatchId::NONE;
            }
            if let Some(slot) = store.watch_list.get_mut(wid.0 as usize) {
                *slot = ObjectHandle::NONE;
            }
        }
        found
    }

    /// The list and watch-id part of `tradeMover` (objmgr.cpp:626-630): the old id stays resolvable, the object gets a
    /// fresh one on its next `watch_id` call, and the good/bad mover lists are re-sorted.
    pub fn trade_mover_lists(&mut self, teams: &crate::ai::Teams, h: ObjectHandle) {
        if let Some(o) = self.get_mut(h) {
            o.c.watch_id = WatchId::NONE;
        }
        self.modify_mover_lists(teams, h, MoverListAction::Trade);
    }

    /// `isRemoved(mover)`: whether the part id is in the removed list.
    pub fn is_removed(&self, part: i32) -> bool {
        self.removed_movers.iter().any(|r| r.part_id == part)
    }

    /// `getCollidableList` / `buildCollidableList` (objmgr.cpp:2521-2572): mechs, vehicles, turrets, gates, carnage,
    /// artillery in array order. The snapshot is only rebuilt when asked (new movers) or the first time.
    pub fn collidable_list(&mut self) -> Vec<ObjectHandle> {
        if self.rebuild_collidables || !self.collidables_built {
            self.build_collidable_list();
        }
        self.collidables.clone()
    }

    /// `buildCollidableList`.
    pub fn build_collidable_list(&mut self) {
        self.rebuild_collidables = false;
        self.collidables_built = true;
        self.collidables.clear();
        let n_mechs = self.num_mechs.min(self.mechs.len());
        let n_veh = self.num_vehicles.min(self.vehicles.len());
        let mut v = Vec::new();
        v.extend_from_slice(&self.mechs[..n_mechs]);
        v.extend_from_slice(&self.vehicles[..n_veh]);
        v.extend_from_slice(&self.turrets);
        v.extend_from_slice(&self.gates);
        v.extend_from_slice(&self.carnage);
        v.extend_from_slice(&self.artillery);
        self.collidables = v;
    }

    /// `updateCaptureList` (objmgr.cpp:2595): per team, the targets of the movers that are executing a capture order.
    pub fn update_capture_list(&mut self, pilots: &crate::ai::PilotPool) {
        for l in self.capture_list.iter_mut() {
            l.clear();
        }
        for &m in &self.movers {
            let Some(o) = self.list.get(m.0 as usize).and_then(|o| o.as_ref()) else {
                continue;
            };
            if o.c.is_disabled() {
                continue;
            }
            let slot = match &o.kind {
                Kind::Mech(s) => s.m.pilot_slot,
                Kind::Vehicle(s) => s.m.pilot_slot,
                _ => continue,
            };
            let Some(pilot) = pilots.slots.get(slot as usize) else {
                continue;
            };
            if pilot.cur_order.code == crate::command::OrderCode::Capture {
                let team = o.team_id();
                if team >= 0 && (team as usize) < MAX_TEAMS {
                    let list = &mut self.capture_list[team as usize];
                    if list.len() < MAX_CAPTURES_PER_TEAM {
                        list.push(pilot.cur_order.target);
                    }
                }
            }
        }
    }

    /// `isTeamCapturing(team, targetWID)`; `None` asks about every team.
    pub fn is_team_capturing(&self, team: Option<usize>, target: WatchId) -> bool {
        match team {
            Some(t) => self
                .capture_list
                .get(t)
                .is_some_and(|l| l.contains(&target)),
            None => self.capture_list.iter().any(|l| l.contains(&target)),
        }
    }

    /// `getWeapon` / `getCarnage` / `getArtillery`: rotating pools (pre-increment; weapons and carnage force-recycle the
    /// victim, artillery returns `None` when the slot is still in use).
    ///
    /// Weapons and carnage `finishNow()` the next victim without checking whether it is in use; the bolt's `finishNow`
    /// belongs to lane C ([`hooks::finish_bolt_now`]), the carnage one is [`CarnageState::finish_now`].
    pub fn next_pool_slot(world: &mut crate::World, pool: Pool) -> Option<ObjectHandle> {
        match pool {
            Pool::Weapons => {
                let n = world.objects.weapons.len();
                if n == 0 || world.objects.weapon_index >= n {
                    return None;
                }
                world.objects.weapon_index += 1;
                if world.objects.weapon_index >= n {
                    world.objects.weapon_index = 0;
                }
                let h = world.objects.weapons[world.objects.weapon_index];
                hooks::finish_bolt_now(world, h);
                Some(h)
            }
            Pool::Carnage => {
                let n = world.objects.carnage.len();
                if n == 0 || world.objects.carnage_index >= n {
                    return None;
                }
                world.objects.carnage_index += 1;
                if world.objects.carnage_index >= n {
                    world.objects.carnage_index = 0;
                }
                let h = world.objects.carnage[world.objects.carnage_index];
                super::carnage::finish_now(world, h);
                Some(h)
            }
            Pool::Artillery => {
                let n = world.objects.artillery.len();
                if n == 0 || world.objects.artillery_index >= n {
                    // The C++ STOPs here; an empty pool cannot happen after setNumObjects.
                    return None;
                }
                world.objects.artillery_index += 1;
                if world.objects.artillery_index >= n {
                    world.objects.artillery_index = 0;
                }
                let h = world.objects.artillery[world.objects.artillery_index];
                // OK to return NULL: lets multiplayer know no more strikes are available.
                if world.objects.get(h).is_some_and(|o| o.exists()) {
                    return None;
                }
                if let Some(o) = world.objects.get_mut(h) {
                    // artillery->init(false); setExists(true) (Artillery::init is lane C; the shared fields reset here)
                    let handle = o.c.handle;
                    o.c = Common::new(ObjectClass::GameObject, handle);
                    o.set_exists(true);
                    o.kind = Kind::Artillery(Box::default());
                }
                Some(h)
            }
        }
    }

    /// `GameObjectManager::update(true, true, true)` in the original list order:
    /// capture list, special buildings, gates, terrain-object blocks (gated by the caller's visibility), mechs, vehicles,
    /// deferred `removeMover`, turrets, weapons, carnage, artillery.
    pub fn update(world: &mut crate::World) {
        Self::update_with(world, &mut NoObserver);
    }

    /// `GameObjectManager::updateAppearancesOnly(true, true, true)` (objmgr.cpp:2775), the paused-frame update.
    ///
    /// Terrain-object blocks (active vertex or `turn < 3`) and turrets run `recalcBounds` (the simulation only sees the
    /// resulting `inView`). Mechs run `recalcBounds`, `setObjectParameters` and `update(false)`: that still advances the
    /// weapon-node recycle timers and, for a destroyed mech, draws the sim `RollDice(50)` (lane F's
    /// [`Mech3DAppearance::update`](crate::appearance::MechAppearance::update)). The camera's level of detail is not an input
    /// yet (lod 0). Vehicle appearances (`GVAppearance`) are not ported, so vehicles only get `windowsVisible`.
    pub fn update_appearances_only(world: &mut crate::World) {
        let turn = world.clock.turn;
        for b in 0..world.objects.blocks.len() {
            if !world.visibility.block_active(b as u32) {
                continue;
            }
            let (first, count) = {
                let blk = &world.objects.blocks[b];
                (blk.first_handle, blk.num_objects)
            };
            for i in 0..count {
                let h = ObjectHandle(first + i);
                let vertex = world.objects.get(h).map(|o| o.c.vertex_num);
                let Some(vertex) = vertex else { continue };
                let exists = world.objects.get(h).is_some_and(|o| o.exists());
                if exists && (world.visibility.vertex_active(vertex as u32) || turn < 3) {
                    hooks::recalc_bounds(world, h);
                }
            }
        }

        // Movers: mechs, then vehicles.
        let mechs: Vec<ObjectHandle> = world.objects.mechs[..world.objects.num_mechs].to_vec();
        for h in mechs {
            let Some(o) = world.objects.get(h) else {
                continue;
            };
            if !o.exists() {
                continue;
            }
            let (part, mut pos, rot) = (o.c.part_id, o.c.position, o.c.rotation);
            let in_view = world.visibility.mover_in_view(part);
            let lod = world.visibility.mover_lod(part);
            let in_view = world.appearance.recalc_bounds(h, in_view, lod);
            if in_view {
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.windows_visible = turn as i32;
                }
            }
            // "Must force here as well": the position is put on the terrain.
            pos.z = super::terr::elevation(&world.terrain, &pos);
            world.appearance.set_object_parameters(h, pos, rot);
            // CROSS-LANE (lane F merge): the world entry point builds the UpdateCtx (clock, SIM stream, terrain).
            crate::appearance::update(world, h, false);
        }
        let vehicles: Vec<ObjectHandle> =
            world.objects.vehicles[..world.objects.num_vehicles].to_vec();
        for h in vehicles {
            if world.objects.get(h).is_some_and(|o| o.exists()) {
                if let Some(o) = world.objects.get_mut(h) {
                    o.c.windows_visible = turn as i32;
                }
            }
        }

        let turrets: Vec<ObjectHandle> = world.objects.turrets.clone();
        for h in turrets {
            if world.objects.get(h).is_some_and(|o| o.exists()) {
                let v = hooks::recalc_bounds(world, h);
                hooks::set_in_view(world, h, v);
            }
        }
    }

    /// [`ObjectStore::update`] with an observer called before every object update (tests use it to replay movers and to
    /// line the RNG stream up with the oracle).
    pub fn update_with(world: &mut crate::World, obs: &mut dyn UpdateObserver) {
        let turn = world.clock.turn;
        // updateCaptureList
        world.objects.update_capture_list(&world.pilots);

        // First update all of the special buildings (they mark themselves updated and do not re-update below).
        let specials: Vec<ObjectHandle> = world.objects.special_buildings.clone();
        for h in specials {
            if world.objects.get(h).is_some_and(|o| o.exists()) && !run_update(world, obs, h) {
                if let Some(o) = world.objects.get_mut(h) {
                    o.set_exists(false);
                }
            }
        }

        // Then update all of the gates (every frame, or they do not open).
        let gates: Vec<ObjectHandle> = world.objects.gates.clone();
        for h in gates {
            if world.objects.get(h).is_some_and(|o| o.exists()) && !run_update(world, obs, h) {
                if let Some(o) = world.objects.get_mut(h) {
                    o.set_exists(false);
                }
            }
        }

        // Terrain-object blocks.
        for b in 0..world.objects.blocks.len() {
            let (first, count) = {
                let blk = &world.objects.blocks[b];
                (blk.first_handle, blk.num_objects)
            };
            if world.visibility.block_active(b as u32) || turn < 3 {
                for i in 0..count {
                    let h = ObjectHandle(first + i);
                    let vertex = match world.objects.get(h) {
                        Some(o) => o.c.vertex_num,
                        None => continue,
                    };
                    let vertex_active = if vertex >= 0 {
                        world.visibility.vertex_active(vertex as u32)
                    } else {
                        false
                    };
                    if (vertex_active || turn < 3)
                        && world.objects.get(h).is_some_and(|o| o.exists())
                        && !run_update(world, obs, h)
                    {
                        if let Some(o) = world.objects.get_mut(h) {
                            o.set_exists(false);
                        }
                    }
                }
            }
        }

        // Movers.
        let mut remove_list: Vec<ObjectHandle> = Vec::new();
        let mut i = 0;
        while i < world.objects.num_mechs {
            let h = match world.objects.mechs.get(i) {
                Some(&h) => h,
                None => break,
            };
            i += 1;
            if world.objects.get(h).is_some_and(|o| o.exists()) {
                if !run_update(world, obs, h) {
                    if let Some(o) = world.objects.get_mut(h) {
                        o.set_exists(false);
                    }
                }
                if world
                    .objects
                    .get(h)
                    .is_some_and(|o| o.c.flag(flags::REMOVED))
                {
                    remove_list.push(h);
                }
            }
        }
        let mut i = 0;
        while i < world.objects.max_vehicles {
            let h = match world.objects.vehicles.get(i) {
                Some(&h) => h,
                None => break,
            };
            i += 1;
            if world.objects.get(h).is_some_and(|o| o.exists()) {
                if !run_update(world, obs, h) {
                    if let Some(o) = world.objects.get_mut(h) {
                        o.set_exists(false);
                    }
                }
                if world
                    .objects
                    .get(h)
                    .is_some_and(|o| o.c.flag(flags::REMOVED))
                {
                    remove_list.push(h);
                }
            }
        }
        for h in remove_list {
            crate::mission::remove_mover(world, h);
        }

        // Turrets again (they have no updatedTurn guard, so they update twice a frame when their vertex is active).
        let turrets: Vec<ObjectHandle> = world.objects.turrets.clone();
        for h in turrets {
            if world.objects.get(h).is_some_and(|o| o.exists()) && !run_update(world, obs, h) {
                if let Some(o) = world.objects.get_mut(h) {
                    o.set_exists(false);
                }
            }
        }

        // Weapon bolts (pool slot order), carnage, artillery.
        let weapons: Vec<ObjectHandle> = world.objects.weapons.clone();
        for h in weapons {
            if world.objects.get(h).is_some_and(|o| o.exists()) && !run_update(world, obs, h) {
                if let Some(o) = world.objects.get_mut(h) {
                    o.set_exists(false);
                }
            }
        }
        let carnage: Vec<ObjectHandle> = world.objects.carnage.clone();
        for h in carnage {
            if world.objects.get(h).is_some_and(|o| o.exists()) && !run_update(world, obs, h) {
                if let Some(o) = world.objects.get_mut(h) {
                    o.set_exists(false);
                }
            }
        }
        let artillery: Vec<ObjectHandle> = world.objects.artillery.clone();
        for h in artillery {
            if world.objects.get(h).is_some_and(|o| o.exists()) && !run_update(world, obs, h) {
                if let Some(o) = world.objects.get_mut(h) {
                    o.set_exists(false);
                }
            }
        }
    }

    /// `createExplosion` (init order matters: effect id, type, position, extent, damage, exists, owner).
    pub fn create_explosion(
        world: &mut crate::World,
        effect_id: i32,
        owner: Option<ObjectHandle>,
        pos: Vec3,
        damage: f32,
        radius: f32,
    ) -> Option<ObjectHandle> {
        let obj_num = world.objects.effect_object_number(effect_id);
        if obj_num == -1 {
            return None;
        }
        let h = Self::next_pool_slot(world, Pool::Carnage)?;
        // getCarnage(CARNAGE_EXPLOSION): finishNow() ran inside next_pool_slot, now init(carnageType)
        if let Some(o) = world.objects.get_mut(h) {
            if let Kind::Carnage(c) = &mut o.kind {
                c.init_type(CarnageType::Explosion);
            }
        }
        let class = world.objects.type_by_num(obj_num).map(type_object_class);
        match class {
            Some(ObjectClass::Explosion) | Some(ObjectClass::Fire) => {
                // Call in this order or badness: init(effectId), init(false, type), setPosition, setExtentRadius, setExplDmg,
                // setExists, setOwner.
                super::carnage::init_instance(world, h, effect_id, obj_num);
                super::set_position(world, h, pos, true);
                if let Some(o) = world.objects.get_mut(h) {
                    if let Kind::Carnage(c) = &mut o.kind {
                        c.set_extent_radius(radius);
                    }
                    o.c.expl_damage = damage;
                    o.set_exists(true);
                }
                super::carnage::set_owner(world, h, owner);
                let sound_id = world.objects.explosion_sound_id(effect_id);
                world.events.push(crate::SimEvent::Explosion {
                    effect_id,
                    sound_id,
                    at: pos,
                    radius,
                });
                Some(h)
            }
            _ => None,
        }
    }

    /// `WeaponEffects::GetEffectName(effectId)`: "NONE" when the id is unknown.
    pub fn effect_name(&self, effect_id: i32) -> &str {
        if effect_id >= 0 && (effect_id as usize) < self.effect_names.len() {
            &self.effect_names[effect_id as usize]
        } else {
            "NONE"
        }
    }

    /// `ExplosionType::soundEffectId` (`carnage.cpp:677-678`) of the object type behind `effect_id`; `None` when the effect has no
    /// explosion type.
    pub fn explosion_sound_id(&self, effect_id: i32) -> Option<i32> {
        let ty = self.type_by_num(self.effect_object_number(effect_id))?;
        match &ty.data {
            mc2_sim_data::objtype::ObjectTypeData::Explosion(explosion) => {
                i32::try_from(explosion.sound_effect_id).ok()
            }
            _ => None,
        }
    }

    /// `WeaponEffects::GetEffectObjNum(effectId)`: -1 when the effect id is unknown.
    pub fn effect_object_number(&self, effect_id: i32) -> i32 {
        if effect_id >= 0 && (effect_id as usize) < self.effect_objects.len() {
            self.effect_objects[effect_id as usize]
        } else {
            -1
        }
    }
}

/// Called before each object update by [`ObjectStore::update_with`]; returning `false` skips that object's update.
pub trait UpdateObserver {
    /// Before `update()` of `h`.
    fn before_update(&mut self, world: &mut crate::World, h: ObjectHandle) -> bool;
    /// After `update()` of `h` ran (not called when `before_update` skipped it).
    fn after_update(&mut self, _world: &mut crate::World, _h: ObjectHandle) {}
}

/// The observer of the normal update: runs everything.
pub struct NoObserver;

impl UpdateObserver for NoObserver {
    fn before_update(&mut self, _world: &mut crate::World, _h: ObjectHandle) -> bool {
        true
    }
}

fn run_update(world: &mut crate::World, obs: &mut dyn UpdateObserver, h: ObjectHandle) -> bool {
    if obs.before_update(world, h) {
        let alive = super::update_object(world, h);
        obs.after_update(world, h);
        alive
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::RngMode;
    use crate::World;

    /// A store with `nm` mechs and `nv` vehicles and no structures (the pools are always there).
    fn store(nm: usize, nv: usize) -> World {
        let mut w = World::new(RngMode::LegacyMsvc, 1);
        let rng = w.rng.clone();
        w.objects.set_num_objects(&rng, nm, nv);
        w
    }

    #[test]
    fn handle_layout_and_pools() {
        let w = store(3, 2);
        let s = &w.objects;
        // maxMechs = numMechs + 8 teams * 16 reinforcements
        assert_eq!((s.max_mechs, s.max_vehicles), (3 + 128, 2 + 128));
        assert_eq!(s.max_movers, 3 + 128 + 2 + 128);
        // mechs 1.., vehicles after them, then (no structures) weapons 100, carnage 50, artillery 130
        assert_eq!(s.mechs[0], ObjectHandle(1));
        assert_eq!(s.vehicles[0], ObjectHandle(1 + s.max_mechs as u32));
        assert_eq!(s.weapons.len(), 100);
        assert_eq!(s.carnage.len(), 50);
        assert_eq!(s.artillery.len(), 130);
        assert_eq!(s.max_objects(), s.max_movers + 100 + 50 + 130);
        // pool objects start as plain GameObjects
        let c = s.get(s.carnage[0]).unwrap();
        assert_eq!(c.c.class, ObjectClass::GameObject);
        assert!(matches!(c.kind, Kind::Carnage(_)));
    }

    #[test]
    fn watch_ids_follow_the_call_order() {
        let mut w = store(2, 0);
        let (a, b) = (w.objects.mechs[0], w.objects.mechs[1]);
        // ids are handed out on first request, not by handle
        assert_eq!(w.objects.watch_id(b), WatchId(1));
        assert_eq!(w.objects.watch_id(a), WatchId(2));
        assert_eq!(w.objects.watch_id(b), WatchId(1));
        assert_eq!(w.objects.by_watch_id(WatchId(2)), Some(a));
        assert_eq!(w.objects.by_watch_id(WatchId(1)), Some(b));
        // 0 and ids never handed out resolve to nothing
        assert_eq!(w.objects.by_watch_id(WatchId(0)), None);
        assert_eq!(w.objects.by_watch_id(WatchId(3)), None);
    }

    #[test]
    fn carnage_pool_increments_before_use_and_wraps() {
        let mut w = store(1, 0);
        let first = w.objects.carnage[0];
        let h1 = ObjectStore::next_pool_slot(&mut w, Pool::Carnage).unwrap();
        // the index is bumped first, so slot 0 is only reached after a full lap
        assert_eq!(h1, w.objects.carnage[1]);
        for _ in 0..48 {
            ObjectStore::next_pool_slot(&mut w, Pool::Carnage).unwrap();
        }
        assert_eq!(w.objects.carnage_index, 49);
        let wrapped = ObjectStore::next_pool_slot(&mut w, Pool::Carnage).unwrap();
        assert_eq!(wrapped, first);
        assert_eq!(w.objects.carnage_index, 0);
    }

    #[test]
    fn new_mover_and_free_mover_keep_the_list_order_like_memmove() {
        let mut w = store(0, 0);
        let handles: Vec<ObjectHandle> = (0..3)
            .map(|_| w.objects.new_mover(ObjectClass::BattleMech).unwrap())
            .collect();
        assert_eq!(handles, w.objects.mechs[..3].to_vec());
        let teams = w.teams.clone();
        for &h in &handles {
            w.objects
                .modify_mover_lists(&teams, h, MoverListAction::Add);
        }
        assert_eq!(w.objects.movers, handles);
        // freeMover(first): the mech array shifts down, the freed handle goes to the end of the whole array (maxMechs - 1),
        // numMechs drops, but the mover list is a swap-remove
        assert!(ObjectStore::free_mover(&mut w, handles[0]));
        assert_eq!(w.objects.num_mechs, 2);
        assert_eq!(w.objects.mechs[..2], [handles[1], handles[2]]);
        assert_eq!(*w.objects.mechs.last().unwrap(), handles[0]);
        assert_eq!(w.objects.movers, vec![handles[2], handles[1]]);
        assert!(!w.objects.get(handles[0]).unwrap().exists());
        // a second free of the same mover finds nothing
        assert!(!ObjectStore::free_mover(&mut w, handles[0]));
    }

    #[test]
    fn modify_mover_lists_add_uses_the_last_team_view() {
        let mut w = store(0, 0);
        let h = w.objects.new_mover(ObjectClass::BattleMech).unwrap();
        // no team yet (team -1): only the plain mover list
        assert!(w.objects.modify_mover_lists_add(h));
        assert_eq!(w.objects.movers, vec![h]);
        assert!(w.objects.good_movers.is_empty() && w.objects.bad_movers.is_empty());
    }

    #[test]
    fn paused_update_marks_vehicles_and_tolerates_movers_without_appearances() {
        let mut w = store(0, 0);
        let v = w.objects.new_mover(ObjectClass::GroundVehicle).unwrap();
        w.clock.turn = 7;
        ObjectStore::update_appearances_only(&mut w);
        assert_eq!(w.objects.get(v).unwrap().c.windows_visible, 7);
    }
}

// ==================================================================================================================
// Save/load (DESIGN.md section 9). Appended by the lane A save work.
//
// `ObjectStore` is a manual impl (not `snapshot_struct!`): `list` and `struct_anim` must overlay per slot, because their
// entries carry static parts that live in the base world (`Kind` payloads through their own `load_into`, `StructAnim::ty`
// through the macro's skipped field). `save` destructures every field, so a new field without a classification is a
// compile error; `load` names every field too.
//
// Static (not written; `load` gives `Default`, `load_into` keeps the base): `types`, `effect_objects`, `effect_names`,
// `hit_sink` (test sink), `anim_types`, `mine_splash`, `weapon_ranges`, `pending` (load staging, taken by
// `load_terrain_objects`). Derived (rebuilt by `load`/`load_into` as the all -1 table of `set_num_objects`, and reset at the
// start of every `Sim::step` before any read): `mover_los_table`.
// ==================================================================================================================

use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};

/// `BTreeMap` (ordered, so the encoding is deterministic). Keys are written in iteration order.
impl<K: Snapshot + Ord, V: Snapshot> Snapshot for BTreeMap<K, V> {
    fn save(&self, w: &mut Writer) {
        w.len(self.len());
        for (k, v) in self {
            k.save(w);
            v.save(w);
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let n = r.length()?;
        let mut out = BTreeMap::new();
        for _ in 0..n {
            let k = K::load(r)?;
            let v = V::load(r)?;
            if out.insert(k, v).is_some() {
                return Err(SaveError("save: duplicate map key".into()));
            }
        }
        Ok(out)
    }
}

/// `Option<Relation>` as one byte: 0 none, 1 friendly, 2 neutral, 3 enemy.
fn save_relation(rel: Option<crate::ai::Relation>, w: &mut Writer) {
    use crate::ai::Relation;
    w.u8(match rel {
        None => 0,
        Some(Relation::Friendly) => 1,
        Some(Relation::Neutral) => 2,
        Some(Relation::Enemy) => 3,
    });
}

fn load_relation(r: &mut Reader<'_>) -> SaveResult<Option<crate::ai::Relation>> {
    use crate::ai::Relation;
    Ok(match r.u8()? {
        0 => None,
        1 => Some(Relation::Friendly),
        2 => Some(Relation::Neutral),
        3 => Some(Relation::Enemy),
        t => return Err(SaveError(format!("save: bad relation tag {t}"))),
    })
}

/// The team facts of the last team-taking call: all dynamic (read by `modify_mover_lists_add`, which runs without teams).
impl Snapshot for TeamView {
    fn save(&self, w: &mut Writer) {
        self.exists.save(w);
        w.len(self.relation_to_home.len());
        for rel in &self.relation_to_home {
            save_relation(*rel, w);
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let exists = Vec::<bool>::load(r)?;
        let n = r.length()?;
        let mut relation_to_home = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            relation_to_home.push(load_relation(r)?);
        }
        Ok(TeamView {
            exists,
            relation_to_home,
        })
    }
}

crate::snapshot_struct!(ObjBlock {
    first_handle,
    num_objects,
    num_collidable,
});

crate::snapshot_struct!(RemovedMover { turn, part_id });

/// Overlays a `Vec<Option<T>>` slot by slot (same bytes as `Vec<Option<T>>::save`): an occupied base slot keeps its static
/// parts through `T::load_into`. `exact` requires the saved length to equal the base length (the object list is fixed by
/// `set_num_objects`); otherwise the vector is resized to the saved length (`struct_anim` grows lazily).
fn load_opt_vec_into<T: Snapshot>(
    dst: &mut Vec<Option<T>>,
    r: &mut Reader<'_>,
    exact: bool,
) -> SaveResult<()> {
    let n = r.length()?;
    if exact && n != dst.len() {
        return Err(SaveError(format!(
            "save: {n} slots in the save, {} in the base world (different mission?)",
            dst.len()
        )));
    }
    dst.resize_with(n, || None);
    for slot in dst.iter_mut() {
        match r.u8()? {
            0 => *slot = None,
            1 => {
                if let Some(v) = slot.as_mut() {
                    v.load_into(r)?;
                } else {
                    *slot = Some(T::load(r)?);
                }
            }
            t => return Err(SaveError(format!("save: bad option tag {t}"))),
        }
    }
    Ok(())
}

/// The LOS memo as `set_num_objects` makes it (all unknown). Derived: `Sim::step` resets it before any read.
fn fresh_los_table(max_movers: usize) -> Vec<i8> {
    vec![-1; (max_movers + 1) * (max_movers + 1)]
}

impl Snapshot for ObjectStore {
    fn save(&self, w: &mut Writer) {
        // Exhaustive destructuring: every field is named, so a new field must be classified here.
        let ObjectStore {
            types: _,
            type_extent,
            list,
            num_mechs,
            max_mechs,
            num_vehicles,
            max_vehicles,
            max_movers,
            mechs,
            vehicles,
            terrain_objects,
            buildings,
            turrets,
            gates,
            special_buildings,
            gate_controls,
            turret_controls,
            power_generators,
            weapons,
            weapon_index,
            carnage,
            carnage_index,
            artillery,
            artillery_index,
            blocks,
            movers,
            good_movers,
            bad_movers,
            team_view,
            next_watch_id,
            watch_list,
            collidables,
            collidables_built,
            rebuild_collidables,
            next_reinforcement_part_id,
            mover_los_table: _,
            use_mover_los_table,
            capture_list,
            removed_movers,
            pending: _,
            count_terrain_objects_total,
            count_buildings_total,
            count_turrets_total,
            count_gates_total,
            effect_objects: _,
            effect_names: _,
            effect_rows: _,
            use_non_weapon_effects: _,
            hit_sink: _,
            mine_splash: _,
            weapon_ranges: _,
            resource_points_gained,
            collision,
        } = self;
        type_extent.save(w);
        list.save(w);
        num_mechs.save(w);
        max_mechs.save(w);
        num_vehicles.save(w);
        max_vehicles.save(w);
        max_movers.save(w);
        mechs.save(w);
        vehicles.save(w);
        terrain_objects.save(w);
        buildings.save(w);
        turrets.save(w);
        gates.save(w);
        special_buildings.save(w);
        gate_controls.save(w);
        turret_controls.save(w);
        power_generators.save(w);
        weapons.save(w);
        weapon_index.save(w);
        carnage.save(w);
        carnage_index.save(w);
        artillery.save(w);
        artillery_index.save(w);
        blocks.save(w);
        movers.save(w);
        good_movers.save(w);
        bad_movers.save(w);
        team_view.save(w);
        next_watch_id.save(w);
        watch_list.save(w);
        collidables.save(w);
        collidables_built.save(w);
        rebuild_collidables.save(w);
        next_reinforcement_part_id.save(w);
        use_mover_los_table.save(w);
        capture_list.save(w);
        removed_movers.save(w);
        count_terrain_objects_total.save(w);
        count_buildings_total.save(w);
        count_turrets_total.save(w);
        count_gates_total.save(w);
        resource_points_gained.save(w);
        collision.save(w);
    }

    /// Fresh store (no base world): the static fields are `Default`, the list entries are rebuilt from the save.
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let mut s = ObjectStore {
            type_extent: BTreeMap::load(r)?,
            list: Vec::load(r)?,
            ..ObjectStore::default()
        };
        s.num_mechs = usize::load(r)?;
        s.max_mechs = usize::load(r)?;
        s.num_vehicles = usize::load(r)?;
        s.max_vehicles = usize::load(r)?;
        s.max_movers = usize::load(r)?;
        s.mechs = Vec::load(r)?;
        s.vehicles = Vec::load(r)?;
        s.terrain_objects = Vec::load(r)?;
        s.buildings = Vec::load(r)?;
        s.turrets = Vec::load(r)?;
        s.gates = Vec::load(r)?;
        s.special_buildings = Vec::load(r)?;
        s.gate_controls = Vec::load(r)?;
        s.turret_controls = Vec::load(r)?;
        s.power_generators = Vec::load(r)?;
        s.weapons = Vec::load(r)?;
        s.weapon_index = usize::load(r)?;
        s.carnage = Vec::load(r)?;
        s.carnage_index = usize::load(r)?;
        s.artillery = Vec::load(r)?;
        s.artillery_index = usize::load(r)?;
        s.blocks = Vec::load(r)?;
        s.movers = Vec::load(r)?;
        s.good_movers = Vec::load(r)?;
        s.bad_movers = Vec::load(r)?;
        s.team_view = TeamView::load(r)?;
        s.next_watch_id = u32::load(r)?;
        s.watch_list = Vec::load(r)?;
        s.collidables = Vec::load(r)?;
        s.collidables_built = bool::load(r)?;
        s.rebuild_collidables = bool::load(r)?;
        s.next_reinforcement_part_id = i32::load(r)?;
        s.use_mover_los_table = bool::load(r)?;
        s.capture_list = <[Vec<WatchId>; MAX_TEAMS]>::load(r)?;
        s.removed_movers = Vec::load(r)?;
        s.count_terrain_objects_total = usize::load(r)?;
        s.count_buildings_total = usize::load(r)?;
        s.count_turrets_total = usize::load(r)?;
        s.count_gates_total = usize::load(r)?;
        s.resource_points_gained = i32::load(r)?;
        s.collision = CollisionSystem::load(r)?;
        s.mover_los_table = fresh_los_table(s.max_movers);
        Ok(s)
    }

    /// Overlay onto a freshly initialised base of the same mission: static fields keep the base value, each list slot
    /// keeps its base static parts, and the object list must have the base's length.
    fn load_into(&mut self, r: &mut Reader<'_>) -> SaveResult<()> {
        self.type_extent.load_into(r)?;
        load_opt_vec_into(&mut self.list, r, true)?;
        self.num_mechs.load_into(r)?;
        self.max_mechs.load_into(r)?;
        self.num_vehicles.load_into(r)?;
        self.max_vehicles.load_into(r)?;
        self.max_movers.load_into(r)?;
        self.mechs.load_into(r)?;
        self.vehicles.load_into(r)?;
        self.terrain_objects.load_into(r)?;
        self.buildings.load_into(r)?;
        self.turrets.load_into(r)?;
        self.gates.load_into(r)?;
        self.special_buildings.load_into(r)?;
        self.gate_controls.load_into(r)?;
        self.turret_controls.load_into(r)?;
        self.power_generators.load_into(r)?;
        self.weapons.load_into(r)?;
        self.weapon_index.load_into(r)?;
        self.carnage.load_into(r)?;
        self.carnage_index.load_into(r)?;
        self.artillery.load_into(r)?;
        self.artillery_index.load_into(r)?;
        self.blocks.load_into(r)?;
        self.movers.load_into(r)?;
        self.good_movers.load_into(r)?;
        self.bad_movers.load_into(r)?;
        self.team_view.load_into(r)?;
        self.next_watch_id.load_into(r)?;
        self.watch_list.load_into(r)?;
        self.collidables.load_into(r)?;
        self.collidables_built.load_into(r)?;
        self.rebuild_collidables.load_into(r)?;
        self.next_reinforcement_part_id.load_into(r)?;
        self.use_mover_los_table.load_into(r)?;
        self.capture_list.load_into(r)?;
        self.removed_movers.load_into(r)?;
        self.count_terrain_objects_total.load_into(r)?;
        self.count_buildings_total.load_into(r)?;
        self.count_turrets_total.load_into(r)?;
        self.count_gates_total.load_into(r)?;
        self.resource_points_gained.load_into(r)?;
        self.collision.load_into(r)?;
        self.mover_los_table = fresh_los_table(self.max_movers);
        Ok(())
    }
}
