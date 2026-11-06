// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Sound, music, Betty and support-officer identifiers.
//!
//! Port of `mclib/sounds.h`. The numbers are packet indices into `sound.pak` (effects), entries of the
//! `[DigitalMusic]` table in `sound.snd` (music), packet indices into `betty.pak` and `support.pak`.
//! Several C++ macros share a value (`GATE_CLOSE` = `GATE_OPEN`, `LOG_SELECT` = `LOG_VIDEOBUTTONS`,
//! `LOG_UNREADY` = `BUTTON7`); they are kept as in the header.

/// Sound effect ids (`SoundBite` index).
pub mod sfx {
    /// Should never be used
    pub const BOMB_EXPLODE: u32 = 0;
    /// ER Laser Fire
    pub const ER_LASER_FIRE: u32 = 1;
    /// Mech Explosion
    pub const BIGDEATH: u32 = 2;
    /// All other Laser Fire/Hit
    pub const LASERHITS: u32 = 3;
    /// Mech Collisions
    pub const MECH_COLLIDE: u32 = 4;
    /// LRM Hit
    pub const LRM_HIT: u32 = 5;
    /// Flamer Fire
    pub const FLAMER_FIRE: u32 = 6;
    /// AutoCannon Fire
    pub const AC2_FIRE: u32 = 7;
    /// Lrm Fire
    pub const LRM_FIRE: u32 = 8;
    /// SRM Hit
    pub const SRM_HIT: u32 = 9;
    /// PPC Fire
    pub const PPC_FIRE: u32 = 10;
    /// SRM Fire
    pub const SRM_FIRE: u32 = 11;
    /// Gauss Rifle Fire
    pub const GAUSS_FIRE: u32 = 12;
    /// Mech Footstep
    pub const FOOTFALL_SMALL: u32 = 13;
    /// TreeFall
    pub const TREEFALL: u32 = 14;
    /// GUI SFX
    pub const BUTTON5: u32 = 15;
    /// GUI SFX
    pub const BUTTON7: u32 = 16;
    /// GUI SFX
    pub const BUTTON6: u32 = 17;
    /// GUI SFX
    pub const BUZZER1: u32 = 18;
    /// Mech Critical Hit
    pub const CRITICAL_HIT_SFX: u32 = 19;
    /// Sensor Contact
    pub const PING_SFX: u32 = 20;
    /// Machine Gun Fire
    pub const MGUN_FIRE: u32 = 21;
    /// Burning Fire
    pub const OBJECT_FIRE: u32 = 22;
    /// ER Laser Hit
    pub const ER_LASER_HIT: u32 = 23;
    /// Gauss Rifle Hit
    pub const GAUSS_HIT: u32 = 24;
    /// AutoCannon Hit
    pub const AC2_HIT: u32 = 25;
    /// Machine Gun Hit
    pub const MGUN_HIT: u32 = 26;
    /// Eject
    pub const EJECT_SFX: u32 = 27;
    /// Treefall
    pub const TREEFALL1: u32 = 28;
    /// Mech Fall Sound
    pub const MECH_FALL_SOUND: u32 = 29;
    /// Elemental Shredding a Mech Sound
    pub const ELEMENTAL_HIT: u32 = 30;
    /// Perimeter alarm general alarm
    pub const GENERAL_ALARM_SFX: u32 = 34;
    pub const BUILDING_EXPLODE: u32 = 35;
    pub const ELECTRIC_HUM: u32 = 36;
    pub const GENERATOR_HUM: u32 = 37;
    pub const RADAR_HUM: u32 = 38;
    pub const HEAVY_ROTATION: u32 = 39;
    pub const ORBITAL_GUN: u32 = 40;
    pub const DEFENSE_SILO: u32 = 41;
    pub const GATE_OPEN: u32 = 42;
    pub const TRAIN_XING: u32 = 43;
    pub const HPG_ANTENNA: u32 = 44;
    pub const GATE_CLOSE: u32 = 42;
    pub const INVALID_GUI: u32 = 46;
    pub const ZOOM_GUI: u32 = 47;
    pub const FORCE_GUI: u32 = 48;
    pub const SENSOR_GUI: u32 = 49;
    pub const MAPPLACE_GUI: u32 = 50;
    pub const MAPBUTTONS_GUI: u32 = 51;
    pub const CLICKOFF_GUI: u32 = 52;
    pub const CLICKON_GUI: u32 = 53;
    pub const FOLDER_GUI: u32 = 54;
    pub const OUTSIDESELECT_GUI: u32 = 55;
    pub const PAGEGUI: u32 = 56;
    pub const SELECT_GUI: u32 = 57;
    pub const STARTMISSION_GUI: u32 = 58;
    pub const TAB_GUI: u32 = 59;
    pub const POWERDOWN_SFX: u32 = 60;
    pub const POWERUP_SFX: u32 = 61;
    pub const INCOMING_ARTILLIRY: u32 = 62;
    pub const INCOMING_AIRSTRIKE: u32 = 63;
    pub const WINDOW_OPEN: u32 = 64;
    pub const WINDOW_CLOSE: u32 = 65;
    pub const START_GROUPSELECT: u32 = 66;
    pub const END_GROUPSELECT: u32 = 67;
    pub const ZOOMIN_GUI: u32 = 68;
    pub const ZOOMOUT_GUI: u32 = 69;
    pub const WRONG_GUI: u32 = 70;
    pub const FRP_BUTTON_GUI: u32 = 71;
    pub const BREAKINGFENCE: u32 = 72;
    pub const BUILDING_COLLAPSE: u32 = 73;
    pub const VTOL_ANIMATE: u32 = 74;
    pub const VTOL_DROP: u32 = 75;
    pub const JUMPJETS: u32 = 76;
    pub const THUNDER1: u32 = 77;
    pub const THUNDER2: u32 = 78;
    pub const THUNDER3: u32 = 79;
    pub const THUNDER4: u32 = 80;
    pub const RAIN1: u32 = 81;
    pub const RAIN2: u32 = 82;
    pub const RAIN3: u32 = 83;
    pub const RAIN4: u32 = 84;
    pub const SENSOR1: u32 = 85;
    pub const SENSOR2: u32 = 86;
    pub const SENSOR3: u32 = 87;
    pub const SENSOR4: u32 = 88;
    pub const LOG_WRONGBUTTON: u32 = 89;
    pub const LOG_VIDEOBUTTONS: u32 = 90;
    pub const LOG_STARTMISSION: u32 = 91;
    /// changing to be the same as video buttons
    pub const LOG_SELECT: u32 = 90;
    pub const LOG_NEXTBACKBUTTONS: u32 = 93;
    pub const LOG_MISSIONSELECT: u32 = 94;
    pub const LOG_MAINMENUBUTTON: u32 = 95;
    pub const LOG_HIGHLIGHTBUTTONS: u32 = 96;
    pub const LOG_EXITGAME: u32 = 97;
    pub const LOG_CLICKONBUTTON: u32 = 98;
    pub const MINE_EXPLOSION: u32 = 99;
    pub const LOG_DIGITALHIGHLIGHT: u32 = 96;
    pub const REPAIRBAY_FX: u32 = 101;
    pub const LOG_PILOTMEDAL: u32 = 102;
    pub const LOG_KILLMARKER: u32 = 103;
    pub const LOG_PROMOTED: u32 = 104;
    pub const MECH_EXPLODE: u32 = 105;
    pub const COPTER_POWERUP: u32 = 106;
    pub const COPTER_POWERDN: u32 = 107;
    pub const VEHICLE_POWERUP: u32 = 108;
    pub const VEHICLE_POWERDN: u32 = 109;
    pub const LOAD_DOORS_CLOSING: u32 = 110;
    pub const LOAD_DOORS_OPENING: u32 = 111;
    pub const LOG_TEAM_CHAT: u32 = 112;
    pub const LOG_CHAT: u32 = 113;
    pub const HELICOPTER_DEATH: u32 = 114;
    pub const LOG_UNREADY: u32 = 16;
    pub const SALVAGE_CRAFT: u32 = 116;

