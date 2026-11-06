// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! LANE A - objects: storage, handles, terrain objects, buildings, turrets (non-weapon part), gates, carnage, collisions,
//! trigger areas.
//!
//! Port of `code/gameobj.{h,cpp}`, `objmgr.{h,cpp}`, `terrobj`, `bldng`, `turret` (everything except fire control, which
//! is lane C), `gate`, `carnage`, `collsn`, `trigger` (see DESIGN.md "Lane A"). The multiplayer chunk code in
//! `gameobj.cpp`, the save/load code in `objmgr.cpp` and the render/mouse-pick code are NOT ported. `light.cpp` is dead
//! in the shipped game (`numLights = 0`).
//!
//! Design: an object is [`Obj`] = [`Common`] (the shared `GameObject` fields) + [`Kind`] (an enum, no inheritance).
//! Per-kind payloads that belong to other lanes (movers: lane B, bolts and artillery: lane C) are defined in those
//! lanes and embedded here. Objects live in [`ObjectStore`], addressed by [`ObjectHandle`] exactly like
//! `GameObjectManager::objList`, so iteration order (which decides update order, `findByPartId` results, collision pair
//! order...) is reproduced.
//!
//! Everything this lane needs from other lanes (appearance gestures, sound/flash requests, resource points, team
//! relations...) goes through [`hooks`], one file, each function documented with the C++ it stands for and what it does
//! until the owning lane replaces it.

pub mod carnage;
pub mod collision;
pub mod digest;
pub mod gate_oracle;
pub mod hooks;
pub mod load;
pub mod store;
pub mod structure;
pub mod terr;
pub mod trigger;

pub use gate_oracle::GateSnapshot;
pub use mc2_sim_data::objtype::ObjectClass;
pub use store::{ObjBlock, ObjectStore, Pool};

use crate::ids::{ObjectHandle, PartId, WatchId};
use crate::math::Vec3;

/// `GameObject::flags` bits (`code/dgameobj.h:39-80`). The top byte is reused per kind.
pub mod flags {
    /// `OBJECT_FLAG_AWAKE`.
    pub const AWAKE: u32 = 0x0000_0001;
    /// `OBJECT_FLAG_EXISTS`.
    pub const EXISTS: u32 = 0x0000_0002;
    /// `OBJECT_FLAG_USEME`.
    pub const USEME: u32 = 0x0000_0004;
    /// `OBJECT_FLAG_CAPTURED`.
    pub const CAPTURED: u32 = 0x0000_0008;
    /// `OBJECT_FLAG_TANGIBLE`.
    pub const TANGIBLE: u32 = 0x0000_0010;
    /// `OBJECT_FLAG_SELECTED`.
    pub const SELECTED: u32 = 0x0000_0020;
    /// `OBJECT_FLAG_TARGETED`.
    pub const TARGETED: u32 = 0x0000_0040;
    /// `OBJECT_FLAG_JUSTCREATED`.
    pub const JUSTCREATED: u32 = 0x0000_0080;
    /// `OBJECT_FLAG_ONFIRE`.
    pub const ONFIRE: u32 = 0x0000_0100;
    /// `OBJECT_FLAG_ANIMATED`.
    pub const ANIMATED: u32 = 0x0000_0200;
    /// `OBJECT_FLAG_CAPTURABLE`.
    pub const CAPTURABLE: u32 = 0x0000_0400;
    /// `OBJECT_FLAG_FALLING`.
    pub const FALLING: u32 = 0x0000_0800;
    /// `OBJECT_FLAG_FALLEN`.
    pub const FALLEN: u32 = 0x0000_1000;
    /// `OBJECT_FLAG_DAMAGED`.
    pub const DAMAGED: u32 = 0x0000_2000;
    /// `OBJECT_FLAG_MECHBAY`.
    pub const MECHBAY: u32 = 0x0000_4000;
    /// `OBJECT_FLAG_CANREFIT`.
    pub const CANREFIT: u32 = 0x0000_8000;
    /// `OBJECT_FLAG_TILECHANGED`.
    pub const TILECHANGED: u32 = 0x0001_0000;
    /// `OBJECT_FLAG_DESTROYED`.
    pub const DESTROYED: u32 = 0x0002_0000;
    /// `OBJECT_FLAG_SPREAD`.
    pub const SPREAD: u32 = 0x0004_0000;
    /// `OBJECT_FLAG_DONE`.
    pub const DONE: u32 = 0x0008_0000;
    /// `OBJECT_FLAG_OBSCURED`.
    pub const OBSCURED: u32 = 0x0010_0000;
    /// `OBJECT_FLAG_SENSOR`.
    pub const SENSOR: u32 = 0x0020_0000;
    /// `OBJECT_FLAG_REMOVED`.
    pub const REMOVED: u32 = 0x0040_0000;
    /// `OBJECT_FLAG_CONTROLBUILDING`.
    pub const CONTROLBUILDING: u32 = 0x0080_0000;
    /// GroundVehicle/Turret: `OBJECT_FLAG_CHECKED_ONSCREEN`.
    pub const CHECKED_ONSCREEN: u32 = 0x0100_0000;
    /// GroundVehicle/Turret: `OBJECT_FLAG_CAN_FIRE`.
    pub const CAN_FIRE: u32 = 0x0200_0000;
    /// GroundVehicle/Turret: `OBJECT_FLAG_VEHICLE_APPR`.
    pub const VEHICLE_APPR: u32 = 0x0400_0000;
    /// GroundVehicle/Turret: `OBJECT_FLAG_NEVER_REVEALED`.
    pub const NEVER_REVEALED: u32 = 0x0800_0000;
    /// GroundVehicle/Turret: `OBJECT_FLAG_FACING_TARGET`.
    pub const FACING_TARGET: u32 = 0x1000_0000;
    /// GroundVehicle/Turret: `OBJECT_FLAG_POP_NEUTRALS`.
    pub const POP_NEUTRALS: u32 = 0x2000_0000;
}

