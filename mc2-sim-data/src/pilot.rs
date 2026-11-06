// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Warrior (pilot) profiles and the logistics pilot roster.
//!
//! Ports the load paths for `data/missions/warriors/*.fit`:
//! - `code/warrior.cpp`, `MechWarrior::init(FitIniFile*)` (lines 1020-1215): the
//!   in-mission warrior loader. `MechWarrior::calcRank` (lines 7232-7249) gives the rank.
//! - `code/logisticspilot.cpp`, `LogisticsPilot::init` (lines 62-141): the campaign-side
//!   loader. `LogisticsData::initPilots` (`code/logisticsdata.cpp` lines 191-216) runs it
//!   over `data/objects/pilots.csv`.
//!
//! Keys are read with the tag the C++ reader matches (`l`, `ul`, `c`, `uc`, `b`, `f`, `st`),
//! so a value written under another tag counts as missing. Defaults follow the C++ code.
//! Fields the C++ never reads are kept and marked "unread by the C++".

use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;
use mc2_formats::FormatError;

/// Warrior file directory, `warriorPath` in the shipped `system.cfg`.
pub const WARRIOR_DIR: &str = "data/missions/warriors/";
/// Logistics pilot list, `objectPath` + `pilots.csv` (`code/logisticsdata.cpp:198`).
pub const PILOTS_CSV: &str = "data/objects/pilots.csv";
/// Pilot portrait directory, `artPath` in the shipped `system.cfg`.
pub const ART_DIR: &str = "data/art/";
/// Number of core skills (`NUM_SKILLS`, `code/warrior.h:187`).
pub const NUM_SKILLS: usize = 3;
/// Number of specialty skills (`NUM_SPECIALTY_SKILLS`, `code/warrior.h:231`).
pub const NUM_SPECIALTY_SKILLS: usize = 23;
/// `[SpecialtySkills]` keys the shipped files carry that no C++ table names. The C++ never reads them.
pub const UNMAPPED_SPECIALTY_KEYS: [&str; 3] = ["HevayACSpecialist", "ReconSpecialist", "Deadeye"];

/// Maps a `mc2_formats` lookup to `Ok(None)` when the key is absent.
fn opt<T>(r: mc2_formats::Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(FormatError::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Selects `block` if it exists. When it does not, the current block is left alone and
/// the caller must not read keys (the C++ returns or skips in the same places).
fn enter(fit: &mut FitIni, block: &str) -> bool {
    fit.has_block(block) && fit.seek_block(block).is_ok()
}

/// Core skill, in `SkillsTable` order (`code/warrior.cpp:197-201`; enum `Skill` in `code/warrior.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skill {
    Piloting,
    Sensors,
    Gunnery,
}

impl Skill {
    /// All core skills in index order.
    pub const ALL: [Skill; NUM_SKILLS] = [Skill::Piloting, Skill::Sensors, Skill::Gunnery];

    /// Key name in `[Skills]` and `[SkillPoints]`.
    pub fn key(self) -> &'static str {
        match self {
            Skill::Piloting => "Piloting",
            Skill::Sensors => "Sensors",
            Skill::Gunnery => "Gunnery",
        }
    }

    /// Array index (`MWS_*`).
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Specialty skill, in C++ enum order (enum `SpecialtySkill` in `code/warrior.h`).
///
/// The FIT key for each index is in `SpecialtySkillsTable` (`code/warrior.cpp:247-270`).
/// The table order matches the enum order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialtySkill {
    LightMech,
    Laser,
    LightAc,
    MediumAc,
    Srm,
    SmallArms,
    SensorProfile,
    Toughness,
    MediumMech,
    PulseLaser,
    ErLaser,
    Lrm,
    Scout,
    LongJump,
    HeavyMech,
    Ppc,
    HeavyAc,
    ShortRange,
    MediumRange,
    LongRange,
    AssaultMech,
    Gauss,
    SharpShooter,
}

/// Classification of a specialty (`SpecialtySkillType`, `code/warrior.h`; table at
/// `code/warrior.cpp:332-360`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialtyKind {
    Weapon,
    Chassis,
    Range,
    Other,
}

impl SpecialtySkill {
    /// All specialties in index order.
    pub const ALL: [SpecialtySkill; NUM_SPECIALTY_SKILLS] = [
        SpecialtySkill::LightMech,
        SpecialtySkill::Laser,
        SpecialtySkill::LightAc,
        SpecialtySkill::MediumAc,
        SpecialtySkill::Srm,
        SpecialtySkill::SmallArms,
        SpecialtySkill::SensorProfile,
        SpecialtySkill::Toughness,
        SpecialtySkill::MediumMech,
        SpecialtySkill::PulseLaser,
        SpecialtySkill::ErLaser,
        SpecialtySkill::Lrm,
        SpecialtySkill::Scout,
        SpecialtySkill::LongJump,
        SpecialtySkill::HeavyMech,
        SpecialtySkill::Ppc,
        SpecialtySkill::HeavyAc,
        SpecialtySkill::ShortRange,
        SpecialtySkill::MediumRange,
        SpecialtySkill::LongRange,
        SpecialtySkill::AssaultMech,
        SpecialtySkill::Gauss,
        SpecialtySkill::SharpShooter,
    ];