    /// Every id of this group with its C++ macro name, in header order (counts excluded).
    pub const ALL: &[(&str, u32)] = &[
        ("BOMB_EXPLODE", 0),
        ("ER_LASER_FIRE", 1),
        ("BIGDEATH", 2),
        ("LASERHITS", 3),
        ("MECH_COLLIDE", 4),
        ("LRM_HIT", 5),
        ("FLAMER_FIRE", 6),
        ("AC2_FIRE", 7),
        ("LRM_FIRE", 8),
        ("SRM_HIT", 9),
        ("PPC_FIRE", 10),
        ("SRM_FIRE", 11),
        ("GAUSS_FIRE", 12),
        ("FOOTFALL_SMALL", 13),
        ("TREEFALL", 14),
        ("BUTTON5", 15),
        ("BUTTON7", 16),
        ("BUTTON6", 17),
        ("BUZZER1", 18),
        ("CRITICAL_HIT_SFX", 19),
        ("PING_SFX", 20),
        ("MGUN_FIRE", 21),
        ("OBJECT_FIRE", 22),
        ("ER_LASER_HIT", 23),
        ("GAUSS_HIT", 24),
        ("AC2_HIT", 25),
        ("MGUN_HIT", 26),
        ("EJECT_SFX", 27),
        ("TREEFALL1", 28),
        ("MECH_FALL_SOUND", 29),
        ("ELEMENTAL_HIT", 30),
        ("GENERAL_ALARM_SFX", 34),
        ("BUILDING_EXPLODE", 35),
        ("ELECTRIC_HUM", 36),
        ("GENERATOR_HUM", 37),
        ("RADAR_HUM", 38),
        ("HEAVY_ROTATION", 39),
        ("ORBITAL_GUN", 40),
        ("DEFENSE_SILO", 41),
        ("GATE_OPEN", 42),
        ("TRAIN_XING", 43),
        ("HPG_ANTENNA", 44),
        ("GATE_CLOSE", 42),
        ("INVALID_GUI", 46),
        ("ZOOM_GUI", 47),
        ("FORCE_GUI", 48),
        ("SENSOR_GUI", 49),
        ("MAPPLACE_GUI", 50),
        ("MAPBUTTONS_GUI", 51),
        ("CLICKOFF_GUI", 52),
        ("CLICKON_GUI", 53),
        ("FOLDER_GUI", 54),
        ("OUTSIDESELECT_GUI", 55),
        ("PAGEGUI", 56),
        ("SELECT_GUI", 57),
        ("STARTMISSION_GUI", 58),
        ("TAB_GUI", 59),
        ("POWERDOWN_SFX", 60),
        ("POWERUP_SFX", 61),
        ("INCOMING_ARTILLIRY", 62),
        ("INCOMING_AIRSTRIKE", 63),
        ("WINDOW_OPEN", 64),
        ("WINDOW_CLOSE", 65),
        ("START_GROUPSELECT", 66),
        ("END_GROUPSELECT", 67),
        ("ZOOMIN_GUI", 68),
        ("ZOOMOUT_GUI", 69),
        ("WRONG_GUI", 70),
        ("FRP_BUTTON_GUI", 71),
        ("BREAKINGFENCE", 72),
        ("BUILDING_COLLAPSE", 73),
        ("VTOL_ANIMATE", 74),
        ("VTOL_DROP", 75),
        ("JUMPJETS", 76),
        ("THUNDER1", 77),
        ("THUNDER2", 78),
        ("THUNDER3", 79),
        ("THUNDER4", 80),
        ("RAIN1", 81),
        ("RAIN2", 82),
        ("RAIN3", 83),
        ("RAIN4", 84),
        ("SENSOR1", 85),
        ("SENSOR2", 86),
        ("SENSOR3", 87),
        ("SENSOR4", 88),
        ("LOG_WRONGBUTTON", 89),
        ("LOG_VIDEOBUTTONS", 90),
        ("LOG_STARTMISSION", 91),
        ("LOG_SELECT", 90),
        ("LOG_NEXTBACKBUTTONS", 93),
        ("LOG_MISSIONSELECT", 94),
        ("LOG_MAINMENUBUTTON", 95),
        ("LOG_HIGHLIGHTBUTTONS", 96),
        ("LOG_EXITGAME", 97),
        ("LOG_CLICKONBUTTON", 98),
        ("MINE_EXPLOSION", 99),
        ("LOG_DIGITALHIGHLIGHT", 96),
        ("REPAIRBAY_FX", 101),
        ("LOG_PILOTMEDAL", 102),
        ("LOG_KILLMARKER", 103),
        ("LOG_PROMOTED", 104),
        ("MECH_EXPLODE", 105),
        ("COPTER_POWERUP", 106),
        ("COPTER_POWERDN", 107),
        ("VEHICLE_POWERUP", 108),
        ("VEHICLE_POWERDN", 109),
        ("LOAD_DOORS_CLOSING", 110),
        ("LOAD_DOORS_OPENING", 111),
        ("LOG_TEAM_CHAT", 112),
        ("LOG_CHAT", 113),
        ("HELICOPTER_DEATH", 114),
        ("LOG_UNREADY", 16),
        ("SALVAGE_CRAFT", 116),
    ];
}