/// `OBJECT_STATUS_*` (`mclib/objstatus.h`). Values appear in the digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum ObjectStatus {
    #[default]
    Normal = 0,
    Disabled,
    Destroyed,
    StartingUp,
    ShuttingDown,
    ShutDown,
}

/// `MIN_REINFORCEMENT_PART_ID` (code/unitdesg.h:62).
pub const MIN_REINFORCEMENT_PART_ID: i32 = 2050;
/// `MAX_REINFORCEMENT_PART_ID` (code/unitdesg.h:63).
pub const MAX_REINFORCEMENT_PART_ID: i32 = 3550;
/// `MIN_MOVER_PART_ID` (code/unitdesg.h:61).
pub const MIN_MOVER_PART_ID: i32 = 512;
/// `MAX_MOVERGROUPS`.
pub const MAX_MOVERGROUPS: i32 = 16;
/// `MAX_MOVERGROUP_COUNT_START`.
pub const MAX_MOVERGROUP_COUNT_START: i32 = 12;
/// `MIN_TERRAIN_PART_ID`.
pub const MIN_TERRAIN_PART_ID: i32 = 4096;
/// `MAX_MAP_CELL_WIDTH`.
pub const MAX_MAP_CELL_WIDTH: i32 = 720;

/// The fields every object has (`GameObject`, `code/gameobj.h:307-345`).
#[derive(Clone, Debug)]
pub struct Common {
    /// `objectClass`. NOTE: carnage instances keep `GameObject` as their class (see DESIGN.md Lane A).
    pub class: ObjectClass,
    /// `handle`: index in the store (1-based).
    pub handle: ObjectHandle,
    /// `partId`.
    pub part_id: PartId,
    /// `watchID`: assigned lazily on first [`ObjectStore::watch_id`] call (assignment order matters).
    pub watch_id: WatchId,
    /// `typeHandle`: object type number (index into `mc2_sim_data::objtype::ObjectTable`).
    pub type_handle: i32,
    /// `position`.
    pub position: Vec3,
    /// `cellPositionRow`, `cellPositionCol` (C `unsigned short`).
    pub cell: (u16, u16),
    /// `d_vertexNum`: terrain vertex the object is lower-right of.
    pub vertex_num: i32,
    /// `flags` (see [`flags`]).
    pub flags: u32,
    /// `debugFlags` (`OBJECT_DFLAG_DISABLE`).
    pub debug_flags: u16,
    /// `status`.
    pub status: ObjectStatus,
    /// `tonnage`.
    pub tonnage: f32,
    /// `rotation` (degrees).
    pub rotation: f32,
    /// `collisionFreeFromWID`.
    pub collision_free_from: WatchId,
    /// `collisionFreeTime`.
    pub collision_free_time: f32,
    /// `explRadius`.
    pub expl_radius: f32,
    /// `explDamage`.
    pub expl_damage: f32,
    /// `windowsVisible`: last turn the object was drawn (kept only for parity with the digest; UI only).
    pub windows_visible: i32,
    /// `maxCV`.
    pub max_cv: i16,
    /// `curCV`.
    pub cur_cv: i16,
    /// `threatRating`.
    pub threat_rating: i16,
    /// `numAttackers` (`GameObject::incrementAttackers` / `decrementAttackers`).
    pub num_attackers: u8,
}

