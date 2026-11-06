// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Campaign progression: groups of missions, which can be played next, what each mission
//! offers, and the C-Bills balance.
//!
//! Port of `code/logisticsmissioninfo.{h,cpp}` (`LogisticsMissionInfo`), single-player part. The
//! static half (names, purchase files, settings) comes from the campaign script loaded by
//! `mc2_sim_data::campaign`; the dynamic half (stage, current mission, completed and hidden
//! flags, videos shown, bonus purchase files, C-Bills) is the state that is saved.
//!
//! Single missions (`setSingleMission`, the main menu's "single mission" start) are ported:
//! [`MissionInfo::new_single`]. Not ported: multiplayer (`setMultiplayer`, the `MPlayer` branch of
//! `setNextMission`); a single mission sets `bMultiplayer` in the original, so choosing a mission
//! is refused here for it (the original would dereference `MPlayer`).

use std::sync::Arc;

use mc2_sim_data::campaign::{Campaign, CampaignMission, DisplayText, DEFAULT_PURCHASE_FILE};

use crate::error::{LogisticsError, LogisticsResult};

/// Largest group the selection screen has buttons for (`MAX_MISSIONS_IN_GROUP`).
pub const MAX_MISSIONS_IN_GROUP: usize = 8;

/// One mission of a group (`LogisticsMissionInfo::MissionInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct MissionEntry {
    /// `fileName`: mission file stem.
    pub file_name: String,
    /// `missionDescriptiveName`.
    pub descriptive_name: Option<DisplayText>,
    /// `description` (the briefing blurb).
    pub description: Option<DisplayText>,
    /// `purchaseFileName`: `data/missions/<PurchaseFile>.fit`, empty when the mission file is
    /// missing.
    pub purchase_file_name: String,
    /// `videoName`: the `VideoOverride`, or empty.
    pub video_name: String,
    /// `mandatory`.
    pub mandatory: bool,
    /// `completePrevious`.
    pub complete_previous: bool,
    /// `completed`.
    pub completed: bool,
    /// `resourcePoints`: in-mission support points (`ResourcePoints` in `[MissionSettings]`).
    pub resource_points: i64,
    /// `additionalCBills`.
    pub additional_cbills: i64,
    /// `dropWeight`: tons the force may weigh.
    pub drop_weight: i64,
    /// `playLogistics`.
    pub play_logistics: bool,
    /// `playSalvage`.
    pub play_salvage: bool,
    /// `playPilotPromotion`.
    pub play_pilot_promotion: bool,
    /// `playPurchasing`.
    pub play_purchasing: bool,
    /// `playMissionSelection`.
    pub play_mission_selection: bool,
    /// `hidden`: not offered until the group's quota is met.
    pub hidden: bool,
    /// `enableSalavageCraft`.
    pub enable_salvage_craft: bool,
    /// `enableRepairTruck`.
    pub enable_repair_truck: bool,
    /// `enableScoutCopter`.
    pub enable_scout_copter: bool,
    /// `enableArtilleryPiece`.
    pub enable_artillery_piece: bool,
    /// `enableAirStrike`.
    pub enable_air_strike: bool,
    /// `enableSensorStrike`.
    pub enable_sensor_strike: bool,
    /// `enableMineLayer`.
    pub enable_mine_layer: bool,
}

impl MissionEntry {
    /// An entry made from a mission file's settings alone, with the defaults a campaign entry
    /// would have.
    fn from_settings(
        file_name: &str,
        settings: &mc2_sim_data::campaign::MissionSettings,
    ) -> MissionEntry {
        MissionEntry {
            file_name: file_name.to_string(),
            descriptive_name: settings.display_name(),
            description: settings.display_blurb(),
            purchase_file_name: String::new(),
            video_name: String::new(),
            mandatory: false,
            complete_previous: false,
            completed: false,
            resource_points: settings.resource_points.unwrap_or(0),
            additional_cbills: settings.additional_cbills.unwrap_or(0),
            drop_weight: settings.drop_weight_limit,
            play_logistics: true,
            play_salvage: true,
            play_pilot_promotion: true,
            play_purchasing: true,
            play_mission_selection: false,
            hidden: false,
            enable_salvage_craft: settings.enable_salvage_craft,
            enable_repair_truck: settings.enable_repair_truck,
            enable_scout_copter: settings.enable_scout_copter,
            enable_artillery_piece: settings.enable_artillery_piece,
            enable_air_strike: settings.enable_air_strike,
            enable_sensor_strike: settings.enable_sensor_strike,
            enable_mine_layer: settings.enable_mine_layer,
        }
    }

