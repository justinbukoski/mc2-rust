// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The [`World`]: all simulation state, no globals.
//!
//! The C++ keeps the simulation in dozens of globals (`ObjectManager`, `land`, `GameMap`, `Team::home`, `MechWarrior::warriorList`,
//! `PathManager`, `SensorManager`, `turn`, `scenarioTime`, `frameLength`, `mission`, tuning constants loaded from gamesys.fit...).
//! `World` owns all of them, grouped by the lane that implements them, so two worlds can run side by side (tests, replays) and a
//! world can be saved and restored.

use std::rc::Rc;

use crate::appearance::AppearanceSystem;
use crate::events::SimEvent;
use crate::mission::MissionState;
use crate::mover::{MoveScratch, PathManager};
use crate::object::ObjectStore;
use crate::rng::{RngStreams, SharedSimRng};
use crate::terrain::Terrain;
use crate::visibility::{AllVisible, Visibility};

/// Static game data loaded once (`ObjectTypeManager`, `MasterComponent::masterList`, gamesys.fit constants, pilot profiles).
/// Shared by reference between worlds.
#[derive(Clone, Debug, Default)]
pub struct GameData {
    /// Object types by number (`data/objects/object2.pak`).
    pub object_types: Option<Rc<mc2_sim_data::objtype::ObjectTable>>,
    /// Master component list (`compbas.csv`).
    pub components: Option<Rc<mc2_sim_data::component::MasterComponentList>>,
    /// Constants from `gamesys.fit` used by lanes B, C, D, E (`WeaponRanges`, `visualRangeTable`, hit tables, timers, ...).
    pub gamesys: GameSystem,
    /// Chassis CSVs by lower-case file stem (`data/objects/<stem>.csv`), loaded for every mech type by `Mission::init`.
    /// Lane E appends; read by `BattleMech::init(variant)` for runtime spawns (`Mission::addMover`).
    pub chassis: std::collections::BTreeMap<String, Rc<mc2_sim_data::mech::chassis::Chassis>>,
    /// Ground vehicle packets of `object2.pak` (`GroundVehicle::init(FitIniFile*)` data) by object type number.
    pub vehicle_objects:
        std::collections::BTreeMap<u32, Rc<mc2_sim_data::mech::vehicle::GroundVehicleObject>>,
    /// Pilot profiles by file stem (`data/missions/warriors/<stem>.fit`), so spawning needs no file system. `None` records a file
    /// that is missing (or has no `[General]` block): `MechWarrior::init(FitIniFile*)` fails at once and the pilot keeps its defaults.
    pub pilot_profiles:
        std::collections::BTreeMap<String, Option<Rc<mc2_sim_data::pilot::WarriorProfile>>>,
}

/// Constants loaded from `gamesys.fit` by `Mission::init` and the `loadGameSystem` functions. Lanes extend this struct with the
/// blocks they own (C: weapon ranges, hit/critical tables, mines, difficulty; B: move yield/timeout, vehicle speeds; D:
/// update frequencies).
#[derive(Clone, Debug, Default)]
pub struct GameSystem {
    /// `visualRangeTable[256]`.
    pub visual_range_table: Vec<i32>,
    /// `maxVisualRange`, `baseSensorRange`, `MaxTreeLOSCellBlock`.
    pub ranges: [f32; 3],
    /// Every table `Mission::init` reads from `gamesys.fit` (weapon ranges, difficulty, mover/mech/vehicle tables, skills, mines),
    /// typed in `mc2_sim_data::gamesys`. Filled by `init_mission`; `visual_range_table` and `ranges` above mirror `general`.
    pub tables: Option<Rc<mc2_sim_data::gamesys::GameSystem>>,
}

