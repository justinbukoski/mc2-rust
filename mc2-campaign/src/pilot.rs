// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The player's pilot roster: skills, rank, medals, wounds and death, and the changes a mission
//! makes to them.
//!
//! Port of `code/logisticspilot.{h,cpp}`: `LogisticsPilot::init` (via [`crate::data::PilotTemplate`]),
//! `update` (the after-mission hand-over from the `MechWarrior`), `promotePilot` and
//! `turnAverageIntoRank`. The skill step that `update` triggers inside the warrior
//! (`MechWarrior::updateMissionSkills`, `code/warrior.cpp:7252-7268`) is ported here too, because
//! it only needs the numbers the mission reports ([`PilotReport`]).

use mc2_sim_data::pilot::NUM_SPECIALTY_SKILLS;

use crate::data::PilotTemplate;
use crate::error::{LogisticsError, LogisticsResult};

/// `MAX_MISSIONS`: size of the per-pilot "missions played" table.
pub const MAX_MISSIONS: usize = 50;
/// `MAX_MEDAL`: number of medals and ribbons.
pub const MAX_MEDAL: usize = 16;

/// `Medals` (`code/logisticspilot.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Medal {
    /// `CAMPAIGN_RIBBON1`: played any Steiner mission.
    CampaignRibbon1 = 0,
    /// `CAMPAIGN_RIBBON2`: played any Liao mission.
    CampaignRibbon2 = 1,
    /// `CAMPAIGN_RIBBON3`: played any Davion mission.
    CampaignRibbon3 = 2,
    /// `CAMPAIGN_FULLRIBBON1`: played every Steiner mission.
    CampaignFullRibbon1 = 3,
    /// `CAMPAIGN_FULLRIBBON2`: played every Liao mission.
    CampaignFullRibbon2 = 4,
    /// `CAMPAIGN_FULLRIBBON3`: played every Davion mission.
    CampaignFullRibbon3 = 5,
    /// `PURPLE_HEART`: wounded.
    PurpleHeart = 6,
    /// `VALOR`: three to six mech kills in one mission.
    Valor = 7,
    /// `UNCOMMON_VALOR`: seven or more mech kills in one mission.
    UncommonValor = 8,
    /// `MISSION_SPEC1`..`4`: unused by the logic.
    MissionSpec1 = 9,
    /// See [`Medal::MissionSpec1`].
    MissionSpec2 = 10,
    /// See [`Medal::MissionSpec1`].
    MissionSpec3 = 11,
    /// See [`Medal::MissionSpec1`].
    MissionSpec4 = 12,
    /// `LIAO_MEDAL`: played mission 12.
    LiaoMedal = 13,
    /// `DAVION_MEDAL`: played mission 21.
    DavionMedal = 14,
    /// `STEINER_MEDAL`: played mission 8.
    SteinerMedal = 15,
}

/// `WarriorRank` values, as stored (`long rank`).
pub const RANK_GREEN: i32 = 0;
/// `WARRIOR_RANK_REGULAR`.
pub const RANK_REGULAR: i32 = 1;
/// `WARRIOR_RANK_VETERAN`.
pub const RANK_VETERAN: i32 = 2;
/// `WARRIOR_RANK_ELITE`.
pub const RANK_ELITE: i32 = 3;
/// `WARRIOR_RANK_ACE`.
pub const RANK_ACE: i32 = 4;
/// `FIRST_VETERAN_SPECIALTY` (`code/warrior.h:245`): the first skill of the veteran choice. The
/// green and regular choices end before it.
pub const FIRST_VETERAN_SPECIALTY: usize = 8;
/// `FIRST_ELITE_SPECIALTY` (`code/warrior.h:246`).
pub const FIRST_ELITE_SPECIALTY: usize = 14;
/// `FIRST_ACE_SPECIALTY` (`code/warrior.h:247`).
pub const FIRST_ACE_SPECIALTY: usize = 20;

/// `MechWarrior::updateMissionSkills` caps a skill here.
pub const MAX_PILOT_SKILL: f32 = 80.0;
/// `LogisticsPilot::update` lets a skill rise by at most this much per mission.
pub const MAX_SKILL_GAIN_PER_MISSION: f32 = 4.0;