impl Common {
    /// `GameObject::init(true)` (gameobj.cpp:1474) for an object of `class` stored at `handle`.
    pub fn new(class: ObjectClass, handle: ObjectHandle) -> Common {
        Common {
            class,
            handle,
            part_id: PartId(0),
            watch_id: WatchId(0),
            type_handle: 0,
            position: Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            cell: (0, 0),
            vertex_num: -1,
            flags: flags::USEME | flags::AWAKE,
            debug_flags: 0,
            status: ObjectStatus::Normal,
            tonnage: 0.0,
            rotation: 0.0,
            collision_free_from: WatchId(0),
            collision_free_time: 0.0,
            expl_radius: 0.0,
            expl_damage: 0.0,
            windows_visible: 0,
            max_cv: 0,
            cur_cv: 0,
            threat_rating: 0,
            num_attackers: 0,
        }
    }

    /// `getFlag`.
    pub fn flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    /// `setFlag(f, on)`.
    pub fn set_flag(&mut self, f: u32, on: bool) {
        if on {
            self.flags |= f;
        } else {
            self.flags &= !f;
        }
    }

    /// `getTangible`.
    pub fn tangible(&self) -> bool {
        self.flag(flags::TANGIBLE)
    }

    /// `setTangible`.
    pub fn set_tangible(&mut self, on: bool) {
        self.set_flag(flags::TANGIBLE, on);
    }

    /// `getAwake`.
    pub fn awake(&self) -> bool {
        self.flag(flags::AWAKE)
    }

    /// `setAwake`.
    pub fn set_awake(&mut self, on: bool) {
        self.set_flag(flags::AWAKE, on);
    }

    /// `GameObject::setStatus(newStatus, forceStatus)` (gameobj.h:586): a destroyed or disabled object keeps its status
    /// unless forced, but `DESTROYED` always wins.
    pub fn set_status(&mut self, new_status: ObjectStatus, force: bool) {
        if (self.status != ObjectStatus::Destroyed && self.status != ObjectStatus::Disabled)
            || force
        {
            self.status = new_status;
        }
        if new_status == ObjectStatus::Destroyed {
            self.status = new_status;
        }
    }

    /// `isDisabled`: disabled or destroyed.
    pub fn is_disabled(&self) -> bool {
        self.status == ObjectStatus::Disabled || self.status == ObjectStatus::Destroyed
    }

    /// `isDestroyed`.
    pub fn is_destroyed(&self) -> bool {
        self.status == ObjectStatus::Destroyed
    }

    /// `GameObject::incrementAttackers` (gameobj.h:1070).
    pub fn increment_attackers(&mut self) {
        self.num_attackers = self.num_attackers.saturating_add(1);
    }