/// Everything about one running mission.
pub struct World {
    /// Next deterministic presentation identity for a weapon shot.
    pub(crate) next_weapon_shot_id: u32,
    /// Preserve the original C++ pathing quirks by default. Applications may disable this when
    /// intentionally opting into corrected Rust path behavior.
    pub legacy_quirks: bool,
    /// Test-only switch `NO-LOF-BESTBEST` (`DIFFERENCES.md`): true makes `no_lof_fallback_goal` pick its cell as the C++ does (a goal stays the last cell the
    /// line-of-sight scan looked at when candidate 0 has no route). The shipped game keeps it false and runs the corrected pick; the fidelity sweep
    /// sets it by default for the missions in `DEFAULT_SWITCHES`, per world.
    pub cpp_no_lof_best_best: bool,
    /// Test-only switch `D-106` (`DIFFERENCES.md`): true makes the vehicle-crew bail-out behave as the C++ does (the crew is removed on its
    /// creation frame whether or not it is in view, and the first crew stays out of the mover lists). Only the fidelity sweep sets it, per
    /// world; the shipped game never does and keeps the visible crew.
    pub cpp_vehicle_pilot_d106: bool,
    /// Test-only switch `JUMP-EDGE-CELLS` (`DIFFERENCES.md`): true makes a jump-capable mover's search never enter row 0 or column 0 of its
    /// window, as the C++ build's `MoveMap::calcPathJUMP` does (move.cpp:5605). Only the fidelity sweep and the pinning test set it, per
    /// world; the shipped game never does.
    pub cpp_jump_edge_cells: bool,
    /// Static data.
    pub data: GameData,
    /// `turn` / `frameLength` / `scenarioTime`: the one and only clock.
    pub clock: crate::time::FrameClock,
    /// The two RNG streams, shared with the ABL host.
    pub rng: SharedSimRng,
    /// Lane T.
    pub terrain: Terrain,
    /// Lane A.
    pub objects: ObjectStore,
    /// Lane F.
    pub appearance: AppearanceSystem,
    /// Lane D.
    pub pilots: crate::ai::PilotPool,
    /// Lane D.
    pub teams: crate::ai::Teams,
    /// Lane B.
    pub paths: PathManager,
    /// Lane B (shared scratch floats, see [`MoveScratch`]).
    pub move_scratch: MoveScratch,
    /// Lane E.
    pub mission: MissionState,
    /// Camera-derived inputs for the current frame.
    pub visibility: Rc<dyn Visibility>,
    /// Output queue for presentation; drained by the application after each step.
    pub events: Vec<SimEvent>,
    /// Every unported pilot behaviour reached this frame (unlike the once-per-pilot [`SimEvent::AiUnavailable`]); cleared at the
    /// start of each [`crate::Sim::step`] and reported in [`crate::FrameOutput::unavailable`].
    pub ai_unavailable_frame: Vec<(i32, crate::ai::AiUnavailable)>,
    /// Pilot ABL native ids reached in this frame, in first-call order. This is
    /// diagnostic evidence only; it is cleared with the other per-frame AI
    /// observations and never changes simulation decisions.
    pub ai_native_calls_frame: Vec<(i32, u16)>,
    /// Kinds already announced through [`SimEvent::AiUnavailable`] with part id 0 (no pilot mover: mission scripts, scripts that
    /// selected a non-mover).
    pub ai_unavailable_unowned: Vec<crate::ai::AiUnavailable>,
    /// The ABL environment (taken out while scripts run).
    pub abl: Option<mc2_abl::Env>,
    /// A script is running (the environment is out of `abl`): brain calls made now wait in `deferred_brain_calls` (never saved; empty
    /// between frames).
    pub abl_running: bool,
    /// Brain runs and alarm handlers requested while a script was running, in request order (see [`crate::ai::brain`]).
    pub deferred_brain_calls: Vec<crate::ai::brain::DeferredBrainCall>,
    /// Brain calls run nested inside natives during the current outermost script (the shared nesting budget).
    pub nested_runs: u32,
    /// Load-time hooks that ran as empty stubs because their lane is not merged (see [`World::note_unported_hook`]). A world with a
    /// non-empty list is an INCOMPLETE load: its digest cannot show the missing state, so no pass on it proves a complete load.
    pub unported_load_hooks: Vec<&'static str>,
    /// Lane D sensor pool and team contact state (`SensorSystemManager`).
    pub sensors: crate::ai::sensors::SensorManager,
    /// Every unported combat behaviour (team LOS for turrets, sensor strikes, vehicle weapon nodes, ...) reached this frame;
    /// cleared at the start of each [`crate::Sim::step`] and reported in [`crate::FrameOutput::unavailable`].
    pub combat_unavailable_frame: Vec<&'static str>,
    /// The attack, guard and jump decisions of the pilots (`combatDecisionTree`, the attack movement decisions, the attack and guard
    /// branches of `calcTacOrder`, executing a jump order) are ported but not verified against the oracle: they are the next slice,
    /// with lane C's firing. They run only when this is set (their unit tests do); otherwise they report
    /// [`AiUnavailable`](crate::ai::AiUnavailable) and change nothing. Appended: lane D.
    pub ai_unverified_decisions: bool,
    /// The camera as mission scripts read it (`GetCamera*` natives). The camera belongs to the application, which stores its
    /// state here before each step; the `SetCamera*` natives also update it so a script reads back what it just set.
    pub camera: crate::events::CameraState,
    /// Movie mode, letterbox, fade, GUI freeze, mouse profile and the mouse callout (`natives_presentation`). The application draws
    /// it; the scripts read part of it back.
    pub presentation: crate::mission::natives_presentation::Presentation,
    /// The sound system as mission scripts read it (`getCurrentMusicId`, `isPlayingVoiceOver`, `playBettySample`). The application
    /// stores its state here before each step; the audio natives also update it at once (see
    /// [`crate::events::AudioReadback`]). Appended: lane audio.
    pub audio: crate::events::AudioReadback,
    /// The mission interface's callout animation (`incallout`, `animationcallout`), which belongs to the application: stored here
    /// before each step like the camera and the sound readback (see [`crate::events::CalloutState`]). Appended: lane audio.
    pub callout: crate::events::CalloutState,
    /// The campaign's pilot roster for recovered mechs (`LogisticsData::getBestPilot`); installed by the host, never saved. Without
    /// one a recovery is refused ([`crate::mission::trade::TradeError::NoPilot`]).
    pub recovery_pilots: Option<Rc<dyn crate::mission::trade::RecoveryPilots>>,
    /// The support-vehicle VTOLs in flight (`vTol[]`); transient GUI state the C++ does not save either.
    pub vtol: crate::mission::vtol::VtolSystem,
}

