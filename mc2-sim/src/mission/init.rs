// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Mission::init` (mission.cpp:1639-3027): the load sequence and its dependency order, single player.
//!
//! Order (each step needs the ones before it; the C++ line ranges are on the helpers):
//!
//! 1.  reset: `terminationResult`, `turn`, team relations (`teamRelationsForSP`);
//! 2.  `gamesys.fit` constants (weapon ranges, `[General]`, difficulty, mover/mech/vehicle tables, mines) BEFORE the master component
//!     list, which needs `baseSensorRange`;
//! 3.  the mission `.fit`: sky, teams and commanders (pre-scan of the raw part team/commander ids), trigger areas, sensor manager,
//!     drop zone, resource points;
//! 4.  object types, terrain (+ move data), the ABL environment, libraries and the mission script;
//! 5.  PILOTS: every `[WarriorN]` takes pool slot N; `MechWarrior::init` draws one `RandomNumber(100)` each;
//! 6.  PARTS: the squad pre-pass and the ONE `RandomNumber(bigAlternatives + 1)` draw, the part records (alternatives applied),
//!     mover counts;
//! 7.  `setNumObjects` (lane A; the turret constructors draw `RandomNumber(10)` each), then the movers team-major (watch ids are
//!     handed out in this order), then `loadTerrainObjects`, then `buildMoverLists`;
//! 8.  time limit, objectives (`Team::loadObjectives`: positions resolve to watch ids now that the objects exist), commander groups
//!     (part ids), team rosters (mover list order), the GUI's `initMechs` selection, clocks reset, `initOptimalCells`.
//!
//! Dependencies on other lanes are explicit calls (`todo!()` in the owning lane's scaffold file until that lane lands): lane T `Terrain::load`
//! and the cell setters (landed), lane A `object::load::build_for_mission` with the mover-creation closure, `ObjectStore::{watch_id,
//! set_part_id, modify_mover_lists}`, `object::set_position`, `object::trigger::set_hit` (landed), lane B `mover::init_optimal_cells`,
//! lane C `combat::init_mover_stats` / `reset_components`, lane D `ai::sensors::{init, register_mover, scan_mover}` and
//! `ai::trigger_alarm`, lane F `AppearanceSystem::{mech_type, create_mech}` (landed). See `rust/docs/porting-log.md` (lane E load).

use std::rc::Rc;

use crate::ai::team::{Commander, Relation, Team};
use crate::math::Vec3;
use crate::mission::plan::{self, PlanInputs};
use crate::mission::{abl_load, objective, pilots, spawn, MissionState};
use crate::object::{flags, Kind};
use mc2_formats::vfs::Vfs;
use mc2_sim_data::gamesys::GameSystem;
use mc2_sim_data::mission::Mission;
use mc2_sim_data::objtype::{ObjectTable, ObjectTypeData, OBJECT_PAK_PATH};
use mc2_sim_data::pilot::WarriorProfile;

/// `MISSION_LOAD_*` (single player only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadType {
    /// `MISSION_LOAD_SP_QUICKSTART`: every part in the mission file is created (the `-mission` command line path).
    SpQuickstart,
    /// `MISSION_LOAD_SP_LOGISTICS`: commander-0 mechs come from the player's force.
    SpLogistics,
}

/// One mech of the player's force handed over by logistics (`LogisticsMech`, as `Logistics::beginMission` reads it).
#[derive(Clone, Debug)]
pub struct ForceMech {
    /// `getFitID()`: the object type number.
    pub object_number: i32,
    /// `getFileName()`: the chassis csv.
    pub chassis_file: String,
    /// The pilot's file stem; `None` for a mech without a pilot (the loop skips it).
    pub pilot_file: Option<String>,
    /// The pilot's gunnery (`getGunnery()`).
    pub gunnery: f32,
    /// The pilot's piloting (`getPiloting()`).
    pub piloting: f32,
    /// The pilot's specialties.
    pub specialty: [bool; 23],
    /// `Some(variant file id)` for a designer mech (loaded as that variant); `None` for a custom loadout (variant 0, then
    /// `resetComponents(components)`).
    pub designer_variant: Option<u32>,
    /// `getName()`: the variant name shown for the mech.
    pub variant_name: String,
    /// `getComponents()`: master component ids of a custom loadout.
    pub components: Vec<i32>,
    /// `baseColor`, `highlightColor1`, `highlightColor2` the mech is painted with (logistics preference colors).
    pub colors: [u32; 3],
}

/// Inputs of a mission load that the original read from globals (`LogisticsData`, prefs, command line).
#[derive(Clone, Debug)]
pub struct LoadSpec {
    /// Mission name (`mc2_01`).
    pub mission: String,
    /// Load type.
    pub load_type: LoadType,
    /// `GameDifficulty` (`options.cfg` `Difficulty`, default 1).
    pub difficulty: i32,
    /// `useUnlimitedAmmo` (default true).
    pub unlimited_ammo: bool,
    /// `MaxResourcePoints` override (-1 none).
    pub max_resource_points: i32,
    /// Player force for `SpLogistics`.
    pub force: Vec<ForceMech>,
    /// RNG seed for both streams.
    pub seed: u32,
    /// RNG algorithm.
    pub rng_mode: crate::rng::RngMode,
    /// `prefs.baseColor`: the paint of a mission part that has no `BaseColor` key (mission.cpp:2647-2653). `prefs.cpp:73` default.
    pub prefs_base_color: u32,
    /// `prefs.highlightColor`: the paint of a part with no `HighlightColor1`/`HighlightColor2` key (mission.cpp:2650-2653).
    /// `prefs.cpp:74` default.
    pub prefs_highlight_color: u32,
}