/// `turnAverageIntoRank`: strict thresholds on the gunnery/piloting average.
pub fn turn_average_into_rank(avg: f32) -> i32 {
    if avg > 79.0 {
        RANK_ACE
    } else if avg > 70.0 {
        RANK_ELITE
    } else if avg > 60.0 {
        RANK_VETERAN
    } else if avg > 50.0 {
        RANK_REGULAR
    } else {
        RANK_GREEN
    }
}

/// Same as [`turn_average_into_rank`], as `mc2_sim_data`'s rank type.
pub fn rank_for_average(avg: f32) -> mc2_sim_data::pilot::WarriorRank {
    mc2_sim_data::pilot::rank_for_average(avg)
}

/// `VehicleClass` indices into [`PilotReport::mission_kills`].
pub mod vehicle_class {
    /// `VEHICLE_CLASS_NONE`.
    pub const NONE: usize = 0;
    /// `VEHICLE_CLASS_LIGHTMECH`.
    pub const LIGHT_MECH: usize = 1;
    /// `VEHICLE_CLASS_MEDIUMMECH`.
    pub const MEDIUM_MECH: usize = 2;
    /// `VEHICLE_CLASS_HEAVYMECH`.
    pub const HEAVY_MECH: usize = 3;
    /// `VEHICLE_CLASS_ASSAULTMECH`.
    pub const ASSAULT_MECH: usize = 4;
    /// `VEHICLE_CLASS_GROUND`.
    pub const GROUND: usize = 5;
    /// `VEHICLE_CLASS_ELEMENTAL`.
    pub const ELEMENTAL: usize = 6;
    /// `NUM_VEHICLE_CLASSES`.
    pub const COUNT: usize = 7;
}

/// What a kill marker draws of the killed mover (`MechIcon::init` / `VehicleIcon::init`): its icon
/// picture and the armour of each location, which the icon's damage colours come from.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KilledUnit {
    /// `getIconPictureIndex()`.
    pub picture: i32,
    /// `(current armour, maximum armour, body location destroyed)` per armour location: a mech's
    /// eleven, a ground vehicle's first five.
    pub armor: [(f32, f32, bool); 11],
}

/// What a killed object was (`pDead->getObjectClass()` in `LogisticsPilot::update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KilledKind {
    /// A battle mech. Air units (helicopters are `BATTLEMECH` objects with `MOVETYPE_AIR`) do
    /// not count toward the valor medals.
    BattleMech {
        /// `getMoveType() == MOVETYPE_AIR`.
        air: bool,
    },
    /// A ground vehicle.
    GroundVehicle,
    /// Anything else.
    Other,
}

/// `WarriorStatus` (`code/warrior.h:263-271`), the end state of a warrior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WarriorStatus {
    /// `WARRIOR_STATUS_NORMAL`.
    Normal,
    /// `WARRIOR_STATUS_WITHDRAWING`.
    Withdrawing,
    /// `WARRIOR_STATUS_WITHDRAWN`.
    Withdrawn,
    /// `WARRIOR_STATUS_EJECTED`.
    Ejected,
    /// `WARRIOR_STATUS_DEAD`: the only status that kills the logistics pilot.
    Dead,
    /// `WARRIOR_STATUS_MIA`.
    Mia,
    /// `WARRIOR_STATUS_CAPTURED`.
    Captured,
    /// `WARRIOR_STATUS_BASECAMP`.
    Basecamp,
}