/// Music ids (`DMS` index in the `[DigitalMusic]` block).
pub mod music {
    /// Mission Start Tune
    pub const MISSION_START_0: u32 = 0;
    /// Mission Start Tune
    pub const MISSION_START_1: u32 = 1;
    /// Mission Start Tune
    pub const MISSION_START_2: u32 = 2;
    /// Mission Start Tune
    pub const MISSION_START_3: u32 = 3;
    /// Mission Start Tune
    pub const MISSION_START_4: u32 = 4;
    pub const NUM_MISSION_TUNES: u32 = 5;
    pub const CONTACT_TUNE_0: u32 = 5;
    pub const CONTACT_TUNE_1: u32 = 6;
    pub const CONTACT_TUNE_2: u32 = 7;
    pub const NUM_CONTACT_TUNES: u32 = 3;
    pub const WIN_TUNE_0: u32 = 8;
    pub const LOSE_TUNE_0: u32 = 9;
    pub const OBJECTIVE_SUCCESS: u32 = 10;
    pub const OBJECTIVE_FAILURE: u32 = 11;
    pub const ENEMY_DONE: u32 = 12;
    pub const FRIENDLY_DONE: u32 = 13;
    /// Combat Tune
    pub const COMBAT_TUNE_0: u32 = 14;
    /// Combat Tune
    pub const COMBAT_TUNE_1: u32 = 15;
    /// Combat Tune
    pub const COMBAT_TUNE_2: u32 = 16;
    /// Combat Tune
    pub const COMBAT_TUNE_3: u32 = 17;
    /// Combat Tune
    pub const COMBAT_TUNE_4: u32 = 18;
    /// Combat Tune
    pub const COMBAT_TUNE_5: u32 = 19;
    pub const NUM_COMBAT_TUNES: u32 = 6;
    /// Played when no other music is playing
    pub const GAME_FINALE: u32 = 20;
    /// Played when no other music is playing
    pub const BACKGROUND_AMBIENT: u32 = 21;
    pub const LOGISTICS_LOOP: u32 = 22;
    pub const CREDITS_TUNE: u32 = 23;
    pub const RESULTS_LOOP: u32 = 28;

