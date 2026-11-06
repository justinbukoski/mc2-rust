// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Headless walk of the shipped `campaign.fit` from the first mission to the campaign end, using
//! only the public `mc2-campaign` API.
//!
//! Every step starts the mission the campaign offers, hands the results back a scripted big win,
//! runs the results flow the screens would run (salvage, pilot review and its promotions, the
//! autosave decision), runs the next mission selection screen and presses a choice on it (every
//! branch of every choice group gets pressed at least once by `every_branch_is_played`), and
//! compares the campaign, before it moves on, with what the shipped data and the C++ rules say.
//! After every step the campaign is saved with `save.rs` as well, and the loaded copy must equal
//! the live one. The walk ends where the campaign data ends it: after its last mission, with the
//! campaign over, having played as many missions as the campaign file lists (24).
//!
//! Everything expected is read here from the shipped data (through the VFS) and from the C++
//! rules named next to it, never from the code under test:
//!
//! * the mission order, the groups and the branches are `data/campaign/campaign.fit` (`Group<i>`,
//!   `Group<i>Mission<j>`, `MissionCount`, `NumberToComplete`, `FileName`, `Mandatory`,
//!   `Hidden`, `logisticsmissioninfo.cpp:124-268`), and the missions' settings are their own
//!   files (`readMissionInfo`, `logisticsmissioninfo.cpp:300-345`);
//! * a campaign starts with its `[Campaign] CBills` and nothing else: the start selects the first
//!   available mission with `setCurrentMission` (`startNewCampaign`, `logisticsdata.cpp:2296-2302`;
//!   `LogisticsData::init`, `:113-117`), which is a press like any other (`setNextMission`,
//!   `logisticsdata.cpp:767-780`), so it pays that mission's `additionalCBills` and gives back
//!   nothing, the campaign starting at `currentMission = -1` (`logisticsmissioninfo.cpp:23`,
//!   `:67`); the shipped missions' are 0, so the bank is the campaign file's own. The campaign is
//!   not paid again when it is
//!   loaded and handed on to its next screen: `LogisticsMissionInfo::load` reads the index, the
//!   C-bills and the completed missions back, and the selection screen only shows them
//!   (`logisticsmissioninfo.cpp:342-452`, `:521-552`; `MissionBegin::begin` sets the screens up
//!   and opens them, `missionbegin.cpp:115-409`; a loaded campaign only restarts the screens, the
//!   same way, `mainmenu.cpp:549-567`);
//! * the selection screen lists the open missions of the current operation and presses the
//!   campaign's own mission; only a press of another mission moves the campaign on
//!   (`MissionSelectionScreen::begin`, `missionselectionscreen.cpp:274-296`; `setMission`,
//!   `:491-503`);
//! * a mission pays its `AdditionalCBills` every time it is pressed on the selection screen,
//!   losing those of the open mission it replaces in the same group (`setNextMission`,
//!   `logisticsmissioninfo.cpp:646-676`), and pays them one last time along with the objectives
//!   that succeeded when it completes (`setMissionComplete`, `logisticsmissioninfo.cpp:785-870`;
//!   `setMissionCompleted`, `logisticsdata.cpp:1315-1408`);
//! * a mech costs its chassis' `baseCost` plus the `cost` of every component it carries, and
//!   buying one takes that off the bank (`LogisticsVariant::getCost`, `logisticsvariant.cpp:493-503`;
//!   `LogisticsComponent::getCost`, `logisticscomponent.h:38`; `LogisticsData::purchaseMech`,
//!   `logisticsdata.cpp:454-472`);
//! * pilot skills follow the shipped `[Skills]` tables of `gamesys.fit` with
//!   `MechWarrior::updateMissionSkills` (`warrior.cpp:7252-7269`), and the campaign caps a
//!   mission's gain at four points with `LogisticsPilot::update` (`logisticspilot.cpp:255-313`);
//! * the pilot review draws the skill a mech's pilot brings into the mission from the logistics
//!   pilot (`getBestPilot`, `logisticsdata.cpp:913-960`; `Mover::loadPilot`,
//!   `mover.cpp:3394-3402`), keeps the flown mech in the bay as a force group mech
//!   (`setMissionCompleted`, `logisticsdata.cpp:1356-1375`), and gives every pilot that was
//!   there `LogisticsPilot::update` (`logisticsdata.cpp:1396-1402`).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use mc2_campaign::interface::{
    LoadType, MissionOptions, MissionResult, MoverReport, ObjectiveReport, ObjectiveStatus, Outcome,
};
use mc2_campaign::pilot::{vehicle_class, KilledKind, PilotReport};
use mc2_campaign::results::{MissionChoice, ResultsStage};
use mc2_campaign::{GameData, Logistics, MechKey};
use mc2_formats::vfs::Vfs;

/// The campaign walked (`data/campaign/campaign.fit`).
const CAMPAIGN: &str = "campaign";
/// The missions the walk must play: the campaign data's own count (`rust/docs/PLAYABLE-PLAN.md`).
const CAMPAIGN_MISSIONS: usize = 24;
/// The mech the script kills: the 55 ton Bushwacker is a medium mech, `VEHICLE_CLASS_MEDIUMMECH`
/// (`warrior.h`), so a non-air battle mech kill scores `KillSkillValues[1]`.
const MEDIUM: usize = vehicle_class::MEDIUM_MECH;
/// `MWS_PILOTING` and `MWS_GUNNERY`, the `SkillsTable` order (`warrior.cpp:197-201`).
const PILOTING: usize = 0;
const GUNNERY: usize = 2;
/// The pilot that flies every mission and the only one in the force: the first warrior of the
/// first purchase list, `Purchase01.fit`, so the bay offers him from the first mission on.
const PILOT: &str = "pmwchopper";

/// What the pilot's mech costs the bay: the Bushwacker Prime the walk buys (`pilot_mech`), its
/// chassis' `baseCost` 34750 plus its components' 21800 (`logisticsvariant.cpp:493-503`, the
/// prices in `data/objects/bushwacker.csv` and `data/objects/compbas.csv`).
const BUSHWACKER: i64 = 56550;

// ------------------------------------------------------------------------------------------------
// the shipped data

/// A mission's `[MissionSettings]`, as `readMissionInfo` reads them (`logisticsmissioninfo.cpp:300-332`).
#[derive(Debug, Clone, Default, PartialEq)]
struct Settings {
    /// `l ResourcePoints`: the campaign bank the mission starts with (unused here).
    resource_points: i64,
    /// `l AdditionalCBills`: paid into the campaign bank when the mission is selected.
    additional_cbills: i64,
    /// `f DropWeightLimit`: the heaviest drop the mission allows. Absent, `readMissionInfo`
    /// leaves the 300 of `LogisticsMissionInfo::clear` (`logisticsmissioninfo.cpp:312-316`).
    drop_weight: f32,
    /// `st AdditionalMission`: the file of the mission this one hands on to, when it names one.
    /// Only `LogisticsMissionInfo::load` writes the key that names it
    /// (`logisticsmissioninfo.cpp:425-426`); no shipped mission carries one.
    additional_mission: Option<String>,
}

/// One `Group<i>Mission<j>` entry of the shipped campaign file.
#[derive(Debug, Clone)]
struct Mission {
    name: String,
    mandatory: bool,
    hidden: bool,
    /// `b PlayLogistics`, the mission plays through the bay unless the key says it does not
    /// (`logisticsmissioninfo.cpp:231-233`).
    play_logistics: bool,
    /// `b PlaySalvage`, read from the campaign entry (`logisticsmissioninfo.cpp:236-237`).
    play_salvage: bool,
    /// The `ResourcePoints` of the home team's objectives. The scripted mission succeeds the
    /// first of them and fails every other one, which `Mission::GameSystem`:
    /// `TaskForce\Objectives` orders, and the mission files give.
    objectives_rp: Vec<i32>,
    settings: Settings,
}

/// One `Group<i>` of the shipped campaign file.
#[derive(Debug, Clone)]
struct Group {
    /// `NumberToComplete`, `-1` when the key is missing (`logisticsmissioninfo.cpp:130`).
    number_to_complete: i64,
    missions: Vec<Mission>,
}

/// The campaign's `[Campaign]` block: its `GroupCount` and its `l CBills`, the bank the campaign
/// starts with (`LogisticsMissionInfo::init`, `logisticsmissioninfo.cpp`).
fn read_script(vfs: &mut Vfs) -> (i64, i64) {
    let mut fit = vfs
        .read_fit(&format!("data/campaign/{CAMPAIGN}.fit"))
        .expect("data/campaign/campaign.fit is readable")
        .unwrap_or_else(|| panic!("data/campaign/{CAMPAIGN}.fit is in the VFS"));
    assert!(
        fit.seek_block("Campaign").is_ok(),
        "campaign.fit [Campaign]"
    );
    let count = fit.read_i64("GroupCount").expect("GroupCount");
    let cbills = fit.read_i64("CBills").expect("CBills");
    (count, cbills)
}

