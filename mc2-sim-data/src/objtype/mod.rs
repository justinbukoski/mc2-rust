// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Object types: the typed loaders for the object packets in `data/objects/object2.pak`.
//!
//! Port of `code/objtype.{h,cpp}` (`ObjectType`, `ObjectTypeManager::load`) and
//! `code/dobjtype.h` (`ObjectTypeClass`). The packet index is the object type
//! number (`Part.ObjectNumber` in missions). A non-empty packet is FIT text. Its
//! `[ObjectClass] ObjectTypeNum` picks the class, as `ObjectTypeManager::load`
//! does (objtype.cpp:343-367). Typed classes read their own block and the shared
//! `[ObjectType]` block. Classes that `load` never constructs keep their raw
//! block list instead of being dropped.
//!
//! Per-class modules:
//! - [`base`]: `[ObjectType]` block (objtype.cpp:129-171).
//! - [`mech`]: `BattleMechType` from `[MechProfile]`.
//! - [`vehicle`]: `GroundVehicleType` from `[General]` and `[InternalStructure]`.
//! - [`building`]: `BuildingType` (`[TreeData]` or `[BuildingData]`).
//! - [`terrain`]: `TerrainObjectType` (`[TerrainObjectData]` or `[TreeData]`) and the
//!   five MC1 misc-terrain hack handles.
//! - [`structures`]: `TurretType`, `GateType`, `ArtilleryType`.
//! - [`effects`]: `ExplosionType`, `FireType`, `LightType`.
//! - [`weaponbolt`]: `WeaponBoltType` (bolt and beam).
//! - [`raw`]: block list parser and the untyped-class fallback.
//!
//! Provenance and the list of quirks are in `rust/docs/porting-log.md`.

pub mod base;
pub mod building;
pub mod effects;
pub mod mech;
pub mod raw;
pub mod structures;
pub mod terrain;
pub mod vehicle;
pub mod weaponbolt;

use std::collections::BTreeMap;
use std::path::Path;

use mc2_formats::ini::FitIni;
use mc2_formats::pak::PakFile;

use crate::error::{Result, SimDataError};

pub use base::ObjectTypeBase;
pub use building::BuildingType;
pub use effects::{ExplosionType, FireType, LightType};
pub use mech::BattleMechType;
pub use raw::RawBlock;
pub use structures::{ArtilleryType, GateType, TurretType};
pub use terrain::{TerrainObjectType, TerrainSubType};
pub use vehicle::GroundVehicleType;
pub use weaponbolt::WeaponBoltType;

/// Number of object type slots (`ObjectTypeManager::init`, `maxObjectTypes = 1024`,
/// objtype.h:223). The game passes no override (objmgr.cpp:285). Indices at or above
/// this value make `ObjectTypeManager::load` call `Fatal` (objtype.cpp:321-322).
pub const MAX_OBJECT_TYPES: u32 = 1024;

/// Name of the object packet archive under the data root.
pub const OBJECT_PAK_PATH: &str = "data/objects/object2.pak";

/// Object type class, `enum ObjectTypeClass` (dobjtype.h:31-59).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ObjectTypeClass {
    /// `CRAPPY_OBJECT`. `load` calls `Fatal` on it (objtype.cpp:374-378).
    Crappy = -1,
    /// `TREE_TYPE`. Loads as [`TerrainObjectType`].
    Tree = 0,
    /// `BUILDING_TYPE`. Loads as [`BuildingType`].
    Building = 1,
    /// `BATTLEMECH_TYPE`. Loads as [`BattleMechType`].
    BattleMech = 2,
    /// `VEHICLE_TYPE`. Loads as [`GroundVehicleType`].
    Vehicle = 3,
    /// `EXPLOSION_TYPE`. Loads as [`ExplosionType`].
    Explosion = 4,
    /// `FIRE_TYPE`. Loads as [`FireType`].
    Fire = 5,
    /// `LASER_TYPE`. Never constructed by `load`; kept raw.
    Laser = 6,
    /// `SMOKES_TYPE`. Never constructed by `load`; kept raw.
    Smokes = 7,
    /// `PROJECTILE_TYPE`. Never constructed by `load`; kept raw.
    Projectile = 8,
    /// `MECHARM_TYPE`. Never constructed by `load`; kept raw.
    MechArm = 9,
    /// `MAPICON_TYPE`. Never constructed by `load`; kept raw.
    MapIcon = 10,
    /// `TERRAINOBJECT_TYPE`. Loads as [`TerrainObjectType`].
    TerrainObject = 11,
    /// `ARTILLERY_TYPE`. Loads as [`ArtilleryType`].
    Artillery = 12,
    /// `MINE_TYPE`. Never constructed by `load`; kept raw.
    Mine = 13,
    /// `ELEMENTAL_TYPE`. Never constructed by `load`; kept raw.
    Elemental = 14,
    /// `BRIDGE_TYPE`. Never constructed by `load`; kept raw.
    Bridge = 15,
    /// `JET_TYPE`. Never constructed by `load`; kept raw.
    Jet = 16,
    /// `PROJLASER_TYPE`. Never constructed by `load`; kept raw.
    ProjLaser = 17,
    /// `TREEBUILDING_TYPE`. Loads as [`BuildingType`] (`TreeData` block).
    TreeBuilding = 18,
    /// `CAMERADRONE_TYPE`. Never constructed by `load`; kept raw.
    CameraDrone = 19,
    /// `TRAINCAR_TYPE`. Never constructed by `load`; kept raw.
    TrainCar = 20,
    /// `TURRET_TYPE`. Loads as [`TurretType`].
    Turret = 21,
    /// `GATE_TYPE`. Loads as [`GateType`].
    Gate = 22,
    /// `KLIEG_LIGHT_TYPE`. `load` has no case for it (objtype.cpp:467-468). Typed
    /// here as [`LightType`] from its `[LightData]` block.
    KliegLight = 23,
    /// `WEAPONBOLT_TYPE`. Loads as [`WeaponBoltType`].
    WeaponBolt = 24,
}