    /// Every id of this group with its C++ macro name, in header order (counts excluded).
    pub const ALL: &[(&str, u32)] = &[
        ("MISSION_START_0", 0),
        ("MISSION_START_1", 1),
        ("MISSION_START_2", 2),
        ("MISSION_START_3", 3),
        ("MISSION_START_4", 4),
        ("CONTACT_TUNE_0", 5),
        ("CONTACT_TUNE_1", 6),
        ("CONTACT_TUNE_2", 7),
        ("WIN_TUNE_0", 8),
        ("LOSE_TUNE_0", 9),
        ("OBJECTIVE_SUCCESS", 10),
        ("OBJECTIVE_FAILURE", 11),
        ("ENEMY_DONE", 12),
        ("FRIENDLY_DONE", 13),
        ("COMBAT_TUNE_0", 14),
        ("COMBAT_TUNE_1", 15),
        ("COMBAT_TUNE_2", 16),
        ("COMBAT_TUNE_3", 17),
        ("COMBAT_TUNE_4", 18),
        ("COMBAT_TUNE_5", 19),
        ("GAME_FINALE", 20),
        ("BACKGROUND_AMBIENT", 21),
        ("LOGISTICS_LOOP", 22),
        ("CREDITS_TUNE", 23),
        ("RESULTS_LOOP", 28),
    ];
}