    /// `GameObject::decrementAttackers` (gameobj.h:1074); avoid underflow on an invalid caller.
    pub fn decrement_attackers(&mut self) {
        self.num_attackers = self.num_attackers.saturating_sub(1);
    }
}

/// What the object is. Movers and weapon bolts carry state owned by other lanes.
#[derive(Clone, Debug)]
pub enum Kind {
    /// Lane B.
    Mech(Box<crate::mover::MechState>),
    /// Lane B.
    Vehicle(Box<crate::mover::VehicleState>),
    /// Trees and other `TerrainObject` instances (class `Tree`/`TerrainObject`).
    Terrain(structure::Structure),
    /// `Building`.
    Building(Box<structure::BuildingState>),
    /// `Turret` (fire control is lane C: [`crate::combat::turret`]).
    Turret(Box<structure::TurretState>),
    /// `Gate`.
    Gate(Box<structure::GateState>),
    /// `WeaponBolt` (lane C).
    WeaponBolt(Box<crate::combat::BoltState>),
    /// `Artillery` (lane C).
    Artillery(Box<crate::combat::ArtilleryState>),
    /// Explosion or fire pooled object.
    Carnage(Box<carnage::CarnageState>),
}

/// One game object.
#[derive(Clone, Debug)]
pub struct Obj {
    /// Shared fields.
    pub c: Common,
    /// Kind-specific state.
    pub kind: Kind,
}

impl Obj {
    /// `getExists()`.
    pub fn exists(&self) -> bool {
        self.c.flags & flags::EXISTS != 0
    }
    /// `setExists`.
    pub fn set_exists(&mut self, on: bool) {
        if on {
            self.c.flags |= flags::EXISTS;
        } else {
            self.c.flags &= !flags::EXISTS;
        }
    }
    /// `isMover()` - mech or ground vehicle.
    pub fn is_mover(&self) -> bool {
        matches!(self.kind, Kind::Mech(_) | Kind::Vehicle(_))
    }
    /// `getTeamId()` (virtual in the C++): the team index of movers, structures, and artillery; -1 for everything else.
    pub fn team_id(&self) -> i32 {
        match &self.kind {
            Kind::Mech(m) => i32::from(m.m.team.0),
            Kind::Vehicle(v) => i32::from(v.m.team.0),
            Kind::Building(b) => i32::from(b.team_id),
            Kind::Turret(t) => i32::from(t.team_id),
            Kind::Gate(g) => i32::from(g.team_id),
            // `Artillery::getTeamId` returns its recorded strike team
            // (artlry.h:304-307). Sensor probes own artillery, so the sensor
            // scan uses this virtual value to exclude friendly movers and to
            // classify an observed enemy for `enemyInLOS`.
            Kind::Artillery(a) => a.team,
            _ => -1,
        }
    }
    /// `getCommanderId()`: movers have one, buildings remember the last capturer, everything else -1.
    pub fn commander_id(&self) -> i32 {
        match &self.kind {
            Kind::Mech(m) => i32::from(m.m.commander.0),
            Kind::Vehicle(v) => i32::from(v.m.commander.0),
            Kind::Building(b) => i32::from(b.commander_id),
            _ => -1,
        }
    }
    /// `getMoveLevel()`: movers only (0 for everything else, `GameObject::getMoveLevel`).
    pub fn move_level(&self) -> i32 {
        match &self.kind {
            Kind::Mech(m) => i32::from(m.m.move_level),
            Kind::Vehicle(v) => i32::from(v.m.move_level),
            _ => 0,
        }
    }
    /// The terrain-object part of terrain objects, buildings, turrets and gates.
    pub fn structure(&self) -> Option<&structure::Structure> {
        match &self.kind {
            Kind::Terrain(s) => Some(s),
            Kind::Building(b) => Some(&b.s),
            Kind::Turret(t) => Some(&t.s),
            Kind::Gate(g) => Some(&g.s),
            _ => None,
        }
    }
    /// Mutable [`Obj::structure`].
    pub fn structure_mut(&mut self) -> Option<&mut structure::Structure> {
        match &mut self.kind {
            Kind::Terrain(s) => Some(s),
            Kind::Building(b) => Some(&mut b.s),
            Kind::Turret(t) => Some(&mut t.s),
            Kind::Gate(g) => Some(&mut g.s),
            _ => None,
        }
    }
}