fn read_campaign(vfs: &mut Vfs) -> Vec<Group> {
    let mut fit = vfs
        .read_fit(&format!("data/campaign/{CAMPAIGN}.fit"))
        .expect("data/campaign/campaign.fit is readable")
        .unwrap_or_else(|| panic!("data/campaign/{CAMPAIGN}.fit is in the VFS"));
    assert!(
        fit.seek_block("Campaign").is_ok(),
        "campaign.fit [Campaign]"
    );
    let count = fit.read_i64("GroupCount").expect("GroupCount");
    let mut groups = Vec::new();
    for i in 0..count {
        let block = format!("Group{i}");
        assert!(fit.seek_block(&block).is_ok(), "campaign.fit [{block}]");
        let number_to_complete = fit.read_i64("NumberToComplete").unwrap_or(-1);
        let mission_count = fit.read_i64("MissionCount").unwrap_or(0);
        let mut missions = Vec::new();
        for j in 0..mission_count {
            let block = format!("Group{i}Mission{j}");
            assert!(fit.seek_block(&block).is_ok(), "campaign.fit [{block}]");
            let name = fit.read_string("FileName", 255).expect("FileName");
            let mandatory = fit.read_bool("Mandatory").unwrap_or(false);
            let hidden = fit.read_bool("Hidden").unwrap_or(false);
            // missing means the bay (`logisticsmissioninfo.cpp:231-233`)
            let play_logistics = fit.read_bool("PlayLogistics").unwrap_or(true);
            let play_salvage = fit.read_bool("PlaySalvage").unwrap_or(true);
            let objectives_rp = common::objectives_of(vfs, &name)
                .iter()
                .map(|o| o.rp)
                .collect();
            let mut mfit = vfs
                .read_fit(&format!("data/missions/{name}.fit"))
                .expect("the mission file is readable")
                .unwrap_or_else(|| panic!("data/missions/{name}.fit is in the VFS"));
            assert!(
                mfit.seek_block("MissionSettings").is_ok(),
                "{name}: [MissionSettings]"
            );
            let settings = Settings {
                resource_points: mfit.read_i64("ResourcePoints").unwrap_or(0),
                additional_mission: mfit
                    .read_string("AdditionalMission", 255)
                    .ok()
                    .map(|s| s.trim_end_matches(".fit").to_ascii_lowercase()),
                // `readMissionInfo` reads it last, out of the `[General]` block it seeks at its
                // end (`logisticsmissioninfo.cpp:262-265`, `:309-310`, `:347`)
                additional_cbills: {
                    mfit.seek_block("General").ok();
                    mfit.read_i64("AdditionalCBills").unwrap_or(0)
                },
                drop_weight: mfit.read_f32("DropWeightLimit").unwrap_or(300.0),
            };
            missions.push(Mission {
                name,
                mandatory,
                hidden,
                play_logistics,
                play_salvage,
                objectives_rp,
                settings,
            });
        }
        groups.push(Group {
            number_to_complete,
            missions,
        });
    }
    groups
}

impl Group {
    /// The missions the group lists to the player: every mission of a group without a
    /// `NumberToComplete` quota, else the missions not hidden until the quota is met
    /// (`LogisticsMissionInfo::load` un-hides the rest, `logisticsmissioninfo.cpp:405-419`;
    /// `setMissionComplete` does the same, `logisticsmissioninfo.cpp:832-857`).
    fn visible_after(&self, completed: &BTreeSet<String>) -> Vec<&Mission> {
        let hidden_until_quota = self.missions.iter().any(|m| m.hidden);
        // a still hidden mission of the group does not show; nothing that comes after it does
        // either, unless it is out of the quota's way
        let open = |hide_hidden: bool| -> Vec<&Mission> {
            let mut hidden = false;
            self.missions
                .iter()
                .filter(|m| {
                    hidden |= hide_hidden && m.hidden;
                    !hidden || !m.mandatory
                })
                .collect()
        };
        if !hidden_until_quota || self.unhidden(completed) {
            return open(false);
        }
        open(true)
    }

    /// The group's quota met, counted over the missions completed (which includes `played`).
    fn quota_met(&self, completed: &BTreeSet<String>) -> bool {
        let done = self
            .missions
            .iter()
            .filter(|m| completed.contains(&m.name))
            .count() as i64;
        // `numberToBeCompleted` is -1 for a group without the key: no quota to meet.
        done >= self.number_to_complete
    }

    /// Whether the group's quota is met, so its hidden missions show.
    fn unhidden(&self, completed: &BTreeSet<String>) -> bool {
        self.quota_met(completed)
    }

    /// Every open mission is one no mandatory mission is left: the group is over.
    fn mandatory_done(&self, completed: &BTreeSet<String>) -> bool {
        self.missions
            .iter()
            .filter(|m| !completed.contains(&m.name))
            .all(|m| !m.mandatory)
    }

    /// The first open mission of the group, `None` when all are done.
    fn first_open(&self, completed: &BTreeSet<String>, unhide: bool) -> Option<&Mission> {
        // a mission still hidden is not open: the C++ sends the player to a listed one
        self.missions
            .iter()
            .find(|m| (unhide || !m.hidden) && !completed.contains(&m.name))
    }

    /// The mission `setMissionComplete` leaves current after `played`, the mission just
    /// completed (`logisticsmissioninfo.cpp:832-868`): the first open mission of the group,
    /// unless the quota is met and every mandatory mission is done, which starts the next group
    /// at its first mission.
    fn next_after<'a>(
        &'a self,
        played: &'a Mission,
        completed: &BTreeSet<String>,
    ) -> Option<&'a Mission> {
        if self.quota_met(completed) && self.mandatory_done(completed) {
            return None;
        }
        self.first_open(completed, self.quota_met(completed))
            .or(Some(played))
    }
}

fn find_group_index(groups: &[Group], mission: &str) -> usize {
    groups
        .iter()
        .position(|g| g.missions.iter().any(|m| m.name == mission))
        .unwrap_or_else(|| panic!("{mission} is a mission of campaign.fit"))
}

fn find_group<'a>(groups: &'a [Group], mission: &str) -> &'a Group {
    groups
        .iter()
        .find(|g| g.missions.iter().any(|m| m.name == mission))
        .unwrap_or_else(|| panic!("{mission} is a mission of campaign.fit"))
}

fn find_mission<'a>(groups: &'a [Group], name: &str) -> &'a Mission {
    find_group(groups, name)
        .missions
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("{name} is a mission of campaign.fit"))
}

/// The `[Skills]` tables of `gamesys.fit`, read by `Mover::initSkills` (`mover.cpp:2147-2164`).
#[derive(Debug, Clone, Copy)]
struct Skills {
    /// `Skill Attempt[4]`: piloting (0), sensors (1), gunnery (2); jumping is `#if 0`-ed out
    /// (`mech.cpp:3551`).
    skill_try: [f32; 4],
    /// `Skill Success[4]`.
    skill_success: [f32; 4],
    /// `KillSkillValues[6]`: light, medium, heavy, assault, vehicle, elemental.
    kill_skill: [f32; 6],
}

fn read_skills(vfs: &mut Vfs) -> Skills {
    let mut fit = vfs
        .read_fit("data/missions/gamesys.fit")
        .expect("gamesys.fit is readable")
        .unwrap_or_else(|| panic!("data/missions/gamesys.fit is in the VFS"));
    assert!(fit.seek_block("Skills").is_ok(), "gamesys.fit [Skills]");
    let four = |fit: &mc2_formats::ini::FitIni, key: &str| -> [f32; 4] {
        let v = fit.read_array::<f32>(key, 4).expect(key);
        [v[0], v[1], v[2], v[3]]
    };
    let six = |fit: &mc2_formats::ini::FitIni, key: &str| -> [f32; 6] {
        let v = fit.read_array::<f32>(key, 6).expect(key);
        [v[0], v[1], v[2], v[3], v[4], v[5]]
    };
    Skills {
        skill_try: four(&fit, "Skill Attempt"),
        skill_success: four(&fit, "Skill Success"),
        kill_skill: six(&fit, "KillSkillValues"),
    }
}

// ------------------------------------------------------------------------------------------------
// the script: what the mission makes the pilot earn

/// The step's mission: the first objective succeeds and every other one fails; the pilot hits
/// `team2_hits` team 2 targets, kills `heavy_kills` heavy mechs, and gets out of
/// `piloting_tries` piloting checks `piloting_hits` times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    /// A careful pilot: nothing scores.
    Quiet,
    /// A busy pilot: nine piloting checks and four passes, three hits, two kills.
    Busy,
    /// A pilot that kills three medium mechs, one per kill class.
    Killer,
}

impl Script {
    /// The script of the pilot's `flown`-th mission: quiet, busy, killer, then the same again.
    fn of(flown: usize) -> Script {
        match flown % 3 {
            0 => Script::Quiet,
            1 => Script::Busy,
            _ => Script::Killer,
        }
    }

    /// `(piloting tries, piloting passes, team 2 shots, hits, medium mech kills)`.
    fn counts(self) -> Counts {
        match self {
            Script::Quiet => Counts::default(),
            Script::Busy => Counts {
                piloting_tries: 9,
                piloting_passes: 4,
                shots: 3,
                hits: 2,
                kills: 2,
            },
            Script::Killer => Counts {
                piloting_tries: 2,
                piloting_passes: 1,
                shots: 1,
                hits: 4,
                kills: 3,
            },
        }
    }