/// What the mission knows about one pilot at its end: the inputs of `LogisticsPilot::update`.
/// Each field names the C++ it comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct PilotReport {
    /// `pWarrior->getStatus()` (`warrior.h`, `LogisticsPilot::update` tests it against
    /// `WARRIOR_STATUS_DEAD`; ejected, MIA and captured warriors live on).
    pub status: WarriorStatus,
    /// `skillRank[MWS_GUNNERY]` before `updateMissionSkills` (`warrior.h:709`). The skill gain
    /// the campaign applies is [`update_mission_skills`] over these numbers, capped at
    /// [`MAX_SKILL_GAIN_PER_MISSION`] by [`Pilot::update`] (`logisticspilot.cpp:277-295`).
    pub gunnery_rank: f32,
    /// `skillRank[MWS_PILOTING]` before `updateMissionSkills`.
    pub piloting_rank: f32,
    /// `skillPoints[MWS_GUNNERY]` earned in the mission (`warrior.h:710`).
    pub gunnery_points: f32,
    /// `skillPoints[MWS_PILOTING]` earned in the mission.
    pub piloting_points: f32,
    /// `numMechKills[class][COMBAT_STAT_MISSION]` (`warrior.h:707`), indexed by [`vehicle_class`].
    pub mission_kills: [i32; vehicle_class::COUNT],
    /// `getRank()` at the end of the mission (`warrior.h:1029`).
    pub rank: i32,
    /// `getWounds()` (`warrior.h:1091`); anything non-zero earns the Purple Heart.
    pub wounds: f32,
    /// The objects this warrior killed (`killed[0..numKilled]`, `warrior.h:714`).
    pub killed: Vec<KilledKind>,
    /// The icon of each entry of `killed` (`pMover->getIconPictureIndex()` and its armour), in the
    /// same order. A shorter list reads as the default unit for the rest.
    pub killed_units: Vec<KilledUnit>,
    /// `specialtySkills[]` at the end of the mission.
    pub specialty_skills: [bool; NUM_SPECIALTY_SKILLS],
}

impl PilotReport {
    /// `getStatus() == WARRIOR_STATUS_DEAD`.
    pub fn is_dead(&self) -> bool {
        self.status == WarriorStatus::Dead
    }

    /// The skill gain `LogisticsPilot::update` applies for a pilot whose current skills are
    /// `(gunnery, piloting)`: the mission's `updateMissionSkills` result minus the current
    /// skill, at most [`MAX_SKILL_GAIN_PER_MISSION`] each.
    pub fn skill_gain(&self, gunnery: f32, piloting: f32) -> (f32, f32) {
        let (g, p) = update_mission_skills(self);
        (
            (g - gunnery).min(MAX_SKILL_GAIN_PER_MISSION),
            (p - piloting).min(MAX_SKILL_GAIN_PER_MISSION),
        )
    }
}

impl Default for PilotReport {
    fn default() -> Self {
        PilotReport {
            status: WarriorStatus::Normal,
            gunnery_rank: 0.0,
            piloting_rank: 0.0,
            gunnery_points: 0.0,
            piloting_points: 0.0,
            mission_kills: [0; vehicle_class::COUNT],
            rank: RANK_GREEN,
            wounds: 0.0,
            killed: Vec::new(),
            killed_units: Vec::new(),
            specialty_skills: [false; NUM_SPECIALTY_SKILLS],
        }
    }
}

/// `MechWarrior::updateMissionSkills` on the reported numbers: gunnery gains a hundredth of the
/// points, piloting gains one free point plus the points, both capped at 80. Returns
/// `(gunnery, piloting)`.
pub fn update_mission_skills(report: &PilotReport) -> (f32, f32) {
    let mut gunnery = report.gunnery_rank;
    let mut piloting = report.piloting_rank;

    gunnery += report.gunnery_points * 0.01f32;
    if gunnery > MAX_PILOT_SKILL {
        gunnery = MAX_PILOT_SKILL;
    }

    piloting += 1.0; // one free point just for coming along
    piloting += report.piloting_points;
    if piloting > MAX_PILOT_SKILL {
        piloting = MAX_PILOT_SKILL;
    }
    (gunnery, piloting)
}

