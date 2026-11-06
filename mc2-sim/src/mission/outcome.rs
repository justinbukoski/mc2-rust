// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Campaign handoff types and builder, matching `mc2-campaign::interface`.

use crate::object::Kind;

use super::objective::objective_status_at;
pub use super::objective::ObjectiveStatus;

/// Mission result enum consumed by the campaign adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MissionResult {
    #[default]
    Playing = 0,
    PlayerLostBig = 1,
    PlayerLostSmall = 2,
    PlayerDraw = 3,
    PlayerWinSmall = 4,
    PlayerWinBig = 5,
}

impl MissionResult {
    fn from_code(code: u32) -> Self {
        match code {
            1 => Self::PlayerLostBig,
            2 => Self::PlayerLostSmall,
            3 => Self::PlayerDraw,
            4 => Self::PlayerWinSmall,
            5 => Self::PlayerWinBig,
            _ => Self::Playing,
        }
    }
}

/// Objective result and reward, same field shape as the campaign's report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectiveReport {
    pub status: ObjectiveStatus,
    pub resource_points: i32,
}

/// `WarriorStatus` at mission end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WarriorStatus {
    #[default]
    Normal,
    Withdrawing,
    Withdrawn,
    Ejected,
    Dead,
    Mia,
    Captured,
    Basecamp,
}

/// A killed object category, in campaign report order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KilledKind {
    BattleMech { air: bool },
    GroundVehicle,
    Other,
}

/// What a kill marker needs of the killed mover: its icon picture and the armour of each location
/// at the end of the mission, which `MechIcon::update` / `VehicleIcon::update` colour the icon by.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KilledUnit {
    /// `getIconPictureIndex()`.
    pub picture: i32,
    /// Per armour location `(current armour, maximum armour, body location destroyed)`: a mech's
    /// eleven `MECH_ARMOR_LOCATION_*`, a ground vehicle's first five.
    pub armor: [(f32, f32, bool); 11],
}

/// Pilot progression and combat report. Kill tracking is populated by lane C/D when those lanes expose their counters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PilotReport {
    pub status: WarriorStatus,
    pub gunnery_rank: f32,
    pub piloting_rank: f32,
    pub gunnery_points: f32,
    pub piloting_points: f32,
    pub mission_kills: [i32; 7],
    pub rank: i32,
    pub wounds: f32,
    pub killed: Vec<KilledKind>,
    /// The icon of each killed object, in the order of `killed` (what `MechIcon::init` /
    /// `VehicleIcon::init` read from the dead mover for the review's kill marker).
    pub killed_units: Vec<KilledUnit>,
    pub specialty_skills: [bool; 23],
}

/// One home force mover, in roster order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoverReport {
    pub on_gui: bool,
    pub is_battle_mech: bool,
    pub home_commander: bool,
    pub is_air: bool,
    pub destroyed: bool,
    pub disabled: bool,
    pub variant_name: String,
    pub pilot_file: Option<String>,
    pub paint_scheme: [u32; 3],
    pub pilot: PilotReport,
}

/// One mech available to salvage evaluation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MechSummary {
    pub variant_name: String,
    pub tonnage: f32,
    pub destroyed: bool,
    pub disabled: bool,
    pub move_level: i32,
    /// The mech's paint scheme (`getPaintScheme`), which the salvage screen's 3D view wears.
    pub paint_scheme: [u32; 3],
    /// `(current armour, maximum armour, body location destroyed)` per armour location, as the damage colours of the wreck's icon read them.
    pub armor: [(f32, f32, bool); 11],
}

impl MechSummary {
    /// `SalvageMechScreen::init`: intact disabled ground mechs can be recovered.
    pub fn is_salvage_candidate(&self) -> bool {
        self.disabled && !self.destroyed && self.move_level != 2
    }
}

/// A salvageable mech.
#[derive(Clone, Debug, PartialEq)]
pub struct SalvageCandidate {
    pub variant_name: String,
    pub tonnage: f32,
    pub paint_scheme: [u32; 3],
    /// `(current armour, maximum armour, body location destroyed)` per armour location, as the damage colours of the wreck's icon read them.
    pub armor: [(f32, f32, bool); 11],
}

/// Mission result returned to campaign logistics, matching its `Outcome` fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub result: MissionResult,
    pub objectives: Vec<ObjectiveReport>,
    pub unlocked_purchase_files: Vec<String>,
    pub movers: Vec<MoverReport>,
    pub mechs: Vec<MechSummary>,
    pub mission_time: f32,
}

