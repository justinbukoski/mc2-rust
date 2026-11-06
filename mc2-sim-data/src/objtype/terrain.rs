// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `TerrainObjectType` (terrobj.cpp:97-232) and the five MC1 misc-terrain hack
//! handles (terrobj.cpp:124-160, objtype.cpp:257-268).
//!
//! Packet class TREE (0) also loads as this type. Its packets have `[TreeData]` only,
//! and the type then sets sub type TREE and object class TREE (terrobj.cpp:177-183).

use mc2_formats::ini::FitIni;

use crate::error::Result;

use super::base::ObjectTypeBase;
use super::{MiscTerrain, ObjectType, ObjectTypeClass, ObjectTypeData};

/// `TERROBJ_*` sub types (terrobj.h:41-47).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TerrainSubType {
    /// `TERROBJ_NONE`.
    None = 0,
    /// `TERROBJ_TREE`.
    Tree = 1,
    /// `TERROBJ_BRIDGE`. Misc-terrain handle only.
    Bridge = 2,
    /// `TERROBJ_FOREST`. Misc-terrain handle only.
    Forest = 3,
    /// `TERROBJ_WALL_HEAVY`. Misc-terrain handle only.
    WallHeavy = 4,
    /// `TERROBJ_WALL_MEDIUM`. Misc-terrain handle only.
    WallMedium = 5,
    /// `TERROBJ_WALL_LIGHT`. Misc-terrain handle only.
    WallLight = 6,
}

/// Class-level fields of a terrain object or tree type.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainObjectType {
    /// `subType` (terrobj.cpp:182).
    pub sub_type: TerrainSubType,
    /// True when the packet has `[TreeData]` and no `[TerrainObjectData]`
    /// (terrobj.cpp:177-183).
    pub from_tree_data: bool,
    /// True for the five hack handles, which have no packet (objtype.cpp:257-268).
    pub misc_terrain: bool,
    /// `DmgLevel` (terrobj.cpp:187). Required, stored as float (terrobj.cpp:189).
    /// Misc-terrain handles use 100.0 (terrobj.cpp:124-150).
    pub damage_level: f32,
    /// `CollisionOffsetX` (terrobj.cpp:192). Default 0.
    pub collision_offset_x: i32,
    /// `CollisionOffsetY` (terrobj.cpp:196). Default 0.
    pub collision_offset_y: i32,
    /// `SetImpassable` (terrobj.cpp:201). True when the value is non-zero. Default false.
    pub set_impassable: bool,
    /// `XImpasse` (terrobj.cpp:206). Default 0.
    pub x_impasse: i32,
    /// `YImpasse` (terrobj.cpp:210). Default 0.
    pub y_impasse: i32,
    /// `ExplosionRadius` (terrobj.cpp:219). Default 0.0.
    pub explosion_radius: f32,
    /// `ExplosionDamage` (terrobj.cpp:225). Default 0.0.
    pub explosion_damage: f32,
    /// `fireTypeHandle` (terrobj.cpp:112). -1, except 1 for forest (terrobj.cpp:145).
    pub fire_type_handle: i32,
}

impl TerrainObjectType {
    /// Reads `[TerrainObjectData]`, or `[TreeData]` as a fallback. Returns the base
    /// with the extent override applied (terrobj.cpp:215, 233-234).
    pub(crate) fn read(fit: &mut FitIni) -> Result<(Self, ObjectTypeBase)> {
        let from_tree_data = fit.seek_block("TerrainObjectData").is_err();
        if from_tree_data {
            fit.seek_block("TreeData")?;
        }
        let sub_type = if from_tree_data {
            TerrainSubType::Tree
        } else {
            TerrainSubType::None
        };

        let damage_level = fit.read_u32("DmgLevel")? as f32;
        let collision_offset_x = fit.read_i32("CollisionOffsetX").unwrap_or(0);
        let collision_offset_y = fit.read_i32("CollisionOffsetY").unwrap_or(0);
        let set_impassable = fit
            .read_i32("SetImpassable")
            .map(|v| v != 0)
            .unwrap_or(false);
        let x_impasse = fit.read_i32("XImpasse").unwrap_or(0);
        let y_impasse = fit.read_i32("YImpasse").unwrap_or(0);
        let real_extent = fit.read_f32("ExtentRadius").unwrap_or(-1.0);
        let explosion_radius = fit.read_f32("ExplosionRadius").unwrap_or(0.0);
        let explosion_damage = fit.read_f32("ExplosionDamage").unwrap_or(0.0);

        let mut base = ObjectTypeBase::read(fit)?;
        base.extent_radius = real_extent;

        Ok((
            Self {
                sub_type,
                from_tree_data,
                misc_terrain: false,
                damage_level,
                collision_offset_x,
                collision_offset_y,
                set_impassable,
                x_impasse,
                y_impasse,
                explosion_radius,
                explosion_damage,
                fire_type_handle: -1,
            },
            base,
        ))
    }
}

/// Builds one of the five misc-terrain hack types (objtype.cpp:333-342 selects it,
/// terrobj.cpp:124-160 fills it). No packet is read, and no raw blocks exist.
pub(crate) fn misc_terrain_type(index: u32, kind: MiscTerrain) -> ObjectType {
    let (sub_type, fire_type_handle) = match kind {
        MiscTerrain::Bridge => (TerrainSubType::Bridge, -1),
        MiscTerrain::Forest => (TerrainSubType::Forest, 1),
        MiscTerrain::WallHeavy => (TerrainSubType::WallHeavy, -1),
        MiscTerrain::WallMedium => (TerrainSubType::WallMedium, -1),
        MiscTerrain::WallLight => (TerrainSubType::WallLight, -1),
    };
    // explosionObject and destroyedObject are 0xFFFFFFFF, which is -1 as a long
    // (terrobj.cpp:124-135). extentRadius -1.0 (terrobj.cpp:131), keepMe, icon -1,
    // team -1 (terrobj.cpp:124-150).
    let base = ObjectTypeBase {
        appearance_name: None,
        explosion_object: -1,
        destroyed_object: -1,
        extent_radius: -1.0,
        icon_number: -1,
        keep_me: true,
        team_id: super::base::TEAM_NEUTRAL,
        potential_contact: false,
    };
    ObjectType {
        index,
        packet_class: None,
        object_type_class: ObjectTypeClass::TerrainObject,
        base,
        data: ObjectTypeData::TerrainObject(TerrainObjectType {
            sub_type,
            from_tree_data: false,
            misc_terrain: true,
            damage_level: 100.0,
            collision_offset_x: 0,
            collision_offset_y: 0,
            set_impassable: false,
            x_impasse: 0,
            y_impasse: 0,
            explosion_radius: 0.0,
            explosion_damage: 0.0,
            fire_type_handle,
        }),
        raw_blocks: Vec::new(),
    }
}