/// One pilot of the roster (`LogisticsPilot`).
#[derive(Debug, Clone, PartialEq)]
pub struct Pilot {
    /// `id`: line number in `pilots.csv`.
    pub id: u32,
    /// Warrior profile stem. The key the rest of the Rust port identifies a pilot by (the C++
    /// matches the localized display name).
    pub file_name: String,
    /// `descIndex`: string id of the display name.
    pub name_id: i64,
    /// `pilotAudio`.
    pub audio: String,
    /// `pilotVideo`.
    pub video: String,
    /// `PictureIndex`.
    pub photo_index: i64,
    /// `FlavorText`: string id.
    pub flavor_text_id: i64,
    /// Portrait file.
    pub icon_file: String,
    /// `rank` (see [`RANK_GREEN`]).
    pub rank: i32,
    /// Current gunnery skill.
    pub gunnery: f32,
    /// Current piloting skill.
    pub piloting: f32,
    /// `mechKills`.
    pub mech_kills: i32,
    /// `vehicleKills`.
    pub vehicle_kills: i32,
    /// `infantryKills`.
    pub infantry_kills: i32,
    /// `missionsCompleted`.
    pub missions_completed: i32,
    /// `missionsPlayed[]`: 1 for every mission id the pilot has flown.
    pub missions_played: [u8; MAX_MISSIONS],
    /// `bIsUsed`: assigned to a mech in the force or part of the last mission.
    pub used: bool,
    /// `bDead`.
    pub dead: bool,
    /// `bAvailable`: listed in the current purchase files.
    pub available: bool,
    /// `bJustDied`: died in the mission just played.
    pub just_died: bool,
    /// `newPiloting`: piloting gained in the last mission.
    pub new_piloting: f32,
    /// `newGunnery`: gunnery gained in the last mission.
    pub new_gunnery: f32,
    /// `specialtySkills[]`.
    pub specialty_skills: [bool; NUM_SPECIALTY_SKILLS],
    /// `medals[]`.
    pub medals: [bool; MAX_MEDAL],
    /// `medalsLastMission[]`: medals earned in the last mission.
    pub medals_last_mission: [bool; MAX_MEDAL],
    /// `killedIcons` (`logisticspilot.h`): what the pilot killed in the last mission, in kill
    /// order, as the pilot review's kill markers show it (battle mechs and ground vehicles only;
    /// the C++ keeps an icon per kill, we keep the kind). Not saved, as in the C++.
    pub killed_icons: Vec<KilledKind>,
    /// The icon of each of `killed_icons`, in the same order (the unit icon the kill marker draws).
    pub killed_units: Vec<KilledUnit>,
}

impl Pilot {
    /// A fresh pilot as `LogisticsPilot::init` leaves it: nothing flown, not available, rank from
    /// the skill average.
    pub fn from_template(t: &PilotTemplate) -> Pilot {
        Pilot {
            id: t.id,
            file_name: t.file_name.clone(),
            name_id: t.name_id,
            audio: t.audio.clone(),
            video: t.video.clone(),
            photo_index: t.photo_index,
            flavor_text_id: t.flavor_text_id,
            icon_file: t.icon_file.clone(),
            rank: turn_average_into_rank((t.gunnery + t.piloting) / 2.0),
            gunnery: t.gunnery,
            piloting: t.piloting,
            mech_kills: 0,
            vehicle_kills: 0,
            infantry_kills: 0,
            missions_completed: 0,
            missions_played: [0; MAX_MISSIONS],
            used: false,
            dead: false,
            available: false,
            just_died: false,
            new_piloting: 0.0,
            new_gunnery: 0.0,
            specialty_skills: t.specialty_skills,
            medals: [false; MAX_MEDAL],
            medals_last_mission: [false; MAX_MEDAL],
            killed_icons: Vec::new(),
            killed_units: Vec::new(),
        }
    }

    /// `isAvailable`: in a purchase list and alive.
    pub fn is_available(&self) -> bool {
        self.available && !self.dead
    }

    /// `getNumberMissions`.
    pub fn number_missions(&self) -> i32 {
        self.missions_completed
    }

    /// `getMissionPlayed`.
    pub fn mission_played(&self, mission_num: i64) -> bool {
        usize::try_from(mission_num)
            .ok()
            .filter(|&m| m < MAX_MISSIONS)
            .is_some_and(|m| self.missions_played[m] == 1)
    }

    /// `getSpecialtySkillCount`.
    pub fn specialty_skill_count(&self) -> usize {
        self.specialty_skills.iter().filter(|&&s| s).count()
    }

    /// `getSpecialtySkills(int*, int&)`: indices of the skills the pilot has.
    pub fn specialty_skill_indices(&self) -> Vec<usize> {
        self.specialty_skills
            .iter()
            .enumerate()
            .filter_map(|(i, &s)| s.then_some(i))
            .collect()
    }