    /// Key name in `[SpecialtySkills]`, as spelled in `SpecialtySkillsTable` (including the
    /// C++ typo `HevayMechSpecialist` and the `SensorProfileSpecialist` name for the sensor slot).
    pub fn key(self) -> &'static str {
        match self {
            SpecialtySkill::LightMech => "LightMechSpecialist",
            SpecialtySkill::Laser => "LaserSpecialist",
            SpecialtySkill::LightAc => "LightACSpecialist",
            SpecialtySkill::MediumAc => "MediumACSpecialist",
            SpecialtySkill::Srm => "SRMSpecialist",
            SpecialtySkill::SmallArms => "SmallArmsSpecialist",
            SpecialtySkill::SensorProfile => "SensorProfileSpecialist",
            SpecialtySkill::Toughness => "ToughnessSpecialist",
            SpecialtySkill::MediumMech => "MediumMechSpecialist",
            SpecialtySkill::PulseLaser => "PulseLaserSpecialist",
            SpecialtySkill::ErLaser => "ERLaserSpecialist",
            SpecialtySkill::Lrm => "LRMSpecialist",
            SpecialtySkill::Scout => "Scout",
            SpecialtySkill::LongJump => "LongJump",
            SpecialtySkill::HeavyMech => "HevayMechSpecialist",
            SpecialtySkill::Ppc => "PPCSpecialist",
            SpecialtySkill::HeavyAc => "HeavyACSpecialist",
            SpecialtySkill::ShortRange => "ShortRangeSpecialist",
            SpecialtySkill::MediumRange => "MediumRangeSpecialist",
            SpecialtySkill::LongRange => "LongRangeSpecialist",
            SpecialtySkill::AssaultMech => "AssaultMechSpecialist",
            SpecialtySkill::Gauss => "GaussSpecialist",
            SpecialtySkill::SharpShooter => "SharpShooter",
        }
    }

    /// Array index (the C++ enum value).
    pub fn index(self) -> usize {
        self as usize
    }

    /// The C++ `skillTypes` classification.
    pub fn kind(self) -> SpecialtyKind {
        match self {
            SpecialtySkill::LightMech
            | SpecialtySkill::MediumMech
            | SpecialtySkill::HeavyMech
            | SpecialtySkill::AssaultMech => SpecialtyKind::Chassis,
            SpecialtySkill::Laser
            | SpecialtySkill::LightAc
            | SpecialtySkill::MediumAc
            | SpecialtySkill::Srm
            | SpecialtySkill::SmallArms
            | SpecialtySkill::PulseLaser
            | SpecialtySkill::ErLaser
            | SpecialtySkill::Lrm
            | SpecialtySkill::Ppc
            | SpecialtySkill::HeavyAc
            | SpecialtySkill::Gauss => SpecialtyKind::Weapon,
            SpecialtySkill::ShortRange
            | SpecialtySkill::MediumRange
            | SpecialtySkill::LongRange => SpecialtyKind::Range,
            SpecialtySkill::SensorProfile
            | SpecialtySkill::Toughness
            | SpecialtySkill::Scout
            | SpecialtySkill::LongJump
            | SpecialtySkill::SharpShooter => SpecialtyKind::Other,
        }
    }
}

/// Warrior rank (enum `WarriorRank` in `code/warrior.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WarriorRank {
    Green,
    Regular,
    Veteran,
    Elite,
    Ace,
}

/// Rank for an average skill value. `MechWarrior::calcRank` (`code/warrior.cpp:7232-7249`)
/// and `LogisticsPilot::turnAverageIntoRank` (`code/logisticspilot.cpp:517-530`) use the
/// same strict thresholds: above 79 is Ace, above 70 Elite, above 60 Veteran, above 50 Regular.
pub fn rank_for_average(avg: f32) -> WarriorRank {
    if avg > 79.0 {
        WarriorRank::Ace
    } else if avg > 70.0 {
        WarriorRank::Elite
    } else if avg > 60.0 {
        WarriorRank::Veteran
    } else if avg > 50.0 {
        WarriorRank::Regular
    } else {
        WarriorRank::Green
    }
}