/// `prefs.cpp:73-74`: the default paint of the player's mech when the preferences file has none.
pub const DEFAULT_PREFS_COLOR: u32 = 0xffff_7e00;

impl LoadSpec {
    /// The `-mission <name>` start: quickstart, difficulty 1, unlimited ammo, no overrides, default preferences.
    pub fn quickstart(mission: &str, seed: u32, rng_mode: crate::rng::RngMode) -> LoadSpec {
        LoadSpec {
            mission: mission.to_string(),
            load_type: LoadType::SpQuickstart,
            difficulty: 1,
            unlimited_ammo: true,
            max_resource_points: -1,
            force: Vec::new(),
            seed,
            rng_mode,
            prefs_base_color: DEFAULT_PREFS_COLOR,
            prefs_highlight_color: DEFAULT_PREFS_COLOR,
        }
    }
}

/// `Mission::init` parts loop (mission.cpp:2647-2653): a part whose `BaseColor` / `HighlightColor1` / `HighlightColor2` key is absent
/// takes the player's preference colour. The file's colours are `Option`s, so an absent key is `None`.
pub fn fill_paint_from_prefs(parts: &mut [plan::PartRec], spec: &LoadSpec) {
    for part in parts {
        part.base_color.get_or_insert(spec.prefs_base_color);
        part.highlight_color1
            .get_or_insert(spec.prefs_highlight_color);
        part.highlight_color2
            .get_or_insert(spec.prefs_highlight_color);
    }
}

/// Parameters of `Mission::addMover` (`MoverInitData`, mover.h:1787).
#[derive(Clone, Debug, Default)]
pub struct MoverInitData {
    /// `pilotFileName` (warrior profile stem).
    pub pilot_file: String,
    /// `overrideLoadedPilot`: replace the loaded pilot's skills with the three fields below.
    pub override_loaded_pilot: bool,
    /// `gunnerySkill`.
    pub gunnery_skill: f32,
    /// `pilotingSkill`.
    pub piloting_skill: f32,
    /// `specialtySkills[23]`.
    pub specialty_skills: [bool; 23],
    /// `brainFileName` (module in the warrior directory).
    pub brain: String,
    /// `csvFileName` (chassis; informational, the object type decides).
    pub csv_file: String,
    /// `objNumber`.
    pub object_number: i32,
    /// `rosterIndex`.
    pub roster_index: i32,
    /// `controlType` (`CONTROL_PLAYER` = 1, `CONTROL_AI` = 2).
    pub control_type: u32,
    /// `controlDataType`.
    pub control_data_type: u32,
    /// `variant`.
    pub variant: u32,
    /// `position`.
    pub position: Vec3,
    /// `rotation` (an `int32_t` in the C++).
    pub rotation: i32,
    /// `teamID`.
    pub team: i8,
    /// `commanderID`.
    pub commander: i8,
    /// `baseColor`, `highlightColor1`, `highlightColor2` (appearance paint, unused by the simulation).
    pub colors: [u32; 3],
    /// `gestureID`.
    pub gesture_id: i8,
    /// `active`.
    pub active: bool,
    /// `exists`.
    pub exists: bool,
    /// `icon`.
    pub icon: i8,
    /// `capturable`.
    pub capturable: bool,
    /// `components` for `resetComponents` (logistics mechs): master ids; empty for none.
    pub components: Vec<i32>,
    /// `Mission::addMover(spec, LogisticsMech*)` for a custom (non-designer) mech: `resetComponents(count, list)` runs even for an
    /// empty list, which clears the replaceable components (mission.cpp:1430, mech.cpp:2065).
    pub custom_loadout: bool,
    /// `strcpy(variantName, mechData->getName())` of the logistics overload (mission.cpp:1428).
    pub variant_name: Option<String>,
}

/// Everything `Mission::init` needs while loading (data access + the world under construction).
pub struct LoadCtx<'a> {
    /// The world being built.
    pub world: &'a mut crate::World,
    /// Virtual file system over the game data.
    pub vfs: &'a mut Vfs,
    /// Load parameters.
    pub spec: &'a LoadSpec,
}

/// `Rotate(Vector3D&, angle)` (mclib/mathfunc.cpp:34): rotates in the x/y plane, `angle` in degrees.
pub fn rotate_xy(v: &mut Vec3, angle: f32) {
    let tx = v.x;
    // DEGREES_TO_RADS is the float literal 0.0174532925199f (mclib/dstd.h:35)
    #[allow(clippy::excessive_precision)]
    let a = angle * 0.0174532925199f32;
    let sine = mc2_libm::sinf(a);
    let cosine = mc2_libm::cosf(a);
    v.x *= cosine;
    v.x += v.y * sine;
    v.y *= cosine;
    v.y -= tx * sine;
}