    /// The end of the skills a promoted pilot picks from, for the rank the pilot has now
    /// (`PilotPromotionArea::setPilot`: `maxSkill`). Each rank adds the next run of skills.
    pub fn specialty_choice_end(&self) -> usize {
        if self.rank > RANK_ELITE {
            NUM_SPECIALTY_SKILLS
        } else if self.rank > RANK_VETERAN {
            FIRST_ACE_SPECIALTY
        } else if self.rank > RANK_REGULAR {
            FIRST_ELITE_SPECIALTY
        } else {
            FIRST_VETERAN_SPECIALTY
        }
    }

    /// The skills the promoted pilot can pick now: those below [`Pilot::specialty_choice_end`]
    /// that the pilot does not have, in index order.
    pub fn specialty_offers(&self) -> Vec<usize> {
        (0..self.specialty_choice_end())
            .filter(|&s| !self.specialty_skills[s])
            .collect()
    }

    /// `setSpecialtySkill`; an out-of-range skill is ignored (the C++ asserts).
    pub fn set_specialty_skill(&mut self, skill: usize, set: bool) {
        if let Some(slot) = self.specialty_skills.get_mut(skill) {
            *slot = set;
        }
    }

    /// `LogisticsPilot::update(pWarrior)`: applies a finished mission to the pilot.
    ///
    /// `mission_id` is `LogisticsData::getCurrentMissionId()` (the flat index of the mission just
    /// played, valid right after `setMissionComplete`). The pilot is marked used, skills rise by
    /// the mission's gain (at most [`MAX_SKILL_GAIN_PER_MISSION`] each), kills are added, medals
    /// are awarded, and a dead warrior marks the pilot dead.
    pub fn update(&mut self, report: &PilotReport, mission_id: i64) -> LogisticsResult<()> {
        // The C++ stops the program here; refuse instead (and before changing anything).
        let mission_id = usize::try_from(mission_id)
            .ok()
            .filter(|&m| m < MAX_MISSIONS)
            .ok_or(LogisticsError::InvalidId)?;

        self.killed_icons.clear();
        self.killed_units.clear();
        self.missions_completed += 1;
        self.missions_played[mission_id] = 1;

        if report.is_dead() {
            self.dead = true;
            self.just_died = true;
        }

        self.used = true;

        let (gunnery_rank, piloting_rank) = update_mission_skills(report);
        self.new_gunnery = gunnery_rank - self.gunnery;
        self.new_piloting = piloting_rank - self.piloting;
        self.gunnery = gunnery_rank;
        self.piloting = piloting_rank;

        // make sure no more than 4 points per mission
        if self.new_gunnery > MAX_SKILL_GAIN_PER_MISSION {
            self.gunnery -= self.new_gunnery;
            self.new_gunnery = MAX_SKILL_GAIN_PER_MISSION;
            self.gunnery += self.new_gunnery;
        }
        if self.new_piloting > MAX_SKILL_GAIN_PER_MISSION {
            self.piloting -= self.new_piloting;
            self.new_piloting = MAX_SKILL_GAIN_PER_MISSION;
            self.piloting += self.new_piloting;
        }

        use vehicle_class::*;
        let k = &report.mission_kills;
        self.infantry_kills += k[ELEMENTAL];
        self.mech_kills += k[LIGHT_MECH] + k[MEDIUM_MECH] + k[HEAVY_MECH] + k[ASSAULT_MECH];
        self.vehicle_kills += k[GROUND];

        self.rank = report.rank;

        let dead_mech_count = report
            .killed
            .iter()
            .filter(|k| matches!(k, KilledKind::BattleMech { air: false }))
            .count();

        // `killedIcons.Append` for each killed mech (air included) and ground vehicle
        for (i, k) in report.killed.iter().copied().enumerate() {
            if !matches!(k, KilledKind::Other) {
                self.killed_icons.push(k);
                self.killed_units
                    .push(report.killed_units.get(i).copied().unwrap_or_default());
            }
        }

        self.medals_last_mission = [false; MAX_MEDAL];

        if dead_mech_count >= 7 {
            self.medals_last_mission[Medal::UncommonValor as usize] = true;
            self.medals[Medal::UncommonValor as usize] = true;
        }
        if (3..7).contains(&dead_mech_count) {
            self.medals_last_mission[Medal::Valor as usize] = true;
            self.medals[Medal::Valor as usize] = true;
        }

        if report.wounds != 0.0 {
            if !self.medals[Medal::PurpleHeart as usize] {
                self.medals_last_mission[Medal::PurpleHeart as usize] = true;
            }
            self.medals[Medal::PurpleHeart as usize] = true;
        }

        // Campaign ribbons and medals: the shipping campaign's mission ids are 0-8 Steiner,
        // 9-16 Liao, 17-23 Davion.
        let played = |range: std::ops::RangeInclusive<usize>| -> i64 {
            range.map(|i| i64::from(self.missions_played[i])).sum()
        };
        let any_steiner = played(0..=8);
        let any_liao = played(9..=16);
        let any_davion = played(17..=23);

        let mut award = |medal: Medal, earned: bool| {
            if earned {
                let i = medal as usize;
                if !self.medals[i] {
                    self.medals_last_mission[i] = true;
                }
                self.medals[i] = true;
            }
        };
        award(Medal::CampaignRibbon1, any_steiner != 0);
        award(Medal::CampaignRibbon2, any_liao != 0);
        award(Medal::CampaignRibbon3, any_davion != 0);
        award(Medal::CampaignFullRibbon1, any_steiner == 9);
        award(Medal::CampaignFullRibbon2, any_liao == 8);
        award(Medal::CampaignFullRibbon3, any_davion == 7);
        let m8 = self.missions_played[8] != 0;
        let m12 = self.missions_played[12] != 0;
        let m21 = self.missions_played[21] != 0;
        award(Medal::SteinerMedal, m8);
        award(Medal::LiaoMedal, m12);
        award(Medal::DavionMedal, m21);

        for (mine, theirs) in self
            .specialty_skills
            .iter_mut()
            .zip(&report.specialty_skills)
        {
            if *theirs {
                *mine = true;
            }
        }
        Ok(())
    }