/// `[General]` block (`code/warrior.cpp:1032-1083`; `code/logisticspilot.cpp:69-99`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct General {
    /// `st Name`. Unread by the C++ (the display name comes from the string table at `desc_index`).
    pub name: Option<String>,
    /// `st Callsign`. Unread by the C++ (the callsign is the uppercased resource name).
    pub callsign: Option<String>,
    /// `ul ID`. Unread by the C++ loaders (`logisticspilot.cpp:99` is commented out).
    pub id: Option<u32>,
    /// `ul Profile`. Unread by the C++ warrior loader (the mission's `Profile` key names the file).
    pub profile: Option<u32>,
    /// `ul HeadIcon`. Unread by the C++.
    pub head_icon: Option<u32>,
    /// `l paintScheme` (`PaintScheme` in C++, `readIdInt`). Absent means -1, the mech's own colour.
    pub paint_scheme: i64,
    /// `st pilotAudio`. Absent means no radio (`warrior.cpp:1063-1068`).
    pub pilot_audio: Option<String>,
    /// `st pilotVideo`.
    pub pilot_video: Option<String>,
    /// `st Picture`, the portrait file name under `artPath`. Read by `LogisticsPilot::init` only.
    pub picture: Option<String>,
    /// `l Rank`. Read by `LogisticsPilot::init` (`logisticspilot.cpp:97`), then overwritten by
    /// the skill average (`logisticspilot.cpp:137`). Unread by `MechWarrior::init`.
    pub rank: Option<i64>,
    /// `l NameIndex`. Absent means -1 (`warrior.cpp:1042-1044`). Unread by `LogisticsPilot`.
    pub name_index: i64,
    /// `l DescIndex`. Absent means -1 (`warrior.cpp:1036-1038`). The string-table id of the name.
    pub desc_index: i64,
    /// `l PictureIndex`. Absent means 0 (`warrior.cpp:1050-1052`).
    pub picture_index: i64,
    /// `st Brain`. Unread by the warrior loaders. The mission's `Brain` key names the ABL brain.
    pub brain: Option<String>,
    /// `b NotMineYet`. Absent means false (`warrior.cpp:1046-1048`).
    pub not_mine_yet: bool,
    /// `uc OldPilot`. Absent means 0 (`warrior.cpp:1059-1061`).
    pub old_pilot: u8,
    /// `ul Video`. Unread by the C++. Present in four `war*` files only.
    pub video: Option<u32>,
    /// `l FlavorText`: string-table id of the pilot's biography (`logisticspilot.cpp:97`).
    /// Read by `LogisticsPilot::init` only.
    pub flavor_text: Option<i64>,
}

/// `[PersonalityTraits]` (`code/warrior.cpp:1089-1107`). Each key is a `c` (char) value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Personality {
    pub professionalism: Option<i8>,
    pub decorum: Option<i8>,
    pub aggressiveness: Option<i8>,
    pub courage: Option<i8>,
}

/// Core skill values from `[Skills]`, `[OriginalSkills]` or `[LatestSkills]` (`c` chars).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Skills {
    pub piloting: Option<i8>,
    pub sensors: Option<i8>,
    pub gunnery: Option<i8>,
}

impl Skills {
    /// The value for `skill`, if present.
    pub fn get(&self, skill: Skill) -> Option<i8> {
        match skill {
            Skill::Piloting => self.piloting,
            Skill::Sensors => self.sensors,
            Skill::Gunnery => self.gunnery,
        }
    }

    /// Per-key fallback. `self` is the block value, `fallback` the current skill. This is the
    /// C++ rule for `[OriginalSkills]` and `[LatestSkills]` (`warrior.cpp:1135-1168`).
    pub fn or(&self, fallback: &Skills) -> Skills {
        Skills {
            piloting: self.piloting.or(fallback.piloting),
            sensors: self.sensors.or(fallback.sensors),
            gunnery: self.gunnery.or(fallback.gunnery),
        }
    }
}

/// Skills the shipped files carry that the C++ warrior loader never reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExtraSkills {
    /// `c Jumping` (unread by the C++).
    pub jumping: Option<i8>,
    /// `c TargetingShort` (unread by the C++).
    pub targeting_short: Option<i8>,
    /// `c TargetingMedium` (unread by the C++).
    pub targeting_medium: Option<i8>,
    /// `c TargetingLong` (unread by the C++).
    pub targeting_long: Option<i8>,
}

/// `[Awards]` (`c` chars). The C++ never reads this block (no reference in `code/` or `mclib/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Awards {
    pub campaign_ribbon1: Option<i8>,
    pub campaign_ribbon1_with_honors: Option<i8>,
    pub campaign_ribbon2: Option<i8>,
    pub campaign_ribbon2_with_honors: Option<i8>,
    pub campaign_ribbon3: Option<i8>,
    pub campaign_ribbon3_with_honors: Option<i8>,
    pub purple_heart: Option<i8>,
    pub valor: Option<i8>,
    pub uncommon_valor: Option<i8>,
}