    fn from_campaign(m: &CampaignMission) -> MissionEntry {
        // `readMissionInfo` leaves fields it cannot read uninitialised; zero and the defaults of
        // `MissionSettings` are used here.
        let settings = m.settings.clone().unwrap_or_default();
        MissionEntry {
            file_name: m.file_name.clone(),
            descriptive_name: settings.display_name(),
            description: settings.display_blurb(),
            purchase_file_name: if m.purchase_file.is_empty() {
                String::new()
            } else {
                format!(
                    "{}{}.fit",
                    mc2_sim_data::campaign::MISSION_DIR,
                    m.purchase_file
                )
            },
            video_name: m.video_override.clone().unwrap_or_default(),
            mandatory: m.mandatory,
            complete_previous: m.complete_previous,
            completed: false,
            resource_points: settings.resource_points.unwrap_or(0),
            additional_cbills: settings.additional_cbills.unwrap_or(0),
            drop_weight: settings.drop_weight_limit,
            play_logistics: m.play_logistics,
            play_salvage: m.play_salvage,
            play_pilot_promotion: m.play_pilot_promotion,
            play_purchasing: m.play_purchasing,
            play_mission_selection: m.play_mission_selection,
            hidden: m.hidden,
            enable_salvage_craft: settings.enable_salvage_craft,
            enable_repair_truck: settings.enable_repair_truck,
            enable_scout_copter: settings.enable_scout_copter,
            enable_artillery_piece: settings.enable_artillery_piece,
            enable_air_strike: settings.enable_air_strike,
            enable_sensor_strike: settings.enable_sensor_strike,
            enable_mine_layer: settings.enable_mine_layer,
        }
    }
}

/// One operation (`LogisticsMissionInfo::MissionGroup`).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// `numberToBeCompleted`: how many missions finish the group (`-1` when the block is
    /// missing).
    pub number_to_be_completed: i64,
    /// `videoFileName`: the operation video; replaced by a hidden mission's override when it is
    /// unhidden.
    pub video_file_name: String,
    /// `operationFileName`.
    pub operation_file_name: String,
    /// `bigVideoName`: the pre-operation video.
    pub big_video_name: String,
    /// `ablBrainName`.
    pub abl_brain_name: String,
    /// `logisticsTuneId`.
    pub logistics_tune_id: i64,
    /// `infos`.
    pub missions: Vec<MissionEntry>,
    /// `bigVideoShown`.
    pub big_video_shown: bool,
    /// `videoShown`.
    pub video_shown: bool,
}

/// The campaign position and balance (`LogisticsMissionInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct MissionInfo {
    /// Campaign script name (`campaign`, `tutorial`).
    pub campaign_name: String,
    /// `NameID` when the script has one.
    pub campaign_name_id: Option<i64>,
    /// `CampaignName` when the script has no `NameID`.
    pub campaign_display_name: Option<String>,
    /// `playerName`.
    pub player_name: String,
    /// `currentMissionName`.
    pub current_mission_name: String,
    /// `finalVideoName`.
    pub final_video_name: String,
    /// `currentStage`: index of the group being played; `group_count` once the campaign is over.
    pub current_stage: i64,
    /// `currentMission`: index in the current group, `-1` before one is chosen.
    pub current_mission: i64,
    /// `lastStage`: stage of the mission completed last.
    pub last_stage: i64,
    /// `lastMission`: index of the mission completed last, in its group.
    pub last_mission: i64,
    /// `lastMissionName`.
    pub last_mission_name: String,
    /// The groups.
    pub groups: Vec<Group>,
    /// `CBills`.
    pub c_bills: i64,
    /// `additionalPurchaseFiles`: bonus lists unlocked so far, in order, duplicates kept.
    pub additional_purchase_files: Vec<String>,
    /// Set by `setSingleMission` (which sets `bMultiplayer` in the original): a one-mission
    /// script made from a map file, outside any campaign.
    pub single_mission: bool,
}