/// The drop zone ring of `Logistics::beginMission` for `MISSION_LOAD_SP_LOGISTICS` (logistics.cpp:733-776): the player's force is
/// added around `dropZoneList[0]` (the first loaded team-0 part), 128 units out, rotated by `360 / count` per mech that has a pilot
/// (a pilotless mech `continue`s past the rotation). Commander and team are the home team (0). Returns the new movers' handles.
pub fn add_force(
    w: &mut crate::World,
    force: &[ForceMech],
) -> Result<Vec<crate::ids::ObjectHandle>, String> {
    let rotation = if force.is_empty() {
        0.0
    } else {
        360.0 / force.len() as f32
    };
    let origin = w
        .mission
        .drop_zone_positions
        .first()
        .copied()
        .ok_or("the mission has no team-0 part to take a drop zone from")?;
    let mut vector = Vec3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    let mut out = Vec::new();
    for m in force {
        let scaled = Vec3 {
            x: vector.x * 128.0,
            y: vector.y * 128.0,
            z: vector.z * 128.0,
        };
        let Some(pilot) = m.pilot_file.clone() else {
            continue;
        };
        let spec = MoverInitData {
            pilot_file: pilot,
            override_loaded_pilot: true,
            gunnery_skill: m.gunnery,
            piloting_skill: m.piloting,
            specialty_skills: m.specialty,
            brain: "pbrain".to_string(),
            csv_file: m.chassis_file.clone(),
            object_number: m.object_number,
            roster_index: 255,
            control_type: 2,
            control_data_type: 1,
            variant: m.designer_variant.unwrap_or(0),
            position: Vec3 {
                x: origin.x + scaled.x,
                y: origin.y + scaled.y,
                z: origin.z + scaled.z,
            },
            rotation: 0,
            team: 0,
            commander: 0,
            colors: m.colors,
            active: true,
            exists: true,
            capturable: false,
            components: if m.designer_variant.is_some() {
                Vec::new()
            } else {
                m.components.clone()
            },
            custom_loadout: m.designer_variant.is_none(),
            variant_name: m.designer_variant.is_none().then(|| m.variant_name.clone()),
            ..MoverInitData::default()
        };
        out.push(crate::mission::spawn::add_mover(w, &spec)?);
        rotate_xy(&mut vector, rotation);
    }
    Ok(out)
}

/// A loaded mission together with an explicit completeness report. `complete` is false while any lane B/C/D load hook is an
/// unported stub; `missing_hooks` then names each one (lane, function, C++ file:line). [`Sim::step`](crate::Sim::step) refuses an
/// incomplete world unless the caller opted in with [`Sim::allow_incomplete`](crate::Sim::allow_incomplete).
pub struct LoadOutcome {
    /// The loaded world (partially initialised when `complete` is false).
    pub world: crate::World,
    /// No load hook was an unported stub.
    pub complete: bool,
    /// The unported hooks that ran, in the order they were first reached.
    pub missing_hooks: Vec<&'static str>,
}

/// A world with the RNG of `spec`, loaded by [`init_mission`], with the completeness report.
///
/// BLOCKED (deferred pilot behavior, see `ai/sensors.rs` module docs and the porting log): moving-contact transition/removal
/// parity of the loaded sensors is not covered; it needs pilots that change sensor power and move units beyond frame 60.
pub fn load_world(vfs: &mut Vfs, spec: &LoadSpec) -> Result<LoadOutcome, String> {
    let mut world = crate::World::new(spec.rng_mode, spec.seed);
    stuff_random_init(&mut world);
    {
        let mut ctx = LoadCtx {
            world: &mut world,
            vfs,
            spec,
        };
        init_mission(&mut ctx)?;
    }
    // Lane C queues brain alarms, order clearing and sensor disabling in `Pilot::pending_calls` (combat/fx.rs); lane D's
    // `ai::calls::run_pending_calls` consumes the queue where each call is made, so no load hook is reported missing any more.
    Ok(LoadOutcome {
        complete: world.load_is_complete(),
        missing_hooks: world.unported_load_hooks.clone(),
        world,
    })
}

/// Engine start-up before `Mission::init`: `Stuff::Random::InitializeClass` (mclib/stuff/random.cpp:21, `Random::Init` line 44) fills
/// its 250-word table with `gos_rand()`. Those calls are not in `visual_rng_sites.txt`'s rewritten list by file but the oracle routes
/// `Stuff::Random` to the VISUAL stream (nothing in the simulation reads `Stuff::Random`), so a fresh world's visual stream has
/// drawn 250 times when the mission starts (`F 0 ... visDraws 250` in every oracle digest). The values are never read.
pub fn stuff_random_init(w: &mut crate::World) {
    let mut r = w.rng.0.borrow_mut();
    for _ in 0..250 {
        r.visual.next15();
    }
}

/// `teamRelationsForSP` (mission.cpp:1655): 0 friendly, 1 neutral, 2 enemy.
fn sp_relations() -> [[Relation; 8]; 8] {
    let mut r = [[Relation::Enemy; 8]; 8];
    for (i, row) in r.iter_mut().enumerate() {
        row[i] = Relation::Friendly;
    }
    r[0][2] = Relation::Neutral;
    r[2][0] = Relation::Neutral;
    r
}