/// `[Affinities]`. The C++ never reads this block. `WeaponTypes` is `uc[2]`, and the
/// shipped files write it as `10,` with `15` on the next line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Affinities {
    pub mech_class: Option<i8>,
    pub mech_type: Option<i8>,
    pub weapon_class: Option<i8>,
    pub weapon_types: Option<Vec<u8>>,
}

/// A warrior profile, typed from one `data/missions/warriors/*.fit` file.
#[derive(Debug, Clone, PartialEq)]
pub struct WarriorProfile {
    /// File stem, without `.fit` and without the directory.
    pub file_name: String,
    pub general: General,
    /// `None` when the `[PersonalityTraits]` block is absent.
    pub personality: Option<Personality>,
    /// Current core skills.
    pub skills: Skills,
    /// Skills the C++ never reads.
    pub extra_skills: ExtraSkills,
    /// `[OriginalSkills]`, falling back per key to `skills` (`warrior.cpp:1135-1147`).
    pub original_skills: Skills,
    /// `[LatestSkills]`, falling back per key to `skills` (`warrior.cpp:1153-1165`).
    pub starting_skills: Skills,
    /// `[SkillPoints]` per core skill, zero when absent (`warrior.cpp:1171-1188`).
    pub skill_points: [f32; NUM_SKILLS],
    /// Specialty flags, indexed by [`SpecialtySkill::index`]. A key counts only when its value is 1
    /// (`warrior.cpp:1123-1132`). Missing keys and a missing block are false.
    pub specialty: [bool; NUM_SPECIALTY_SKILLS],
    /// Values under the [`UNMAPPED_SPECIALTY_KEYS`] names, in key order. Unused by the C++.
    pub specialty_extra: Vec<(String, i8)>,
    /// `None` when the `[Awards]` block is absent.
    pub awards: Option<Awards>,
    /// `None` when the `[Affinities]` block is absent.
    pub affinities: Option<Affinities>,
    /// `c Wounds` from `[Status]` (`warrior.cpp:1193-1199`). `None` when the block is absent.
    pub wounds: Option<i8>,
}