    /// Whether the mission is one of the spot checks (every script is).
    fn name(self) -> &'static str {
        match self {
            Script::Quiet => "quiet",
            Script::Busy => "busy",
            Script::Killer => "killer",
        }
    }

    /// `skillPoints[MWS_GUNNERY]` at the end of the mission. The `Skill` order is piloting,
    /// sensors, gunnery (`warrior.h:183-188`): a shot adds `SkillTry[MWS_GUNNERY]`
    /// (`mech.cpp:8060-8062`), a hit on a team 2 target adds `SkillSuccess[MWS_GUNNERY]`
    /// (`mech.cpp:8076-8079`), and a kill adds `KillSkill[vehicleClass]` (`warrior.cpp:7068-7130`),
    /// where a non-air battle mech is killed in its own weight class (`warrior.cpp:7068-7074`).
    /// The points are whole: the fractional `SkillTry` a shot on a team 1 target adds
    /// (`mech.cpp:8064`) is nothing here, as the shipped values are whole.
    fn gunnery_points(self, s: &Skills) -> f32 {
        let c = self.counts();
        c.shots as f32 * s.skill_try[GUNNERY]
            + c.hits as f32 * s.skill_success[GUNNERY]
            + c.kills as f32 * s.kill_skill[MEDIUM]
    }

    /// Whether the pilot is a passenger: the mission drops no mech for them, so they score
    /// nothing and only collect the free point (`warrior.cpp:7263`). `LogisticsData` only updates,
    /// and so only counts, the pilots of the mechs the mission dropped (`logisticsdata.cpp:1348-1399`).
    fn passenger(self, drops: bool) -> Script {
        if drops {
            self
        } else {
            Script::Quiet
        }
    }

    /// `skillPoints[MWS_PILOTING]`: `SkillTry[MWS_PILOTING]` per check and
    /// `SkillSuccess[MWS_PILOTING]` more when it passes (`mech.cpp:3559-3564`).
    fn piloting_points(self, s: &Skills) -> f32 {
        let c = self.counts();
        c.piloting_tries as f32 * s.skill_try[PILOTING]
            + c.piloting_passes as f32 * s.skill_success[PILOTING]
    }

    /// The kills as `handleKilledTarget` records them (`warrior.cpp:7093-7104`): the campaign
    /// tallies them by class (`logisticspilot.cpp:305-311`).
    fn killed(self) -> Vec<KilledKind> {
        vec![KilledKind::BattleMech { air: false }; self.counts().kills as usize]
    }
}

// ------------------------------------------------------------------------------------------------
// the hand arithmetic, straight from the C++ formulas

/// What one scripted mission makes the pilot do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    piloting_tries: i64,
    piloting_passes: i64,
    shots: i64,
    hits: i64,
    kills: i64,
}

/// The highest skill a pilot reaches (`MaxPilotSkill`, `warrior.cpp:7256` and `:7264`).
const MAX_PILOT_SKILL: f32 = 80.0;
/// The most a pilot gains from one mission (`LogisticsPilot::update`, `logisticspilot.cpp`).
const MAX_GAIN: f32 = 4.0;

/// `MechWarrior::updateMissionSkills` (`warrior.cpp:7252-7269`): gunnery takes the points in
/// hundredths, piloting one point for coming along plus the points, both stop at 80 and the
/// gains are then held to the four points a mission can give (`LogisticsPilot::update`,
/// `logisticspilot.cpp:284-305`).
fn warrior_skills_after(before: (f32, f32), points: (f32, f32)) -> (f32, f32) {
    let (mut gunnery, mut piloting) = (
        (before.0 + points.0 * 0.01).min(MAX_PILOT_SKILL),
        (before.1 + 1.0 + points.1).min(MAX_PILOT_SKILL),
    );
    if piloting - before.1 > MAX_GAIN {
        piloting = before.1 + MAX_GAIN;
    }
    if gunnery - before.0 > MAX_GAIN {
        gunnery = before.0 + MAX_GAIN;
    }
    (gunnery, piloting)
}

/// `LogisticsPilot::update` (`logisticspilot.cpp:255-345`): the skills the campaign keeps and
/// the gains `newPiloting`/`newGunnery` hold, neither gain above four. Piloting is whole, so a
/// piloting gain over four is the four points; a gunnery gain is kept to hundredths and floored
/// at four, and the skill follows the kept gain.
fn logistics_pilot_update(before: (f32, f32), warrior: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    let mut gain = (warrior.0 - before.0, warrior.1 - before.1);
    let mut skills = warrior;
    if gain.1 > MAX_GAIN {
        gain.1 = MAX_GAIN;
        skills.1 = before.1 + MAX_GAIN;
    }
    if gain.0 > MAX_GAIN {
        gain.0 = (gain.0 * 100.0).floor() / 100.0;
        gain.0 = gain.0.min(MAX_GAIN);
        skills.0 = before.0 + gain.0;
    }
    (skills, gain)
}

// ------------------------------------------------------------------------------------------------
// driving the screens

/// How the walk presses at a mission selection screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chooser {
    /// The mission the screen opens with (the campaign's own choice).
    Current,
    /// The first enabled mission that is not the one the screen opens with: a branch.
    Other,
    /// The last enabled mission.
    Last,
}

/// One mech of the force: `(variant, pilot, slot)`.
type ForceEntry = (String, String, i32);

fn force_state(l: &Logistics) -> Vec<ForceEntry> {
    let mut force: Vec<ForceEntry> = l
        .inventory()
        .iter()
        .filter(|m| m.force_group != 0)
        .map(|m| {
            (
                l.variant(m.variant).unwrap().name.clone(),
                m.pilot
                    .map(|p| l.pilots()[p].file_name.clone())
                    .unwrap_or_default(),
                m.force_group,
            )
        })
        .collect();
    force.sort();
    force
}

/// Picks the mission the walk presses on the selection screen the campaign just opened: the
/// walk's choice among the enabled ones.
///
/// The other missions are taken once: `Other` presses one this walk has not flown yet, in the
/// order the screen lists them, so that a walk covers the ones the campaign's own choice skips.
fn press<'a>(choices: &'a [MissionChoice], flown: &BTreeSet<String>, chooser: Chooser) -> &'a str {
    let enabled: Vec<&MissionChoice> = choices.iter().filter(|c| c.available).collect();
    assert!(!enabled.is_empty(), "the screen offers a mission");
    let picked = match chooser {
        Chooser::Current => enabled
            .iter()
            .find(|c| c.selected)
            .or_else(|| enabled.first()),
        Chooser::Other => enabled
            .iter()
            .find(|c| !c.selected && !flown.contains(&c.name))
            .or_else(|| enabled.iter().find(|c| !c.selected))
            .or_else(|| enabled.first()),
        Chooser::Last => enabled.last(),
    }
    .map(|c| c.name.as_str())
    .unwrap();
    picked
}

/// Puts the pilot's mech in the force, buying the pilot a mech on the first step. The pilot
/// flies a 55 ton Prime: a medium mech (`VEHICLE_CLASS_MEDIUMMECH`), the class the script kills.
/// Returns the mech and whether it drops: a mech the drop weight limit does not leave room for
/// stays in the bay (`canAddMechToForceGroup`, `logisticsdata.cpp:1859-1895`).
fn pilot_mech(l: &mut Logistics, pilot: usize, step: usize) -> (MechKey, bool) {
    let key = match l.inventory().iter().find(|m| m.pilot == Some(pilot)) {
        Some(m) => m.key,
        None => {
            let pick = l
                .purchasable_mechs()
                .into_iter()
                .filter(|&v| {
                    l.variant_chassis(v).is_some_and(|c| c.max_weight == 55.0)
                        && l.variant(v).unwrap().name.contains("Prime")
                })
                .min_by_key(|&v| l.variant_cost(v).unwrap())
                .unwrap_or_else(|| panic!("step {step}: a 55 ton mech is on sale"));
            let key = l.purchase_mech(pick).expect("the pilot's mech is bought");
            l.set_mech_pilot(key, Some(pilot));
            key
        }
    };
    let mech = l.mech(key).expect("the mech is in the bay");
    assert_eq!(
        l.variant_chassis(mech.variant).unwrap().max_weight,
        55.0,
        "step {step}: the pilot flies a medium mech"
    );
    if mech.force_group != 0 {
        return (key, true);
    }
    if !l.can_add_mech_to_force_group(key) {
        return (key, false);
    }
    let slot = (1..=mc2_campaign::logistics::MAX_FORCE_GROUP_SLOT)
        .find(|s| l.inventory().iter().all(|m| m.force_group != *s))
        .expect("a free force group slot");
    l.add_mech_to_force_group(key, slot)
        .expect("the mech deploys");
    (key, true)
}

/// The scripted mission: the force comes out of it undamaged, the first objective succeeds and
/// the others fail, and the pilot earns what the script says. `mechs` is the lance that flew;
/// the pilot of `mech`, the flag mech, earns the points, the other pilots fly as `Quiet`.
fn outcome_of(
    l: &Logistics,
    mission: &Mission,
    mechs: &[MechKey],
    mech: MechKey,
    script: Script,
    points: (f32, f32),
) -> Outcome {
    let mut movers = Vec::new();
    for m in l.inventory().iter().filter(|m| mechs.contains(&m.key)) {
        let p = m.pilot.expect("every mech in the force flies with a pilot");
        let pilot = &l.pilots()[p];
        let mut report = PilotReport {
            // `loadMoverSkills` copies the roster's `getGunnery()`/`getPiloting()`
            // (`mover.cpp:3398-3399`), the pilot's last `MechWarrior::skillRank`, saved rounded to
            // the hundredth (`mechwarrior.cpp:4962-4963`). The bay keeps that saved value, not the
            // `f32` sum `LogisticsPilot::update` arrived at (`logisticspilot.cpp:286-290`), so the
            // next mission starts from the rounded one.
            gunnery_rank: round_to_hundredth(pilot.gunnery),
            piloting_rank: round_to_hundredth(pilot.piloting),
            rank: pilot.rank,
            ..PilotReport::default()
        };
        if m.key == mech {
            report.gunnery_points = points.0;
            report.piloting_points = points.1;
            report.killed = script.killed();
        }
        movers.push(MoverReport {
            on_gui: true,
            is_battle_mech: true,
            home_commander: true,
            is_air: false,
            destroyed: false,
            disabled: false,
            variant_name: l.variant(m.variant).unwrap().name.clone(),
            pilot_file: Some(pilot.file_name.clone()),
            paint_scheme: [m.colors[1], m.colors[2], m.colors[0]],
            pilot: report,
        });
    }
    let objectives = mission
        .objectives_rp
        .iter()
        .enumerate()
        .map(|(i, &rp)| ObjectiveReport {
            status: if i == 0 {
                ObjectiveStatus::Successful
            } else {
                ObjectiveStatus::Failed
            },
            resource_points: rp,
        })
        .collect();
    Outcome {
        result: MissionResult::PlayerWinBig,
        objectives,
        movers,
        // `SalvageMechScreen::init` scans every mission mech and offers an intact disabled
        // ground mech (`salvagemecharea.cpp:155`). Use a real campaign variant so the public
        // results API must build and process an offer on each non-final step.
        mechs: {
            let (key, variant) = l
                .variants()
                .min_by_key(|(key, _)| l.variant_cost(*key).unwrap())
                .expect("the campaign has a salvageable variant");
            vec![mc2_campaign::interface::MechSummary::wreck(
                &variant.name,
                l.variant_chassis(key).unwrap().max_weight,
            )]
        },
        ..Outcome::won()
    }
}