fn schema<E: std::fmt::Display>(what: &str) -> impl Fn(E) -> String + '_ {
    move |e| format!("{what}: {e}")
}

/// Step 2: the `gamesys.fit` tables into `world.data` (before the component list).
fn load_gamesys(ctx: &mut LoadCtx<'_>) -> Result<(), String> {
    if ctx.world.data.gamesys.tables.is_some() {
        return Ok(());
    }
    let tables = GameSystem::load(ctx.vfs).map_err(schema("gamesys.fit"))?;
    let gs = &mut ctx.world.data.gamesys;
    gs.visual_range_table = tables.general.visual_range_table.clone();
    gs.ranges = [
        tables.general.max_visual_range,
        tables.general.base_sensor_range,
        tables.general.max_tree_los_cell_block as f32,
    ];
    gs.tables = Some(Rc::new(tables));
    Ok(())
}

/// Object types (and the vehicle packets) of `object2.pak`, once per `GameData`.
fn load_object_types(world: &mut crate::World, vfs: &mut Vfs) -> Result<(), String> {
    let data = &mut world.data;
    if data.object_types.is_some() {
        return Ok(());
    }
    let bytes = vfs
        .read(OBJECT_PAK_PATH)
        .map_err(schema(OBJECT_PAK_PATH))?
        .ok_or_else(|| format!("{OBJECT_PAK_PATH}: not found"))?;
    let pak = mc2_formats::pak::PakFile::from_bytes(bytes).map_err(schema(OBJECT_PAK_PATH))?;
    let table: ObjectTable =
        mc2_sim_data::objtype::load_object_table_from_pak(&pak).map_err(schema(OBJECT_PAK_PATH))?;
    // The vehicle instance data (armor, inventory) of every vehicle type. The C++ reads a packet when a mission uses the type, so a
    // packet that does not parse (a few unused types lack `CurTonnage`) is skipped here and only fails if a mover is made from it.
    for (&index, t) in &table.types {
        if !matches!(t.data, ObjectTypeData::GroundVehicle(_)) {
            continue;
        }
        let Ok(packet) = pak.read_packet(index as usize) else {
            continue;
        };
        let Ok(mut fit) = mc2_formats::ini::FitIni::from_bytes(packet) else {
            continue;
        };
        if let Ok(v) =
            mc2_sim_data::mech::vehicle::GroundVehicleObject::from_fit(OBJECT_PAK_PATH, &mut fit)
        {
            data.vehicle_objects.insert(index, Rc::new(v));
        }
    }
    data.object_types = Some(Rc::new(table));
    Ok(())
}

/// Chassis CSVs for the mech types in `types` (lower-case stem keys), if not cached.
fn load_chassis_for(world: &mut crate::World, vfs: &mut Vfs, types: &[u32]) -> Result<(), String> {
    let table = world
        .data
        .object_types
        .clone()
        .ok_or("object types are not loaded")?;
    for &n in types {
        let Some(t) = table.get(n) else { continue };
        if !matches!(t.data, ObjectTypeData::BattleMech(_)) {
            continue;
        }
        let Some(name) = t.base.appearance_name.clone() else {
            continue;
        };
        let key = name.to_ascii_lowercase();
        if world.data.chassis.contains_key(&key) {
            continue;
        }
        let chassis = mc2_sim_data::mech::load_chassis(vfs, &name)
            .map_err(|e| format!("chassis {name}: {e}"))?;
        world.data.chassis.insert(key, Rc::new(chassis));
    }
    Ok(())
}