impl WarriorProfile {
    /// Parses a warrior profile from an already opened FIT file.
    ///
    /// Fails only when `[General]` is missing or a value is malformed. Missing keys become
    /// `None` or the C++ default. Use [`WarriorProfile::mission_init_gaps`] to see what
    /// `MechWarrior::init` would reject.
    pub fn from_fit(file_name: &str, fit: &mut FitIni) -> Result<Self> {
        if !enter(fit, "General") {
            return Err(SimDataError::schema(file_name, "missing [General] block"));
        }
        let general = General {
            name: opt(fit.read_string("Name", 255))?,
            callsign: opt(fit.read_string("Callsign", 255))?,
            id: opt(fit.read_u32("ID"))?,
            profile: opt(fit.read_u32("Profile"))?,
            head_icon: opt(fit.read_u32("HeadIcon"))?,
            paint_scheme: opt(fit.read_i64("paintScheme"))?.unwrap_or(-1),
            pilot_audio: opt(fit.read_string("pilotAudio", 255))?,
            pilot_video: opt(fit.read_string("pilotVideo", 255))?,
            picture: opt(fit.read_string("Picture", 255))?,
            rank: opt(fit.read_i64("Rank"))?,
            name_index: opt(fit.read_i64("NameIndex"))?.unwrap_or(-1),
            desc_index: opt(fit.read_i64("DescIndex"))?.unwrap_or(-1),
            picture_index: opt(fit.read_i64("PictureIndex"))?.unwrap_or(0),
            brain: opt(fit.read_string("Brain", 255))?,
            not_mine_yet: opt(fit.read_bool("NotMineYet"))?.unwrap_or(false),
            old_pilot: opt(fit.read_u8("OldPilot"))?.unwrap_or(0),
            video: opt(fit.read_u32("Video"))?,
            flavor_text: opt(fit.read_i64("FlavorText"))?,
        };

        let personality = if enter(fit, "PersonalityTraits") {
            Some(Personality {
                professionalism: opt(fit.read_i8("Professionalism"))?,
                decorum: opt(fit.read_i8("Decorum"))?,
                aggressiveness: opt(fit.read_i8("Aggressiveness"))?,
                courage: opt(fit.read_i8("Courage"))?,
            })
        } else {
            None
        };

        let (skills, extra_skills) = if enter(fit, "Skills") {
            let core = read_skills(fit)?;
            let extra = ExtraSkills {
                jumping: opt(fit.read_i8("Jumping"))?,
                targeting_short: opt(fit.read_i8("TargetingShort"))?,
                targeting_medium: opt(fit.read_i8("TargetingMedium"))?,
                targeting_long: opt(fit.read_i8("TargetingLong"))?,
            };
            (core, extra)
        } else {
            (Skills::default(), ExtraSkills::default())
        };

        let mut specialty = [false; NUM_SPECIALTY_SKILLS];
        let mut specialty_extra = Vec::new();
        if enter(fit, "SpecialtySkills") {
            for s in SpecialtySkill::ALL {
                specialty[s.index()] = opt(fit.read_i8(s.key()))? == Some(1);
            }
            for key in UNMAPPED_SPECIALTY_KEYS {
                if let Some(v) = opt(fit.read_i8(key))? {
                    specialty_extra.push((key.to_string(), v));
                }
            }
        }

        let original_skills = if enter(fit, "OriginalSkills") {
            read_skills(fit)?.or(&skills)
        } else {
            skills
        };
        let starting_skills = if enter(fit, "LatestSkills") {
            read_skills(fit)?.or(&skills)
        } else {
            skills
        };

        let mut skill_points = [0.0f32; NUM_SKILLS];
        if enter(fit, "SkillPoints") {
            for s in Skill::ALL {
                skill_points[s.index()] = opt(fit.read_f32(s.key()))?.unwrap_or(0.0);
            }
        }

        let awards = if enter(fit, "Awards") {
            Some(Awards {
                campaign_ribbon1: opt(fit.read_i8("CampaignRibbon1"))?,
                campaign_ribbon1_with_honors: opt(fit.read_i8("CampaignRibbon1WithHonors"))?,
                campaign_ribbon2: opt(fit.read_i8("CampaignRibbon2"))?,
                campaign_ribbon2_with_honors: opt(fit.read_i8("CampaignRibbon2WithHonors"))?,
                campaign_ribbon3: opt(fit.read_i8("CampaignRibbon3"))?,
                campaign_ribbon3_with_honors: opt(fit.read_i8("CampaignRibbon3WithHonors"))?,
                purple_heart: opt(fit.read_i8("PurpleHeart"))?,
                valor: opt(fit.read_i8("Valor"))?,
                uncommon_valor: opt(fit.read_i8("UncommonValor"))?,
            })
        } else {
            None
        };

        let affinities = if enter(fit, "Affinities") {
            let weapon_types = match opt(fit.array_len::<u8>("WeaponTypes"))? {
                Some(n) => opt(fit.read_array::<u8>("WeaponTypes", n))?,
                None => None,
            };
            Some(Affinities {
                mech_class: opt(fit.read_i8("MechClass"))?,
                mech_type: opt(fit.read_i8("MechType"))?,
                weapon_class: opt(fit.read_i8("WeaponClass"))?,
                weapon_types,
            })
        } else {
            None
        };

        let wounds = if enter(fit, "Status") {
            opt(fit.read_i8("Wounds"))?
        } else {
            None
        };

        Ok(WarriorProfile {
            file_name: file_name.to_string(),
            general,
            personality,
            skills,
            extra_skills,
            original_skills,
            starting_skills,
            skill_points,
            specialty,
            specialty_extra,
            awards,
            affinities,
            wounds,
        })
    }

    /// Loads `WARRIOR_DIR/<file_name>.fit` through the VFS.
    pub fn load(vfs: &mut Vfs, file_name: &str) -> Result<Self> {
        let path = format!("{WARRIOR_DIR}{file_name}.fit");
        let mut fit = vfs
            .read_fit(&path)?
            .ok_or_else(|| SimDataError::MissingFile { name: path.clone() })?;
        Self::from_fit(file_name, &mut fit)
    }