impl ObjectTypeClass {
    /// Maps an `ObjectTypeNum` value to its class. Returns `None` for codes the
    /// enum does not name.
    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            -1 => Self::Crappy,
            0 => Self::Tree,
            1 => Self::Building,
            2 => Self::BattleMech,
            3 => Self::Vehicle,
            4 => Self::Explosion,
            5 => Self::Fire,
            6 => Self::Laser,
            7 => Self::Smokes,
            8 => Self::Projectile,
            9 => Self::MechArm,
            10 => Self::MapIcon,
            11 => Self::TerrainObject,
            12 => Self::Artillery,
            13 => Self::Mine,
            14 => Self::Elemental,
            15 => Self::Bridge,
            16 => Self::Jet,
            17 => Self::ProjLaser,
            18 => Self::TreeBuilding,
            19 => Self::CameraDrone,
            20 => Self::TrainCar,
            21 => Self::Turret,
            22 => Self::Gate,
            23 => Self::KliegLight,
            24 => Self::WeaponBolt,
            _ => return None,
        })
    }

    /// The numeric code (`ObjectTypeNum`).
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// Runtime object class, `enum ObjectClass` (mclib/dobjclass.h). Set by each type's
/// init to say what instances of the type are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ObjectClass {
    Invalid = -1,
    BaseObject = 0,
    TerrN = 1,
    BattleMech = 2,
    GroundVehicle = 3,
    Elemental = 4,
    Explosion = 5,
    Fire = 6,
    Artillery = 7,
    Mover = 8,
    GameObject = 9,
    BigGameObject = 10,
    Component = 11,
    Weapon = 12,
    Projectile = 13,
    LaserWeapon = 14,
    Ppc = 15,
    Building = 16,
    Smoke = 17,
    Bullet = 18,
    Debris = 19,
    MapIcon = 20,
    Tree = 21,
    TerrainObject = 22,
    Mine = 23,
    Bridge = 24,
    Jet = 25,
    ProjLaser = 26,
    TreeBuilding = 27,
    CameraDrone = 28,
    TrainCar = 29,
    Turret = 30,
    Gate = 31,
    KliegLight = 32,
    WeaponBolt = 33,
}

/// Type-specific data of an object type.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectTypeData {
    BattleMech(BattleMechType),
    GroundVehicle(GroundVehicleType),
    Building(BuildingType),
    TerrainObject(TerrainObjectType),
    Turret(TurretType),
    Gate(GateType),
    Artillery(ArtilleryType),
    Explosion(ExplosionType),
    Fire(FireType),
    Light(LightType),
    WeaponBolt(WeaponBoltType),
    /// A class with no typed loader. The block list is in [`ObjectType::raw_blocks`].
    Raw,
}

impl ObjectTypeData {
    /// Whether `ObjectTypeManager::load` constructs this class. The C++ switch
    /// (objtype.cpp:370-467) has cases for mech, vehicle, building, tree, tree building,
    /// terrain object, weapon bolt, turret, explosion, fire, gate and artillery. Light
    /// (class 23) and the raw classes fall through to `return(NULL)` (objtype.cpp:467-468),
    /// so they are typed or kept raw here but never reachable from the game.
    pub fn is_dispatched(&self) -> bool {
        !matches!(self, ObjectTypeData::Raw | ObjectTypeData::Light(_))
    }
}

/// One object type. Fields follow the C++ `ObjectType` (objtype.h:37-53) and the
/// typed subclass.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectType {
    /// Packet index, the object type number (`objTypeNum`).
    pub index: u32,
    /// `ObjectTypeNum` from the packet's `[ObjectClass]` block. `None` for the
    /// misc-terrain hack handles, which have no packet.
    pub packet_class: Option<ObjectTypeClass>,
    /// `objectTypeClass` after the type's init. Some types set it to a value other
    /// than their packet class (see each module).
    pub object_type_class: ObjectTypeClass,
    /// The `[ObjectType]` block fields and the C++ `ObjectType` defaults.
    pub base: ObjectTypeBase,
    /// The typed data for the class.
    pub data: ObjectTypeData,
    /// Every block of the packet, as raw lines. Kept for all loaded packets so later
    /// lanes (dynamics, inventory) can read blocks this crate does not type.
    pub raw_blocks: Vec<RawBlock>,
}