/// Loads the lane F appearance type of every mech type in `types` (and the MADCAT fallback the C++ uses when a mech has none).
fn load_mech_appearance_types(
    world: &mut crate::World,
    vfs: &mut Vfs,
    types: &[u32],
) -> Result<(), String> {
    let table = world.data.object_types.clone().ok_or("object types")?;
    let mut names: Vec<String> = Vec::new();
    for &n in types {
        let Some(t) = table.get(n) else { continue };
        if !matches!(t.data, ObjectTypeData::BattleMech(_)) {
            continue;
        }
        // A mech type names its appearance in shipped data; one whose appearance fails to load falls back to MADCAT (below,
        // mech.cpp:1296-1297), and the nameless-type APC fallback is the vehicle path's.
        let Some(name) = t.base.appearance_name.clone() else {
            continue;
        };
        if !names.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    for name in names {
        if world.appearance.mech_type(vfs, &name).is_err() {
            world
                .appearance
                .mech_type(vfs, "MADCAT")
                .map_err(|e| format!("mech appearance MADCAT: {e}"))?;
        }
    }
    Ok(())
}

/// Loads the lane F appearance type of every ground vehicle type in `types`, and the `APC` fallback the C++ uses for a vehicle
/// whose appearance is missing ("Load a dummy appearance until all vehicles ready", gvehicl.cpp:962-969).
fn load_vehicle_appearance_types(
    world: &mut crate::World,
    vfs: &mut Vfs,
    types: &[u32],
) -> Result<(), String> {
    let table = world.data.object_types.clone().ok_or("object types")?;
    let mut names: Vec<String> = Vec::new();
    for &n in types {
        let Some(t) = table.get(n) else { continue };
        if !matches!(t.data, ObjectTypeData::GroundVehicle(_)) {
            continue;
        }
        // A vehicle type without an `AppearanceName` finds no appearance type either: it gets the APC (gvehicl.cpp:963-969).
        let name = t.base.appearance_name.as_deref().unwrap_or("APC");
        if !names.iter().any(|x| x.eq_ignore_ascii_case(name)) {
            names.push(name.to_string());
        }
    }
    for name in names {
        if world.appearance.vehicle_type(vfs, &name).is_err() {
            world
                .appearance
                .vehicle_type(vfs, "APC")
                .map_err(|e| format!("vehicle appearance APC: {e}"))?;
        }
    }
    Ok(())
}

/// The profile `data/missions/warriors/<name>.fit`, cached. `None` when the file does not exist or has no `[General]` block: the C++
/// `MechWarrior::init(FitIniFile*)` then returns at its first `seekBlock` and every caller ignores the result, so the pilot keeps the
/// `init(bool)` defaults and draws no ejection roll. Any other parse error is a data error.
pub fn load_profile(
    world: &mut crate::World,
    vfs: &mut Vfs,
    name: &str,
) -> Result<Option<Rc<WarriorProfile>>, String> {
    let key = name.to_ascii_lowercase();
    if let Some(p) = world.data.pilot_profiles.get(&key) {
        return Ok(p.clone());
    }
    let loaded = match WarriorProfile::load(vfs, name) {
        Ok(p) => Some(Rc::new(p)),
        Err(mc2_sim_data::SimDataError::MissingFile { .. }) => None,
        Err(mc2_sim_data::SimDataError::Schema { detail, .. })
            if detail.contains("missing [General]") =>
        {
            None
        }
        Err(e) => return Err(format!("pilot {name}: {e}")),
    };
    world.data.pilot_profiles.insert(key, loaded.clone());
    Ok(loaded)
}

/// Everything `Mission::addMover` reads from disk, cached ahead of the spawn (the C++ opens the files on demand): the chassis CSV and the
/// mech appearance type of every type in `types`, and the pilot profile of every file in `pilots`. Called by [`init_mission`] for the
/// logistics force, and by whoever spawns at run time (reinforcements, `create_infantry`) before [`spawn::add_mover`].
pub fn prepare_spawn(ctx: &mut LoadCtx<'_>, types: &[u32], pilots: &[&str]) -> Result<(), String> {
    prepare_spawn_in(ctx.world, ctx.vfs, types, pilots)
}

/// [`prepare_spawn`] without a [`LoadCtx`]: `save::load_world` replays the reinforcements of a save with it (`spawn::replay_spawn`), so
/// the base world loads the same chassis, appearance types and profiles the original run loaded at the spawn. The object table is
/// loaded first if the base does not have it yet.
pub fn prepare_spawn_in(
    world: &mut crate::World,
    vfs: &mut Vfs,
    types: &[u32],
    pilots: &[&str],
) -> Result<(), String> {
    load_object_types(world, vfs)?;
    load_chassis_for(world, vfs, types)?;
    load_mech_appearance_types(world, vfs, types)?;
    load_vehicle_appearance_types(world, vfs, types)?;
    for p in pilots {
        load_profile(world, vfs, p)?;
    }
    Ok(())
}

/// Step 5: `MechWarrior::setup` and one `newWarrior` + `init` + brain per `[WarriorN]`, then `loadBrainParameters` for all.
fn load_pilots(ctx: &mut LoadCtx<'_>, mission: &Mission) -> Result<(), String> {
    let tables = ctx
        .world
        .data
        .gamesys
        .tables
        .clone()
        .ok_or("gamesys tables are not loaded")?;
    pilots::setup_pool(&mut ctx.world.pilots, tables.warrior.default_attack_radius);
    let n = ctx.world.pilots.slots.len();
    ctx.world.mission.pilot_loads = vec![pilots::PilotLoad::default(); n];
    for w in &mission.warriors {
        let slot = pilots::new_warrior(&mut ctx.world.mission.pilot_loads)
            .ok_or("Too many pilots in this mission!")?;
        if slot != w.index as usize {
            return Err(format!("[Warrior{}] got pilot slot {slot}", w.index));
        }
        let profile = load_profile(ctx.world, ctx.vfs, &w.profile)?;
        {
            let streams = ctx.world.rng.0.clone();
            let mut r = streams.borrow_mut();
            let world = &mut *ctx.world;
            // an incomplete or missing profile leaves the defaults (the C++ ignores the result); the shipped missions' are complete
            let _progress = pilots::init_pilot(
                &mut world.pilots,
                &mut world.mission.pilot_loads,
                slot,
                profile,
                &w.brain,
                &mut r.sim,
            );
        }
        abl_load::make_pilot_brain(ctx.world, slot, &w.brain)?;
    }
    // loadBrainParameters: memory cells and static variables of every warrior (the C++ result is ignored)
    for w in &mission.warriors {
        let slot = w.index as usize;
        pilots::apply_brain_cells(&mut ctx.world.pilots.slots[slot], w)?;
        abl_load::apply_static_vars(ctx.world, slot, w)?;
    }
    Ok(())
}

/// Reads `[Squads]`-less shipped data: the mission file twice (the second open gives the `[Squads]` override reader a fresh cursor).
fn read_mission(ctx: &mut LoadCtx<'_>) -> Result<(Mission, Vec<Option<u32>>), String> {
    let path = format!("data/missions/{}.fit", ctx.spec.mission);
    let mut fit = ctx
        .vfs
        .read_fit(&path)
        .map_err(schema(&path))?
        .ok_or_else(|| format!("Unable to open Mission File {path}"))?;
    let mission = Mission::load(&mut fit, &path).map_err(schema(&path))?;
    let overrides = plan::read_squad_overrides(&mut fit, &mission.parts);
    Ok((mission, overrides))
}

/// `create_infantry`'s object type and pilot file (ablmc2.cpp:3042-3050).
pub const INFANTRY_OBJECT: u32 = 53;
/// See [`INFANTRY_OBJECT`].
pub const INFANTRY_PILOT: &str = "pmw00080";

/// What [`prepare`] decided and [`populate`] needs.
pub struct Prepared {
    /// The mission file data.
    pub mission: Rc<Mission>,
    /// The load plan (parts, creation order, mover counts).
    pub plan: plan::LoadPlan,
    /// `commandersToLoad`.
    pub commanders: [[i8; 3]; plan::MAX_COMMANDERS],
}

/// `Mission::init` (single player). On success the world is in the state the oracle's frame-0 digest records.
pub fn init_mission(ctx: &mut LoadCtx<'_>) -> Result<(), String> {
    let prepared = prepare(ctx)?;
    populate(ctx, prepared)?;
    if ctx.spec.load_type == LoadType::SpLogistics {
        // Logistics::beginMission: the player's force, then missionInterface->initMechs() once more
        let force = ctx.spec.force.clone();
        // a force mech without a pilot is skipped before addMover (logistics.cpp:759): nothing of it is loaded
        let objs: Vec<u32> = force
            .iter()
            .filter(|m| m.pilot_file.is_some())
            .map(|m| m.object_number as u32)
            .collect();
        let pilots: Vec<&str> = force
            .iter()
            .filter_map(|m| m.pilot_file.as_deref())
            .collect();
        prepare_spawn(ctx, &objs, &pilots)?;
        add_force(ctx.world, &force)?;
        init_mechs(ctx.world, 0);
    }
    Ok(())
}

/// First half of `Mission::init`: everything up to and including the pilots and the part records (steps 1-6 above). Needs no object
/// storage: the world afterwards has the gamesys tables, object types, teams and commanders, terrain, the ABL environment with the
/// mission and pilot brains, the pilot pool, and the plan. The sim RNG has been drawn for the pilots and the squad alternative.
pub fn prepare(ctx: &mut LoadCtx<'_>) -> Result<Prepared, String> {
    let spec: &LoadSpec = ctx.spec;
    let logistics = spec.load_type == LoadType::SpLogistics;
    let commanders = plan::commanders_to_load(logistics);

    // 1. reset (mission.cpp:1641-1700). The C++ never reset the mission globals between missions; a world starts clean.
    ctx.world.mission = MissionState::default();
    ctx.world.mission.name = spec.mission.clone();
    ctx.world.mission.difficulty = spec.difficulty;
    ctx.world.mission.load_type = Some(spec.load_type);
    ctx.world.mission.term.result = 0; // mis_PLAYING
    ctx.world.teams.relations = sp_relations();
    ctx.world.clock.turn = 0;

    // 2. gamesys.fit, then the master components (they read baseSensorRange)
    load_gamesys(ctx)?;
    if ctx.world.data.components.is_none() {
        let comps = mc2_sim_data::component::MasterComponentList::load(ctx.vfs)
            .map_err(schema("compbas.csv"))?;
        ctx.world.data.components = Some(Rc::new(comps));
    }
    load_object_types(ctx.world, ctx.vfs)?;

    // 3. the mission file
    let (mission, squad_overrides) = read_mission(ctx)?;
    let mission = Rc::new(mission);
    ctx.world.mission.sky_number = mission.sky.sky_number;
    ctx.world.mission.drop_zone = (mission.drop_zone.x, mission.drop_zone.y);
    ctx.world.mission.tune_num = mission.settings.scenario_tune_num;
    ctx.world.mission.resource_points = if spec.max_resource_points > -1 {
        spec.max_resource_points
    } else {
        mission.settings.resource_points
    };
    ctx.world.mission.script_name = mission.script.scenario_script.clone();
    ctx.world.mission.time_limit = mission.settings.time_limit; // m_timeLimit (mission.cpp:2803-2812)
    ctx.world.mission.file = Some(mission.clone());

    // teams and commanders (mission.cpp:2020-2092)
    let (max_team_id, max_commander_id) = plan::max_ids(&mission.parts);
    if max_team_id < 0 {
        return Err("mission has no parts: Team::home would be NULL".into());
    }
    // the C++ indexes `Team::teams[]` and `Commander::commanders[]` with these unchecked; a mission file that exceeds them is a data error
    if max_team_id as usize >= crate::ids::MAX_TEAMS
        || max_commander_id as usize >= crate::ids::MAX_COMMANDERS
    {
        return Err(format!(
            "mission parts use team {max_team_id} / commander {max_commander_id}; at most {} teams and {} commanders exist",
            crate::ids::MAX_TEAMS,
            crate::ids::MAX_COMMANDERS
        ));
    }
    ctx.world.mission.max_team_id = max_team_id;
    ctx.world.mission.max_commander_id = max_commander_id;
    ctx.world.teams.teams = vec![None; crate::ids::MAX_TEAMS];
    ctx.world.teams.commanders = vec![None; crate::ids::MAX_COMMANDERS];
    for t in 0..=max_team_id as usize {
        ctx.world.teams.teams[t] = Some(Team::default());
    }
    let cteams = plan::commander_teams(max_team_id, max_commander_id, &commanders);
    for (c, team) in cteams.iter().enumerate() {
        if let Some(team) = team {
            ctx.world.teams.commanders[c] = Some(Commander {
                team: team.unwrap_or(-1),
            });
        }
    }
    ctx.world.teams.home = 0;

    // trigger areas (fresh manager)
    ctx.world.mission.triggers = Default::default();

    // 4. terrain (lane T) with its move data, then the ABL environment, libraries and the mission script
    ctx.world.terrain = crate::terrain::Terrain::load(ctx)?;
    let sources = abl_load::collect_sources(ctx.vfs)?;
    abl_load::init_env(ctx.world, sources)?;
    abl_load::load_mission_script(ctx.world, &mission.script.scenario_script)?;

    // 5. pilots: before the parts, because a part's pilot is a pool slot
    load_pilots(ctx, &mission)?;

    // 6. parts: the squad alternative draw and the part records
    let table = ctx.world.data.object_types.clone().ok_or("object types")?;
    let lp = {
        let streams = ctx.world.rng.0.clone();
        let mut r = streams.borrow_mut();
        let inputs = PlanInputs {
            difficulty: spec.difficulty,
            squad_overrides: &squad_overrides,
            logistics,
        };
        plan::plan_parts(&mission, &table, &inputs, &mut r.sim)?
    };
    ctx.world.mission.parts = lp.parts.clone();
    fill_paint_from_prefs(&mut ctx.world.mission.parts, spec);
    ctx.world.mission.squads = lp.squads.clone();
    // dropZoneList (mission.cpp:2640-2644): positions of the loaded parts of team `dropZoneID` (0), in file order
    ctx.world.mission.drop_zone_positions = lp
        .parts
        .iter()
        .skip(1)
        .filter(|p| p.loaded && p.team_id == 0)
        .map(|p| p.position)
        .collect();
    ctx.world.mission.unlimited_ammo = spec.unlimited_ammo;

    // pilots of parts that are not loaded go back to the pool (mission.cpp:2778)
    let attack_radius = ctx
        .world
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or(pilots::DEFAULT_ATTACK_RADIUS, |t| {
            t.warrior.default_attack_radius
        });
    for &p in &lp.freed_pilots {
        let slot = p as usize;
        if slot < ctx.world.mission.pilot_loads.len() && slot < ctx.world.pilots.slots.len() {
            pilots::clear_slot(
                &mut ctx.world.pilots,
                &mut ctx.world.mission.pilot_loads,
                slot,
                attack_radius,
            );
        }
    }

    Ok(Prepared {
        mission,
        plan: lp,
        commanders,
    })
}

/// Second half of `Mission::init`: the objects and everything that needs them (steps 7-8 above).
pub fn populate(ctx: &mut LoadCtx<'_>, prepared: Prepared) -> Result<(), String> {
    let Prepared {
        mission,
        plan: lp,
        commanders,
    } = prepared;
    let max_team_id = ctx.world.mission.max_team_id;

    // `WeaponRanges[SHORT/MEDIUM/LONG]` (mission.cpp:1729-1746, read with the rest of gamesys.fit): turrets and structures choose
    // their weapons' range class from it (`weapon_range_limits`). The object store keeps its own copy.
    if let Some(t) = ctx.world.data.gamesys.tables.clone() {
        ctx.world.objects.weapon_ranges = t.general.weapon_ranges;
        // `MineSplashDamage`, `MineSplashRange` (mission.cpp:1857-1925): the mine explosion's damage and range in meters.
        ctx.world.objects.mine_splash = (t.mine.mine_splash_damage, t.mine.mine_splash_range);
    }

    // the sensor manager (lane D): one team sensor system per team, before any mover registers its sensor
    crate::ai::sensors::init(ctx.world);

    // 7. objects: setNumObjects (turret constructors draw), the movers team-major, the terrain objects, the mover lists
    // the parts' types, plus what scripts spawn later: `create_infantry` makes object 53 with pilot pmw00080 (ablmc2.cpp:3042)
    let mut types: Vec<u32> = lp.parts.iter().skip(1).map(|p| p.obj_number).collect();
    types.push(INFANTRY_OBJECT);
    // and the crew a destroyed vehicle bails out as (`createVehiclePilot`, gvehicl.cpp:2707)
    types.push(spawn::VEHICLEPILOT_ID);
    // chassis CSVs, mech appearance types (`appearanceTypeList->getAppearance`; a missing one falls back to MADCAT) and the profile
    prepare_spawn(ctx, &types, &[INFANTRY_PILOT])?;
    let home_commander = 0i8; // Commander::home
    let order = lp.order.clone();
    // countTerrainObjects, setNumObjects (turret constructors draw), [our movers, team-major], loadTerrainObjects, buildMoverLists
    crate::object::load::build_for_mission(ctx, lp.num_mechs, lp.num_vehicles, |w| {
        for m in &order {
            spawn::create_part_object(w, m.part, m.class, m.slot, home_commander)?;
        }
        Ok(())
    })?;

    // 8a. objectives (mission.cpp:2823-2850), team by team
    let mut sets = Vec::new();
    for t in 0..=max_team_id as usize {
        let data = mission.teams.get(t).cloned().unwrap_or_default();
        let set = objective::load_team_objectives(ctx.world, t, &data);
        if let Some(Some(team)) = ctx.world.teams.teams.get_mut(t) {
            team.num_primary_objectives = set.num_primary;
        }
        sets.push(set);
    }
    ctx.world.mission.objectives = sets;

    // 8b. commander groups (mission.cpp:2855-2895): part ids and group ids
    let members = {
        let parts = &ctx.world.mission.parts;
        plan::group_members(&mission, &commanders, &|i| {
            parts.get(i).is_some_and(|p| !p.object_wid.is_none())
        })
    };
    for g in &members {
        let wid = ctx.world.mission.parts[g.part].object_wid;
        let Some(h) = ctx.world.objects.by_watch_id(wid) else {
            continue;
        };
        // ObjectManager->setPartId(mover, commander, group, mate)
        ctx.world
            .objects
            .set_part_id(h, g.commander as i32, g.group as i32, g.mate as i32);
        // `MoverGroup::add` -> `Mover::setGroupId(id, true)` (group.cpp:111, mover.cpp:2738)
        if ctx.world.objects.get(h).is_some_and(|o| o.is_mover()) {
            // `Mover::unitGroup` belongs to the mover rather than its part id: `Mover::init`
            // sets the default bit for group 1 (`mover.cpp:2217,2278`), and it remains after
            // this final `setPartId` (`mission.cpp:2875`, `objmgr.cpp:2503-2504`).  The Rust
            // mirror is keyed by part id, so record the default only after that final ID exists.
            let final_part_id = ctx.world.objects.get(h).map(|object| object.c.part_id);
            if let Some(final_part_id) = final_part_id {
                ctx.world.mission.unit_groups.insert(final_part_id, 1);
            }
            crate::ai::calls::set_group_id(ctx.world, h, g.group as i32, true);
        }
    }
    ctx.world.mission.groups = members;

    // 8c. team rosters in mover list order (Team::buildRoster)
    let movers = ctx.world.objects.movers.clone();
    let mut rosters: Vec<Vec<crate::ids::WatchId>> = vec![Vec::new(); crate::ids::MAX_TEAMS];
    for h in movers {
        let (team, wid) = match ctx.world.objects.get(h).map(|o| (&o.kind, o.c.watch_id)) {
            Some((Kind::Mech(m), wid)) => (m.m.team.0, wid),
            Some((Kind::Vehicle(v), wid)) => (v.m.team.0, wid),
            _ => continue,
        };
        if team >= 0 {
            rosters[team as usize].push(wid);
        }
    }
    for (t, roster) in rosters.into_iter().enumerate() {
        if let Some(Some(team)) = ctx.world.teams.teams.get_mut(t) {
            team.roster = roster;
        }
    }

    // clocks: scenarioTime = 0, actualTime = 0, LastTimeGetTime reset
    ctx.world.clock.reset();
    ctx.world.mission.actual_time = 0.0;

    // missionInterface->initMechs() (ControlGui::initMechs): the home commander's existing, on-GUI movers start selected
    init_mechs(ctx.world, home_commander);

    // Mover::initOptimalCells(32)
    crate::mover::init_optimal_cells(ctx.world, 32);

    Ok(())
}

/// `ControlGui::initMechs` (controlgui.cpp:1305-1330): selection is the only effect the simulation sees (`OBJECT_FLAG_SELECTED`).
pub fn init_mechs(w: &mut crate::World, home_commander: i8) {
    let roster = match w.teams.teams.get(w.teams.home) {
        Some(Some(t)) => t.roster.clone(),
        _ => return,
    };
    for wid in roster {
        let Some(h) = w.objects.by_watch_id(wid) else {
            continue;
        };
        let Some(o) = w.objects.get_mut(h) else {
            continue;
        };
        let exists = o.exists();
        let (cmdr, on_gui) = match &o.kind {
            Kind::Mech(m) => (m.m.commander.0, m.m.flags.on_gui),
            Kind::Vehicle(v) => (v.m.commander.0, v.m.flags.on_gui),
            _ => continue,
        };
        if exists && on_gui && cmdr == home_commander {
            o.c.flags |= flags::SELECTED;
        }
    }
}

// ---- save/load (DESIGN.md section 9) ----

impl crate::save::Snapshot for LoadType {
    fn save(&self, w: &mut crate::save::Writer) {
        w.u8(match self {
            LoadType::SpQuickstart => 0,
            LoadType::SpLogistics => 1,
        });
    }
    fn load(r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        match r.u8()? {
            0 => Ok(LoadType::SpQuickstart),
            1 => Ok(LoadType::SpLogistics),
            t => Err(crate::save::SaveError(format!(
                "save: unknown load type {t}"
            ))),
        }
    }
}