/// Per-frame object update dispatch: `GameObject::update()` for every kind. Returns `false` when the object no longer
/// exists (the manager then calls `setExists(false)`). Called by [`ObjectStore::update`] in the original list order.
pub fn update_object(world: &mut crate::World, handle: ObjectHandle) -> bool {
    enum Which {
        Mech,
        Vehicle,
        Terrain,
        Building,
        Turret,
        Gate,
        Bolt,
        Artillery,
        Carnage,
    }
    let which = match world.objects.get(handle).map(|o| &o.kind) {
        Some(Kind::Mech(_)) => Which::Mech,
        Some(Kind::Vehicle(_)) => Which::Vehicle,
        Some(Kind::Terrain(_)) => Which::Terrain,
        Some(Kind::Building(_)) => Which::Building,
        Some(Kind::Turret(_)) => Which::Turret,
        Some(Kind::Gate(_)) => Which::Gate,
        Some(Kind::WeaponBolt(_)) => Which::Bolt,
        Some(Kind::Artillery(_)) => Which::Artillery,
        Some(Kind::Carnage(_)) => Which::Carnage,
        None => return true,
    };
    match which {
        Which::Mech => match crate::mover::update_mech(world, handle) {
            Ok(updated) => updated,
            Err(decision) => {
                let part_id = world.objects.get(handle).map_or(0, |obj| obj.c.part_id.0);
                world
                    .events
                    .push(crate::SimEvent::MovementUnavailable { part_id, decision });
                true
            }
        },
        Which::Vehicle => match crate::mover::update_vehicle(world, handle) {
            Ok(updated) => updated,
            Err(decision) => {
                let part_id = world.objects.get(handle).map_or(0, |obj| obj.c.part_id.0);
                world
                    .events
                    .push(crate::SimEvent::MovementUnavailable { part_id, decision });
                true
            }
        },
        Which::Terrain => structure::update_terrain_object(world, handle),
        Which::Building => structure::update_building(world, handle),
        Which::Turret => structure::update_turret(world, handle),
        Which::Gate => structure::update_gate(world, handle),
        Which::Bolt => crate::combat::update_bolt(world, handle),
        Which::Artillery => crate::combat::artillery::update(world, handle),
        Which::Carnage => carnage::update(world, handle),
    }
}

/// `ObjectManager::updateAppearancesOnly(true, true, true)` (objmgr.cpp): while paused only appearances are advanced (no AI,
/// movement, weapons).
pub fn update_appearances_only(world: &mut crate::World) {
    ObjectStore::update_appearances_only(world);
}

/// `GameObject::setPosition(newPosition, calcPositions)` (gameobj.cpp:1739): sets the position and, when asked, recomputes
/// `cell` and `vertex_num` from `worldToTileCell`. Movers go through `Mover::setPosition` (lane B), which ends in this.
pub fn set_position(world: &mut crate::World, handle: ObjectHandle, pos: Vec3, calc: bool) {
    let derived = if calc {
        let (tile_r, tile_c, cell_r, cell_c) = terr::world_to_tile_cell(&world.terrain, &pos);
        let side = terr::vertices_per_side(&world.terrain);
        Some((
            (cell_c + tile_c * 3) as u16,
            (cell_r + tile_r * 3) as u16,
            tile_r.wrapping_mul(side).wrapping_add(tile_c),
        ))
    } else {
        None
    };
    if let Some(o) = world.objects.get_mut(handle) {
        o.c.position = pos;
        if let Some((col, row, vertex)) = derived {
            o.c.cell = (row, col);
            o.c.vertex_num = vertex;
        }
    }
}