impl Outcome {
    /// The mechs a salvage crew could recover.
    pub fn salvage_candidates(&self) -> Vec<SalvageCandidate> {
        self.mechs
            .iter()
            .filter(|mech| mech.is_salvage_candidate())
            .map(|mech| SalvageCandidate {
                variant_name: mech.variant_name.clone(),
                tonnage: mech.tonnage,
                paint_scheme: mech.paint_scheme,
                armor: mech.armor,
            })
            .collect()
    }
}

/// `LogisticsData::setMissionCompleted` (logisticsdata.cpp:1320-1400) and `LogisticsPilot::update`, read from the world.
///
/// Takes the world mutably: the original asks every objective of the home team for its status here
/// (`Status(Team::home->objectives)`, logisticsdata.cpp:1329-1336), which can run actions, and it calls them in list order.
/// Each status is that call's result, not the cached runtime status.
pub fn build(w: &mut crate::World) -> Outcome {
    let home = w.teams.home;
    let count = w
        .mission
        .objectives
        .get(home)
        .map_or(0, |team| team.entries.len());
    let mut objectives = Vec::with_capacity(count);
    for index in 0..count {
        let status = objective_status_at(w, home, index);
        let points = w.mission.objectives[home].entries[index]
            .def
            .resource_points;
        objectives.push(ObjectiveReport {
            status,
            resource_points: points.min(i32::MAX as u32) as i32,
        });
    }
    let w = &*w;

    let mut movers = Vec::new();
    if let Some(Some(team)) = w.teams.teams.get(w.teams.home) {
        for &wid in &team.roster {
            let Some(handle) = w.objects.by_watch_id(wid) else {
                continue;
            };
            let Some(object) = w.objects.get(handle) else {
                continue;
            };
            let (mech, commander, on_gui, air, variant_name, pilot_slot) = match &object.kind {
                Kind::Mech(m) => (
                    true,
                    m.m.commander.0,
                    m.m.flags.on_gui,
                    m.m.move_level == 2,
                    m.variant_name.clone(),
                    m.m.pilot_slot,
                ),
                Kind::Vehicle(v) => (
                    false,
                    v.m.commander.0,
                    v.m.flags.on_gui,
                    v.m.move_level == 2,
                    String::new(),
                    v.m.pilot_slot,
                ),
                _ => continue,
            };
            let pilot = w.pilots.get(pilot_slot);
            let load = w.mission.pilot_loads.get(pilot_slot as usize);
            // The pilot's own profile, whichever path loaded it: a mission-file warrior or a pilot added later, such as the
            // logistics force (`add_force` -> `add_mover`). The campaign matches pilots by this file name (logistics.rs:1701). The
            // C++ hands over `getPilot()->getName()` (logisticsdata.cpp:1351-1360); the port's pilot key is the profile file.
            let pilot_file = load
                .and_then(|load| load.profile.as_ref())
                .map(|profile| profile.file_name.clone());
            let pilot_report = match (pilot, load) {
                (Some(pilot), Some(load)) => {
                    // `getRank()` / `skillRank` / `skillPoints` come from the load record, the store lane E keeps for them.
                    // TODO(lane D): `handleKilledTarget` adds the kill skill points there (warrior.cpp:7120); until it does, the
                    // progression reported here is the load's.
                    // `getStatus()` (warrior.h:1107), not the wound count: `injure` sets DEAD only on its death paths, so six wounds
                    // with an escape by ejection leave the pilot alive (warrior.cpp:1951-1982).
                    PilotReport {
                        status: pilot.status,
                        gunnery_rank: load.skill_rank[mc2_sim_data::pilot::Skill::Gunnery.index()],
                        piloting_rank: load.skill_rank
                            [mc2_sim_data::pilot::Skill::Piloting.index()],
                        gunnery_points: load.skill_points
                            [mc2_sim_data::pilot::Skill::Gunnery.index()],
                        piloting_points: load.skill_points
                            [mc2_sim_data::pilot::Skill::Piloting.index()],
                        mission_kills: std::array::from_fn(|i| pilot.num_mech_kills[i][1]),
                        rank: load.rank.map_or(0, |rank| match rank {
                            mc2_sim_data::pilot::WarriorRank::Green => 0,
                            mc2_sim_data::pilot::WarriorRank::Regular => 1,
                            mc2_sim_data::pilot::WarriorRank::Veteran => 2,
                            mc2_sim_data::pilot::WarriorRank::Elite => 3,
                            mc2_sim_data::pilot::WarriorRank::Ace => 4,
                        }),
                        wounds: pilot.wounds,
                        killed: pilot
                            .killed
                            .iter()
                            .map(|&wid| killed_kind(w, wid))
                            .collect(),
                        killed_units: pilot
                            .killed
                            .iter()
                            .map(|&wid| killed_unit(w, wid))
                            .collect(),
                        specialty_skills: pilot.specialty,
                    }
                }
                _ => PilotReport::default(),
            };
            let paint_scheme = w
                .mission
                .paint_schemes
                .iter()
                .find(|(painted, _)| *painted == wid)
                .map_or([0; 3], |(_, colors)| *colors);
            movers.push(MoverReport {
                on_gui,
                is_battle_mech: mech,
                home_commander: commander == 0,
                is_air: air,
                destroyed: object.c.is_destroyed(),
                disabled: object.c.is_disabled(),
                variant_name,
                pilot_file,
                paint_scheme,
                pilot: pilot_report,
            });
        }
    }

    let mechs = w
        .objects
        .mechs
        .iter()
        .take(w.objects.num_mechs)
        .filter_map(|&handle| w.objects.get(handle).map(|object| (handle, object)))
        .filter_map(|(handle, object)| match &object.kind {
            Kind::Mech(mech) => Some(MechSummary {
                variant_name: mech.variant_name.clone(),
                tonnage: object.c.tonnage,
                destroyed: object.c.is_destroyed(),
                disabled: object.c.is_disabled(),
                move_level: i32::from(mech.m.move_level),
                // a painted mech has had its watch id assigned
                paint_scheme: w
                    .mission
                    .paint_schemes
                    .iter()
                    .find(|(painted, _)| *painted == w.objects.watch_id_peek(handle))
                    .map_or([0; 3], |(_, colors)| *colors),
                armor: armor_of(object, true),
            }),
            _ => None,
        })
        .collect();

    Outcome {
        result: MissionResult::from_code(w.mission.term.scenario_result),
        objectives,
        unlocked_purchase_files: w.mission.unlocked_purchase_files.clone(),
        movers,
        mechs,
        mission_time: w.mission.actual_time,
    }
}