    /// Keys that `MechWarrior::init` returns early on when absent (`warrior.cpp:1089-1199`):
    /// the four personality traits, the three core skills and `Status.Wounds`.
    /// The shipped `war0xx` files lack `Gunnery`, so they are not mission-loadable.
    pub fn mission_init_gaps(&self) -> Vec<&'static str> {
        let mut gaps = Vec::new();
        match self.personality {
            None => gaps.push("PersonalityTraits"),
            Some(p) => {
                if p.professionalism.is_none() {
                    gaps.push("PersonalityTraits.Professionalism");
                }
                if p.decorum.is_none() {
                    gaps.push("PersonalityTraits.Decorum");
                }
                if p.aggressiveness.is_none() {
                    gaps.push("PersonalityTraits.Aggressiveness");
                }
                if p.courage.is_none() {
                    gaps.push("PersonalityTraits.Courage");
                }
            }
        }
        for skill in Skill::ALL {
            if self.skills.get(skill).is_none() {
                gaps.push(match skill {
                    Skill::Piloting => "Skills.Piloting",
                    Skill::Sensors => "Skills.Sensors",
                    Skill::Gunnery => "Skills.Gunnery",
                });
            }
        }
        if self.wounds.is_none() {
            gaps.push("Status.Wounds");
        }
        gaps
    }

    /// `MechWarrior::calcRank` over the loaded core skills: the average of gunnery and piloting.
    /// `None` when either skill is absent.
    pub fn calc_rank(&self) -> Option<WarriorRank> {
        let gunnery = f32::from(self.skills.gunnery?);
        let piloting = f32::from(self.skills.piloting?);
        Some(rank_for_average((gunnery + piloting) / 2.0))
    }

    /// `MechWarrior::isToughnessSpecialist` (`warrior.h:1180-1183`).
    pub fn is_toughness_specialist(&self) -> bool {
        self.specialty[SpecialtySkill::Toughness.index()]
    }
}

fn read_skills(fit: &FitIni) -> Result<Skills> {
    Ok(Skills {
        piloting: opt(fit.read_i8(Skill::Piloting.key()))?,
        sensors: opt(fit.read_i8(Skill::Sensors.key()))?,
        gunnery: opt(fit.read_i8(Skill::Gunnery.key()))?,
    })
}

/// One pilot from `pilots.csv`, typed as `LogisticsPilot` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PilotRecord {
    /// 1-based line number in `pilots.csv`. Skipped lines leave gaps (`logisticsdata.cpp:191-216`).
    pub id: u32,
    /// Warrior file stem from the CSV line.
    pub file_name: String,
    /// `General.descIndex` (`logisticspilot.cpp:85`). The key is read case-insensitively.
    pub desc_index: i64,
    /// `General.PictureIndex`, default 0 (`logisticspilot.cpp:92`).
    pub photo_index: i64,
    /// `General.FlavorText`, 0 when absent (`logisticspilot.cpp:97`).
    pub flavor_text: i64,
    /// `General.Rank` as read. The C++ then overwrites `rank` (see `rank`).
    pub file_rank: Option<i64>,
    /// `General.pilotAudio`.
    pub audio: Option<String>,
    /// `General.pilotVideo`.
    pub video: Option<String>,
    /// Portrait path: `ART_DIR` + `Picture`. When `Picture` is absent, the C++ uses whichever
    /// string was read last into its buffer, so this is `pilotVideo` (`logisticspilot.cpp:107-113`).
    pub icon_file: String,
    /// `Skills.Piloting`, 0 when absent (the C++ reads an uninitialised char).
    pub piloting: i8,
    /// `Skills.Gunnery`, 0 when absent (the C++ reads an uninitialised char).
    pub gunnery: i8,
    /// `[SpecialtySkills]` flags, in [`SpecialtySkill::index`] order.
    pub specialty: [bool; NUM_SPECIALTY_SKILLS],
    /// Rank from `(gunnery + piloting) / 2`, the `turnAverageIntoRank` rule (`logisticspilot.cpp:137`).
    pub rank: WarriorRank,
}

impl PilotRecord {
    /// Builds the roster record from a parsed profile, following `LogisticsPilot::init`.
    pub fn from_profile(id: u32, profile: &WarriorProfile) -> Self {
        let g = &profile.general;
        let piloting = profile.skills.piloting.unwrap_or(0);
        let gunnery = profile.skills.gunnery.unwrap_or(0);
        let stale = g
            .pilot_video
            .clone()
            .or_else(|| g.pilot_audio.clone())
            .unwrap_or_default();
        let icon = g.picture.clone().unwrap_or(stale);
        PilotRecord {
            id,
            file_name: profile.file_name.clone(),
            desc_index: g.desc_index,
            photo_index: g.picture_index,
            flavor_text: g.flavor_text.unwrap_or(0),
            file_rank: g.rank,
            audio: g.pilot_audio.clone(),
            video: g.pilot_video.clone(),
            icon_file: format!("{ART_DIR}{icon}"),
            piloting,
            gunnery,
            specialty: profile.specialty,
            rank: rank_for_average((f32::from(gunnery) + f32::from(piloting)) / 2.0),
        }
    }
}

/// A `pilots.csv` line whose warrior file is not in the VFS. `LogisticsPilot::init` returns -1
/// for these, and the C++ drops them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPilot {
    pub id: u32,
    pub file_name: String,
}

/// The logistics pilot list read from `pilots.csv`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PilotRoster {
    pub pilots: Vec<PilotRecord>,
    pub skipped: Vec<SkippedPilot>,
}