/// `GameObject::lineOfSight(target)` with the per-frame mover pair memo (`moverLineOfSightTable`, reset each frame).
///
/// gameobj.cpp:1846. The visual range lookup uses the altitude table (`visualRangeTable[256]`, loaded from gamesys.fit
/// into [`GameSystem`](crate::world::GameSystem)); the ray itself is lane D's [`crate::ai::sensors::line_of_sight_ray`].
pub fn line_of_sight(
    world: &mut crate::World,
    from: ObjectHandle,
    to: ObjectHandle,
    start_ext_rad: f32,
    check_visible_bits: bool,
) -> bool {
    let _ = check_visible_bits;
    hooks::line_of_sight(world, from, to, start_ext_rad)
}

/// Append the digest lines of lane A objects (`B`, `T`, `G`, `W` records; see oracle/DIGEST.md) after the mover lines.
pub fn digest(world: &crate::World, level: u32, out: &mut Vec<String>) {
    digest::object_lines(world, level, out);
}

// ==================================================================================================================
// Save/load (DESIGN.md section 9). Appended by the lane A save work; the classification of each field is in its impl.
// ==================================================================================================================

use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};

// `Vec3` (`mc2_path::Vec3`) has its Snapshot impl in save.rs.

impl Snapshot for ObjectStatus {
    fn save(&self, w: &mut Writer) {
        w.u8(match self {
            ObjectStatus::Normal => 0,
            ObjectStatus::Disabled => 1,
            ObjectStatus::Destroyed => 2,
            ObjectStatus::StartingUp => 3,
            ObjectStatus::ShuttingDown => 4,
            ObjectStatus::ShutDown => 5,
        });
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(match r.u8()? {
            0 => ObjectStatus::Normal,
            1 => ObjectStatus::Disabled,
            2 => ObjectStatus::Destroyed,
            3 => ObjectStatus::StartingUp,
            4 => ObjectStatus::ShuttingDown,
            5 => ObjectStatus::ShutDown,
            t => return Err(SaveError(format!("save: bad object status tag {t}"))),
        })
    }
}

/// Every [`ObjectClass`] value. `ObjectClass` is foreign (mc2-sim-data), so it is saved as its `i32` discriminant and decoded
/// by searching this list.
const ALL_OBJECT_CLASSES: [ObjectClass; 35] = [
    ObjectClass::Invalid,
    ObjectClass::BaseObject,
    ObjectClass::TerrN,
    ObjectClass::BattleMech,
    ObjectClass::GroundVehicle,
    ObjectClass::Elemental,
    ObjectClass::Explosion,
    ObjectClass::Fire,
    ObjectClass::Artillery,
    ObjectClass::Mover,
    ObjectClass::GameObject,
    ObjectClass::BigGameObject,
    ObjectClass::Component,
    ObjectClass::Weapon,
    ObjectClass::Projectile,
    ObjectClass::LaserWeapon,
    ObjectClass::Ppc,
    ObjectClass::Building,
    ObjectClass::Smoke,
    ObjectClass::Bullet,
    ObjectClass::Debris,
    ObjectClass::MapIcon,
    ObjectClass::Tree,
    ObjectClass::TerrainObject,
    ObjectClass::Mine,
    ObjectClass::Bridge,
    ObjectClass::Jet,
    ObjectClass::ProjLaser,
    ObjectClass::TreeBuilding,
    ObjectClass::CameraDrone,
    ObjectClass::TrainCar,
    ObjectClass::Turret,
    ObjectClass::Gate,
    ObjectClass::KliegLight,
    ObjectClass::WeaponBolt,
];

/// `ObjectClass` in a save: the `i32` discriminant. Free functions rather than a `Snapshot` impl because the type is
/// mc2-sim-data's (static game data).
fn save_object_class(c: ObjectClass, w: &mut Writer) {
    w.i32(c as i32);
}

/// Decodes [`save_object_class`]; an unknown discriminant is an error.
fn load_object_class(r: &mut Reader<'_>) -> SaveResult<ObjectClass> {
    let v = r.i32()?;
    ALL_OBJECT_CLASSES
        .iter()
        .copied()
        .find(|c| *c as i32 == v)
        .ok_or_else(|| SaveError(format!("save: bad object class {v}")))
}