/// The pilot's `getGunnery`/`getPiloting` as the save rounds them: `floorf(x * 100.f + .5f) / 100.f`
/// (`mechwarrior.cpp:4962-4963`), read back by `savedMechWarrior` (`:1196-1209`).
fn round_to_hundredth(x: f32) -> f32 {
    (x * 100.0 + 0.5).floor() / 100.0
}

/// The first objective's `ResourcePoints`: the only one the script lets succeed.
fn first_rp(mission: &Mission) -> i32 {
    mission.objectives_rp.first().copied().unwrap_or(0)
}

/// The missions a mission's win hands on to, in order: the `AdditionalMission` chain, following
/// `setNextMission` (`logisticsmissioninfo.cpp:767-783`) the way `Mission::GameSystem` ends the
/// mission into its additional mission (`LogisticsMissionInfo::load` names the key,
/// `logisticsmissioninfo.cpp:425-426`). A win completes a mission that hands on to another
/// without completing the campaign, so the campaign index walks the chain
/// (`setMissionComplete`, `logisticsmissioninfo.cpp:785-870`). No shipped mission names one.
fn mission_chain<'a>(groups: &'a [Group], mission: &'a Mission) -> Vec<&'a Mission> {
    let mut chain = Vec::new();
    let mut next = mission.settings.additional_mission.as_deref();
    while let Some(name) = next {
        assert!(
            chain.len() < 32,
            "{name}: the additional missions do not chain in a circle"
        );
        let m = find_mission(groups, name);
        assert!(
            !std::ptr::eq(m, mission),
            "{name}: the additional missions do not come back to where they started"
        );
        chain.push(m);
        next = m.settings.additional_mission.as_deref();
    }
    chain
}

/// What the walk worked out for one mission: the pilot's skills and gains, and the C-bills the
/// mission pays.
#[derive(Debug, Clone, Copy)]
struct Expected {
    /// The pilot's skills before the mission.
    before: (f32, f32),
    /// The points the mission's report gave the pilot, gunnery and piloting, as the report
    /// itself says them.
    points: (f32, f32),
    script: Script,
    warrior: (f32, f32),
    keeper: ((f32, f32), (f32, f32)),
    /** Its payout before salvage. */
    paid: i64,
    /** Whether its win completes the mission the campaign holds (it hands nothing on to). */
    keeps: bool,
    /** What the selection screen's press of the mission moved the bank by. */
    pressed: i64,
}

/// What one step of the walk did.
#[derive(Debug, Clone)]
struct Step {
    mission: String,
    cbills: (i64, i64),
    salvage: i64,
    force_kept: bool,
    /// What the bay charged before the mission besides the selection screen's press: the pilot's
    /// mech.
    extra: i64,
    expected: Option<Box<Expected>>,
}

#[derive(Debug, Default)]
struct Walk {
    steps: Vec<Step>,
    log: String,
    /// The step and mission the campaign ended on.
    completed_at: Option<(usize, String)>,
    /// The missions this walk has pressed on the selection screen.
    flown: BTreeSet<String>,
}