impl MissionInfo {
    /// `LogisticsMissionInfo::init`: a fresh campaign. C-Bills start at the script's `CBills`;
    /// nothing is completed.
    pub fn new(campaign_name: &str, campaign: &Arc<Campaign>) -> MissionInfo {
        let group_count = usize::try_from(campaign.group_count).unwrap_or(0);
        let mut groups: Vec<Group> = (0..group_count)
            .map(|_| Group {
                number_to_be_completed: -1,
                video_file_name: String::new(),
                operation_file_name: String::new(),
                big_video_name: String::new(),
                abl_brain_name: String::new(),
                logistics_tune_id: 0,
                missions: Vec::new(),
                big_video_shown: false,
                video_shown: false,
            })
            .collect();
        for g in &campaign.groups {
            let Some(slot) = groups.get_mut(g.index) else {
                continue;
            };
            *slot = Group {
                number_to_be_completed: g.number_to_complete,
                video_file_name: g.video.clone().unwrap_or_default(),
                operation_file_name: g.operation_file.clone(),
                big_video_name: g.pre_video.clone().unwrap_or_default(),
                abl_brain_name: g.abl_script.clone(),
                logistics_tune_id: g.logistics_tune_id,
                missions: g.missions.iter().map(MissionEntry::from_campaign).collect(),
                big_video_shown: false,
                video_shown: false,
            };
        }
        MissionInfo {
            campaign_name: campaign_name.to_string(),
            campaign_name_id: campaign.name_id,
            campaign_display_name: campaign.campaign_name.clone(),
            player_name: String::new(),
            current_mission_name: String::new(),
            final_video_name: campaign.final_video.clone().unwrap_or_default(),
            current_stage: 0,
            current_mission: -1,
            last_stage: 0,
            last_mission: 0,
            last_mission_name: String::new(),
            groups,
            c_bills: campaign.cbills,
            additional_purchase_files: Vec::new(),
            single_mission: false,
        }
    }

    /// `LogisticsMissionInfo::setSingleMission`: one group with one mission, read from a map's
    /// `[MissionSettings]`. The player starts with the mission's `AdditionalCBills`, or when it
    /// has none `1200 * DropWeightLimit + 100000`. `raw_drop_weight` is the untruncated
    /// `DropWeightLimit` (the C++ reads it as a float; `None` when the key is absent, then the
    /// settings' default is used). Fields the C++ leaves uninitialised take the campaign
    /// defaults (logistics and purchasing played, no mission selection); salvage and pilot
    /// review are off, as the C++ sets them. The entry's file name is the map's name (the C++
    /// stores the global `missionName` there, a slip).
    pub fn new_single(
        mission_file_name: &str,
        settings: &mc2_sim_data::campaign::MissionSettings,
        raw_drop_weight: Option<f32>,
    ) -> MissionInfo {
        let raw = raw_drop_weight.unwrap_or(settings.drop_weight_limit as f32);
        let additional = match settings.additional_cbills {
            Some(v) if v != 0 => v,
            _ => (1200.0f32 * raw + 100_000.0f32) as i64,
        };
        let mut entry = MissionEntry::from_settings(mission_file_name, settings);
        entry.drop_weight = raw as i64;
        entry.additional_cbills = additional;
        entry.play_salvage = false;
        entry.play_pilot_promotion = false;
        entry.play_purchasing = true;
        entry.play_mission_selection = false;
        MissionInfo {
            campaign_name: String::new(),
            campaign_name_id: None,
            campaign_display_name: None,
            player_name: String::new(),
            current_mission_name: mission_file_name.to_string(),
            final_video_name: String::new(),
            current_stage: 0,
            current_mission: 0,
            last_stage: 0,
            last_mission: 0,
            last_mission_name: String::new(),
            groups: vec![Group {
                number_to_be_completed: 1,
                video_file_name: String::new(),
                operation_file_name: "MCL_CM_Op1_1.fit".into(),
                big_video_name: String::new(),
                abl_brain_name: String::new(),
                logistics_tune_id: 0,
                missions: vec![entry],
                big_video_shown: true,
                video_shown: false,
            }],
            c_bills: additional,
            additional_purchase_files: Vec::new(),
            single_mission: true,
        }
    }

    /// Number of groups.
    pub fn group_count(&self) -> i64 {
        self.groups.len() as i64
    }

    fn stage(&self) -> Option<&Group> {
        usize::try_from(self.current_stage)
            .ok()
            .and_then(|s| self.groups.get(s))
    }

    fn stage_mut(&mut self) -> Option<&mut Group> {
        usize::try_from(self.current_stage)
            .ok()
            .and_then(|s| self.groups.get_mut(s))
    }

    /// The mission selected in the current group.
    pub fn current_entry(&self) -> Option<&MissionEntry> {
        let m = usize::try_from(self.current_mission).ok()?;
        self.stage()?.missions.get(m)
    }

