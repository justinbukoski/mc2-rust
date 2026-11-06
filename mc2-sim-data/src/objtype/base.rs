// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The `[ObjectType]` block shared by every typed object (objtype.cpp:129-171).

use mc2_formats::ini::FitIni;

use crate::error::Result;

/// `TEAM` value every object type starts with: neutral (objtype.cpp:168).
pub const TEAM_NEUTRAL: i32 = -1;

/// Fields of the C++ `ObjectType` base class (objtype.h:37-53).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectTypeBase {
    /// `AppearanceName` (objtype.cpp:143). Base name of the appearance files.
    /// `None` when the key is missing (appearName stays NULL, objtype.cpp:144).
    pub appearance_name: Option<String>,
    /// `ExplosionObject` (objtype.cpp:150). Required. Default -1 from `init()`.
    pub explosion_object: i32,
    /// `DestroyedObject` (objtype.cpp:154). Required. Default -1 from `init()`.
    pub destroyed_object: i32,
    /// `ExtentRadius` (objtype.cpp:158). Missing gives 0.0. Subclasses can overwrite it
    /// after this init (building.rs, terrain.rs).
    pub extent_radius: f32,
    /// `IconNumber` (objtype.cpp:163). Missing gives -1 (no icon).
    pub icon_number: i32,
    /// `keepMe` (objtype.cpp:167). Always true after a successful init ("never cache
    /// out"). The C++ `init()` default is false.
    pub keep_me: bool,
    /// `teamId` (objtype.cpp:168). Always -1 after init, whatever a subclass read
    /// into it first.
    pub team_id: i32,
    /// `potentialContact` (objtype.h:47). Never set by any loader.
    pub potential_contact: bool,
}

impl ObjectTypeBase {
    /// State left by the C++ `ObjectType::init(void)` (objtype.h:60-75), for types
    /// whose `[ObjectType]` block is not read.
    ///
    /// C++ leaves `teamId` unset here. -1 is used, matching the neutral team every
    /// other path sets.
    pub fn ctor_defaults() -> Self {
        Self {
            appearance_name: None,
            explosion_object: -1,
            destroyed_object: -1,
            extent_radius: 0.0,
            icon_number: -1,
            keep_me: false,
            team_id: TEAM_NEUTRAL,
            potential_contact: false,
        }
    }

    /// Reads `[ObjectType]` (`ObjectType::init(FitIniFilePtr)`, objtype.cpp:129-171).
    /// Moves the file's current block to `ObjectType`.
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("ObjectType")?;
        let appearance_name = fit.read_string("AppearanceName", 512).ok();
        let explosion_object = fit.read_i32("ExplosionObject")?;
        let destroyed_object = fit.read_i32("DestroyedObject")?;
        let extent_radius = fit.read_f32("ExtentRadius").unwrap_or(0.0);
        let icon_number = fit.read_i32("IconNumber").unwrap_or(-1);
        Ok(Self {
            appearance_name,
            explosion_object,
            destroyed_object,
            extent_radius,
            icon_number,
            keep_me: true,
            team_id: TEAM_NEUTRAL,
            potential_contact: false,
        })
    }
}