/// `MechArmorToBodyLocation` (`mech.cpp:427`): the body location whose `damageState` marks an armour
/// location destroyed.
const ARMOR_TO_BODY: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 1, 2, 3];

/// `(current armour, maximum armour, body location destroyed)` of each armour location of a mech or vehicle, which the icon's
/// damage colours (`MechIcon::update`) read.
fn armor_of(object: &crate::object::Obj, mech: bool) -> [(f32, f32, bool); 11] {
    let mut armor = [(0.0, 0.0, false); 11];
    if let Some(l) = crate::combat::loadout_of(object) {
        for (i, slot) in armor.iter_mut().enumerate() {
            let (cur, max) = l.armor.get(i).copied().unwrap_or((0.0, 0));
            let destroyed = mech
                && l.body
                    .get(ARMOR_TO_BODY[i])
                    .is_some_and(|b| b.damage_state == crate::combat::stats::IS_DAMAGE_DESTROYED);
            *slot = (cur, f32::from(max), destroyed);
        }
    }
    armor
}

/// What the review's kill marker reads of the killed object (`killed[i]`): the icon picture index
/// `MechIcon::init` / `VehicleIcon::init` take from the mover (`getIconPictureIndex`: the chassis'
/// `icon_picture_index` for a mech, the vehicle's `IconIndex`) and its armour. All zero when the
/// object or its data is gone.
fn killed_unit(w: &crate::World, wid: crate::ids::WatchId) -> KilledUnit {
    let Some(object) = w.objects.by_watch_id(wid).and_then(|h| w.objects.get(h)) else {
        return KilledUnit::default();
    };
    let (picture, mech) = match &object.kind {
        Kind::Mech(_) => (
            w.data
                .object_types
                .as_deref()
                .and_then(|t| t.get(object.c.type_handle as u32))
                .and_then(|t| t.base.appearance_name.as_ref())
                .and_then(|n| w.data.chassis.get(&n.to_ascii_lowercase()))
                .and_then(|c| c.icon_picture_index)
                .unwrap_or(0),
            true,
        ),
        Kind::Vehicle(_) => (
            w.data
                .vehicle_objects
                .get(&(object.c.type_handle as u32))
                .map_or(0, |v| v.icon_index),
            false,
        ),
        _ => return KilledUnit::default(),
    };
    let armor = armor_of(object, mech);
    KilledUnit { picture, armor }
}