impl PilotRoster {
    /// The pilot whose warrior file stem matches `file_name`, ignoring ASCII case.
    pub fn find(&self, file_name: &str) -> Option<&PilotRecord> {
        self.pilots
            .iter()
            .find(|p| p.file_name.eq_ignore_ascii_case(file_name))
    }
}

/// Reads one line the way `File::readLine` does: stop at CR or LF, and skip the byte after
/// a CR. Lines longer than `max_len` are cut at `max_len`.
fn read_line(data: &[u8], pos: &mut usize, max_len: usize) -> Vec<u8> {
    let start = (*pos).min(data.len());
    let mut end = start;
    while end < data.len() && end - start < max_len && data[end] != b'\r' && data[end] != b'\n' {
        end += 1;
    }
    *pos = if end >= data.len() {
        end + 1
    } else if data[end] == b'\r' {
        end + 2
    } else if data[end] == b'\n' {
        end + 1
    } else {
        end
    };
    data[start..end].to_vec()
}

/// Names from `pilots.csv` in file order. The C++ loop (`logisticsdata.cpp:186-215`) stops at
/// the first line shorter than two bytes, so a blank line ends the list.
pub fn roster_names(data: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut pos = 0usize;
    loop {
        let line = read_line(data, &mut pos, 255);
        if line.len() < 2 {
            break;
        }
        names.push(String::from_utf8_lossy(&line).into_owned());
    }
    names
}