impl ObjectType {
    /// Whether the game can reach this index at all. `ObjectTypeManager::load`
    /// rejects indices at or above [`MAX_OBJECT_TYPES`] (objtype.cpp:321-322).
    pub fn game_loadable(&self) -> bool {
        self.index < MAX_OBJECT_TYPES && self.data.is_dispatched()
    }
}

/// A packet that failed to load, with its index.
#[derive(Debug)]
pub struct PacketError {
    /// Packet index.
    pub index: u32,
    /// The failure.
    pub error: SimDataError,
}

/// All loaded object types, keyed by packet index (= object type number).
#[derive(Debug, Default)]
pub struct ObjectTable {
    /// Loaded types, in index order. Empty packets, index 0, and packets whose class has no
    /// C++ loader (`load()` returns NULL) are absent.
    pub types: BTreeMap<u32, ObjectType>,
    /// Packets that failed to load. The C++ loads types on demand, so one bad packet
    /// does not stop the others. Packet 55 is the one expected failure in the shipped file.
    pub failures: Vec<PacketError>,
}

impl ObjectTable {
    /// The type at `index`, if loaded.
    pub fn get(&self, index: u32) -> Option<&ObjectType> {
        self.types.get(&index)
    }

    /// Number of loaded types.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// True when no type is loaded.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Count of loaded types per packet class (`None` for the misc-terrain handles).
    pub fn class_counts(&self) -> BTreeMap<Option<i32>, usize> {
        let mut counts = BTreeMap::new();
        for t in self.types.values() {
            *counts
                .entry(t.packet_class.map(ObjectTypeClass::code))
                .or_insert(0) += 1;
        }
        counts
    }
}

/// Loads one object type from its packet.
///
/// Returns `Ok(None)` for index 0 (never loaded, objtype.cpp:324-325) and for an
/// empty packet. The five misc-terrain indices return the hack type without reading
/// the packet (objtype.cpp:333-342). Other packets are parsed as FIT text, then
/// dispatched on `[ObjectClass] ObjectTypeNum`. CRAPPY (-1) is an error (the C++ `Fatal`s,
/// objtype.cpp:374-378). An unknown code returns `Ok(None)`, as the C++ default case returns
/// NULL (objtype.cpp:467-468).
pub fn load_object_type(index: u32, packet: &[u8]) -> Result<Option<ObjectType>> {
    if index == 0 {
        return Ok(None);
    }
    if let Some(kind) = MiscTerrain::for_index(index) {
        return Ok(Some(terrain::misc_terrain_type(index, kind)));
    }
    if packet.is_empty() {
        return Ok(None);
    }

    let mut fit = FitIni::from_bytes(packet.to_vec())?;
    fit.seek_block("ObjectClass")?;
    let code = i64::from(fit.read_i32("ObjectTypeNum")?);
    // An unknown code is not an error: `ObjectTypeManager::load`'s default case returns
    // NULL (objtype.cpp:467-468), so the type is simply not created.
    let Some(class) = ObjectTypeClass::from_code(code) else {
        return Ok(None);
    };
    if class == ObjectTypeClass::Crappy {
        return Err(SimDataError::out_of_range(
            "object2.pak",
            "ObjectTypeNum -1 (CRAPPY_OBJECT) cannot be loaded (objtype.cpp:374-378)",
        ));
    }

    let raw_blocks = raw::parse_blocks(packet);
    let (object_type_class, base, data) = match class {
        ObjectTypeClass::BattleMech => {
            let data = mech::BattleMechType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            (
                ObjectTypeClass::BattleMech,
                base,
                ObjectTypeData::BattleMech(data),
            )
        }
        ObjectTypeClass::Vehicle => {
            let data = vehicle::GroundVehicleType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            (
                ObjectTypeClass::Vehicle,
                base,
                ObjectTypeData::GroundVehicle(data),
            )
        }
        ObjectTypeClass::Building | ObjectTypeClass::TreeBuilding => {
            let (data, base) = building::BuildingType::read(index, &mut fit)?;
            (
                ObjectTypeClass::Building,
                base,
                ObjectTypeData::Building(data),
            )
        }
        ObjectTypeClass::Tree | ObjectTypeClass::TerrainObject => {
            let (data, base) = terrain::TerrainObjectType::read(&mut fit)?;
            (
                ObjectTypeClass::TerrainObject,
                base,
                ObjectTypeData::TerrainObject(data),
            )
        }
        ObjectTypeClass::Turret => {
            let data = structures::TurretType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            (ObjectTypeClass::Turret, base, ObjectTypeData::Turret(data))
        }
        ObjectTypeClass::Gate => {
            let data = structures::GateType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            // GateType never sets objectTypeClass, so it keeps -1 (objtype.h:62).
            (ObjectTypeClass::Crappy, base, ObjectTypeData::Gate(data))
        }
        ObjectTypeClass::Artillery => {
            let data = structures::ArtilleryType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            (
                ObjectTypeClass::Artillery,
                base,
                ObjectTypeData::Artillery(data),
            )
        }
        ObjectTypeClass::Explosion => {
            let data = effects::ExplosionType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            (
                ObjectTypeClass::Explosion,
                base,
                ObjectTypeData::Explosion(data),
            )
        }
        ObjectTypeClass::Fire => {
            let data = effects::FireType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            (ObjectTypeClass::Fire, base, ObjectTypeData::Fire(data))
        }
        ObjectTypeClass::KliegLight => {
            let data = effects::LightType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            // LightType never sets objectTypeClass (light.cpp has no assignment), so it
            // keeps the ObjectType::init value -1 (objtype.h:62).
            (ObjectTypeClass::Crappy, base, ObjectTypeData::Light(data))
        }
        ObjectTypeClass::WeaponBolt => {
            let data = weaponbolt::WeaponBoltType::read(&mut fit)?;
            let base = ObjectTypeBase::read(&mut fit)?;
            (
                ObjectTypeClass::WeaponBolt,
                base,
                ObjectTypeData::WeaponBolt(data),
            )
        }
        _ => (class, ObjectTypeBase::ctor_defaults(), ObjectTypeData::Raw),
    };

    Ok(Some(ObjectType {
        index,
        packet_class: Some(class),
        object_type_class,
        base,
        data,
        raw_blocks,
    }))
}