/// Runs a campaign from the first mission to the campaign end, going through every screen the
/// game goes through, and checking what the campaign does against the data and the formulas.
///
/// `on_step` is called after every step, with the campaign and the text of its save.
fn walk(
    data: &Arc<GameData>,
    groups: &[Group],
    skills: &Skills,
    start_cbills: i64,
    route: Chooser,
    mut on_step: impl FnMut(&Logistics, &str, &Step) -> Logistics,
) -> Walk {
    // the campaign's own copy of its missions, to check the walk's reading of the file
    let campaign = data.campaigns[CAMPAIGN].clone();
    assert_eq!(
        i64::try_from(groups.len()).unwrap(),
        campaign.group_count,
        "campaign.fit lists the groups the data has"
    );

    let mut l = Logistics::with_campaign(Arc::clone(data), CAMPAIGN).unwrap();
    // nothing is chosen yet, so the bank is what the campaign's own start left
    assert_eq!(
        l.c_bills(),
        start_cbills,
        "the campaign starts with its CBills, bank {}",
        l.c_bills()
    );
    let mut out = Walk::default();
    let mut done: BTreeSet<String> = BTreeSet::new();
    // `startNewCampaign` selects the first available mission and pays nothing for it
    // (`logisticsdata.cpp:2272-2302`; `setCurrentMission` pays only the missions it replaces,
    // `logisticsmissioninfo.cpp:664-673`), and the campaign is built the same way; the selection
    // screen then opens on it without pressing it (`MissionSelectionScreen::begin`,
    // `missionselectionscreen.cpp:274-296`)
    assert_eq!(
        l.current_mission(),
        groups[0]
            .missions
            .first()
            .map(|m| m.name.as_str())
            .unwrap_or(""),
        "the campaign starts on the first mission of its first group"
    );
    let mut choices = l.mission_choices();
    assert_eq!(
        choices.iter().find(|c| c.selected).map(|c| c.name.as_str()),
        groups[0].missions.first().map(|m| m.name.as_str()),
        "the screen opens on the first mission of the first group"
    );

    while !l.campaign_over() {
        let step = out.steps.len();
        let name = press(&choices, &out.flown, route).to_string();
        // The press gives back the `AdditionalCBills` of the mission the campaign has current, if
        // that one is not completed, before it pays the pressed one (`logisticsmissioninfo.cpp:664-673`).
        // The screen opens with the current mission selected (`MissionSelectionScreen::begin`,
        // `missionselectionscreen.cpp:274-296`), so a press on anything else replaces it, and one
        // on the selected mission replaces itself.
        let replacing = if choices.iter().any(|c| c.name == name && c.selected) {
            None
        } else {
            Some(l.current_mission().to_string())
        };
        let before_press = l.c_bills();
        l.choose_mission(&name).expect("the mission is pressed");
        out.flown.insert(name.clone());
        // what the press moved the bank by
        let moved = l.c_bills() - before_press;
        let mission = find_mission(groups, &name);
        let group = find_group(groups, &name);
        assert!(!done.contains(&name), "step {step}: {name} is open");
        // what the press moved the bank by: it gives back the mission of the group it is in that
        // the campaign had current, if that one is not completed, and pays the one pressed
        // (`logisticsmissioninfo.cpp:664-673`); every shipped mission has 0, so it moves nothing
        let previous = replacing
            .as_deref()
            .and_then(|r| group.missions.iter().find(|m| m.name == r))
            .filter(|m| !done.contains(&m.name));
        let press_by = mission.settings.additional_cbills
            - previous.map_or(0, |m| m.settings.additional_cbills);
        // the screen opens before the press (`MissionSelectionScreen::begin`), on the bank the
        // last step left
        let at_screen = before_press;
        assert_eq!(
            moved, press_by,
            "step {step}: the press of {name} moves the bank"
        );

        // ---- the screen lists the group's visible missions, open ones enabled, this one
        // pressed (`MissionSelectionScreen::begin`, `results.rs`)
        let listed: Vec<&str> = choices.iter().map(|c| c.name.as_str()).collect();
        let listed_want: Vec<&str> = group
            .visible_after(&done)
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(
            listed, listed_want,
            "step {step}: the screen of {}'s group",
            name
        );
        // `MissionSelectionScreen::begin` leaves a mission open on the screen unless it is done
        // (`missionselectionscreen.cpp:289`; `getMissionAvailable`, `logisticsmissioninfo.cpp:949`)
        for c in &choices {
            assert_eq!(
                c.available,
                !done.contains(&c.name),
                "step {step}: {} open on the screen",
                c.name
            );
        }
        // the screen is pressed on a mission it enables; the campaign's own pick is `selected`
        assert!(
            choices.iter().any(|c| c.name == name && c.available),
            "step {step}: {name} is pressed"
        );
        assert!(
            choices.iter().any(|c| c.selected),
            "step {step}: the screen starts on a mission"
        );

        // ---- the selection set the mission up (`setCurrentMission`)
        assert_eq!(l.current_mission(), name);
        assert_eq!(
            l.max_drop_weight(),
            mission.settings.drop_weight as i64,
            "step {step}: {name} drop weight"
        );
        assert_eq!(
            l.resource_points(),
            mission.settings.resource_points,
            "step {step}: {name} resource points"
        );
        // the bank the step accounts for, and the screens the mission plays with: the bay
        // unless the mission's own `PlayLogistics` says `skipLogistics`
        // (`logisticsmissioninfo.cpp:1096-1105`)
        let cbills = l.c_bills();
        assert_eq!(
            l.skip_logistics(),
            !mission.play_logistics,
            "step {step}: {name} plays through the bay"
        );
        if l.skip_logistics() {
            assert_eq!(
                l.load_spec(MissionOptions::default()).load_type,
                LoadType::SpQuickstart,
                "step {step}: {name} loads the game's own lance"
            );
            // the mission's own lance flew: it cost the bay nothing and brought nothing home
            let no_mechs: [MechKey; 0] = [];
            let won: Outcome = outcome_of(&l, mission, &no_mechs, 0, Script::Quiet, (0.0, 0.0));
            l.apply_outcome(&won).unwrap();
            let mut flow = l.begin_results(&won);
            if mission.play_salvage {
                assert_eq!(
                    flow.stage(),
                    ResultsStage::Salvage,
                    "step {step}: {name} opens the data-enabled salvage screen"
                );
                assert!(
                    !flow.offers().is_empty(),
                    "step {step}: {name} offers salvage"
                );
                flow.finish_salvage(&mut l);
            } else {
                assert_eq!(
                    flow.stage(),
                    ResultsStage::PilotReview,
                    "step {step}: {name} skips salvage as campaign.fit says"
                );
            }
            if flow.stage() == ResultsStage::PilotReview {
                while let Some(pilot) = flow.pending_specialty(&l) {
                    let skill = l.pilots()[pilot].specialty_offers()[0];
                    flow.choose_specialty(&mut l, pilot, skill).unwrap();
                }
                flow.finish_pilot_review(&mut l);
            }
            assert!(flow.is_done(), "step {step}: quickstart results finish");
            let autosave = flow.end(&l);
            assert_eq!(
                autosave.is_some(),
                !l.campaign_over(),
                "step {step}: {name} autosave decision"
            );
            if let Some(request) = &autosave {
                assert_eq!(
                    request.mission(),
                    name,
                    "step {step}: autosave names the mission"
                );
            }
            // the win still checks a scripted outcome against nothing: the mission pays its own
            // `AdditionalCBills` and objectives, and no pilot of the bay gains
            let quiet = Expected {
                before: (0.0, 0.0),
                points: (0.0, 0.0),
                script: Script::Quiet,
                warrior: (0.0, 0.0),
                keeper: ((0.0, 0.0), (0.0, 0.0)),
                paid: mission.settings.additional_cbills + i64::from(first_rp(mission)),
                keeps: true,
                pressed: 0,
            };
            assert_eq!(
                l.c_bills(),
                cbills + quiet.paid,
                "step {step}: {name} pays without the bay"
            );
            done.insert(name.clone());
            let expected_next = group
                .next_after(mission, &done)
                .map(|next| next.name.clone())
                .or_else(|| {
                    groups
                        .get(find_group_index(groups, &name) + 1)
                        .and_then(|next_group| next_group.missions.first())
                        .map(|next| next.name.clone())
                });
            if let Some(expected_next) = expected_next.as_deref() {
                assert_eq!(
                    l.current_mission(),
                    expected_next,
                    "step {step}: quickstart win sets campaign.fit next mission before save"
                );
            } else {
                assert!(
                    l.campaign_over(),
                    "step {step}: quickstart finishes the campaign"
                );
            }
            out.steps.push(Step {
                mission: name.clone(),
                cbills: (cbills, l.c_bills()),
                salvage: 0,
                force_kept: true,
                extra: 0,
                expected: Some(Box::new(quiet)),
            });
            let text = l.save_to_string().expect("the campaign saves");
            let step_data = out.steps.last().cloned().unwrap();
            l = on_step(&l, &text, &step_data);
            if l.campaign_over() {
                out.completed_at = Some((step, name));
                break;
            }
            choices = l.begin_mission_selection().unwrap();
            assert_eq!(
                l.current_mission(),
                expected_next.unwrap(),
                "step {step}: quickstart reload preserves campaign.fit next mission"
            );
            let next_group = find_group(groups, l.current_mission());
            assert_eq!(
                next_group
                    .visible_after(&done)
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>(),
                choices
                    .iter()
                    .filter(|c| c.available)
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>(),
                "step {step}: quickstart selection choices match campaign.fit"
            );
            continue;
        }

        // ---- the pilot flies
        let pilot = l.pilot_index(PILOT).expect("the pilot is on the roster");
        assert!(
            l.pilots()[pilot].available,
            "step {step}: {name} lists {PILOT}"
        );
        // the mech is bought only the first time; the purchased mech stays in the bay and is
        // flown again while the campaign keeps it, so the bay changes the bank once
        let had_mech = l.inventory().iter().any(|m| m.pilot == Some(pilot));
        let (mech, drops) = pilot_mech(&mut l, pilot, step);
        let bank = if had_mech {
            cbills
        } else {
            cbills
                - i64::from(
                    l.variant_cost(l.mech(mech).expect("the mech is in the bay").variant)
                        .expect("the mech's variant is priced"),
                )
        };
        assert_eq!(
            l.c_bills(),
            bank,
            "step {step}: buying the mech is the only purchase"
        );
        let cbills = bank;
        let force: Vec<usize> = l
            .inventory()
            .iter()
            .filter(|m| m.force_group != 0)
            .filter_map(|m| m.pilot)
            .collect();
        assert_eq!(
            force.contains(&pilot),
            drops,
            "step {step}: the pilot flies when the bay finds room for the mech"
        );

        // ---- what the mission makes the pilot earn
        let script = Script::of(step).passenger(drops);
        let points = (
            script.gunnery_points(skills),
            script.piloting_points(skills),
        );
        let before = {
            let p = &l.pilots()[pilot];
            (p.gunnery, p.piloting)
        };
        let warrior = warrior_skills_after(before, points);
        // a mission that hands on to another does not complete the campaign: the index follows
        // the in-mission chain (`logisticsmissioninfo.cpp:767-783`, `:425-426`)
        let chain = mission_chain(groups, mission);
        let expected = Expected {
            before,
            points,
            script,
            warrior,
            keeper: logistics_pilot_update(before, warrior),
            paid: mission.settings.additional_cbills
                + chain
                    .iter()
                    .map(|m| m.settings.additional_cbills)
                    .sum::<i64>()
                + i64::from(first_rp(mission)),
            keeps: mission.settings.additional_mission.is_none(),
            pressed: 0,
        };

        // ---- the mission
        let spec = l.load_spec(MissionOptions::default());
        assert_eq!(
            spec.load_type,
            LoadType::SpLogistics,
            "step {step}: {name} loads the force group"
        );
        assert_eq!(spec.mission, name, "step {step}: the mission handed over");
        assert_eq!(
            spec.force.len(),
            force.len(),
            "step {step}: the force set up flies"
        );
        let flown: Vec<MechKey> = l
            .inventory()
            .iter()
            .filter(|m| m.force_group != 0)
            .map(|m| m.key)
            .collect();
        let force_before = force_state(&l);
        let mut salvage = 0i64;
        let outcome = outcome_of(&l, mission, &flown, mech, script, points);
        l.apply_outcome(&outcome)
            .expect("the win completes the mission");
        let inventory_before = l.inventory().len();

        // ---- the results begin at once (`logistics.cpp:138-141`); the last mission ends the
        // campaign instead, so its results never start (`logistics.cpp:146-152`); every other win
        // runs the salvage screen and the pilot review (`missionresults.cpp:62-96`)
        let mut flow = l.begin_results(&outcome);
        if !l.campaign_over() {
            if mission.play_salvage {
                assert_eq!(
                    flow.stage(),
                    ResultsStage::Salvage,
                    "step {step}: {name} opens the data-enabled salvage screen"
                );
                assert!(
                    !flow.offers().is_empty(),
                    "step {step}: {name} offers salvage"
                );
                // `SalvageListItem::init` charges .8 * getCost (`salvagemecharea.cpp:403`),
                // where `LogisticsVariant::getCost` is chassis base cost plus each component
                // (`logisticsvariant.cpp:493-503`). Recompute that formula here, independently of
                // `Logistics::salvage_offers`, then make the data-driven deterministic choice.
                let offer = &flow.offers()[0];
                let offer_variant = offer.variant;
                let offer_cost = offer.cost;
                let variant = l.variant(offer_variant).expect("salvage variant exists");
                let chassis = l
                    .variant_chassis(offer_variant)
                    .expect("salvage chassis exists");
                let ctx = l.ctx_for(variant);
                let cost = chassis.base_cost as i32
                    + variant
                        .components
                        .iter()
                        .map(|part| {
                            ctx.catalog
                                .get(part.component)
                                .expect("component exists")
                                .cost
                        })
                        .sum::<i32>();
                let exact_cost = 0.8f32 * cost as f32;
                assert_eq!(
                    offer.exact_cost, exact_cost,
                    "step {step}: salvage price formula"
                );
                assert_eq!(offer_cost, exact_cost as i32, "step {step}: salvage charge");
                assert!(
                    exact_cost <= l.c_bills() as f32,
                    "step {step}: deterministic salvage offer is affordable"
                );
                assert!(
                    flow.offer_enabled(&l, 0),
                    "step {step}: salvage offer is enabled"
                );
                let take_salvage = name == "mc2_05";
                if take_salvage {
                    let before_salvage = l.c_bills();
                    assert!(flow.toggle_salvage(&mut l, 0).unwrap());
                    salvage = i64::from(offer_cost);
                    assert_eq!(
                        l.c_bills(),
                        before_salvage - salvage,
                        "step {step}: ticking salvage charges the displayed amount"
                    );
                }
                let ticked = flow.ticked().len();
                let added = flow.finish_salvage(&mut l);
                assert_eq!(
                    added.len(),
                    ticked,
                    "step {step}: the ticked mechs join the bay"
                );
                assert_eq!(
                    added.len(),
                    usize::from(take_salvage),
                    "step {step}: salvage choice is deterministic"
                );
                assert_eq!(
                    l.inventory().len(),
                    inventory_before + added.len(),
                    "step {step}: salvage choice controls the bay inventory"
                );
                if take_salvage {
                    assert!(
                        l.inventory()
                            .iter()
                            .any(|m| m.variant == offer_variant && m.force_group == 0),
                        "step {step}: the selected salvage joins the bay"
                    );
                } else {
                    assert_eq!(
                        l.inventory().len(),
                        inventory_before,
                        "step {step}: unticked salvage does not change the bay"
                    );
                }
            } else {
                assert_eq!(
                    flow.stage(),
                    ResultsStage::PilotReview,
                    "step {step}: {name} skips salvage as campaign.fit says"
                );
                assert!(
                    flow.offers().is_empty(),
                    "step {step}: skipped salvage has no offers"
                );
            }
        } else {
            assert_eq!(
                flow.stage(),
                ResultsStage::Done,
                "step {step}: the campaign end skips the results screens"
            );
        }
        // ---- C-bills: the mission's `AdditionalCBills` (`setMissionComplete`) and its
        // successful objectives (`setMissionCompleted`), less the salvage
        assert_eq!(
            l.c_bills(),
            cbills - moved + expected.paid - salvage,
            "step {step}: {name} pays {} less {salvage} of salvage (from {cbills} before the \
             press, which moved {moved}, now {})",
            expected.paid,
            l.c_bills()
        );

        // ---- the pilot review shows the gains the formulas give
        if !l.campaign_over() && !l.skip_logistics() {
            assert_eq!(
                flow.stage(),
                ResultsStage::PilotReview,
                "step {step}: {name} enters the pilot review after salvage"
            );
            let review = flow.pilot_review().expect("the pilot review runs");
            let shown = review
                .active
                .iter()
                .find(|r| r.pilot == pilot)
                .unwrap_or_else(|| panic!("step {step}: the pilot is reviewed"));
            assert_eq!(shown.gain, expected.keeper.1, "step {step}: {name} gains");
            while let Some(p) = flow.pending_specialty(&l) {
                let skill = l.pilots()[p].specialty_offers()[0];
                flow.choose_specialty(&mut l, p, skill).unwrap();
            }
            flow.finish_pilot_review(&mut l);
            assert!(flow.is_done(), "step {step}: the results are done");
        } else if flow.pilot_review().is_some() {
            while let Some(p) = flow.pending_specialty(&l) {
                let skill = l.pilots()[p].specialty_offers()[0];
                flow.choose_specialty(&mut l, p, skill).unwrap();
            }
            flow.finish_pilot_review(&mut l);
        }

        // no screen follows the last mission's win, so its pilot review is never shown
        // (`logistics.cpp:146-152`)
        // ---- the pilot after the mission
        let p = &l.pilots()[pilot];
        assert_eq!(
            (p.gunnery, p.piloting),
            expected.keeper.0,
            "step {step}: {name} raises the pilot as `LogisticsPilot::update` keeps the gain"
        );
        assert_eq!((p.new_gunnery, p.new_piloting), expected.keeper.1);
        assert_eq!(
            p.missions_completed as usize, step,
            "step {step}: {name} updates the pilot once"
        );
        assert_eq!(
            force_state(&l),
            force_before,
            "step {step}: the selected force carries over intact"
        );

        // `MissionResults::end` decides whether to write the autosave after every mission's
        // results screens (`missionresults.cpp:117-127`). The campaign-over win has no following
        // results flow (`logistics.cpp:146-152`); every earlier campaign win returns to logistics.
        let autosave = flow.end(&l);
        assert_eq!(
            autosave.is_some(),
            !l.campaign_over(),
            "step {step}: {name} autosave decision"
        );
        if let Some(request) = &autosave {
            assert_eq!(
                request.mission(),
                name,
                "step {step}: autosave names the mission"
            );
        }

        done.insert(name.clone());
        // the game has no screen after the win of the last mission (`logistics.cpp:146-152`), and
        // a mission that hands on to another completes only that one (`logisticsmissioninfo.cpp:767-783`)
        let ends = l.campaign_over() || !expected.keeps;
        let _ = writeln!(
            out.log,
            "{step:>2} {name:<6} {:<8} gunnery {:.2}+{:.2}->{:.2} piloting {:.2}+{:.0}->{:.2} \
             cbills {cbills:>6}+{}->{}",
            script.name(),
            before.0,
            points.0,
            p.gunnery,
            before.1,
            points.1,
            p.piloting,
            expected.paid - salvage,
            l.c_bills()
        );
        out.steps.push(Step {
            mission: name.clone(),
            cbills: (cbills, l.c_bills()),
            salvage,
            extra: at_screen,
            force_kept: force
                .iter()
                .all(|&p| l.pilots()[p].used && !l.pilots()[p].dead),
            expected: Some(Box::new(Expected {
                pressed: moved,
                ..expected
            })),
        });
        if ends {
            out.completed_at = Some((step, name.clone()));
            // the save the game writes before it plays its final video (`logistics.cpp:151-160`)
            let text = l.save_to_string().expect("the campaign saves");
            let step_data = out.steps.last().cloned().unwrap();
            l = on_step(&l, &text, &step_data);
            assert!(
                l.campaign_over() || !expected.keeps,
                "step {step}: the save reloads at the campaign's end"
            );
            // and the screen after it: a campaign over lists no mission, and one that goes on
            // lists the next mission of the chain
            let listed = l.begin_mission_selection().unwrap();
            if l.campaign_over() {
                assert_eq!(listed, Vec::new());
                assert_eq!(
                    l.current_mission(),
                    name,
                    "step {step}: the campaign over, no mission is set up anew"
                );
            } else {
                assert_eq!(
                    l.current_mission(),
                    mission_chain(groups, mission)[0].name,
                    "step {step}: {name} hands on to the mission its win names"
                );
            }
            break;
        }
        if !group.quota_met(&done) {
            // the operation goes on, another of its missions still has to be flown
            let next_in_group = group
                .next_after(mission, &done)
                .expect("the unfinished operation has another mission")
                .name
                .clone();
            assert_eq!(
                l.current_mission(),
                next_in_group,
                "step {step}: campaign.fit names the next mission before save"
            );
            let text = l.save_to_string().expect("the campaign saves");
            let step_data = out.steps.last().cloned().unwrap();
            l = on_step(&l, &text, &step_data);
            choices = l.begin_mission_selection().unwrap();
            assert_eq!(
                l.current_mission(),
                next_in_group,
                "step {step}: save preserves campaign.fit next mission while quota is open"
            );
            assert_eq!(
                l.mission_info().current_stage,
                i64::try_from(find_group_index(groups, &name)).unwrap(),
                "step {step}: {name} keeps its operation the current one"
            );
            let offered: Vec<&str> = choices
                .iter()
                .filter(|c| c.available)
                .map(|c| c.name.as_str())
                .collect();
            let expected: Vec<&str> = group
                .visible_after(&done)
                .into_iter()
                .filter(|m| !done.contains(&m.name))
                .map(|m| m.name.as_str())
                .collect();
            assert_eq!(
                offered, expected,
                "step {step}: next mission choices in the current group match campaign.fit"
            );
            continue;
        }
        // the group's quota is met and every mandatory mission flown: `setMissionComplete`
        // moves the campaign on (`logisticsmissioninfo.cpp:832-868`); a group whose mandatory
        // mission is hidden has its quota met before it is flown
        assert!(
            group.mandatory_done(&done) || group.unhidden(&done),
            "step {step}: {name} leaves no mandatory mission open"
        );

        // ---- where the mission leaves the campaign
        // the autosave decision above is present on each mission that returns to logistics.
        // `setMissionComplete` names the next mission, and `setCurrentMission` makes it current
        // by name before the screen lets the player pick (`missionselectionscreen.cpp`)
        let next = match group.next_after(mission, &done) {
            Some(next) => next.name.clone(),
            None => groups[find_group_index(groups, &name) + 1].missions[0]
                .name
                .clone(),
        };
        assert_eq!(
            l.current_mission(),
            next,
            "step {step}: {name} hands over to the mission campaign.fit names next"
        );

        // ---- the save, and the campaign carried on from it
        let text = l.save_to_string().expect("the campaign saves");
        let step_data = out.steps.last().cloned().unwrap();
        l = on_step(&l, &text, &step_data);
        choices = l.begin_mission_selection().unwrap();
        assert_eq!(
            l.current_mission(),
            next,
            "step {step}: save preserves the next mission named by campaign.fit"
        );
        let next_group = find_group(groups, &next);
        let offered: Vec<&str> = choices
            .iter()
            .filter(|c| c.available)
            .map(|c| c.name.as_str())
            .collect();
        let expected: Vec<&str> = next_group
            .visible_after(&done)
            .into_iter()
            .filter(|m| !done.contains(&m.name))
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(
            offered, expected,
            "step {step}: next mission group choices match campaign.fit"
        );
    }
    assert!(l.campaign_over(), "the walk ends at the campaign's end");
    // walking the campaign's own choices plays every mission of it; another route plays the
    // ones it takes to fill each group's quota
    if route == Chooser::Current {
        assert_eq!(
            out.steps.len(),
            CAMPAIGN_MISSIONS,
            "every mission of the campaign"
        );
    } else {
        assert!(!out.steps.is_empty() && out.steps.len() <= CAMPAIGN_MISSIONS);
    }
    out
}