/// `LogisticsPilot::update`'s view of one kill (logisticspilot.cpp:318-345): a mech or ground vehicle by class. A target that no
/// longer resolves counts as `Other`; the original dereferences it.
fn killed_kind(w: &crate::World, wid: crate::ids::WatchId) -> KilledKind {
    let Some(object) = w.objects.by_watch_id(wid).and_then(|h| w.objects.get(h)) else {
        return KilledKind::Other;
    };
    match &object.kind {
        Kind::Mech(m) => KilledKind::BattleMech {
            air: m.m.move_level == 2,
        },
        Kind::Vehicle(_) => KilledKind::GroundVehicle,
        _ => KilledKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::pilot::Pilot;
    use crate::ai::team::Team;
    use crate::ids::{CommanderId, ObjectHandle};
    use crate::mission::pilots::PilotLoad;
    use crate::mover::MechState;
    use crate::object::{flags, Common, Obj, ObjectClass};
    use crate::rng::RngMode;
    use crate::World;
    use mc2_formats::ini::FitIni;
    use mc2_sim_data::pilot::WarriorProfile;
    use std::rc::Rc;

    fn profile(file_name: &str) -> Rc<WarriorProfile> {
        const FIT: &str = "FITini\n\n[General]\nst Name = \"Force\"\n[PersonalityTraits]\n\
            c Professionalism = 50\nc Decorum = 50\nc Aggressiveness = 40\nc Courage = 50\n\
            [Skills]\nc Piloting = 56\nc Sensors = 43\nc Gunnery = 52\n[Status]\nc Wounds = 0\nFITEnd\n";
        let mut fit = FitIni::from_bytes(FIT.as_bytes().to_vec()).expect("FIT header");
        Rc::new(WarriorProfile::from_fit(file_name, &mut fit).expect("profile"))
    }

    /// One home force: a mech in roster slot 1 with the given pilot record and status, as `add_force` leaves it.
    fn force_world(pilot: Pilot, load: PilotLoad) -> (World, ObjectHandle) {
        let mut w = World::new(RngMode::Pcg32, 1);
        let handle = ObjectHandle(1);
        let mut common = Common::new(ObjectClass::BattleMech, handle);
        common.flags |= flags::EXISTS;
        let mut mech = MechState::default();
        mech.m.commander = CommanderId(0);
        mech.m.flags.on_gui = true;
        mech.m.pilot_slot = 1;
        mech.variant_name = "force mech".into();
        w.objects.list = vec![
            None,
            Some(Obj {
                c: common,
                kind: Kind::Mech(Box::new(mech)),
            }),
        ];
        let wid = w.objects.watch_id(handle);
        w.teams.teams = vec![Some(Team {
            roster: vec![wid],
            num_primary_objectives: 0,
        })];
        w.pilots.slots = vec![Pilot::default(), pilot];
        w.mission.pilot_loads = vec![PilotLoad::default(), load];
        (w, handle)
    }

    /// A logistics-force pilot is added by `add_mover`, so it is not in the mission file's warrior list. The campaign still needs
    /// its identity to match the pilot it owns (logisticsdata.cpp:1351-1360, logistics.rs:1782-1790). Without a profile the pilot
    /// has no identity, as the C++ `getName()` of a default pilot is empty.
    #[test]
    fn logistics_force_pilot_keeps_its_identity() {
        let load = PilotLoad {
            profile: Some(profile("pmp_force")),
            ..PilotLoad::default()
        };
        let (mut w, _) = force_world(Pilot::default(), load);

        let report = build(&mut w);
        assert_eq!(report.movers.len(), 1);
        assert_eq!(report.movers[0].pilot_file.as_deref(), Some("pmp_force"));

        w.mission.pilot_loads[1].profile = None;
        let report = build(&mut w);
        assert_eq!(report.movers[0].pilot_file, None);
    }

    /// `MechWarrior::injure` sets DEAD only on its death paths (warrior.cpp:1976-1982), so six wounds with an escape leave the pilot
    /// alive. The campaign reads the status (`getStatus`, warrior.h:1107), so the report must say `Normal`, not `Dead`.
    #[test]
    fn six_wounds_alone_do_not_report_a_dead_pilot() {
        let pilot = Pilot {
            wounds: 6.0,
            ..Pilot::default()
        };
        let (mut w, _) = force_world(pilot, PilotLoad::default());
        assert_eq!(build(&mut w).movers[0].pilot.status, WarriorStatus::Normal);

        w.pilots.slots[1].status = WarriorStatus::Dead;
        assert_eq!(build(&mut w).movers[0].pilot.status, WarriorStatus::Dead);
    }
}