/// `Common`: every field is dynamic (the class and handle are set at allocation and never change, but they are cheap and
/// make a mismatched load fail loudly). Nothing is static here.
impl Snapshot for Common {
    fn save(&self, w: &mut Writer) {
        save_object_class(self.class, w);
        self.handle.save(w);
        self.part_id.save(w);
        self.watch_id.save(w);
        self.type_handle.save(w);
        self.position.save(w);
        self.cell.save(w);
        self.vertex_num.save(w);
        self.flags.save(w);
        self.debug_flags.save(w);
        self.status.save(w);
        self.tonnage.save(w);
        self.rotation.save(w);
        self.collision_free_from.save(w);
        self.collision_free_time.save(w);
        self.expl_radius.save(w);
        self.expl_damage.save(w);
        self.windows_visible.save(w);
        self.max_cv.save(w);
        self.cur_cv.save(w);
        self.threat_rating.save(w);
        self.num_attackers.save(w);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(Common {
            class: load_object_class(r)?,
            handle: ObjectHandle::load(r)?,
            part_id: PartId::load(r)?,
            watch_id: WatchId::load(r)?,
            type_handle: i32::load(r)?,
            position: Vec3::load(r)?,
            cell: <(u16, u16)>::load(r)?,
            vertex_num: i32::load(r)?,
            flags: u32::load(r)?,
            debug_flags: u16::load(r)?,
            status: ObjectStatus::load(r)?,
            tonnage: f32::load(r)?,
            rotation: f32::load(r)?,
            collision_free_from: WatchId::load(r)?,
            collision_free_time: f32::load(r)?,
            expl_radius: f32::load(r)?,
            expl_damage: f32::load(r)?,
            windows_visible: i32::load(r)?,
            max_cv: i16::load(r)?,
            cur_cv: i16::load(r)?,
            threat_rating: i16::load(r)?,
            num_attackers: u8::load(r)?,
        })
    }
}

impl Kind {
    /// The variant tag written before the payload.
    fn save_tag(&self) -> u8 {
        match self {
            Kind::Mech(_) => 0,
            Kind::Vehicle(_) => 1,
            Kind::Terrain(_) => 2,
            Kind::Building(_) => 3,
            Kind::Turret(_) => 4,
            Kind::Gate(_) => 5,
            Kind::WeaponBolt(_) => 6,
            Kind::Artillery(_) => 7,
            Kind::Carnage(_) => 8,
        }
    }

    /// Decode a payload of the given tag (a fresh value).
    fn load_payload(tag: u8, r: &mut Reader<'_>) -> SaveResult<Kind> {
        Ok(match tag {
            0 => Kind::Mech(Box::<crate::mover::MechState>::load(r)?),
            1 => Kind::Vehicle(Box::<crate::mover::VehicleState>::load(r)?),
            2 => Kind::Terrain(structure::Structure::load(r)?),
            3 => Kind::Building(Box::<structure::BuildingState>::load(r)?),
            4 => Kind::Turret(Box::<structure::TurretState>::load(r)?),
            5 => Kind::Gate(Box::<structure::GateState>::load(r)?),
            6 => Kind::WeaponBolt(Box::<crate::combat::BoltState>::load(r)?),
            7 => Kind::Artillery(Box::<crate::combat::ArtilleryState>::load(r)?),
            8 => Kind::Carnage(Box::<carnage::CarnageState>::load(r)?),
            t => return Err(SaveError(format!("save: bad object kind tag {t}"))),
        })
    }