// ------------------------------------------------------------------------------------------------
// the tests

fn data() -> Option<(Vfs, Arc<GameData>)> {
    let vfs = common::vfs()?;
    let data = common::game()?;
    Some((vfs, data))
}

/// Checks the walk's own reading of `campaign.fit` against the parsed campaign: the walk must
/// be checking the campaign the game runs.
fn check_reading(vfs: &mut Vfs, data: &Arc<GameData>, groups: &[Group]) {
    let campaign = &data.campaigns[CAMPAIGN];
    assert_eq!(i64::try_from(groups.len()).unwrap(), campaign.group_count);
    let mut listed = 0;
    for (g, group) in groups.iter().zip(&campaign.groups) {
        assert_eq!(group.number_to_complete, g.number_to_complete);
        // the mission files say what the campaign data holds: the same settings, the same flags
        for (m, entry) in g.missions.iter().zip(&group.missions) {
            assert_eq!(m.name, entry.file_name);
            assert_eq!(m.mandatory, entry.mandatory);
            assert_eq!(m.hidden, entry.hidden);
            assert_eq!(
                m.play_salvage, entry.play_salvage,
                "{}: PlaySalvage",
                m.name
            );
            let s = entry
                .settings
                .as_ref()
                .expect("the mission file is in the data");
            assert_eq!(
                m.settings.resource_points,
                s.resource_points.unwrap_or(0),
                "{}",
                m.name
            );
            assert_eq!(
                m.settings.additional_cbills,
                s.additional_cbills.unwrap_or(0),
                "{}",
                m.name
            );
            assert_eq!(
                m.settings.drop_weight, s.drop_weight_limit as f32,
                "{}",
                m.name
            );
            // the objectives, again from the mission file
            let objectives = common::objectives_of(vfs, &m.name);
            assert_eq!(m.objectives_rp.len(), objectives.len(), "{}", m.name);
        }
        listed += group.missions.len();
    }
    assert_eq!(
        listed, CAMPAIGN_MISSIONS,
        "campaign.fit lists the campaign's missions"
    );
}