    /// `promotePilot`: re-derives the rank from the new averages. Returns `true` when the pilot
    /// moved up a rank and is now above green (the promotion screen shows then).
    pub fn promote_pilot(&mut self) -> bool {
        let old_gunnery = self.gunnery - self.new_gunnery;
        let old_piloting = self.piloting - self.new_piloting;
        let old_avg = (old_gunnery + old_piloting) / 2.0;
        let new_avg = (self.gunnery + self.piloting) / 2.0;

        let old_rank = turn_average_into_rank(old_avg);
        let new_rank = turn_average_into_rank(new_avg);

        if self.rank != new_rank {
            self.rank = new_rank;
        }

        if old_rank != new_rank {
            // the C++ asserts that a rank never drops here
            return self.rank > RANK_GREEN;
        }
        false
    }

    /// `clearIcons` + `setJustDied(0)` as the pilot review screen does when it closes.
    pub fn acknowledge_review(&mut self) {
        self.just_died = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn pilot(gunnery: f32, piloting: f32) -> Pilot {
        Pilot::from_template(&PilotTemplate {
            id: 1,
            file_name: "pmwtest".into(),
            name_id: 5,
            audio: String::new(),
            video: String::new(),
            photo_index: 0,
            flavor_text_id: 0,
            icon_file: String::new(),
            gunnery,
            piloting,
            specialty_skills: [false; NUM_SPECIALTY_SKILLS],
        })
    }

    fn report(gunnery: f32, piloting: f32, gp: f32, pp: f32) -> PilotReport {
        PilotReport {
            gunnery_rank: gunnery,
            piloting_rank: piloting,
            gunnery_points: gp,
            piloting_points: pp,
            ..PilotReport::default()
        }
    }

    #[test]
    fn rank_thresholds_are_strict() {
        assert_eq!(turn_average_into_rank(50.0), RANK_GREEN);
        assert_eq!(turn_average_into_rank(50.5), RANK_REGULAR);
        assert_eq!(turn_average_into_rank(60.0), RANK_REGULAR);
        assert_eq!(turn_average_into_rank(61.0), RANK_VETERAN);
        assert_eq!(turn_average_into_rank(70.0), RANK_VETERAN);
        assert_eq!(turn_average_into_rank(70.1), RANK_ELITE);
        assert_eq!(turn_average_into_rank(79.0), RANK_ELITE);
        assert_eq!(turn_average_into_rank(79.5), RANK_ACE);
    }

    #[test]
    fn mission_skills_follow_update_mission_skills() {
        // gunnery: +points/100; piloting: +1 free, +points; both cap at 80.
        assert_eq!(
            update_mission_skills(&report(50.0, 50.0, 200.0, 2.0)),
            (52.0, 53.0)
        );
        assert_eq!(
            update_mission_skills(&report(79.9, 79.5, 100.0, 5.0)),
            (80.0, 80.0)
        );
        assert_eq!(
            update_mission_skills(&report(40.0, 40.0, 0.0, 0.0)),
            (40.0, 41.0)
        );
    }

    #[test]
    fn update_caps_the_gain_at_four_points() {
        let mut p = pilot(50.0, 50.0);
        // gunnery +6 -> capped to +4; piloting +1 free +2 = +3 -> kept.
        p.update(&report(50.0, 50.0, 600.0, 2.0), 0).unwrap();
        assert_eq!(p.new_gunnery, 4.0);
        assert_eq!(p.gunnery, 54.0);
        assert_eq!(p.new_piloting, 3.0);
        assert_eq!(p.piloting, 53.0);
        assert!(p.used);
        assert_eq!(p.missions_completed, 1);
        assert!(p.mission_played(0));
        assert!(!p.mission_played(1));
    }

    #[test]
    fn update_rejects_a_mission_id_outside_the_table() {
        let mut p = pilot(50.0, 50.0);
        assert_eq!(
            p.update(&report(50.0, 50.0, 0.0, 0.0), 50),
            Err(LogisticsError::InvalidId)
        );
        assert_eq!(
            p.update(&report(50.0, 50.0, 0.0, 0.0), -1),
            Err(LogisticsError::InvalidId)
        );
        assert_eq!(p.missions_completed, 0, "nothing changed");
    }

    #[test]
    fn kills_are_added_by_class() {
        let mut p = pilot(50.0, 50.0);
        let mut r = report(50.0, 50.0, 0.0, 0.0);
        r.mission_kills[vehicle_class::LIGHT_MECH] = 2;
        r.mission_kills[vehicle_class::ASSAULT_MECH] = 1;
        r.mission_kills[vehicle_class::GROUND] = 4;
        r.mission_kills[vehicle_class::ELEMENTAL] = 3;
        r.mission_kills[vehicle_class::NONE] = 99;
        p.update(&r, 3).unwrap();
        assert_eq!((p.mech_kills, p.vehicle_kills, p.infantry_kills), (3, 4, 3));
    }

    #[test]
    fn valor_medals_count_non_air_mech_kills() {
        let mech = KilledKind::BattleMech { air: false };
        let copter = KilledKind::BattleMech { air: true };
        let mut p = pilot(50.0, 50.0);
        let mut r = report(50.0, 50.0, 0.0, 0.0);
        r.killed = vec![mech, mech, copter, KilledKind::GroundVehicle];
        p.update(&r, 0).unwrap();
        assert!(!p.medals[Medal::Valor as usize]);

        r.killed = vec![mech, mech, mech, copter];
        p.update(&r, 1).unwrap();
        assert!(p.medals[Medal::Valor as usize] && p.medals_last_mission[Medal::Valor as usize]);
        assert!(!p.medals[Medal::UncommonValor as usize]);

        r.killed = vec![mech; 7];
        p.update(&r, 2).unwrap();
        assert!(p.medals[Medal::UncommonValor as usize]);
        // Valor is no longer "last mission": the array is rebuilt each update.
        assert!(!p.medals_last_mission[Medal::Valor as usize]);
        assert!(p.medals[Medal::Valor as usize], "earned medals stay");
    }

    #[test]
    fn purple_heart_is_new_only_the_first_time() {
        let mut p = pilot(50.0, 50.0);
        let mut r = report(50.0, 50.0, 0.0, 0.0);
        r.wounds = 1.0;
        p.update(&r, 0).unwrap();
        assert!(p.medals_last_mission[Medal::PurpleHeart as usize]);
        p.update(&r, 1).unwrap();
        assert!(!p.medals_last_mission[Medal::PurpleHeart as usize]);
        assert!(p.medals[Medal::PurpleHeart as usize]);
    }

    #[test]
    fn campaign_ribbons_and_medals_follow_the_mission_ids() {
        let mut p = pilot(50.0, 50.0);
        let r = report(50.0, 50.0, 0.0, 0.0);
        p.update(&r, 0).unwrap();
        assert!(p.medals[Medal::CampaignRibbon1 as usize]);
        assert!(p.medals_last_mission[Medal::CampaignRibbon1 as usize]);
        assert!(!p.medals[Medal::CampaignRibbon2 as usize]);
        assert!(!p.medals[Medal::SteinerMedal as usize]);

        for m in 1..=8 {
            p.update(&r, m).unwrap();
        }
        assert!(p.medals[Medal::CampaignFullRibbon1 as usize]);
        assert!(p.medals[Medal::SteinerMedal as usize], "mission 8 played");

        p.update(&r, 12).unwrap();
        assert!(p.medals[Medal::LiaoMedal as usize]);
        assert!(p.medals[Medal::CampaignRibbon2 as usize]);
        assert!(!p.medals[Medal::CampaignFullRibbon2 as usize]);
        p.update(&r, 21).unwrap();
        assert!(p.medals[Medal::DavionMedal as usize]);
        assert!(p.medals[Medal::CampaignRibbon3 as usize]);
    }

    #[test]
    fn death_marks_the_pilot_dead_and_just_died() {
        let mut p = pilot(50.0, 50.0);
        p.available = true;
        let mut r = report(50.0, 50.0, 0.0, 0.0);
        r.status = WarriorStatus::Dead;
        p.update(&r, 0).unwrap();
        assert!(p.dead && p.just_died && p.used);
        assert!(!p.is_available(), "dead pilots are not available");
        p.acknowledge_review();
        assert!(!p.just_died);
        assert!(p.dead);
    }

    #[test]
    fn specialty_skills_only_accumulate() {
        let mut p = pilot(50.0, 50.0);
        p.set_specialty_skill(3, true);
        let mut r = report(50.0, 50.0, 0.0, 0.0);
        r.specialty_skills[5] = true;
        p.update(&r, 0).unwrap();
        assert_eq!(p.specialty_skill_indices(), vec![3, 5]);
        assert_eq!(p.specialty_skill_count(), 2);
        p.set_specialty_skill(99, true); // out of range: ignored
        assert_eq!(p.specialty_skill_count(), 2);
    }

    #[test]
    fn specialty_offers_follow_the_rank() {
        let mut p = pilot(50.0, 50.0);
        p.rank = RANK_GREEN;
        assert_eq!(
            p.specialty_offers(),
            (0..FIRST_VETERAN_SPECIALTY).collect::<Vec<_>>()
        );
        p.rank = RANK_ELITE;
        assert_eq!(p.specialty_choice_end(), FIRST_ACE_SPECIALTY);
        p.rank = RANK_ACE;
        assert_eq!(p.specialty_choice_end(), NUM_SPECIALTY_SKILLS);
        // a skill the pilot has is not offered again
        p.set_specialty_skill(2, true);
        assert!(!p.specialty_offers().contains(&2));
        assert_eq!(p.specialty_offers().len(), NUM_SPECIALTY_SKILLS - 1);
    }

    #[test]
    fn promotion_reports_a_rank_change_above_green() {
        // 49/49 avg 49 green. +4 gunnery, +4 piloting -> 53/53 regular.
        let mut p = pilot(49.0, 49.0);
        p.gunnery = 53.0;
        p.piloting = 53.0;
        p.new_gunnery = 4.0;
        p.new_piloting = 4.0;
        assert!(p.promote_pilot());
        assert_eq!(p.rank, RANK_REGULAR);
        // A second call sees the same old/new ranks, still true (the rank is re-derived).
        assert!(p.promote_pilot());
        // No change of rank: false.
        let mut q = pilot(55.0, 55.0);
        q.new_gunnery = 1.0;
        q.new_piloting = 1.0;
        assert!(!q.promote_pilot());
    }

    #[test]
    fn promotion_to_green_is_not_reported() {
        // Rank can only change upward, but if the old rank is below green there is no such thing;
        // a pilot that stays green reports false.
        let mut p = pilot(30.0, 30.0);
        p.new_gunnery = 2.0;
        p.new_piloting = 2.0;
        assert!(!p.promote_pilot());
    }
}