/// Reads every packet of `object2.pak` at `path`. A packet that fails is recorded in
/// [`ObjectTable::failures`] and the rest still load, as the C++ loads types on demand.
/// The `Err` case is only an archive that cannot be opened.
pub fn load_object_table(path: impl AsRef<Path>) -> Result<ObjectTable> {
    let pak = PakFile::open(path)?;
    load_object_table_from_pak(&pak)
}

/// [`load_object_table`] over a packet file already in memory (for example read through the VFS).
pub fn load_object_table_from_pak(pak: &PakFile) -> Result<ObjectTable> {
    let mut table = ObjectTable::default();
    for i in 0..pak.packet_count() {
        let index = u32::try_from(i).map_err(|_| {
            SimDataError::out_of_range("object2.pak", format!("packet index {i} too large"))
        })?;
        let packet = match pak.read_packet(i) {
            Ok(bytes) => bytes,
            Err(e) => {
                table.failures.push(PacketError {
                    index,
                    error: e.into(),
                });
                continue;
            }
        };
        match load_object_type(index, &packet) {
            Ok(Some(obj)) => {
                table.types.insert(index, obj);
            }
            Ok(None) => {}
            Err(error) => table.failures.push(PacketError { index, error }),
        }
    }
    Ok(table)
}

/// The five MC1 misc-terrain "hack" handles (objtype.cpp:257-268). They occupy the
/// last five slots of the table, so they shadow packets at those indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiscTerrain {
    Bridge,
    Forest,
    WallHeavy,
    WallMedium,
    WallLight,
}

impl MiscTerrain {
    /// The index of this handle. Uses [`MAX_OBJECT_TYPES`] as the table size.
    pub fn index(self) -> u32 {
        let base = MAX_OBJECT_TYPES - 5;
        base + match self {
            MiscTerrain::Bridge => 0,
            MiscTerrain::Forest => 1,
            MiscTerrain::WallHeavy => 2,
            MiscTerrain::WallMedium => 3,
            MiscTerrain::WallLight => 4,
        }
    }