    /// `getAvailableMissions`: the not yet completed missions of the current group, in order.
    pub fn available_missions(&self) -> Vec<String> {
        self.stage()
            .map(|g| {
                g.missions
                    .iter()
                    .filter(|m| !m.completed)
                    .map(|m| m.file_name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `getCurrentMissions`: the missions of the current group that are not hidden, in order
    /// (what the selection screen lists).
    pub fn current_missions(&self) -> Vec<String> {
        self.stage()
            .map(|g| {
                g.missions
                    .iter()
                    .filter(|m| !m.hidden)
                    .map(|m| m.file_name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `getMissionAvailable`: `true` when the mission is in the current group and not completed.
    pub fn mission_available(&self, mission_name: &str) -> bool {
        self.stage()
            .and_then(|g| {
                g.missions
                    .iter()
                    .find(|m| m.file_name.eq_ignore_ascii_case(mission_name))
            })
            .is_some_and(|m| !m.completed)
    }

    /// `setNextMission` (single-player branch): selects a mission of the current group. The
    /// mission's `AdditionalCBills` are paid when it is chosen, and taken back when another
    /// not yet completed mission is chosen instead.
    pub fn set_next_mission(&mut self, mission_name: &str) -> LogisticsResult<()> {
        if mission_name.is_empty() {
            return Err(LogisticsError::Failed);
        }
        if self.single_mission {
            // `bMultiplayer` branch: it reads the multiplayer settings, which do not exist here
            return Err(LogisticsError::NotYetImplemented);
        }
        let Some(stage) = usize::try_from(self.current_stage)
            .ok()
            .filter(|&s| s < self.groups.len())
        else {
            return Err(LogisticsError::InvalidId);
        };

        let Some(position) = self.groups[stage]
            .missions
            .iter()
            .position(|m| m.file_name.eq_ignore_ascii_case(mission_name))
        else {
            return Err(LogisticsError::InvalidId); // INVALID_MISSION
        };

        if self.groups[stage].missions[position].completed {
            return Err(LogisticsError::MissionAlreadyDone);
        }

        if self.current_mission != -1 {
            if let Some(prev) = usize::try_from(self.current_mission)
                .ok()
                .and_then(|m| self.groups[stage].missions.get(m))
            {
                if !prev.completed {
                    self.c_bills -= prev.additional_cbills;
                }
            }
        }
        self.current_mission = position as i64;
        let entry = &self.groups[stage].missions[position];
        self.current_mission_name = entry.file_name.trim_end_matches(".fit").to_string();
        self.c_bills += entry.additional_cbills;
        Ok(())
    }

    /// `setMissionComplete`: records the mission just played as the last one, pays its
    /// `AdditionalCBills`, and moves on. When the group's quota is met and every mandatory
    /// mission is done the next group starts; when the quota is met but a mandatory mission is
    /// open the hidden missions are revealed; otherwise the next open mission is selected.
    pub fn set_mission_complete(&mut self) {
        let group_count = self.group_count();
        if self.current_stage >= group_count {
            return;
        }

        // Needed to tell pilots which mission they just completed.
        self.last_stage = self.current_stage;
        self.last_mission = self.current_mission;
        self.last_mission_name = self.current_mission_name.clone();

        let stage = self.current_stage as usize;
        if self.current_mission > -1 {
            if let Some(m) = self.groups[stage]
                .missions
                .get(self.current_mission as usize)
            {
                self.c_bills += m.additional_cbills;
            }
        }

        let current_mission = self.current_mission;
        let mut number_completed = 0i64;
        let mut all_mandatory_completed = true;
        let mut next_available: i64 = -1;
        for (count, m) in (0i64..).zip(self.groups[stage].missions.iter_mut()) {
            if current_mission == count {
                m.completed = true;
            }
            if m.completed {
                number_completed += 1;
            } else {
                if next_available == -1 {
                    next_available = count;
                }
                if m.mandatory {
                    all_mandatory_completed = false;
                }
            }
        }

        if number_completed >= self.groups[stage].number_to_be_completed {
            if all_mandatory_completed {
                self.current_stage += 1;
                self.current_mission = 0;
            } else {
                self.unhide_missions(stage);
                self.current_mission = next_available;
            }
        } else {
            // the C++ asserts that something is left to play
            self.current_mission = next_available;
        }

        if self.current_stage < group_count {
            let stage = self.current_stage as usize;
            if let Some(m) = usize::try_from(self.current_mission)
                .ok()
                .and_then(|i| self.groups[stage].missions.get(i))
            {
                self.current_mission_name = m.file_name.clone();
            }
        }
    }

    /// Unhides the hidden missions of a group; a hidden mission's video override becomes the
    /// group's video (and will be shown again).
    fn unhide_missions(&mut self, stage: usize) {
        let group = &mut self.groups[stage];
        let mut video = None;
        for m in &mut group.missions {
            if m.hidden && !m.video_name.is_empty() {
                video = Some(m.video_name.clone());
            }
            m.hidden = false;
        }
        if let Some(v) = video {
            group.video_file_name = v;
            group.video_shown = false;
        }
    }

    /// `getCurrentMissionId`: the flat index of the mission completed last across all groups
    /// (missions of earlier groups plus its index in its own group). Pilots use it to record
    /// which missions they flew.
    pub fn current_mission_id(&self) -> i64 {
        let mut id = 0i64;
        let last = usize::try_from(self.last_stage).unwrap_or(0);
        for (i, g) in self.groups.iter().enumerate().take(last + 1) {
            if i == last {
                id += self.last_mission;
            } else {
                id += g.missions.len() as i64;
            }
        }
        id
    }

    /// `getCurrentPurchaseFile`: path of the purchase list of the current mission, empty when
    /// there is none (callers fall back to `data/missions/purchase.fit`).
    pub fn current_purchase_file(&self) -> &str {
        self.current_entry()
            .map_or("", |m| m.purchase_file_name.as_str())
    }

    /// `getCurrentDropWeight`: tons the force may weigh; 10000 when no mission is chosen.
    pub fn current_drop_weight(&self) -> i64 {
        match self.current_entry() {
            Some(m) if self.current_mission != -1 => m.drop_weight,
            _ => 10000,
        }
    }

    /// `getCurrentRP`: the in-mission resource points of the current mission.
    pub fn current_rp(&self) -> i64 {
        if self.current_mission == -1 {
            return 0;
        }
        self.current_entry().map_or(0, |m| m.resource_points)
    }

    /// `getCurrentOperationFile`.
    pub fn current_operation_file(&self) -> Option<&str> {
        self.stage().map(|g| g.operation_file_name.as_str())
    }

    /// `getCurrentVideo`: the current mission's override, else the group video.
    pub fn current_video(&self) -> Option<&str> {
        let g = self.stage()?;
        if let Some(m) = self.current_entry() {
            if !m.video_name.is_empty() {
                return Some(m.video_name.as_str());
            }
        }
        Some(g.video_file_name.as_str())
    }

    /// `getCurrentLogisticsTuneId`: `-1` once the campaign is over.
    pub fn current_logistics_tune_id(&self) -> i64 {
        self.stage().map_or(-1, |g| g.logistics_tune_id)
    }

    /// `getCurrentMissionDescription`.
    pub fn current_mission_description(&self) -> Option<&DisplayText> {
        self.current_entry().and_then(|m| m.description.as_ref())
    }

    /// `getCurrentMissionFriendlyName`.
    pub fn current_mission_friendly_name(&self) -> Option<&DisplayText> {
        self.current_entry()
            .and_then(|m| m.descriptive_name.as_ref())
    }

    /// `getMissionFriendlyName`: name of a mission of the current group.
    pub fn mission_friendly_name(&self, mission_name: &str) -> Option<&DisplayText> {
        self.stage()?
            .missions
            .iter()
            .find(|m| m.file_name.eq_ignore_ascii_case(mission_name))
            .and_then(|m| m.descriptive_name.as_ref())
    }

    /// `getCurrentABLScriptName`: the group's brain script (`NONE` when it has none).
    pub fn current_abl_script_name(&self) -> Option<&str> {
        self.stage().map(|g| g.abl_brain_name.as_str())
    }

    /// `campaignOver`.
    pub fn campaign_over(&self) -> bool {
        self.current_stage == self.group_count()
    }

    /// `getCurrentBigVideo`: the pre-operation video the first time it is asked for in a group,
    /// then `None`. Asking marks it shown.
    pub fn take_current_big_video(&mut self) -> Option<String> {
        let g = self.stage_mut()?;
        if g.big_video_shown {
            return None;
        }
        g.big_video_shown = true;
        Some(g.big_video_name.clone())
    }

    /// `getFinalVideo`.
    pub fn final_video(&self) -> &str {
        &self.final_video_name
    }

    /// `addBonusPurchaseFile`.
    pub fn add_bonus_purchase_file(&mut self, file_name: &str) {
        self.additional_purchase_files.push(file_name.to_string());
    }

    /// `skipLogistics`: the current mission is played with the force its file defines.
    pub fn skip_logistics(&self) -> bool {
        if self.current_stage >= self.group_count() {
            return false;
        }
        self.current_entry().is_some_and(|m| !m.play_logistics)
    }

    /// `showChooseMission`.
    pub fn show_choose_mission(&self) -> bool {
        self.current_entry()
            .is_some_and(|m| m.play_mission_selection)
    }

    /// `skipPurchasing`.
    pub fn skip_purchasing(&self) -> bool {
        self.current_entry().is_some_and(|m| !m.play_purchasing)
    }

    /// `getPreviousMission`: the mission before the current one (the one just completed, since
    /// completing advances the position).
    fn previous_mission(&self) -> Option<&MissionEntry> {
        let stage = usize::try_from(self.current_stage).ok()?;
        if self.current_mission > 0 {
            return self
                .groups
                .get(stage)?
                .missions
                .get((self.current_mission - 1) as usize);
        }
        if self.current_stage > 0 {
            return self.groups.get(stage - 1)?.missions.last();
        }
        if self.current_stage == 0 {
            // There may be only one stage (the tutorials).
            return self.groups.first()?.missions.last();
        }
        None
    }

    /// `skipPilotReview`.
    pub fn skip_pilot_review(&self) -> bool {
        self.previous_mission()
            .is_some_and(|m| !m.play_pilot_promotion)
    }

    /// `skipSalvageScreen`.
    pub fn skip_salvage_screen(&self) -> bool {
        self.previous_mission().is_some_and(|m| !m.play_salvage)
    }

    /// `isSingleMission`: a script with one group of one mission.
    pub fn is_single_mission(&self) -> bool {
        self.groups.len() == 1 && self.groups[0].missions.len() == 1
    }

    /// `canHaveSalavageCraft`.
    pub fn can_have_salvage_craft(&self) -> bool {
        self.current_entry().is_some_and(|m| m.enable_salvage_craft)
    }
    /// `canHaveRepairTruck`.
    pub fn can_have_repair_truck(&self) -> bool {
        self.current_entry().is_some_and(|m| m.enable_repair_truck)
    }
    /// `canHaveScoutCopter`.
    pub fn can_have_scout_copter(&self) -> bool {
        self.current_entry().is_some_and(|m| m.enable_scout_copter)
    }
    /// `canHaveArtilleryPiece`.
    pub fn can_have_artillery_piece(&self) -> bool {
        self.current_entry()
            .is_some_and(|m| m.enable_artillery_piece)
    }
    /// `canHaveAirStrike`.
    pub fn can_have_air_strike(&self) -> bool {
        self.current_entry().is_some_and(|m| m.enable_air_strike)
    }
    /// `canHaveSensorStrike`.
    pub fn can_have_sensor_strike(&self) -> bool {
        self.current_entry().is_some_and(|m| m.enable_sensor_strike)
    }
    /// `canHaveMineLayer`.
    pub fn can_have_mine_layer(&self) -> bool {
        self.current_entry().is_some_and(|m| m.enable_mine_layer)
    }

    /// `getVideoShown`.
    pub fn video_shown(&self) -> bool {
        self.stage().is_some_and(|g| g.video_shown)
    }

    /// `setVideoShown`.
    pub fn set_video_shown(&mut self) {
        if let Some(g) = self.stage_mut() {
            g.video_shown = true;
        }
    }

    /// `getCurrentMissionNumber` / `setCurrentMissionNumber`: index of the current mission in
    /// its group.
    pub fn current_mission_number(&self) -> i64 {
        self.current_mission
    }

    /// Setter for [`current_mission_number`](Self::current_mission_number).
    pub fn set_current_mission_number(&mut self, mission: i64) {
        self.current_mission = mission;
    }
}

/// The purchase list a mission falls back to when it names none or its file is missing
/// (`missionPath` + `purchase.fit`, `logisticsdata.cpp:1471-1475`).
pub const FALLBACK_PURCHASE_FILE: &str = "data/missions/purchase.fit";

/// The list a mission uses when `PurchaseFile` is absent (`purchase_All`).
pub const DEFAULT_PURCHASE_STEM: &str = DEFAULT_PURCHASE_FILE;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mc2_sim_data::campaign::{CampaignGroup, MissionSettings};

    fn mission(name: &str, mandatory: bool, hidden: bool, extra: Option<&str>) -> CampaignMission {
        CampaignMission {
            index: 0,
            file_name: name.into(),
            mandatory,
            complete_previous: false,
            hidden,
            video_override: extra.map(str::to_string),
            purchase_file: format!("P_{name}"),
            play_logistics: true,
            play_salvage: true,
            play_pilot_promotion: true,
            play_purchasing: true,
            play_mission_selection: false,
            settings: Some(MissionSettings {
                resource_points: Some(1000),
                additional_cbills: Some(500),
                drop_weight_limit: 200,
                ..MissionSettings::default()
            }),
        }
    }

    /// Group 0: a, b (quota 1, none mandatory). Group 1: c (mandatory), d (hidden, mandatory),
    /// quota 1. Group 2: e.
    pub(crate) fn campaign() -> Arc<Campaign> {
        let mk = |index: usize, ntc: i64, ms: Vec<CampaignMission>| CampaignGroup {
            index,
            number_to_complete: ntc,
            logistics_tune_id: 20 + index as i64,
            video: Some(format!("VID{index}")),
            pre_video: (index == 1).then(|| "PRE1".to_string()),
            operation_file: format!("op{index}"),
            abl_script: "NONE".into(),
            mission_count: ms.len() as i64,
            missions: ms,
        };
        Arc::new(Campaign {
            file_name: "campaign".into(),
            name_id: Some(60021),
            campaign_name: None,
            group_count: 3,
            cbills: 80_000,
            final_video: Some("FINAL".into()),
            groups: vec![
                mk(
                    0,
                    1,
                    vec![
                        mission("a", false, false, None),
                        mission("b", false, false, None),
                    ],
                ),
                mk(
                    1,
                    1,
                    vec![
                        mission("c", false, false, None),
                        mission("d", true, true, Some("D_VIDEO.bik")),
                    ],
                ),
                mk(2, 1, vec![mission("e", true, false, None)]),
            ],
            missing_groups: vec![],
            missing_mission_files: vec![],
            bonus_purchase_files: vec![],
        })
    }

    #[test]
    fn a_new_campaign_starts_with_the_script_cbills_and_nothing_chosen() {
        let mi = MissionInfo::new("campaign", &campaign());
        assert_eq!(mi.c_bills, 80_000);
        assert_eq!(mi.current_stage, 0);
        assert_eq!(mi.current_mission, -1);
        assert_eq!(mi.available_missions(), vec!["a", "b"]);
        assert!(!mi.campaign_over());
        assert_eq!(mi.current_drop_weight(), 10000, "no mission chosen yet");
        assert_eq!(mi.current_rp(), 0);
        assert_eq!(mi.current_logistics_tune_id(), 20);
        assert_eq!(mi.final_video(), "FINAL");
    }

    #[test]
    fn choosing_a_mission_pays_its_additional_cbills_and_switching_refunds() {
        let mut mi = MissionInfo::new("campaign", &campaign());
        mi.set_next_mission("a").unwrap();
        assert_eq!(mi.c_bills, 80_500);
        assert_eq!(mi.current_mission_name, "a");
        assert_eq!(mi.current_rp(), 1000);
        assert_eq!(mi.current_drop_weight(), 200);
        assert_eq!(mi.current_purchase_file(), "data/missions/P_a.fit");
        mi.set_next_mission("B").unwrap(); // case-insensitive
        assert_eq!(mi.c_bills, 80_500, "refund a, pay b");
        assert_eq!(mi.current_mission, 1);
        assert_eq!(mi.set_next_mission("zzz"), Err(LogisticsError::InvalidId));
        assert_eq!(mi.set_next_mission(""), Err(LogisticsError::Failed));
    }

    #[test]
    fn completing_pays_again_and_advances_when_the_quota_is_met() {
        let mut mi = MissionInfo::new("campaign", &campaign());
        mi.set_next_mission("a").unwrap();
        mi.set_mission_complete();
        // +500 on choosing, +500 on completing.
        assert_eq!(mi.c_bills, 81_000);
        assert_eq!((mi.last_stage, mi.last_mission), (0, 0));
        assert_eq!(mi.last_mission_name, "a");
        // Quota 1 met, nothing mandatory open: next group, first mission.
        assert_eq!(mi.current_stage, 1);
        assert_eq!(mi.current_mission, 0);
        assert_eq!(mi.current_mission_name, "c");
        assert_eq!(mi.current_mission_id(), 0);
        assert_eq!(mi.available_missions(), vec!["c", "d"]);
        assert_eq!(mi.current_missions(), vec!["c"], "d is hidden");
        assert_eq!(
            mi.set_next_mission("a"),
            Err(LogisticsError::InvalidId),
            "a belongs to the previous group now"
        );
    }

    #[test]
    fn a_completed_mission_cannot_be_chosen_again() {
        let mut mi = MissionInfo::new("campaign", &campaign());
        // Group quota is 1 but a second mission in group 0 is still selectable until done.
        mi.set_next_mission("a").unwrap();
        mi.set_mission_complete();
        // Back into group 0 is impossible; use a copy of the group to test the error.
        let mut mi2 = MissionInfo::new("campaign", &campaign());
        mi2.groups[0].number_to_be_completed = 2;
        mi2.set_next_mission("a").unwrap();
        mi2.set_mission_complete();
        assert_eq!(mi2.current_stage, 0);
        assert_eq!(
            mi2.set_next_mission("a"),
            Err(LogisticsError::MissionAlreadyDone)
        );
        assert_eq!(mi2.current_mission, 1, "moved on to b");
    }

    #[test]
    fn open_mandatory_missions_hold_the_group_and_unhide_the_hidden_one() {
        let mut mi = MissionInfo::new("campaign", &campaign());
        mi.set_next_mission("a").unwrap();
        mi.set_mission_complete();
        assert_eq!(mi.current_stage, 1);
        mi.set_next_mission("c").unwrap();
        mi.set_mission_complete();
        // c done (quota 1 met) but d is mandatory and open: stay, reveal d, select it.
        assert_eq!(mi.current_stage, 1);
        assert_eq!(mi.current_mission, 1);
        assert_eq!(mi.current_mission_name, "d");
        assert_eq!(mi.current_missions(), vec!["c", "d"]);
        assert_eq!(mi.groups[1].video_file_name, "D_VIDEO.bik");
        assert!(!mi.groups[1].video_shown);
        assert_eq!(mi.current_video(), Some("D_VIDEO.bik"));
        // Mission id counts group 0 (2 missions) + c's index... last was c (group 1, index 0).
        assert_eq!(mi.current_mission_id(), 2);
        mi.set_mission_complete();
        assert_eq!(mi.current_stage, 2);
        assert_eq!(mi.current_mission_name, "e");
        assert_eq!(
            mi.current_mission_id(),
            2 + 1,
            "d: group0=2, group1 index 1"
        );
    }

    #[test]
    fn finishing_the_last_group_ends_the_campaign() {
        let mut mi = MissionInfo::new("campaign", &campaign());
        for name in ["a", "c", "d", "e"] {
            mi.set_next_mission(name).unwrap();
            mi.set_mission_complete();
        }
        assert!(mi.campaign_over());
        assert_eq!(mi.current_stage, 3);
        assert_eq!(mi.current_logistics_tune_id(), -1);
        assert!(mi.available_missions().is_empty());
        assert!(mi.current_abl_script_name().is_none());
        assert!(!mi.skip_logistics());
        mi.set_mission_complete(); // a no-op after the end
        assert_eq!(mi.current_stage, 3);
    }

    #[test]
    fn big_video_is_handed_out_once_per_group() {
        let mut mi = MissionInfo::new("campaign", &campaign());
        // Group 0 has no PreVideo: the name is empty but the flag still flips.
        assert_eq!(mi.take_current_big_video(), Some(String::new()));
        assert_eq!(mi.take_current_big_video(), None);
        mi.set_next_mission("a").unwrap();
        mi.set_mission_complete();
        assert_eq!(mi.take_current_big_video(), Some("PRE1".into()));
        assert_eq!(mi.take_current_big_video(), None);
    }

    #[test]
    fn skip_flags_look_at_the_current_or_previous_mission() {
        let mut c = (*campaign()).clone();
        c.groups[0].missions[0].play_logistics = false;
        c.groups[0].missions[0].play_salvage = false;
        c.groups[0].missions[0].play_pilot_promotion = false;
        c.groups[0].missions[1].play_purchasing = false;
        c.groups[0].missions[1].play_mission_selection = true;
        let mut mi = MissionInfo::new("campaign", &Arc::new(c));
        mi.set_next_mission("a").unwrap();
        assert!(mi.skip_logistics());
        assert!(!mi.show_choose_mission());
        assert!(!mi.skip_purchasing());
        // Before anything is completed, the "previous" mission of stage 0 index 0 is the last
        // mission of the first group (the tutorial case): b, which has defaults.
        assert!(!mi.skip_salvage_screen());
        mi.set_next_mission("b").unwrap();
        assert!(mi.skip_purchasing());
        assert!(mi.show_choose_mission());
        // With b current, the previous mission is a.
        assert!(mi.skip_salvage_screen());
        assert!(mi.skip_pilot_review());
    }

    #[test]
    fn bonus_files_accumulate_without_deduplication() {
        let mut mi = MissionInfo::new("campaign", &campaign());
        mi.add_bonus_purchase_file("data\\missions\\PurBonus08.fit");
        mi.add_bonus_purchase_file("data\\missions\\PurBonus08.fit");
        assert_eq!(mi.additional_purchase_files.len(), 2);
    }
}