impl World {
    /// Allocate a unique per-world weapon shot identity without consuming simulation RNG.
    pub(crate) fn allocate_weapon_shot_id(&mut self) -> u32 {
        let id = self.next_weapon_shot_id;
        self.next_weapon_shot_id = self.next_weapon_shot_id.wrapping_add(1);
        id
    }

    /// Record that an unported combat behaviour was reached this frame (once per hook name, in order of first occurrence).
    ///
    pub fn note_combat_unavailable(&mut self, hook: &'static str) {
        if !self.combat_unavailable_frame.contains(&hook) {
            self.combat_unavailable_frame.push(hook);
        }
    }

    /// Record a per-frame unavailable input or behaviour (once per name per frame) for [`crate::FrameOutput::unavailable`] only. Like
    /// [`World::note_combat_unavailable`] it does not mark the world as holding an unported load hook, so stepping continues.
    pub fn note_frame_unavailable(&mut self, hook: &'static str) {
        if !self.combat_unavailable_frame.contains(&hook) {
            self.combat_unavailable_frame.push(hook);
        }
    }

    /// Record that an empty load-time stub ran (once per hook name).
    pub fn note_unported_hook(&mut self, name: &'static str) {
        if !self.unported_load_hooks.contains(&name) {
            self.unported_load_hooks.push(name);
        }
    }

    /// True only when no load-time stub ran, i.e. the mission load is complete.
    pub fn load_is_complete(&self) -> bool {
        self.unported_load_hooks.is_empty()
    }

    /// An empty world with the given RNG algorithm and seed. Missions are loaded with
    /// [`mission::init::init_mission`](crate::mission::init::init_mission).
    pub fn new(rng_mode: crate::rng::RngMode, seed: u32) -> World {
        World {
            next_weapon_shot_id: 1,
            legacy_quirks: true,
            cpp_no_lof_best_best: false,
            cpp_vehicle_pilot_d106: false,
            cpp_jump_edge_cells: false,
            data: GameData::default(),
            clock: Default::default(),
            rng: SharedSimRng::new(RngStreams::new(rng_mode, seed)),
            terrain: Terrain::default(),
            objects: ObjectStore::default(),
            appearance: AppearanceSystem::default(),
            pilots: Default::default(),
            teams: Default::default(),
            paths: PathManager::default(),
            move_scratch: MoveScratch::default(),
            mission: MissionState::default(),
            visibility: Rc::new(AllVisible),
            events: Vec::new(),
            ai_unavailable_frame: Vec::new(),
            ai_native_calls_frame: Vec::new(),
            combat_unavailable_frame: Vec::new(),
            ai_unavailable_unowned: Vec::new(),
            abl: None,
            abl_running: false,
            deferred_brain_calls: Vec::new(),
            nested_runs: 0,
            unported_load_hooks: Vec::new(),
            sensors: Default::default(),
            ai_unverified_decisions: false,
            camera: Default::default(),
            presentation: Default::default(),
            audio: Default::default(),
            callout: Default::default(),
            recovery_pilots: None,
            vtol: Default::default(),
        }
    }
}