/// Betty voice sample ids (`betty.pak` packet).
pub mod betty {
    pub const BETTY_DROP_WEIGHT_EXCEEDED: u32 = 0;
    pub const BETTY_AIR_SUPERIORITY: u32 = 1;
    pub const BETTY_COMPONENTS_CAPTURED: u32 = 2;
    pub const BETTY_CANNOT_COMP_OBJ: u32 = 3;
    pub const BETTY_OBJECTIVE_COMPLETE: u32 = 4;
    pub const BETTY_NEW_LEADER: u32 = 5;
    pub const BETTY_RESOURCE_CAPTURED: u32 = 6;
    pub const BETTY_THIRTY_LEFT: u32 = 7;
    pub const BETTY_TWO_MIN_LEFT: u32 = 8;
    pub const BETTY_PERIMETER_BREACH: u32 = 9;
    pub const BETTY_LOW_DROP_WEIGHT: u32 = 10;
    pub const BETTY_MISSION_LOST: u32 = 11;
    pub const BETTY_PRI_OBJ_1_WON: u32 = 12;
    pub const BETTY_PRI_OBJ_2_WON: u32 = 13;
    pub const BETTY_PRI_OBJ_3_WON: u32 = 14;
    pub const BETTY_SEC_OBJ_1_WON: u32 = 15;
    pub const BETTY_SEC_OBJ_2_WON: u32 = 16;
    pub const BETTY_SEC_OBJ_3_WON: u32 = 17;
    pub const BETTY_MISSION_WON: u32 = 18;
    pub const BETTY_DEPLOY_MSG: u32 = 19;
    pub const BETTY_REPAIR_COMPLETE: u32 = 20;
    pub const BETTY_REPAIR_INCOMPLETE: u32 = 21;
    pub const BETTY_ARTILLERY_AVAILABLE: u32 = 22;
    pub const BETTY_SENSOR_PROBE_AVAIL: u32 = 23;
    pub const BETTY_CAMERA_DRONE_AVAIL: u32 = 24;
    pub const BETTY_NEW_CAMPAIGN: u32 = 25;
    pub const BETTY_OP_VIDEO: u32 = 26;
    pub const BETTY_BAD_FORCE_SELECTION: u32 = 27;
    pub const BETTY_NEW_EQUIPMENT: u32 = 28;
    pub const BETTY_NEW_COMPONENT: u32 = 29;
    pub const BETTY_NEW_MECH: u32 = 30;
    pub const BETTY_NEW_PILOT: u32 = 31;
    pub const BETTY_NEW_MECHWARRIOR: u32 = 32;
    pub const BETTY_NEW_VEHICLE: u32 = 33;
    pub const BETTY_SENS_TWR_CONTACT: u32 = 34;
    pub const BETTY_SENS_TWR_DESTROYED: u32 = 35;
    pub const BETTY_PERIMETER_ALARM_DEST: u32 = 36;
    pub const BETTY_SECOND_OBJ_NO_COMP: u32 = 37;
    pub const BETTY_POWERUP: u32 = 38;
    pub const BETTY_RESOURCES: u32 = 39;
    pub const BETTY_GUARDATTACK: u32 = 40;
    pub const BETTY_GUARDDESSTROYED: u32 = 41;
    pub const BETTY_PROMOREG: u32 = 42;
    pub const BETTY_PROMOVET: u32 = 43;
    pub const BETTY_PROMOELI: u32 = 44;
    pub const BETTY_PROMOACE: u32 = 45;
    pub const BETTY_NEW_MECHS: u32 = 46;
    pub const BETTY_NEW_PILOTS: u32 = 47;
    pub const BETTY_NEW_WEAPONS: u32 = 48;
    pub const BETTY_BUILDING_RECAPTURED: u32 = 49;
    pub const BETTY_GENERATOR_DESTROYED: u32 = 50;
    pub const BETTY_REPAIR_GONE: u32 = 51;
    pub const BETTY_MINES_GONE: u32 = 52;
    pub const BETTY_SENSOR_CAPTURED: u32 = 53;
    pub const MAX_BETTY_SAMPLES: u32 = 54;