    /// Overlay the payload onto this variant. Each payload goes through its own `load_into`, so static fields the payload
    /// keeps from the base world survive (`Box<T>`'s default would replace the box and lose them).
    fn load_payload_into(&mut self, r: &mut Reader<'_>) -> SaveResult<()> {
        match self {
            Kind::Mech(m) => Snapshot::load_into(&mut **m, r),
            Kind::Vehicle(v) => Snapshot::load_into(&mut **v, r),
            Kind::Terrain(s) => Snapshot::load_into(s, r),
            Kind::Building(b) => Snapshot::load_into(&mut **b, r),
            Kind::Turret(t) => Snapshot::load_into(&mut **t, r),
            Kind::Gate(g) => Snapshot::load_into(&mut **g, r),
            Kind::WeaponBolt(b) => Snapshot::load_into(&mut **b, r),
            Kind::Artillery(a) => Snapshot::load_into(&mut **a, r),
            Kind::Carnage(c) => Snapshot::load_into(&mut **c, r),
        }
    }
}

/// `Kind`: a one-byte tag, then the payload. Unknown tags are errors.
impl Snapshot for Kind {
    fn save(&self, w: &mut Writer) {
        w.u8(self.save_tag());
        match self {
            Kind::Mech(m) => m.save(w),
            Kind::Vehicle(v) => v.save(w),
            Kind::Terrain(s) => s.save(w),
            Kind::Building(b) => b.save(w),
            Kind::Turret(t) => t.save(w),
            Kind::Gate(g) => g.save(w),
            Kind::WeaponBolt(b) => b.save(w),
            Kind::Artillery(a) => a.save(w),
            Kind::Carnage(c) => c.save(w),
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let tag = r.u8()?;
        Kind::load_payload(tag, r)
    }
    fn load_into(&mut self, r: &mut Reader<'_>) -> SaveResult<()> {
        let tag = r.u8()?;
        if tag == self.save_tag() {
            self.load_payload_into(r)
        } else {
            // Different variant than the base world: the base has nothing to keep.
            *self = Kind::load_payload(tag, r)?;
            Ok(())
        }
    }
}

/// `Obj` = `Common` + `Kind`. The common part has no static fields, so its `load_into` is the default (replace).
impl Snapshot for Obj {
    fn save(&self, w: &mut Writer) {
        self.c.save(w);
        self.kind.save(w);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let c = Common::load(r)?;
        let kind = Kind::load(r)?;
        Ok(Obj { c, kind })
    }
    fn load_into(&mut self, r: &mut Reader<'_>) -> SaveResult<()> {
        self.c.load_into(r)?;
        self.kind.load_into(r)
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;
    use crate::rng::RngMode;
    use crate::save::{Reader, Writer};
    use crate::World;

    fn bytes_of(store: &ObjectStore) -> Vec<u8> {
        let mut w = Writer::new();
        store.save(&mut w);
        w.into_bytes()
    }

    /// Store with two mechs and one vehicle, one of the mechs placed and given a watch id.
    fn populated(rng: &crate::rng::SharedSimRng) -> ObjectStore {
        let mut s = ObjectStore::default();
        s.set_num_objects(rng, 2, 1);
        let m = s.new_mover(ObjectClass::BattleMech).unwrap();
        let _v = s.new_mover(ObjectClass::GroundVehicle).unwrap();
        s.get_mut(m).unwrap().c.position = Vec3 {
            x: 12.5,
            y: -3.0,
            z: 0.25,
        };
        s.get_mut(m).unwrap().c.flags |= flags::TANGIBLE;
        s.watch_id(m);
        s
    }

    #[test]
    fn object_store_round_trips_byte_exact() {
        let w = World::new(RngMode::LegacyMsvc, 1);
        let rng = w.rng.clone();
        let saved = populated(&rng);
        let first = bytes_of(&saved);

        // The overlay target is a freshly initialised base of the same layout.
        let mut base = ObjectStore::default();
        base.set_num_objects(&rng, 2, 1);
        base.load_into(&mut Reader::new(&first)).unwrap();
        let second = bytes_of(&base);
        assert_eq!(first, second);

        // A fresh load (no base) gives the same bytes too.
        let fresh = ObjectStore::load(&mut Reader::new(&first)).unwrap();
        assert_eq!(bytes_of(&fresh), first);
    }
}