/// The whole campaign, from the first mission to the end: every mission is played, the campaign
/// ends where its data ends it, and at every step the C-bills and the pilot agree with the
/// formulas.
#[test]
fn the_campaign_walks_from_its_first_mission_to_its_end() {
    let Some((mut vfs, data)) = data() else {
        eprintln!("MC2_DATA unset: skipping");
        return;
    };
    let (group_count, start_cbills) = read_script(&mut vfs);
    let groups = read_campaign(&mut vfs);
    check_reading(&mut vfs, &data, &groups);
    assert_eq!(start_cbills, data.campaigns[CAMPAIGN].cbills);
    assert_eq!(
        i64::try_from(groups.len()).unwrap(),
        group_count,
        "campaign.fit's GroupCount is the number of groups it lists"
    );
    let skills = read_skills(&mut vfs);

    let mut reloads = 0usize;
    let walk = walk(
        &data,
        &groups,
        &skills,
        start_cbills,
        Chooser::Current,
        |l, text, step| {
            reloads += 1;
            // the save restores exactly the campaign it saved
            let loaded = Logistics::load_from_str(Arc::clone(&data), text).expect("the save loads");
            assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
            assert_eq!(
                loaded.logistics.to_save(),
                l.to_save(),
                "{}: the save restores the campaign",
                step.mission
            );
            loaded.logistics
        },
    );
    eprintln!("{}", walk.log);

    // the walk played the campaign's missions, each exactly once
    let played: Vec<&str> = walk.steps.iter().map(|s| s.mission.as_str()).collect();
    assert_eq!(played.len(), CAMPAIGN_MISSIONS);
    let unique: BTreeSet<&str> = played.iter().copied().collect();
    assert_eq!(unique.len(), CAMPAIGN_MISSIONS, "no mission twice");
    let all: BTreeSet<&str> = groups
        .iter()
        .flat_map(|g| g.missions.iter().map(|m| m.name.as_str()))
        .collect();
    assert_eq!(unique, all, "every mission of campaign.fit is played");

    // the campaign ended where its data says: the last group's last mission, the one it
    // completes with, is the one the walk stopped at
    let (step, last) = walk
        .completed_at
        .as_ref()
        .expect("the walk ends the campaign");
    assert_eq!(*step, CAMPAIGN_MISSIONS - 1);
    let last_group = groups.last().expect("the campaign has a last group");
    assert_eq!(
        last_group.missions.last().map(|m| m.name.as_str()),
        Some(last.as_str()),
        "the campaign ends at the last mission of its last group"
    );
    assert!(
        last_group.quota_met(&walk.steps.iter().map(|s| s.mission.clone()).collect()),
        "with the campaign's missions done, the last group's quota is met"
    );

    // the formulas: at every step the mission pays its AdditionalCBills and its successful
    // objective, and the pilot gains what the script and the tables give
    for step in &walk.steps {
        let e = step
            .expected
            .as_deref()
            .expect("every step is worked out by hand");
        assert_eq!(
            step.cbills.1 - step.cbills.0,
            e.paid - step.salvage,
            "{}: C-bills",
            step.mission
        );
        assert!(step.force_kept, "{}: the force carries over", step.mission);
    }
    // the three scripted missions are the spot checks: a quiet mission still pays the piloting
    // point for coming along, a busy one scores hits and kills, the killer kills heavy mechs
    let scripts: BTreeSet<&str> = walk
        .steps
        .iter()
        .filter_map(|s| s.expected.as_deref().map(|e| e.script.name()))
        .collect();
    assert_eq!(scripts.len(), 3);
    // every mission, including the first quickstart mission and the campaign-ending win, is
    // saved and picked up from the save before the walk continues or stops
    assert_eq!(
        reloads,
        walk.steps.len(),
        "every step is saved and reloaded"
    );

    // ---- the spot checks: three steps worked out on paper from the formulas, not from the code
    // under test
    let table: BTreeMap<&str, &Step> = walk.steps.iter().map(|s| (s.mission.as_str(), s)).collect();
    spot_check(
        &table,
        &skills,
        mission_named(&groups, "mc2_02"),
        start_cbills,
        &groups,
        "mc2_02",
    );
    spot_check(
        &table,
        &skills,
        mission_named(&groups, "mc2_03"),
        start_cbills,
        &groups,
        "mc2_03",
    );
    spot_check(
        &table,
        &skills,
        mission_named(&groups, "mc2_04"),
        start_cbills,
        &groups,
        "mc2_04",
    );
    eprintln!("{} steps, each one saved and reloaded", walk.steps.len());
}

/// The campaign's mission of that name.
fn mission_named<'a>(groups: &'a [Group], name: &str) -> &'a Mission {
    find_mission(groups, name)
}