    /// The handle for `index`, if it is one of the five.
    pub fn for_index(index: u32) -> Option<Self> {
        [
            MiscTerrain::Bridge,
            MiscTerrain::Forest,
            MiscTerrain::WallHeavy,
            MiscTerrain::WallMedium,
            MiscTerrain::WallLight,
        ]
        .into_iter()
        .find(|k| k.index() == index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packet with the FIT header, a class block and the given body.
    fn packet(body: &str) -> Vec<u8> {
        format!("FITini\n{body}").into_bytes()
    }

    /// A class block, the class body, and the `[ObjectType]` block every typed class
    /// needs. Each block ends with a comment line. A key on the last line of the
    /// final block of a file cannot be matched, since that block ends at the file end
    /// (inifile.cpp:1002-1004, with the size set in seekBlock at inifile.cpp:839-846). Real packets
    /// never end on a key line, so the helper keeps the test packets the same way.
    fn typed(code: i32, body: &str) -> Vec<u8> {
        packet(&format!(
            "[ObjectClass]\nl ObjectTypeNum = {code}\n{body}\n// end\n[ObjectType]\nl IconNumber = 7\nl ExplosionObject = 4\nl DestroyedObject = 5\n// end\n"
        ))
    }

    fn load(index: u32, body: &[u8]) -> ObjectType {
        load_object_type(index, body)
            .expect("load ok")
            .expect("packet not empty")
    }

    #[test]
    fn class_codes_round_trip() {
        for code in -1..=24 {
            let class = ObjectTypeClass::from_code(code).expect("named class");
            assert_eq!(i64::from(class.code()), code);
        }
        assert!(ObjectTypeClass::from_code(25).is_none());
        assert!(ObjectTypeClass::from_code(-2).is_none());
    }

    #[test]
    fn index_zero_and_empty_packets_are_none() {
        assert!(load_object_type(0, &packet("")).unwrap().is_none());
        assert!(load_object_type(40, &[]).unwrap().is_none());
    }

    #[test]
    fn crappy_is_an_error_and_unknown_classes_are_not_created() {
        let crappy = typed(-1, "");
        assert!(load_object_type(40, &crappy).is_err());
        // An unknown code is NULL in the C++ default case (objtype.cpp:467-468), not an error.
        let unknown = typed(99, "");
        assert!(load_object_type(40, &unknown).unwrap().is_none());
    }

    #[test]
    fn base_block_defaults_and_team() {
        let mut fit = mc2_formats::ini::FitIni::from_bytes(packet(
            "[ObjectType]\nl ExplosionObject = 9\nl DestroyedObject = -1\nf ExtentRadius = 2.5\n// end\n",
        ))
        .expect("fit");
        let base = ObjectTypeBase::read(&mut fit).expect("base");
        assert_eq!(base.explosion_object, 9);
        assert_eq!(base.destroyed_object, -1);
        assert_eq!(base.icon_number, -1);
        assert_eq!(base.extent_radius, 2.5);
        assert!(base.keep_me);
        assert_eq!(base.team_id, -1);
        assert!(base.appearance_name.is_none());
    }

    #[test]
    fn raw_class_has_no_base_fields() {
        let obj = load(
            40,
            &packet(
                "[ObjectClass]\nl ObjectTypeNum = 6\n[ObjectType]\nl ExplosionObject = 9\n// end\n",
            ),
        );
        assert_eq!(obj.data, ObjectTypeData::Raw);
        assert_eq!(obj.base.explosion_object, -1);
        assert!(!obj.base.keep_me);
    }

    #[test]
    fn missing_explosion_object_is_an_error() {
        let p = packet("[ObjectClass]\nl ObjectTypeNum = 4\n[ExplosionData]\nul DmgLevel = 1\nul SoundEffectId = 2\n[ObjectType]\nl DestroyedObject = 5\n");
        assert!(load_object_type(40, &p).is_err());
    }

    #[test]
    fn building_reads_block_and_extent_override() {
        let body = "[BuildingData]\nul DmgLevel = 500\nb CanRefit = TRUE\nb MechBay = TRUE\nf ExtentRadius = 12.5\nl TeamID = 3\nul ActivityEffectID = 77\n";
        let obj = load(40, &typed(1, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::Building);
        assert_eq!(obj.packet_class, Some(ObjectTypeClass::Building));
        assert_eq!(obj.base.icon_number, 7);
        assert_eq!(
            obj.base.team_id, -1,
            "TeamID is overwritten by ObjectType::init"
        );
        assert_eq!(obj.base.extent_radius, 12.5);
        let ObjectTypeData::Building(b) = &obj.data else {
            panic!("expected building");
        };
        assert!(!b.from_tree_data);
        assert_eq!(b.damage_level, 500.0);
        assert!(b.can_refit && b.mech_bay);
        assert_eq!(b.declared_team_id, 3);
        assert_eq!(b.activity_effect_id, 77);
        assert_eq!(b.building_name_id, None);
        assert_eq!(b.sub_type, building::BuildingSubType::None);
    }

    #[test]
    fn building_perimeter_alarm_overrides_extent_and_wall_subtype() {
        let body = "[BuildingData]\nul DmgLevel = 1\nf ExtentRadius = 12.5\nf PerimeterAlarmRange = 30.0\n";
        let obj = load(26, &typed(1, body));
        assert_eq!(obj.base.extent_radius, 30.0);
        let ObjectTypeData::Building(b) = &obj.data else {
            panic!("expected building");
        };
        assert_eq!(b.sub_type, building::BuildingSubType::Wall);
        let bridge = load(607, &typed(1, body));
        let ObjectTypeData::Building(bb) = &bridge.data else {
            panic!("expected building");
        };
        assert_eq!(bb.sub_type, building::BuildingSubType::LandBridge);
    }

    #[test]
    fn tree_building_uses_tree_block() {
        let body = "[TreeData]\nul DmgLevel = 2\nf AttackRadius = 1\n";
        let obj = load(41, &typed(18, body));
        assert_eq!(obj.packet_class, Some(ObjectTypeClass::TreeBuilding));
        assert_eq!(obj.object_type_class, ObjectTypeClass::Building);
        let ObjectTypeData::Building(b) = &obj.data else {
            panic!("expected building");
        };
        assert!(b.from_tree_data);
        assert_eq!(b.damage_level, 2.0);
    }

    #[test]
    fn tree_loads_as_terrain_object_from_tree_data() {
        let body = "[TreeData]\nul DmgLevel = 0\nl SetImpassable = 1\nl XImpasse = 2\nf ExtentRadius = 4.0\n";
        let obj = load(41, &typed(0, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::TerrainObject);
        assert_eq!(obj.packet_class, Some(ObjectTypeClass::Tree));
        assert_eq!(obj.base.extent_radius, 4.0);
        let ObjectTypeData::TerrainObject(t) = &obj.data else {
            panic!("expected terrain");
        };
        assert_eq!(t.sub_type, TerrainSubType::Tree);
        assert!(t.from_tree_data);
        assert!(t.set_impassable);
        assert_eq!(t.x_impasse, 2);
        assert_eq!(t.fire_type_handle, -1);
    }

    #[test]
    fn terrain_object_data_block_has_no_tree_subtype() {
        let body = "[TerrainObjectData]\nul DmgLevel = 3\nf ExtentRadius = -1.0\n";
        let obj = load(42, &typed(11, body));
        let ObjectTypeData::TerrainObject(t) = &obj.data else {
            panic!("expected terrain");
        };
        assert_eq!(t.sub_type, TerrainSubType::None);
        assert!(!t.from_tree_data);
        assert_eq!(t.damage_level, 3.0);
        assert_eq!(obj.base.extent_radius, -1.0);
    }

    #[test]
    fn mech_profile_fields_and_defaults() {
        let body = "[MechProfile]\nst ProfileName = \"PUMA\"\nf LOSFactor = 1.5\n";
        let obj = load(40, &typed(2, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::BattleMech);
        let ObjectTypeData::BattleMech(m) = &obj.data else {
            panic!("expected mech");
        };
        assert_eq!(m.profile_name, "PUMA");
        assert_eq!(m.move_type, 0);
        assert_eq!(m.los_factor, 1.5);
        assert_eq!(m.destruct_damage, 20.0);
        assert_eq!(m.explosion_damage, 10.0);
        assert_eq!(m.explosion_radius, 25.0);
    }

    #[test]
    fn mech_without_profile_is_an_error() {
        let p = typed(2, "[Header]\nst FileType = \"MechType\"\n");
        assert!(load_object_type(55, &p).is_err());
    }

    #[test]
    fn vehicle_general_and_internal_structure() {
        let body = "[General]\nl MoveType = 1\nuc Chassis = 9\nb PathLocks = FALSE\n[InternalStructure]\nuc Front = 10\nuc Left = 11\nuc Right = 12\nuc Rear = 13\nuc Turret = 14\n[VehicleDynamics]\nl maxTurretYawRate = 90\nl maxTurretYaw = 120\nl maxVehicleYawRate = 300\nf maxAccel = 0\nf maxVelocity = 10\n";
        let obj = load(43, &typed(3, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::Vehicle);
        let ObjectTypeData::GroundVehicle(v) = &obj.data else {
            panic!("expected vehicle");
        };
        assert_eq!(v.move_type, 1);
        assert_eq!(v.chassis, 9);
        assert!(!v.path_locks);
        assert!(v.is_sensor_contact);
        assert_eq!(v.max_internal_structure, [10, 11, 12, 13, 14]);
        // [VehicleDynamics] is typed: the yaw rate is raised to 720, the pivot defaults to
        // 720 * 0.25, and accel is speed * 5 (mover.cpp:1631-1667).
        assert_eq!(v.dynamics.turret_yaw_rate, 90);
        assert_eq!(v.dynamics.turret_yaw, 120);
        assert_eq!(v.dynamics.yaw_rate, 720);
        assert_eq!(v.dynamics.pivot_rate, 180);
        assert_eq!(v.dynamics.speed, 10.0);
        assert_eq!(v.dynamics.accel, 50.0);
        assert!(obj.raw_blocks.iter().any(|b| b.id == "InternalStructure"));
    }

    #[test]
    fn vehicle_without_maxaccel_is_an_error() {
        let body = "[General]\nuc Chassis = 9\n[InternalStructure]\nuc Front = 1\nuc Left = 1\nuc Right = 1\nuc Rear = 1\nuc Turret = 1\n[VehicleDynamics]\nl maxTurretYawRate = 90\nl maxTurretYaw = 120\nl maxVehicleYawRate = 300\nf maxVelocity = 10\n";
        let p = typed(3, body);
        assert!(load_object_type(43, &p).is_err());
    }

    #[test]
    fn turret_weapons_and_effect_defaults() {
        let body = "[TurretData]\nul DmgLevel = 100\nf AttackRadius = 300.0\nf MaxTurretYawRate = 1.5\nl WeaponType = 3\nl WeaponType2 = 5\nl PilotSkill = 2\n";
        let obj = load(44, &typed(21, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::Turret);
        let ObjectTypeData::Turret(t) = &obj.data else {
            panic!("expected turret");
        };
        assert_eq!(t.weapon_master_id, [3, -1, 5, -1]);
        assert_eq!(
            t.blown_effect_id, 0,
            "missing key reads as 0 (inifile.cpp:1236)"
        );
        assert_eq!(t.little_extent, 20.0);
        assert_eq!(t.pilot_skill, 2);
    }

    #[test]
    fn gate_requires_open_radius() {
        let ok = typed(
            22,
            "[GateData]\nul DmgLevel = 1\nf OpenRadius = 25.0\nb BlocksLineOfFire = TRUE\n",
        );
        let obj = load(45, &ok);
        let ObjectTypeData::Gate(g) = &obj.data else {
            panic!("expected gate");
        };
        assert_eq!(g.open_radius, 25.0);
        assert!(g.blocks_line_of_fire);
        assert_eq!(obj.object_type_class, ObjectTypeClass::Crappy);
        let missing = typed(22, "[GateData]\nul DmgLevel = 1\n");
        assert!(load_object_type(45, &missing).is_err());
    }

    #[test]
    fn artillery_without_explosions_and_default_launch_time() {
        let body = "[Artillery]\nst ArtillerySpriteName = \"arty\"\nul FrameCount = 10\nul StartFrame = 0\nf FrameRate = 15\nf NominalTimeToImpact = 30\nf NominalDamage = 0\nf NominalMajorRange = 1\nf NominalMajorHits = 2\nf NominalMinorRange = 3\nf NominalMinorHits = 4\nf NominalSensorTime = 5\nf NominalSensorRange = 6\nf fontScale = 1\nf fontXOffset = 0\nf fontYOffset = 0\nul fontColor = 0xFFFFFF\n";
        let obj = load(46, &typed(12, body));
        let ObjectTypeData::Artillery(a) = &obj.data else {
            panic!("expected artillery");
        };
        assert_eq!(a.sprite_name, "arty");
        assert_eq!(a.nominal_time_to_launch, 20.0);
        assert_eq!(a.font_color, 0xFF_FFFF);
        assert!(a.explosions.is_none());
    }

    #[test]
    fn artillery_with_explosions() {
        let body = "[Artillery]\nst ArtillerySpriteName = \"arty\"\nul FrameCount = 1\nul StartFrame = 0\nf FrameRate = 1\nf NominalTimeToImpact = 30\nf NominalTimeToLaunch = 12\nf NominalDamage = 9\nf NominalMajorRange = 1\nf NominalMajorHits = 1\nf NominalMinorRange = 1\nf NominalMinorHits = 1\nf NominalSensorTime = 1\nf NominalSensorRange = 1\nf fontScale = 1\nf fontXOffset = 0\nf fontYOffset = 0\nul fontColor = 1\nl NumExplosions = 1\nf ExplosionDelay0 = 0.5\nf ExplosionOffsetX0 = 1.0\nf ExplosionOffsetY0 = 2.0\nl ExplosionsPerExplosion = 3\nl ExplosionRandomOffsetX = 4\nl ExplosionRandomOffsetY = 5\n";
        let obj = load(46, &typed(12, body));
        let ObjectTypeData::Artillery(a) = &obj.data else {
            panic!("expected artillery");
        };
        assert_eq!(a.nominal_time_to_launch, 12.0);
        let ex = a.explosions.as_ref().expect("explosions read");
        assert_eq!(ex.explosions.len(), 1);
        assert_eq!(ex.explosions[0].offset_y, 2.0);
        assert_eq!(ex.min_artillery_head_range, 5);
        assert_eq!(ex.per_explosion, 3);
    }

    #[test]
    fn explosion_with_light_block() {
        let body = "[ExplosionData]\nul DmgLevel = 20\nul SoundEffectId = 8\nf DelayUntilCollidable = 1.5\n[LightData]\nf LightMinMaxRadius = 1\nf LightMaxMaxRadius = 2\nf LightOutMinRadius = 3\nf LightOutMaxRadius = 4\nul LightRGB = 0x00102030\nf MaxIntensity = 5\nf MinIntensity = 6\nf Duration = 7\n";
        let obj = load(47, &typed(4, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::Explosion);
        let ObjectTypeData::Explosion(e) = &obj.data else {
            panic!("expected explosion");
        };
        assert_eq!(e.damage_level, 20.0);
        assert_eq!(e.chunk_size, 5.0);
        assert_eq!(e.delay_until_collidable, 1.5);
        assert_eq!(e.light_object_id, 0xFFFF_FFFF);
        let light = e.light.as_ref().expect("light");
        assert_eq!(light.light_rgb, 0x0010_2030);
        assert_eq!(light.duration, 7.0);
    }

    #[test]
    fn fire_type_loop_fields() {
        let body = "[FireData]\nul DmgLevel = 3\nul SoundEffectId = 1\nul startLoopFrame = 2\nul numLoops = 4\nul endLoopFrame = 8\n";
        let obj = load(48, &typed(5, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::Fire);
        let ObjectTypeData::Fire(f) = &obj.data else {
            panic!("expected fire");
        };
        assert_eq!(f.num_loops, 4);
        assert_eq!(f.total_fires, 1);
        assert_eq!(f.light_object_id, 0xFFFF_FFFF);
    }

    #[test]
    fn kliegs_light_block_is_typed() {
        let body = "[LightData]\nb OneShotFlag = TRUE\nf AltitudeOffset = 2.5\n";
        let obj = load(49, &typed(23, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::Crappy);
        let ObjectTypeData::Light(l) = &obj.data else {
            panic!("expected light");
        };
        assert!(l.one_shot_flag);
        assert_eq!(l.altitude_offset, 2.5);
        // Class 23 has a typed block but no case in ObjectTypeManager::load (objtype.cpp:467-468).
        assert!(!obj.game_loadable());
    }

    #[test]
    fn unknown_class_code_is_not_created() {
        // The C++ default case returns NULL, so an unknown code is not an error.
        // The key is not the last line of the file: a key on a block's last line is not found.
        let p =
            packet("[ObjectClass]\nl ObjectTypeNum = 99\n[ObjectType]\nl ExplosionObject = 1\n");
        assert!(load_object_type(50, &p).unwrap().is_none());
    }

    #[test]
    fn stuff_small_is_the_style_cpp_value() {
        // Stuff::SMALL=1e-4f (mclib/stuff/style.cpp:21), the UnitLength floor (weaponbolt.cpp:140-141).
        assert_eq!(weaponbolt::STUFF_SMALL, 1.0e-4_f32);
    }

    #[test]
    fn bolt_projectile_swaps_bgr_colours() {
        let body = "[BoltProjectileData]\nf ProjectileLength = 2\nf BulgeLength = 3\nf BulgeWidth = 4\nf Velocity = 5\nl HitEffect = 6\nl MissEffect = 7\nl FiringSoundFX = 8\nul FrontRGB = 0x000000FF\nul MidEdgeRGB = 0x00FF0000\nul MiddleRGB = 0x0000FF00\nul BackRGB = 0x00123456\nuc EdgeAlpha = 200\nuc BoltAlpha = 100\nb PointLight = TRUE\nul LightRGB = 0x000000FF\nf MaxRadius = 1\nf OutRadius = 2\nf Intensity = 3\n";
        let obj = load(50, &typed(24, body));
        assert_eq!(obj.object_type_class, ObjectTypeClass::WeaponBolt);
        let ObjectTypeData::WeaponBolt(WeaponBoltType::Bolt(b)) = &obj.data else {
            panic!("expected bolt");
        };
        assert_eq!(b.front_rgb, 0x00FF_0000);
        assert_eq!(b.mid_edge_rgb, 0x0000_00FF);
        assert_eq!(b.middle_rgb, 0x0000_FF00);
        assert_eq!(b.back_rgb, 0x0056_3412);
        assert_eq!(b.hit_effect, 6);
        assert_eq!(b.arc_height, -1.0);
        assert_eq!(b.light.as_ref().map(|l| l.light_rgb), Some(0x00FF_0000));
    }

    #[test]
    fn bolt_missing_required_key_is_an_error() {
        let body = "[BoltProjectileData]\nf ProjectileLength = 2\n";
        assert!(load_object_type(50, &typed(24, body)).is_err());
    }

    #[test]
    fn beam_projectile_is_the_fallback_form() {
        let body = "[BeamProjectileData]\nb IsBeam = TRUE\nf UnitLength = 0\nst TextureName = \"beam.tga\"\nb PointLight = FALSE\n";
        let obj = load(51, &typed(24, body));
        let ObjectTypeData::WeaponBolt(WeaponBoltType::Beam(b)) = &obj.data else {
            panic!("expected beam");
        };
        assert!(b.is_beam);
        assert_eq!(b.unit_length, 1.0);
        assert_eq!(b.texture_name, "beam.tga");
        assert_eq!(b.mid_edge_rgb, 0x00FF_FFFF);
        assert!(b.light.is_none());
    }

    #[test]
    fn unlisted_class_keeps_raw_blocks() {
        let p = packet("[ObjectClass]\nl ObjectTypeNum = 13\n[MineData]\nul DmgLevel = 1\nst Name = \"mine\"\n// note\n[ObjectType]\nl ExplosionObject = 1\n");
        let obj = load(52, &p);
        assert_eq!(obj.data, ObjectTypeData::Raw);
        assert_eq!(obj.packet_class, Some(ObjectTypeClass::Mine));
        assert_eq!(obj.object_type_class, ObjectTypeClass::Mine);
        let mine = raw::find_block(&obj.raw_blocks, "MineData").expect("block kept");
        assert_eq!(mine.lines.len(), 3);
        assert_eq!(mine.lines[0], "ul DmgLevel = 1");
    }

    #[test]
    fn misc_terrain_handles_do_not_read_packets() {
        let obj = load_object_type(MAX_OBJECT_TYPES - 2, &[])
            .unwrap()
            .expect("misc handle");
        assert_eq!(obj.index, 1022);
        assert!(obj.packet_class.is_none());
        assert_eq!(obj.object_type_class, ObjectTypeClass::TerrainObject);
        let ObjectTypeData::TerrainObject(t) = &obj.data else {
            panic!("expected terrain");
        };
        assert!(t.misc_terrain);
        assert_eq!(t.sub_type, TerrainSubType::WallMedium);
        assert_eq!(t.damage_level, 100.0);
        assert_eq!(obj.base.explosion_object, -1);
        assert_eq!(obj.base.extent_radius, -1.0);
        assert!(obj.game_loadable());
        let forest = load_object_type(MiscTerrain::Forest.index(), &[])
            .unwrap()
            .unwrap();
        let ObjectTypeData::TerrainObject(f) = &forest.data else {
            panic!("expected terrain");
        };
        assert_eq!(f.fire_type_handle, 1);
    }

    #[test]
    fn packets_past_the_table_are_flagged_unreachable() {
        let obj = load(1030, &typed(11, "[TerrainObjectData]\nul DmgLevel = 1\n"));
        assert!(!obj.game_loadable());
    }
}