    /// Every id of this group with its C++ macro name, in header order (counts excluded).
    pub const ALL: &[(&str, u32)] = &[
        ("BETTY_DROP_WEIGHT_EXCEEDED", 0),
        ("BETTY_AIR_SUPERIORITY", 1),
        ("BETTY_COMPONENTS_CAPTURED", 2),
        ("BETTY_CANNOT_COMP_OBJ", 3),
        ("BETTY_OBJECTIVE_COMPLETE", 4),
        ("BETTY_NEW_LEADER", 5),
        ("BETTY_RESOURCE_CAPTURED", 6),
        ("BETTY_THIRTY_LEFT", 7),
        ("BETTY_TWO_MIN_LEFT", 8),
        ("BETTY_PERIMETER_BREACH", 9),
        ("BETTY_LOW_DROP_WEIGHT", 10),
        ("BETTY_MISSION_LOST", 11),
        ("BETTY_PRI_OBJ_1_WON", 12),
        ("BETTY_PRI_OBJ_2_WON", 13),
        ("BETTY_PRI_OBJ_3_WON", 14),
        ("BETTY_SEC_OBJ_1_WON", 15),
        ("BETTY_SEC_OBJ_2_WON", 16),
        ("BETTY_SEC_OBJ_3_WON", 17),
        ("BETTY_MISSION_WON", 18),
        ("BETTY_DEPLOY_MSG", 19),
        ("BETTY_REPAIR_COMPLETE", 20),
        ("BETTY_REPAIR_INCOMPLETE", 21),
        ("BETTY_ARTILLERY_AVAILABLE", 22),
        ("BETTY_SENSOR_PROBE_AVAIL", 23),
        ("BETTY_CAMERA_DRONE_AVAIL", 24),
        ("BETTY_NEW_CAMPAIGN", 25),
        ("BETTY_OP_VIDEO", 26),
        ("BETTY_BAD_FORCE_SELECTION", 27),
        ("BETTY_NEW_EQUIPMENT", 28),
        ("BETTY_NEW_COMPONENT", 29),
        ("BETTY_NEW_MECH", 30),
        ("BETTY_NEW_PILOT", 31),
        ("BETTY_NEW_MECHWARRIOR", 32),
        ("BETTY_NEW_VEHICLE", 33),
        ("BETTY_SENS_TWR_CONTACT", 34),
        ("BETTY_SENS_TWR_DESTROYED", 35),
        ("BETTY_PERIMETER_ALARM_DEST", 36),
        ("BETTY_SECOND_OBJ_NO_COMP", 37),
        ("BETTY_POWERUP", 38),
        ("BETTY_RESOURCES", 39),
        ("BETTY_GUARDATTACK", 40),
        ("BETTY_GUARDDESSTROYED", 41),
        ("BETTY_PROMOREG", 42),
        ("BETTY_PROMOVET", 43),
        ("BETTY_PROMOELI", 44),
        ("BETTY_PROMOACE", 45),
        ("BETTY_NEW_MECHS", 46),
        ("BETTY_NEW_PILOTS", 47),
        ("BETTY_NEW_WEAPONS", 48),
        ("BETTY_BUILDING_RECAPTURED", 49),
        ("BETTY_GENERATOR_DESTROYED", 50),
        ("BETTY_REPAIR_GONE", 51),
        ("BETTY_MINES_GONE", 52),
        ("BETTY_SENSOR_CAPTURED", 53),
    ];
}

/// Support-officer sample ids (`support.pak` packet).
pub mod support {
    pub const SUPPORT_AIRSTRIKE: u32 = 0;
    pub const SUPPORT_MINELAYER: u32 = 1;
    pub const SUPPORT_PROBE: u32 = 2;
    pub const SUPPORT_RECOVER: u32 = 3;
    pub const SUPPORT_REPAIR: u32 = 4;
    pub const SUPPORT_SCOUT: u32 = 5;
    pub const SUPPORT_ARTILLERY: u32 = 6;
    pub const MAX_SUPPORT_SAMPLES: u32 = 7;

    /// Every id of this group with its C++ macro name, in header order (counts excluded).
    pub const ALL: &[(&str, u32)] = &[
        ("SUPPORT_AIRSTRIKE", 0),
        ("SUPPORT_MINELAYER", 1),
        ("SUPPORT_PROBE", 2),
        ("SUPPORT_RECOVER", 3),
        ("SUPPORT_REPAIR", 4),
        ("SUPPORT_SCOUT", 5),
        ("SUPPORT_ARTILLERY", 6),
    ];
}

/// Pilot message index (`ACKNOWLEDGE`).
pub const ACKNOWLEDGE: u32 = 2;
/// Pilot message index (`CALLSIGN`).
pub const CALLSIGN: u32 = 10;