/// Worked out on paper for the walk's own pilot (`PILOT`), straight from the C++ formulas.
///
/// It starts at the skills the campaign gives its pilots on load: `LogisticsPilot::load` reads
/// each pilot file's `[Skills]` bytes (`logisticspilot.cpp:115-124`), `Logistics` hands them to
/// the mission (`logistics.cpp:764-766`), and `Mover::init` copies the campaign's numbers over
/// the pilot's (`mover.cpp:3396-3402`). Nothing converts them on the way, so the starting skills
/// are the pilot file's own, gunnery 51 and piloting 45. The walk reads them off the loaded
/// campaign and checks them here.
///
/// The sound effects the walk reads from `gamesys.fit` (`mover.cpp:2147-2158`) ship as `Skill Attempt = [0, 0, 0,
/// 0]`, `Skill Success = [1, 0, 0, 0]` and `KillSkillValues = [45, 60, 70, 80, 20, 6]` (light,
/// medium, heavy, assault, vehicle, elemental).
///
/// Where the bank starts: the campaign's own C-bills, `[Campaign] CBills` of
/// `data/campaign/campaign.fit`. `startNewCampaign` takes the first of `getAvailableMissions`
/// (`logisticsmissioninfo.cpp:521-552`), which for an untouched campaign is operation 0's first
/// mission, and presses it (`logisticsdata.cpp:2296-2302`; `setNextMission`,
/// `logisticsmissioninfo.cpp:664-673`). The campaign starts at `currentMission = -1` (`:23`,
/// `:67`), so the press gives nothing back and pays only that mission's own
/// `AdditionalCBills`, and the shipped ones are 0. A screen press later takes the pressed
/// mission's `AdditionalCBills` off the bank after giving back the current one's, unless that
/// one is completed.
///
/// mc2_01 is the first mission of the first operation and has `PlayLogistics = 0`, so the walk
/// buys its pilot no mech, and never presses it, the campaign having started on it
/// (`logisticsdata.cpp:2296-2302`); its win pays the 0 and the 4500 and moves the campaign on to
/// the next operation's first mission (`setMissionComplete`, `:824-847`). No shipped mission names an `AdditionalMission`, so a win
/// always completes the mission it was flown from. Every mission in `data/campaign/campaign.fit`
/// has `AdditionalCBills = 0` in `[General]` (the block `readMissionInfo` ends on,
/// `logisticsmissioninfo.cpp:262-265`, `:309-310`, `:347-348`), so the campaign's C-bills are the
/// bank it starts with (`[Campaign] CBills`, `:91-118`) plus the objectives' `ResourcePoints`.
///
/// mc2_02, step 1, scripted `busy`: three shots, two of them hitting, two kills of medium
/// mechs, nine piloting checks, four of them passing (`mech.cpp:8060-8079` for the shots,
/// `warrior.cpp:7068-7130` for the kills, `mech.cpp:3559-3564` for the checks):
///   gunnery points = 3 * Skill Attempt[2] + 2 * Skill Success[2] + 2 * KillSkillValues[1]
///                  = 0 + 0 + 2 * 70 = 140
///   piloting points = 9 * Skill Attempt[0] + 4 * Skill Success[0] = 0 + 4 = 4
/// `MechWarrior::updateMissionSkills` (`warrior.cpp:7252-7269`):
///   gunnery = 51 + 140 * 0.01 = 52.4, under the `MaxPilotSkill` 80
///   piloting = 45 + 1 + 4 = 50, under 80
/// `LogisticsPilot::update` (`logisticspilot.cpp:284-305`): gain = 1.4 gunnery, 5 piloting;
/// the gain cap 4 keeps 1.4 and 4, so the campaign's skills become 52.4 and 49.
/// C-bills: mc2_01's win paid its first objective's `ResourcePoints` 4000, so the screen holds
/// 80000 + 4000 = 84000. The screen opens on mc2_02 itself, the new operation's first mission
/// (`setMissionComplete`, `:836-840`; `missionselectionscreen.cpp:274-296`), so the press pays
/// mc2_02's 0 and gives nothing back, since the current mission is the one pressed
/// (`logisticsmissioninfo.cpp:664-673`). The screen goes on to the bay, where the pilot's mech
/// is bought (`pilot_mech`), a 55 ton Prime: the Bushwacker Prime, nine components weighing 38
/// tons, which `LogisticsVariant::cost` prices at the chassis' `baseCost` 34750 plus its
/// components' 21800, 56550
/// (`logisticsvariant.cpp:493-503`, the rows of `data/objects/bushwacker.csv` and
/// `data/objects/compbas.csv`). The mission starts with 84000 - 56550 = 27450, pays its
/// `AdditionalCBills` 0 in its win (`logisticsmissioninfo.cpp:799-801`) and its first
/// objective's `ResourcePoints` 4500 (`logisticsdata.cpp:1315-1408`), and the walk takes no
/// salvage: 31950.
///
/// mc2_03, step 2, scripted `killer`: one shot that hits, three kills of medium mechs, two
/// piloting checks, one of them passing (the hit is no gunnery bonus: `Skill Success[2]` is 0):
///   gunnery points = 1 * 0 + 1 * 0 + 3 * 70 = 210 -> 52.4 + 2.1 = 54.5, gain 2.1
///   piloting points = 2 * 0 + 1 * 1 = 1 -> 49 + 1 + 1 = 51, gain 2, under the cap
/// C-bills: `setMissionComplete` kept the position, so the current mission is mc2_02, completed,
/// and the press gives nothing back (`:664-667`) and pays mc2_03's 0; the win pays the 0 and
/// the 7000: 27450 + 4500 + 7000 = 38950.
///
/// mc2_04, step 3, scripted `quiet`: nothing scores:
///   gunnery points = 0 -> 54.5, gain 0
///   piloting = 51 + 1 + 0 = 52, gain 1
/// C-bills: the press gives back mc2_03's nothing, which is completed, and pays mc2_04's 0;
/// the win pays the 0 and the 14000: 38950 + 14000 = 52950.
///
/// The values are the numbers above with the floats' own rounding, which is the one difference
/// the brief allows.
///
/// `pressed` is what the selection screen's press moved the bank by, and `at_screen` the bank
/// the mission's own selection screen holds, after that press and mc2_01's own objective; the
/// bay then pays `BUSHWACKER` for the pilot's mech, the one cost worked out above.
fn spot_check(
    table: &BTreeMap<&str, &Step>,
    skills: &Skills,
    mission: &Mission,
    start_cbills: i64,
    groups: &[Group],
    name: &str,
) {
    let step = table[name];
    let script = match name {
        "mc2_02" => Script::Busy,
        "mc2_03" => Script::Killer,
        _ => Script::Quiet,
    };
    let (gunnery_points, piloting_points) = (
        script.gunnery_points(skills),
        script.piloting_points(skills),
    );

    assert_eq!(
        (gunnery_points, piloting_points),
        match name {
            "mc2_02" => (140.0, 4.0),
            "mc2_03" => (210.0, 1.0),
            _ => (0.0, 0.0),
        },
        "{name}: the tables give the points worked out on paper"
    );
    // `MechWarrior::updateMissionSkills`: the skill plus the points, capped at `MaxPilotSkill`,
    // the piloting gaining its point for the mission first
    let (gunnery_before, piloting_before) = match name {
        "mc2_02" => (51.0, 45.0),
        "mc2_03" => (52.4, 49.0),
        _ => (54.5, 51.0),
    };
    let warrior = warrior_skills_after(
        (gunnery_before, piloting_before),
        (gunnery_points, piloting_points),
    );
    // `LogisticsPilot::update`: the pilot's unclamped gain
    let gain = (warrior.0 - gunnery_before, warrior.1 - piloting_before);
    // ...and the skills the campaign keeps, which is what the briefing shows
    let kept = logistics_pilot_update((gunnery_before, piloting_before), warrior).0;
    let paid = mission.settings.additional_cbills + i64::from(first_rp(mission));
    let (before, after) = step.cbills;
    let e = step.expected.as_deref().expect("worked out by hand");
    assert_eq!(
        e.paid, paid,
        "{name}: the mission pays what campaign.fit says"
    );
    let near = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4;
    // The C-bills are worked out in two parts, so that the mech the bay buys after the press is
    // not lost in a number worked out after it: what the press moves, and the bank before the
    // press (`at_screen`), each worked out from the one before: the first from the campaign's own
    // C-bills and mc2_01's own objective, the second from where the first mission's win left the
    // bank, the third from where the second's did.
    // (warrior, gain, kept, paid, press, bank at the screen, what the bay charged): the screen
    // opens on the bank the previous step's win left, and the press moves that; the bay then
    // charges for the pilot's mech, the one time only
    let paper = match name {
        "mc2_02" => (
            (52.4, 49.0),
            (1.4, 4.0),
            (52.4, 49.0),
            4500,
            0,
            start_cbills + i64::from(first_rp(mission_named(groups, "mc2_01"))),
            BUSHWACKER,
        ),
        "mc2_03" => (
            (54.5, 51.0),
            (2.1, 2.0),
            (54.5, 51.0),
            7000,
            0,
            start_cbills
                + i64::from(first_rp(mission_named(groups, "mc2_01")))
                + i64::from(first_rp(mission_named(groups, "mc2_02")))
                - BUSHWACKER,
            0,
        ),
        _ => (
            (54.5, 52.0),
            (0.0, 1.0),
            (54.5, 52.0),
            14000,
            0,
            start_cbills
                + i64::from(first_rp(mission_named(groups, "mc2_01")))
                + i64::from(first_rp(mission_named(groups, "mc2_02")))
                + i64::from(first_rp(mission_named(groups, "mc2_03")))
                - BUSHWACKER,
            0,
        ),
    };
    assert!(near(warrior, paper.0), "{name}: the warrior {warrior:?}");
    assert!(near(gain, paper.1), "{name}: the gain {gain:?}");
    assert!(near(kept, paper.2), "{name}: the skills kept {kept:?}");
    assert_eq!(
        paid, paper.3,
        "{name}: the mission pays what campaign.fit says"
    );
    // the screen's press, the bay charging the pilot's mech (`logisticsdata.cpp:454-472`,
    // `pilot_mech`), and the bank the mission starts at
    assert_eq!(
        e.pressed, paper.4,
        "{name}: the press moves the bank as campaign.fit says"
    );
    assert_eq!(
        step.extra, paper.5,
        "{name}: the bank the screen shows, before the press"
    );
    assert_eq!(
        before,
        paper.5 + paper.4 - paper.6,
        "{name}: C-bills when the mission starts, the bay's mech paid"
    );
    assert_eq!(after, before + paper.3, "{name}: C-bills after the win");
    assert_eq!(
        e.paid - step.salvage,
        after - before,
        "{name}: C-bills on paper"
    );
    assert!(
        (e.points.0 - gunnery_points).abs() < 1e-4 && (e.points.1 - piloting_points).abs() < 1e-4,
        "{name}: the report's points {:?} are not the ones worked out by hand \
         ({gunnery_points}, {piloting_points})",
        e.points
    );
    assert_eq!(step.salvage, 0, "{name}: the walk takes no salvage");
    assert!(
        near(e.warrior, warrior),
        "{name}: the walk works out the warrior {:?} by hand, {:?} in the walk",
        warrior,
        e.warrior
    );
    assert!(
        near(e.keeper.0, kept),
        "{name}: the walk works out the skills kept {kept:?} by hand, {:?} in the walk",
        e.keeper.0
    );
    // the pilot's gains, `LogisticsPilot::update`: the gunnery by the points in hundredths, the
    // piloting by the points and the mission's own point
    assert!(
        near(e.keeper.1, paper.1),
        "{name}: the walk works out the gain"
    );
    assert!(
        near(e.before, (gunnery_before, piloting_before)),
        "{name}: the pilot starts at the campaign's skills"
    );
}

/// The same walk taken through the other missions of every choice group: the branch that is not
/// the campaign's own choice is pressed as soon as the screen offers it.
#[test]
fn every_branch_is_played() {
    let Some((mut vfs, data)) = data() else {
        eprintln!("MC2_DATA unset: skipping");
        return;
    };
    let (_, start_cbills) = read_script(&mut vfs);
    let groups = read_campaign(&mut vfs);
    let skills = read_skills(&mut vfs);

    // every mission of a group that offers a choice, played at least once
    let branchy: BTreeSet<&str> = groups
        .iter()
        .filter(|g| g.missions.iter().any(|m| m.hidden))
        .flat_map(|g| g.missions.iter().map(|m| m.name.as_str()))
        .collect();
    let mut played: BTreeSet<String> = BTreeSet::new();
    for (i, route) in [Chooser::Current, Chooser::Other, Chooser::Last]
        .into_iter()
        .enumerate()
    {
        let walk = walk(
            &data,
            &groups,
            &skills,
            start_cbills,
            route,
            |l, text, step| {
                let loaded =
                    Logistics::load_from_str(Arc::clone(&data), text).expect("the save loads");
                assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
                assert_eq!(
                    loaded.logistics.to_save(),
                    l.to_save(),
                    "{}: the branch save restores the campaign",
                    step.mission
                );
                loaded.logistics
            },
        );
        // each route takes the missions the campaign opens from its start to its end, as many
        // missions as it needs to finish every group's quota
        assert_eq!(
            walk.steps.last().map(|s| s.mission.as_str()),
            walk.completed_at.as_ref().map(|(_, m)| m.as_str()),
            "walk {i} ends the campaign"
        );
        for s in &walk.steps {
            played.insert(s.mission.clone());
        }
    }
    let played: BTreeSet<&str> = played.iter().map(|s| s.as_str()).collect();
    let missing: Vec<&&str> = branchy.difference(&played).collect();
    assert!(missing.is_empty(), "branches never played: {missing:?}");
}