/// Loads `pilots.csv` and every warrior file it names.
///
/// The C++ opens the CSV without checking it (`logisticsdata.cpp:197-201`). A missing CSV is an
/// error here, not an empty roster.
pub fn load_roster(vfs: &mut Vfs) -> Result<PilotRoster> {
    let data = vfs
        .read(PILOTS_CSV)?
        .ok_or_else(|| SimDataError::MissingFile {
            name: PILOTS_CSV.to_string(),
        })?;
    let mut roster = PilotRoster::default();
    for (i, name) in roster_names(&data).into_iter().enumerate() {
        let id = (i + 1) as u32;
        match WarriorProfile::load(vfs, &name) {
            Ok(profile) => roster.pilots.push(PilotRecord::from_profile(id, &profile)),
            Err(SimDataError::MissingFile { .. }) => roster.skipped.push(SkippedPilot {
                id,
                file_name: name,
            }),
            Err(e) => return Err(e),
        }
    }
    Ok(roster)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(text.as_bytes().to_vec()).expect("FIT header")
    }

    const FULL: &str = "FITini\n\n[General]\n\
st Name =   \"Flash\"\n\
l Rank =   3 \n\
l DescIndex =   42005 \n\
l NameIndex = 29\n\
ul ID = 80\n\
l paintScheme = 31\n\
st pilotAudio = \"pilotg\"\n\
st pilotVideo = \"PILOT29\"\n\
[PersonalityTraits]\n\
c Professionalism = 50\n\
c Decorum = 50\n\
c Aggressiveness = 40\n\
c Courage = 50\n\
[Skills]\n\
c Piloting = 56\n\
c Sensors = 43\n\
c Gunnery = 52\n\
c Jumping = 9\n\
[SpecialtySkills]\n\
c LightMechSpecialist= 0\n\
c LaserSpecialist= 1\n\
c ToughnessSpecialist= 1\n\
c LongJumpc \n\
c Deadeye= 1\n\
c HeavyACSpecialist = 2\n\
[OriginalSkills]\n\
c Gunnery = 40\n\
[Status]\n\
c Wounds = 0\n\
FITEnd\n";

    #[test]
    fn parses_general_personality_skills_and_status() {
        let mut f = fit(FULL);
        let p = WarriorProfile::from_fit("pmp_flash", &mut f).unwrap();
        assert_eq!(p.general.name.as_deref(), Some("Flash"));
        assert_eq!(p.general.rank, Some(3));
        assert_eq!(p.general.desc_index, 42005);
        assert_eq!(p.general.name_index, 29);
        assert_eq!(p.general.picture_index, 0);
        assert_eq!(p.general.paint_scheme, 31);
        assert_eq!(p.general.id, Some(80));
        assert_eq!(p.general.pilot_video.as_deref(), Some("PILOT29"));
        assert!(p.general.picture.is_none());
        let pers = p.personality.unwrap();
        assert_eq!(pers.aggressiveness, Some(40));
        assert_eq!(p.skills.gunnery, Some(52));
        assert_eq!(p.extra_skills.jumping, Some(9));
        assert_eq!(p.wounds, Some(0));
        assert!(p.mission_init_gaps().is_empty());
        assert_eq!(p.calc_rank(), Some(WarriorRank::Regular));
    }

    #[test]
    fn specialty_flags_use_value_one_and_unmapped_keys() {
        let mut f = fit(FULL);
        let p = WarriorProfile::from_fit("x", &mut f).unwrap();
        assert!(!p.specialty[SpecialtySkill::LightMech.index()]);
        assert!(p.specialty[SpecialtySkill::Laser.index()]);
        assert!(p.is_toughness_specialist());
        // LongJump line is malformed in the shipped data, so it reads as absent.
        assert!(!p.specialty[SpecialtySkill::LongJump.index()]);
        // HeavyACSpecialist is read as 2, which is not 1, so the flag stays false.
        assert!(!p.specialty[SpecialtySkill::HeavyAc.index()]);
        assert_eq!(p.specialty_extra, vec![("Deadeye".to_string(), 1)]);
    }

    #[test]
    fn original_skills_fall_back_per_key_and_latest_to_current() {
        let mut f = fit(FULL);
        let p = WarriorProfile::from_fit("x", &mut f).unwrap();
        assert_eq!(p.original_skills.gunnery, Some(40));
        assert_eq!(p.original_skills.piloting, Some(56));
        assert_eq!(p.starting_skills, p.skills);
        assert_eq!(p.skill_points, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn missing_gunnery_is_reported_as_mission_init_gap() {
        let text = FULL.replace("c Gunnery = 52\n", "");
        let mut f = fit(&text);
        let p = WarriorProfile::from_fit("war", &mut f).unwrap();
        assert_eq!(p.mission_init_gaps(), vec!["Skills.Gunnery"]);
        assert_eq!(p.calc_rank(), None);
    }

    #[test]
    fn missing_general_block_is_an_error() {
        let mut f = fit("FITini\n[Skills]\nc Piloting = 1\nFITEnd\n");
        assert!(WarriorProfile::from_fit("x", &mut f).is_err());
    }

    #[test]
    fn absent_blocks_leave_defaults_and_do_not_read_previous_block() {
        // [Status] is missing, so the Wounds lookup must not run inside [Skills].
        let text = "FITini\n[General]\nst Name = \"A\"\n[Skills]\nc Piloting = 5\nc Sensors = 5\nc Gunnery = 5\nFITEnd\n";
        let mut f = fit(text);
        let p = WarriorProfile::from_fit("x", &mut f).unwrap();
        assert_eq!(p.wounds, None);
        assert!(p.personality.is_none());
        assert!(p.awards.is_none());
        assert_eq!(p.general.paint_scheme, -1);
        assert_eq!(p.general.picture_index, 0);
        assert_eq!(p.general.desc_index, -1);
    }

    #[test]
    fn rank_thresholds_are_strict() {
        assert_eq!(rank_for_average(50.0), WarriorRank::Green);
        assert_eq!(rank_for_average(50.5), WarriorRank::Regular);
        assert_eq!(rank_for_average(60.0), WarriorRank::Regular);
        assert_eq!(rank_for_average(70.0), WarriorRank::Veteran);
        assert_eq!(rank_for_average(79.0), WarriorRank::Elite);
        assert_eq!(rank_for_average(79.5), WarriorRank::Ace);
    }

    #[test]
    fn pilot_icon_falls_back_to_video_name_when_picture_is_absent() {
        let mut f = fit(FULL);
        let p = WarriorProfile::from_fit("pmp_flash", &mut f).unwrap();
        let rec = PilotRecord::from_profile(7, &p);
        assert_eq!(rec.icon_file, "data/art/PILOT29");
        assert_eq!(rec.id, 7);
        assert_eq!(rec.rank, WarriorRank::Regular);
        assert_eq!(rec.gunnery, 52);
        assert_eq!(rec.piloting, 56);
    }

    #[test]
    fn roster_lines_stop_at_blank_line_and_accept_crlf() {
        let data = b"pmwflash\r\npmp_flash\r\n\r\nignored\r\n";
        assert_eq!(roster_names(data), vec!["pmwflash", "pmp_flash"]);
        assert_eq!(roster_names(b"a1\nb2\n"), vec!["a1", "b2"]);
        assert!(roster_names(b"").is_empty());
    }

    #[test]
    fn array_weapon_types_parse_across_lines() {
        let text = "FITini\n[General]\nst Name = \"A\"\n[Affinities]\nc MechClass = 0\nuc[2] WeaponTypes = 10,\n\t\t\t\t\t\t\t\t15\nFITEnd\n";
        let mut f = fit(text);
        let p = WarriorProfile::from_fit("x", &mut f).unwrap();
        let aff = p.affinities.unwrap();
        assert_eq!(aff.mech_class, Some(0));
        assert_eq!(aff.weapon_types, Some(vec![10, 15]));
    }
}
