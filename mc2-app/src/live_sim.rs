// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The live simulation: [`mc2_sim::Sim`] behind the application's [`SimDriver`] seam.
//!
//! One [`SimDriver::step`] is one `Sim::step` at the fixed simulation rate with the camera
//! visibility the application computed for that frame (`DESIGN.md` section 5), the player
//! commands of the frame, and the virtual clock of [`crate::sim::SimClock`].  Between steps the
//! renderer interpolates mover transforms by the clock's `alpha`; the simulation never sees the
//! interpolation.
//!
//! Presentation is driven from the simulation's state each frame: mover positions and yaw, and
//! the mech gesture and frame (`MechAppearance::current_gesture/current_frame`, played through the
//! gesture's `.agl` the appearance type loaded), structure status.  Anything the simulation or
//! the renderer cannot supply stays *explicitly unavailable*: it is counted in
//! [`LiveSim::unavailable`], logged once, and never replaced by a made-up value.

use crate::game::GameConfig;
use crate::mission_view::MissionView;
use crate::sim::{CursorWorldFacts, FrameInput, FrameOutput, SimDriver};
use crate::visibility::{CameraVisibility, ObjectVisibility};
use mc2_render::appearance::{
    state_pose, AppearanceState, ObjectStatus as RenderObjectStatus, ShapeChoice, Shown,
};
use mc2_render::camera::GameCamera;
use mc2_render::flash::BuildingFlash;
use mc2_render::game::GameData;
use mc2_render::math::{Quat, Vec3};
use mc2_render::populate::{select_shapes, selection_box_screen_bounds, visible_screen_triangles};
use mc2_render::scene::{SceneRenderer, ShapeInstance};
use mc2_render::shape::AnimationClip;
use mc2_sim::appearance::Appearance;
use mc2_sim::events::CameraRequest;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::ObjectStatus;
use mc2_sim::visibility::Visibility;
use mc2_sim::{ObjectHandle, PlayerCommand, SimEvent};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

/// Mover instances farther than this between two simulation frames are not interpolated (a jump
/// or a teleport).
const MAX_INTERPOLATED_STEP: f32 = 256.0;

/// `Mission::update` increments its turn before object appearance updates.  The terrain-object
/// `turn < 3` warmup therefore applies to the turn being stepped, not the previous value held
/// immediately before [`mc2_sim::Sim::step`].
fn terrain_object_recalculates(stepping_turn: i64, block_and_vertex_active: bool) -> bool {
    stepping_turn < 3 || block_and_vertex_active
}

fn appearances_complete(unbound_movers: usize, unmatched_structures: usize) -> bool {
    unbound_movers == 0 && unmatched_structures == 0
}

fn visibility_trace_note(complete: bool) -> String {
    if complete {
        "object-visibility=production driver=LiveSim (R1 select_shapes: class gate, rough rectangle, type box, status transition and LOD are selected before Sim::step)".into()
    } else {
        "object-visibility=incomplete driver=LiveSim (one or more live objects have no bound production appearance)".into()
    }
}

/// What a recorded simulation frame was fed (see [`LiveSim::record_inputs`]).
#[derive(Clone)]
pub struct RecordedFrame {
    pub frame_rate: f32,
    pub now_ms: u32,
    pub visibility: Rc<CameraVisibility>,
    pub commands: Vec<PlayerCommand>,
    /// The camera the mission script could read this frame (`World::camera`).
    pub camera: mc2_sim::events::CameraState,
    /// The sound system the mission script could read this frame (`World::audio`).
    pub audio: mc2_sim::events::AudioReadback,
    /// The mission interface's callout animation the mission script could read this frame (`World::callout`).
    pub callout: mc2_sim::events::CalloutState,
    /// `Sim::digest_with_pilots(frame, 2)` after the step, `frame` counted from 1 like the oracle.
    pub digest: String,
}

struct MoverBinding {
    handle: ObjectHandle,
    /// Index in `MissionView::instances` of the mover's static instance, which this replaces
    /// (`None` for a mover the simulation created after the load, bound to a support template).
    static_index: Option<usize>,
    /// The R1 appearance state used for this object's pre-simulation `recalcBounds` call.
    /// It is also the exact instance submitted to the renderer: its selected destroyed/damage
    /// shape must not be reconstructed as the static LOD at draw time.
    visibility: ShapeInstance,
}

/// A strike's bomber on its bomb run (`Artillery::bombRunStarted`), from the [`SimEvent::AirstrikeFlyover`] that started it.
struct BomberRun {
    /// The strike position the bomber is placed at (`bomber->setObjectParameters(position, ...)`).
    at: Vec3,
    /// The simulation turn the run started on.
    start_turn: i64,
}

struct StructureBinding {
    handle: ObjectHandle,
    static_index: usize,
    /// Retained across frames for the appearance's `wasInView` state.
    visibility: ShapeInstance,
    /// The final selection rectangle from the last `recalcBounds` pass. A terrain object can
    /// retain it while inactive, just as it retains its appearance `inView` and LOD.
    selection_box: Option<[f32; 4]>,
}

/// `mc2_sim::Sim` as the application's simulation driver.
pub struct LiveSim {
    sim: mc2_sim::Sim,
    /// The inputs the mission was loaded with: an in-mission load re-initialises the same mission.
    spec: LoadSpec,
    /// The campaign launch the mission was started from (its roster); the preparation of a re-initialised base needs it.
    launch: Option<mc2_campaign::LoadSpec>,
    /// The selection boxes of the last `recalcBounds` pass: what a click is tested against.
    pick_boxes: crate::hud_data::PickBoxes,
    movers: Vec<MoverBinding>,
    /// Home-painted instances of the support vehicles a VTOL drops, by object number.
    support_templates: HashMap<i32, ShapeInstance>,
    /// Movers created after the load that no template covers.
    late_unbound: BTreeSet<ObjectHandle>,
    /// Movers and structures the load-time binding found no model for (empty when complete).
    load_unbound: Vec<ObjectHandle>,
    load_unmatched: Vec<ObjectHandle>,
    /// Commands a host feeds outside the order bridge, added to the next step's.
    injected: Vec<PlayerCommand>,
    structures: Vec<StructureBinding>,
    /// `(gesture, frame, turret_yaw, object_rotation)` of each building instance the last
    /// `append_instances` drew from the sim, in output order (the tail of the appended instances).
    drawn_buildings: Vec<(i32, f32, f32, f32)>,
    /// Static instances of the view that the simulation draws instead (movers) or hides.
    hidden: Vec<bool>,
    /// Building flashes in progress (`BldgAppearance::flashBuilding`), advanced every sim frame.
    flashes: HashMap<ObjectHandle, BuildingFlash>,
    /// Static instances hidden only for the frame a flashing building is drawn tinted in their place.
    flash_hidden: Vec<usize>,
    /// Bombers flying over an airstrike's target, until the strike is over.
    bomber_runs: Vec<BomberRun>,
    /// Transforms at the start of the last step, for interpolation.
    previous: HashMap<ObjectHandle, (Vec3, f32)>,
    clipped_models: std::collections::HashSet<usize>,
    clips: HashMap<(usize, i32, i32), Option<AnimationClip>>,
    camera_requests: Vec<CameraRequest>,
    /// Mission text messages of the last step, until the HUD takes them.
    hud_messages: Vec<crate::hud_data::HudMessage>,
    /// Radio notices emitted by the most recent simulation step, retained for headless acceptance checks.
    last_radio_events: Vec<(u32, i32)>,
    /// Unported or unavailable behaviour reached so far and how often.
    unavailable: BTreeMap<String, u64>,
    logged: BTreeSet<String>,
    /// Every live object whose `inView` is consumed by the sim has a renderer appearance binding.
    /// A missing binding remains an explicit unavailable input; it is never reduced to a point.
    object_appearances_complete: bool,
    high_detail: bool,
    recorded: Option<Vec<RecordedFrame>>,
    /// The commands of each frame that had any (frame counted from 1 like the oracle), when asked for.
    command_log: Option<Vec<(u64, Vec<PlayerCommand>)>>,
    /// Why the last step could not run, until the application takes it.
    failure: Option<String>,
    /// Frames stepped.
    pub frames: u64,
}

/// The simulation's load inputs for `mission`: the campaign launch (`Logistics::beginMission`: the
/// selected force, difficulty, ammo option, resource points and paint) when the UI host made one,
/// else the `-mission` quickstart.
pub fn sim_load_spec(mission: &str, config: &GameConfig) -> LoadSpec {
    let mut spec = LoadSpec::quickstart(mission, config.sim_seed, config.sim_rng);
    let Some(launch) = &config.campaign_launch else {
        return spec;
    };
    spec.load_type = match launch.load_type {
        mc2_campaign::interface::LoadType::SpQuickstart => {
            mc2_sim::mission::init::LoadType::SpQuickstart
        }
        mc2_campaign::interface::LoadType::SpLogistics => {
            mc2_sim::mission::init::LoadType::SpLogistics
        }
    };
    spec.difficulty = launch.difficulty;
    spec.unlimited_ammo = launch.unlimited_ammo;
    spec.max_resource_points = launch.resource_points.clamp(-1, i64::from(i32::MAX)) as i32;
    if let Some(first) = launch.force.first() {
        spec.prefs_base_color = first.colors[0];
        spec.prefs_highlight_color = first.colors[1];
    }
    spec.force = launch
        .force
        .iter()
        .map(|m| mc2_sim::mission::init::ForceMech {
            object_number: m.object_number as i32,
            chassis_file: m.chassis_file.clone(),
            pilot_file: Some(m.pilot_file.clone()),
            gunnery: m.gunnery,
            piloting: m.piloting,
            specialty: m.specialty_skills,
            designer_variant: m.designer.then_some(m.variant_id as u32),
            variant_name: m.variant_name.clone(),
            components: m.components.iter().map(|c| c.component_id).collect(),
            colors: m.colors,
        })
        .collect();
    spec
}

/// `LogisticsData::getBestPilot` over the launch's roster (`logisticsdata.cpp:913`): the best free pilot for a recovered mech is
/// marked used. Candidates are the available pilots not already in a live slot (`warriorInUse`) and not used; a pilot kept on an
/// inventory mech ranks last (`comparePilots`, `:1000`); then rank, gunnery and piloting, higher first (the first of equals).
struct CampaignRoster(std::cell::RefCell<Vec<mc2_campaign::interface::RosterPilot>>);

impl mc2_sim::mission::trade::RecoveryPilots for CampaignRoster {
    fn has_pilot(&self, in_use: &dyn Fn(&str) -> bool) -> bool {
        self.0.borrow().iter().any(|p| Self::free(p, in_use))
    }

    fn best_pilot(
        &self,
        _tonnage: i64,
        in_use: &dyn Fn(&str) -> bool,
    ) -> Option<mc2_sim::mission::trade::RecoveryPilot> {
        let mut roster = self.0.borrow_mut();
        let mut best: Option<usize> = None;
        for (i, p) in roster.iter().enumerate() {
            if !Self::free(p, in_use) {
                continue;
            }
            // a later pilot replaces the best only when strictly better, so the first of equals wins
            let better = best.is_none_or(|b| {
                let q = &roster[b];
                let key = |r: &mc2_campaign::interface::RosterPilot| (!r.in_inventory, r.rank);
                (key(p), p.gunnery, p.piloting).partial_cmp(&(key(q), q.gunnery, q.piloting))
                    == Some(std::cmp::Ordering::Greater)
            });
            if better {
                best = Some(i);
            }
        }
        let p = &mut roster[best?];
        p.used = true;
        Some(mc2_sim::mission::trade::RecoveryPilot {
            file: p.file_name.clone(),
            skills: Some((p.gunnery, p.piloting, p.specialty_skills)),
        })
    }
}

impl CampaignRoster {
    fn free(p: &mc2_campaign::interface::RosterPilot, in_use: &dyn Fn(&str) -> bool) -> bool {
        p.available && !p.used && !in_use(&p.file_name)
    }
}

fn v3(v: mc2_sim::math::Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

impl LiveSim {
    /// `Artillery::render` for the bomber (`artlry.cpp:1407-1411`): every strike on its bomb run shows the Shilone at the strike
    /// position, posed on the animation frame the run has reached, until the animation has finished.
    fn bomber_instances(
        &mut self,
        scene: &SceneRenderer,
        view: &MissionView,
        out: &mut Vec<ShapeInstance>,
    ) {
        let sun = view.mission.light.terrain_light_direction();
        let w = &self.sim.world;
        let Some(rig) = view.bomber.as_ref() else {
            return;
        };
        // the bomber outlives the sim strike object: the C++ deletes it when the hit effect finishes (`artlry.cpp:903-907`)
        self.bomber_runs
            .retain(|r| !rig.run_finished(w.clock.turn - r.start_turn, w.clock.frame_length));
        for r in &self.bomber_runs {
            let frame = rig.frame_after(w.clock.turn - r.start_turn, w.clock.frame_length);
            let intensity = view.mission.terrain.light_intensity_at(r.at.x, r.at.y, sun);
            out.push(rig.instance(scene, r.at, frame, view.mission.light.ramp(intensity)));
        }
    }

    /// `(gesture, frame, turret_yaw, object_rotation)` of the buildings the last `append_instances`
    /// posed from the sim, in the order their instances were appended (after the movers).
    pub fn drawn_building_states(&self) -> &[(i32, f32, f32, f32)] {
        &self.drawn_buildings
    }

    /// The retained instance which `append_instances` submits for one mover.  This makes the
    /// production appearance handoff observable without rebuilding an instance from its static
    /// mission-view model.
    pub fn mover_instance(&self, handle: ObjectHandle) -> Option<&ShapeInstance> {
        self.movers
            .iter()
            .find(|binding| binding.handle == handle)
            .map(|binding| &binding.visibility)
    }

    /// The last projected appearance box for a mover, in viewport pixels.
    pub fn mover_screen_box(&self, handle: ObjectHandle) -> Option<[f32; 4]> {
        let part = self.sim.world.objects.get(handle)?.c.part_id.0;
        self.pick_boxes.movers.get(&part).copied()
    }

    /// Routes the sim's frame events that `LiveSim` itself keeps (camera requests, movement notes, structure flashes).
    pub fn apply_events(&mut self, events: &[SimEvent]) {
        for e in events {
            match e {
                SimEvent::Camera(r) => self.camera_requests.push(*r),
                SimEvent::MovementUnavailable { decision, .. } => {
                    self.note(&format!("mover: {decision:?}"));
                }
                SimEvent::Flash { object, argb } => self.flash_structure(*object, *argb),
                SimEvent::AirstrikeFlyover { at, .. } => self.bomber_runs.push(BomberRun {
                    at: v3(*at),
                    start_turn: self.sim.world.clock.turn,
                }),
                _ => {}
            }
        }
    }

    /// `appearance->flashBuilding(5.0, 0.5, color)` for one structure: what a [`SimEvent::Flash`] does.
    pub fn flash_structure(&mut self, handle: ObjectHandle, argb: u32) {
        self.flashes
            .entry(handle)
            .or_default()
            .start_structure(argb);
    }

    /// Handles of the simulation's structures that are bound to a static instance.
    pub fn structure_handles(&self) -> Vec<ObjectHandle> {
        self.structures.iter().map(|s| s.handle).collect()
    }

    /// The retained appearance instance of a structure (its selected LOD or damage shape).
    pub fn structure_instance(&self, handle: ObjectHandle) -> Option<&ShapeInstance> {
        self.structures
            .iter()
            .find(|s| s.handle == handle)
            .map(|s| &s.visibility)
    }

    /// The structure's selection rectangle from the last `recalcBounds` pass.
    pub fn structure_screen_box(&self, handle: ObjectHandle) -> Option<[f32; 4]> {
        self.structures
            .iter()
            .find(|s| s.handle == handle)
            .and_then(|s| s.selection_box)
    }

    /// The `(pilot, message)` radio notices from the most recent simulation step.
    pub fn last_radio_events(&self) -> &[(u32, i32)] {
        &self.last_radio_events
    }

    /// Loads `view`'s mission into a fresh simulation (`Mission::init`, single-player quickstart:
    /// the `-mission` start the application itself offers) and binds the simulation's movers and
    /// structures to the view's static instances.
    pub fn load(
        game: &mut GameData,
        view: &MissionView,
        config: &GameConfig,
        high_detail: bool,
    ) -> Result<Self, String> {
        let spec = sim_load_spec(&view.mission.name, config);
        let outcome = mc2_sim::mission::init::load_world(&mut game.vfs, &spec)
            .map_err(|e| format!("simulation load of {}: {e}", view.mission.name))?;
        let mut live = Self {
            pick_boxes: crate::hud_data::PickBoxes::default(),
            movers: Vec::new(),
            support_templates: view.support_templates.iter().cloned().collect(),
            late_unbound: BTreeSet::new(),
            load_unbound: Vec::new(),
            load_unmatched: Vec::new(),
            injected: Vec::new(),
            structures: Vec::new(),
            drawn_buildings: Vec::new(),
            hidden: vec![false; view.instances.len()],
            flashes: HashMap::new(),
            flash_hidden: Vec::new(),
            bomber_runs: Vec::new(),
            previous: HashMap::new(),
            clipped_models: std::collections::HashSet::new(),
            clips: HashMap::new(),
            camera_requests: Vec::new(),
            hud_messages: Vec::new(),
            last_radio_events: Vec::new(),
            unavailable: BTreeMap::new(),
            logged: BTreeSet::new(),
            object_appearances_complete: false,
            high_detail,
            recorded: None,
            command_log: None,
            failure: None,
            frames: 0,
            sim: mc2_sim::Sim::new(outcome),
            spec,
            launch: config.campaign_launch.clone(),
        };
        if !live.sim.world.load_is_complete() {
            for hook in live.sim.world.unported_load_hooks.clone() {
                live.note(&format!("load hook not ported: {hook}"));
            }
        }
        live.prepare_base(game, config.campaign_launch.as_ref());
        live.bind(view);
        Ok(live)
    }

    /// What a freshly loaded base world needs before play: the campaign roster and the support vehicles a purchase drops.
    /// A mission load restores a save over such a base, so it runs the same preparation.
    fn prepare_base(&mut self, game: &mut GameData, launch: Option<&mc2_campaign::LoadSpec>) {
        let live = self;
        if let Some(launch) = launch {
            // the roster's profiles, which `Mover::loadPilot` opens when a recovered mech gets its pilot
            let files: Vec<&str> = launch.roster.iter().map(|p| p.file_name.as_str()).collect();
            if let Err(e) = mc2_sim::mission::init::prepare_spawn_in(
                &mut live.sim.world,
                &mut game.vfs,
                &[],
                &files,
            ) {
                live.note(&format!("campaign roster pilots: {e}"));
            }
            live.sim.world.recovery_pilots = Some(std::rc::Rc::new(CampaignRoster(
                std::cell::RefCell::new(launch.roster.clone()),
            )));
        }
        // the support vehicles and the VTOL animations a purchase drops (`beginVtol` opens them on demand in the C++)
        if let Err(e) = mc2_sim::mission::vtol::prepare(&mut live.sim.world, &mut game.vfs) {
            live.note(&format!("support vehicles: {e}"));
        }
    }

    /// Adds `commands` to the next step (the commands the order bridge would have produced).
    pub fn inject_commands(&mut self, commands: Vec<PlayerCommand>) {
        self.injected.extend(commands);
    }

    /// The simulation (read-only: digests, world state).
    pub fn sim(&self) -> &mc2_sim::Sim {
        &self.sim
    }

    /// The selection boxes a click is tested against.
    pub fn pick_boxes(&self) -> &crate::hud_data::PickBoxes {
        &self.pick_boxes
    }

    /// The eight type-box corners (world axes) of the mover with `handle`, from its shipped
    /// appearance extents (`mech3d.cpp:2173-2203`; vehicles rotate theirs, so only mechs are
    /// answered), for tests that project them through the camera themselves.
    pub fn mech_type_box_world(
        &self,
        scene: &SceneRenderer,
        handle: ObjectHandle,
    ) -> Option<[Vec3; 8]> {
        let binding = self.movers.iter().find(|b| b.handle == handle)?;
        let inst = &binding.visibility;
        if inst.shapes.kind != mc2_render::appearance::AppearanceKind::Mech {
            return None;
        }
        let model = scene.model(inst.model);
        let h = inst.root_height_difference;
        Some(match inst.shapes.selection_bounds {
            Some(b) => mc2_render::shape::ShapeModel::mech_designer_selection_box_world_at_height(
                b,
                inst.position,
                h,
            ),
            None => model.mech_selection_box_world_at_height(inst.position, h),
        })
    }

    /// Every triangle of the shape a structure currently shows, in world space (the model's vertices
    /// through the instance's node matrices), for tests that derive a click's expected hit from the
    /// shape and the camera instead of from the picker's own screen triangles.
    pub fn structure_world_triangles(
        &self,
        scene: &SceneRenderer,
        handle: ObjectHandle,
    ) -> Vec<[Vec3; 3]> {
        let Some(binding) = self.structures.iter().find(|b| b.handle == handle) else {
            return Vec::new();
        };
        let inst = &binding.visibility;
        let model = scene.model(inst.model);
        let mats =
            mc2_render::shape::instance_node_matrices(&inst.pose, inst.position, inst.yaw_degrees);
        let mut out = Vec::new();
        for batch in &model.batches {
            let first = batch.first_vertex as usize;
            let verts = &model.vertices[first..first + batch.vertex_count as usize];
            for tri in verts.as_chunks::<3>().0 {
                let mut corners = [Vec3::new(0.0, 0.0, 0.0); 3];
                let mut ok = true;
                for (c, v) in corners.iter_mut().zip(tri) {
                    match mats.get(v.node as usize) {
                        Some(m) => {
                            *c = m.transform_point(Vec3::new(
                                v.position[0],
                                v.position[1],
                                v.position[2],
                            ))
                        }
                        None => ok = false,
                    }
                }
                if ok {
                    out.push(corners);
                }
            }
        }
        out
    }

    /// The selection boxes a click is tested against (a declared input for tests that place an
    /// object where the camera did not).
    pub fn pick_boxes_mut(&mut self) -> &mut crate::hud_data::PickBoxes {
        &mut self.pick_boxes
    }

    /// The simulation, mutable (tests that need an unavailable step).
    pub fn sim_mut(&mut self) -> &mut mc2_sim::Sim {
        &mut self.sim
    }

    /// Keeps every frame's inputs (visibility, commands, clock) for [`Self::recorded_frames`].
    pub fn record_inputs(&mut self, on: bool) {
        self.recorded = on.then(Vec::new);
    }

    /// Keeps the commands of every frame that has any, for [`Self::command_log`] (a light
    /// alternative to [`Self::record_inputs`]).
    pub fn log_commands(&mut self, on: bool) {
        self.command_log = on.then(Vec::new);
    }

    /// `(frame, commands)` for each frame that had commands, the frame counted from 1 like the oracle.
    pub fn command_log(&self) -> &[(u64, Vec<PlayerCommand>)] {
        self.command_log.as_deref().unwrap_or(&[])
    }

    pub fn recorded_frames(&self) -> &[RecordedFrame] {
        self.recorded.as_deref().unwrap_or(&[])
    }

    /// The movers the load-time binding found no model for.
    pub fn load_unbound_movers(&self) -> &[ObjectHandle] {
        &self.load_unbound
    }

    /// The structures the load-time binding found no placed model for.
    pub fn load_unmatched_structures(&self) -> &[ObjectHandle] {
        &self.load_unmatched
    }

    /// Unavailable behaviour reached so far, with counts.
    pub fn unavailable(&self) -> &BTreeMap<String, u64> {
        &self.unavailable
    }

    fn note(&mut self, what: &str) {
        *self.unavailable.entry(what.to_string()).or_insert(0) += 1;
        if self.logged.insert(what.to_string()) {
            log::warn!("simulation unavailable: {what}");
        }
    }

    /// Matches the simulation's objects to the static instances `populate` made: movers by the
    /// nearest unused part instance, structures by their exact placed position.
    fn bind(&mut self, view: &MissionView) {
        let world = &self.sim.world;
        let mut used = BTreeSet::new();
        let mut movers = Vec::new();
        let mut skipped = 0;
        for &h in &world.objects.movers {
            let Some(o) = world.objects.get(h) else {
                continue;
            };
            // an unloaded part's mover slot (`exists() == false`, e.g. the mission file's own home
            // parts that a logistics launch replaces) is never drawn, as `populate` skips it
            if !o.exists() {
                skipped += 1;
                continue;
            }
            let best = view
                .part_instances
                .iter()
                .copied()
                .filter(|i| !used.contains(i))
                .map(|i| {
                    let p = view.instances[i].position;
                    let d = (p.x - o.c.position.x).hypot(p.y - o.c.position.y);
                    (d, i)
                })
                .filter(|(d, _)| *d < 128.0)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, i)) = best {
                used.insert(i);
                movers.push(MoverBinding {
                    handle: h,
                    static_index: Some(i),
                    visibility: view.instances[i].clone(),
                });
            }
        }
        let bound_handles: BTreeSet<ObjectHandle> = movers.iter().map(|m| m.handle).collect();
        let unbound_handles: Vec<ObjectHandle> = world
            .objects
            .movers
            .iter()
            .copied()
            .filter(|h| {
                !bound_handles.contains(h) && world.objects.get(*h).is_some_and(|o| o.exists())
            })
            .collect();
        let unbound = world.objects.movers.len() - movers.len() - skipped;
        let mut by_source: HashMap<(i32, u32, u32), Vec<usize>> = HashMap::new();
        for source in &view.terrain_instances {
            by_source
                .entry((
                    source.object_type,
                    source.position.x.to_bits(),
                    source.position.y.to_bits(),
                ))
                .or_default()
                .push(source.index);
        }
        let mut structures = Vec::new();
        let mut structure_handles: Vec<ObjectHandle> = Vec::new();
        for list in [
            &world.objects.terrain_objects,
            &world.objects.buildings,
            &world.objects.turrets,
            &world.objects.gates,
            &world.objects.special_buildings,
        ] {
            structure_handles.extend(list.iter().copied());
        }
        structure_handles.sort_by_key(|h| h.0);
        structure_handles.dedup();
        let mut unmatched = 0usize;
        let mut unmatched_handles = Vec::new();
        for h in structure_handles {
            let Some(o) = world.objects.get(h) else {
                continue;
            };
            let key = (
                o.c.type_handle,
                o.c.position.x.to_bits(),
                o.c.position.y.to_bits(),
            );
            match by_source.get_mut(&key).and_then(|v| v.pop()) {
                Some(i) => structures.push(StructureBinding {
                    handle: h,
                    static_index: i,
                    visibility: view.instances[i].clone(),
                    selection_box: None,
                }),
                None => {
                    unmatched += 1;
                    unmatched_handles.push(h);
                }
            }
        }
        for i in movers.iter().filter_map(|m| m.static_index) {
            self.hidden[i] = true;
        }
        self.movers = movers;
        self.structures = structures;
        self.load_unbound = unbound_handles;
        self.load_unmatched = unmatched_handles;
        self.object_appearances_complete = appearances_complete(unbound, unmatched);
        if unbound > 0 {
            self.note(&format!(
                "{unbound} simulation movers have no model in the view (drawn nowhere)"
            ));
        }
        if unmatched > 0 {
            self.note(&format!(
                "{unmatched} simulation structures have no placed model (appearance not driven)"
            ));
        }
    }

    /// Binds the movers the simulation created after the load (a dropped support vehicle) to the
    /// home-painted template of their object type, and returns how many existing movers still have
    /// no model (reported unavailable; never drawn as a point).
    fn bind_new_movers(&mut self) -> usize {
        let world = &self.sim.world;
        let bound: BTreeSet<ObjectHandle> = self.movers.iter().map(|m| m.handle).collect();
        let mut fresh = Vec::new();
        let mut missing = Vec::new();
        for &h in &world.objects.movers {
            let Some(o) = world.objects.get(h) else {
                continue;
            };
            if !o.exists() || bound.contains(&h) || self.late_unbound.contains(&h) {
                continue;
            }
            match self.support_templates.get(&o.c.type_handle) {
                Some(t) => fresh.push(MoverBinding {
                    handle: h,
                    static_index: None,
                    visibility: t.clone(),
                }),
                None => missing.push(h),
            }
        }
        self.movers.extend(fresh);
        for h in missing {
            self.late_unbound.insert(h);
            self.note("a mover created after the load has no model in the view (drawn nowhere)");
        }
        self.late_unbound.len()
    }

    fn snapshot_transforms(&mut self) {
        self.previous.clear();
        for m in &self.movers {
            if let Some(o) = self.sim.world.objects.get(m.handle) {
                self.previous
                    .insert(m.handle, (v3(o.c.position), o.c.rotation));
            }
        }
    }

    fn render_status(status: ObjectStatus) -> RenderObjectStatus {
        match status {
            ObjectStatus::Disabled => RenderObjectStatus::Disabled,
            ObjectStatus::Destroyed => RenderObjectStatus::Destroyed,
            _ => RenderObjectStatus::Normal,
        }
    }

    fn install_visibility_state(
        inst: &mut ShapeInstance,
        position: Vec3,
        yaw: f32,
        state: AppearanceState,
        shown: Shown,
        current_lod: usize,
    ) {
        inst.position = position;
        inst.yaw_degrees = yaw;
        inst.state = state;
        inst.choice = ShapeChoice {
            shown,
            current_lod,
            status: state.status,
        };
        if let Some(model) = inst.shapes.model(shown) {
            inst.model = model;
        }
    }

    /// Runs R1's production `select_shapes` before `Sim::step`, in the same camera frame that
    /// will be drawn. The retained bound instances carry the C++ appearance latches across
    /// frames; `mc2-render` is only read through this selection result.
    fn object_visibility(
        &mut self,
        scene: &SceneRenderer,
        camera: &GameCamera,
        terrain: &CameraVisibility,
    ) -> ObjectVisibility {
        let late_unbound = self.bind_new_movers();
        let w = &self.sim.world;
        // The C++ first mission update reaches `recalcBounds` before the renderer has installed
        // its screen resolution. Its turn-1 visibility trace therefore has no objects in view;
        // use that same zero-sized selection viewport only for this pre-step appearance pass.
        let first_stepping_turn = w.clock.turn + 1 == 1;
        let mut first_turn_camera = camera.clone();
        if first_stepping_turn {
            first_turn_camera.viewport = (0.0, 0.0);
        }
        let selection_camera = if first_stepping_turn {
            &first_turn_camera
        } else {
            camera
        };
        let mut mover_triangles = BTreeMap::new();
        // Mover `recalcBounds` runs on every appearance update.  Unlike structures, a
        // mover that is no longer in the current viewport must stop being pickable now;
        // retaining its prior turn here lets E select movers left behind by a camera pan.
        let mut mover_drawn_turn = BTreeMap::new();
        let mut structure_triangles = std::mem::take(&mut self.pick_boxes.structure_triangles);
        let mut out = ObjectVisibility {
            complete: self.object_appearances_complete && late_unbound == 0,
            ..ObjectVisibility::default()
        };
        for binding in &mut self.movers {
            let Some(object) = w.objects.get(binding.handle) else {
                out.complete = false;
                continue;
            };
            let position = v3(object.c.position);
            let status = Self::render_status(object.c.status);
            match w.appearance.by_handle.get(binding.handle.0 as usize) {
                Some(Some(Appearance::Mech(a))) => {
                    let shown = match a.shape_kind {
                        mc2_sim::appearance::mech::ShapeKind::Lod(lod) => Shown::Lod(lod),
                        mc2_sim::appearance::mech::ShapeKind::ForwardDamage => Shown::Fall(0),
                        mc2_sim::appearance::mech::ShapeKind::BackwardDamage => Shown::Fall(1),
                    };
                    Self::install_visibility_state(
                        &mut binding.visibility,
                        position,
                        object.c.rotation,
                        AppearanceState {
                            status,
                            gesture: a.bound_gesture,
                            frame: a.shape.frame_num,
                            fall_shape: match a.shape_kind {
                                mc2_sim::appearance::mech::ShapeKind::ForwardDamage => Some(0),
                                mc2_sim::appearance::mech::ShapeKind::BackwardDamage => Some(1),
                                mc2_sim::appearance::mech::ShapeKind::Lod(_) => None,
                            },
                            base_root_node_height: (a.base_root_node_height != -99999.9)
                                .then_some(a.base_root_node_height),
                            is_helicopter: a.is_helicopter,
                        },
                        shown,
                        a.current_lod,
                    );
                }
                Some(Some(Appearance::Vehicle(a))) => {
                    let shown = match a.shape_kind {
                        mc2_sim::appearance::vehicle::GvShapeKind::Lod(lod) => Shown::Lod(lod),
                        mc2_sim::appearance::vehicle::GvShapeKind::Damage => Shown::Damaged,
                    };
                    Self::install_visibility_state(
                        &mut binding.visibility,
                        position,
                        object.c.rotation,
                        AppearanceState {
                            status,
                            gesture: a.current_gesture.try_into().ok(),
                            frame: a.shape.frame_num,
                            ..AppearanceState::default()
                        },
                        shown,
                        a.current_lod,
                    );
                }
                _ => {
                    out.complete = false;
                    continue;
                }
            }
            // `GameObjectManager::updateAppearancesOnly` visits all movers every frame after
            // the terrain-object pass (`code/objmgr.cpp:2813-2855`).  Their appearance bounds
            // are consequently always fresh before the mover simulation consumes `inView`.
            let selection_box =
                selection_box_screen_bounds(scene, &mut binding.visibility, selection_camera);
            select_shapes(
                scene,
                std::slice::from_mut(&mut binding.visibility),
                selection_camera,
                self.high_detail,
            );
            if binding.visibility.in_view {
                out.movers_in_view.insert(object.c.part_id.0);
                mover_drawn_turn.insert(object.c.part_id.0, w.clock.turn as i32);
                mover_triangles.insert(
                    object.c.part_id.0,
                    Arc::from(visible_screen_triangles(
                        scene,
                        &mut binding.visibility,
                        selection_camera,
                    )),
                );
            }
            out.mover_lods
                .insert(object.c.part_id.0, binding.visibility.choice.current_lod);
            out.mover_selection_boxes
                .insert(object.c.part_id.0, selection_box);
        }
        for binding in &mut self.structures {
            let Some(object) = w.objects.get(binding.handle) else {
                out.complete = false;
                continue;
            };
            let position = v3(object.c.position);
            let status = Self::render_status(object.c.status);
            let (shown, current_lod, gesture, frame) = match w
                .appearance
                .by_handle
                .get(binding.handle.0 as usize)
            {
                Some(Some(Appearance::Building(a))) => (
                    match a.shape_kind {
                        mc2_sim::appearance::structure::BdShapeKind::Lod(lod) => Shown::Lod(lod),
                        mc2_sim::appearance::structure::BdShapeKind::Damage => Shown::Damaged,
                    },
                    a.current_lod,
                    a.current_gesture.try_into().ok(),
                    a.shape.frame_num,
                ),
                Some(Some(Appearance::Tree(a))) => (
                    match a.shape_kind {
                        mc2_sim::appearance::structure::BdShapeKind::Lod(lod) => Shown::Lod(lod),
                        mc2_sim::appearance::structure::BdShapeKind::Damage => Shown::Damaged,
                    },
                    a.current_lod,
                    None,
                    0.0,
                ),
                _ => {
                    out.complete = false;
                    continue;
                }
            };
            Self::install_visibility_state(
                &mut binding.visibility,
                position,
                object.c.rotation,
                AppearanceState {
                    status,
                    gesture,
                    frame,
                    ..AppearanceState::default()
                },
                shown,
                current_lod,
            );
            // `GameObjectManager::updateAppearancesOnly` only runs a terrain object's
            // `recalcBounds` while its block and vertex are active (or during turns 0..2).
            // Turrets, gates and special buildings are separate update lists, so they always
            // refresh.  Every other object retains its prior appearance `inView` and LOD.
            let block_recalculates = w.objects.blocks.iter().enumerate().any(|(index, block)| {
                binding.handle.0 >= block.first_handle
                    && binding.handle.0 < block.first_handle + block.num_objects
                    && terrain_object_recalculates(
                        w.clock.turn + 1,
                        terrain.block_active(index as u32)
                            && object.c.vertex_num >= 0
                            && terrain.vertex_active(object.c.vertex_num as u32),
                    )
            });
            let always_recalculates = w.objects.turrets.contains(&binding.handle)
                || w.objects.gates.contains(&binding.handle)
                || w.objects.special_buildings.contains(&binding.handle);
            if block_recalculates || always_recalculates {
                let selection_box =
                    selection_box_screen_bounds(scene, &mut binding.visibility, selection_camera);
                select_shapes(
                    scene,
                    std::slice::from_mut(&mut binding.visibility),
                    selection_camera,
                    self.high_detail,
                );
                binding.selection_box = Some(selection_box);
                if binding.visibility.in_view {
                    structure_triangles.insert(
                        binding.handle.0,
                        Arc::from(visible_screen_triangles(
                            scene,
                            &mut binding.visibility,
                            selection_camera,
                        )),
                    );
                }
            }
            if binding.visibility.in_view {
                out.structures_in_view.insert(binding.handle.0);
            }
            out.structure_lods
                .insert(binding.handle.0, binding.visibility.choice.current_lod);
            if let Some(selection_box) = binding.selection_box {
                out.structure_selection_boxes
                    .insert(binding.handle.0, selection_box);
            }
        }
        // The trace header describes the complete run, not merely the bindings found while the
        // mission loaded.  A live object can lose its appearance after binding; once that has
        // happened this run must keep reporting object visibility as incomplete.
        self.object_appearances_complete &= out.complete;
        self.pick_boxes = crate::hud_data::PickBoxes {
            movers: out.mover_selection_boxes.clone(),
            structures: out.structure_selection_boxes.clone(),
            mover_triangles,
            structure_triangles,
            mover_drawn_turn,
        };
        out
    }
}

impl SimDriver for LiveSim {
    const IS_LIVE_SIM: bool = true;
    type Command = PlayerCommand;
    type Event = SimEvent;
    type World = mc2_sim::World;

    fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    fn set_use_non_weapon_effects(&mut self, on: bool) {
        self.sim.world.objects.use_non_weapon_effects = on;
    }

    fn step(&mut self, mut input: FrameInput<PlayerCommand>) -> FrameOutput<SimEvent> {
        input.commands.append(&mut self.injected);
        let frame_length = 1.0 / input.frame_rate;
        self.snapshot_transforms();
        let c = input.camera;
        let w = |v: Vec3| mc2_sim::math::Vec3 {
            x: v.x,
            y: v.y,
            z: v.z,
        };
        self.sim.world.audio = input.audio;
        self.sim.world.callout = input.callout;
        self.sim.world.camera = mc2_sim::events::CameraState {
            position: w(c.position),
            goal_position: w(c.goal_position),
            rotation: w(c.rotation),
            goal_rotation: w(c.goal_rotation),
            velocity: w(c.velocity),
            goal_velocity: w(c.goal_velocity),
            fov: c.fov,
            goal_fov: c.goal_fov,
            look_object: c.look_object,
        };
        let mut commands = input.commands;
        // The GUI's presentation keys are player input of this frame, like orders (the oracle sends them at the same point,
        // oracle_main.cpp's camera script): ESC in movie mode and the mouse profile.
        if input.presentation.force_movie_end {
            commands.push(PlayerCommand::ForceMovieEnd);
        }
        if let Some(aoe) = input.presentation.aoe_style {
            commands.push(PlayerCommand::SetAoeStyle(aoe));
        }
        if let Some(log) = self.command_log.as_mut() {
            if !commands.is_empty() {
                log.push((self.frames + 1, commands.clone()));
            }
        }
        let record = self.recorded.is_some().then(|| {
            (
                input.frame_rate,
                input.now_ms,
                input.visibility.clone(),
                commands.clone(),
                self.sim.world.camera,
                self.sim.world.audio.clone(),
                self.sim.world.callout,
            )
        });
        let visibility: Rc<dyn Visibility> = input.visibility;
        let result = self.sim.step(mc2_sim::FrameInput {
            frame_rate: input.frame_rate,
            now_ms: input.now_ms,
            visibility,
            commands,
        });
        self.frames += 1;
        if let (
            Some(rec),
            Some((frame_rate, now_ms, visibility, commands, camera, audio, callout)),
        ) = (self.recorded.as_mut(), record)
        {
            rec.push(RecordedFrame {
                frame_rate,
                now_ms,
                visibility,
                commands,
                camera,
                audio,
                callout,
                digest: self.sim.digest_with_pilots(self.frames as i64, 2),
            });
        }
        crate::hud_data::ensure_pick_watch_ids(&mut self.sim.world);
        let out = match result {
            Ok(out) => out,
            Err(e) => {
                // An incomplete load cannot be stepped faithfully: say so every frame, run nothing.
                self.note(&format!("{e}"));
                self.failure = Some(e.to_string());
                return FrameOutput::default();
            }
        };
        for u in &out.unavailable {
            let key = match u {
                mc2_sim::FrameUnavailable::Ai { what, .. } => format!("pilot AI: {what:?}"),
                mc2_sim::FrameUnavailable::ObjectInView => {
                    "object in-view (a live object has no bound appearance)".to_string()
                }
                mc2_sim::FrameUnavailable::VisualLos { .. } => {
                    "sensor visual line of sight".to_string()
                }
                mc2_sim::FrameUnavailable::Combat { hook } => format!("combat: {hook}"),
            };
            self.note(&key);
        }
        self.hud_messages
            .extend(crate::hud_data::messages(&out.events));
        self.last_radio_events.clear();
        self.last_radio_events
            .extend(out.events.iter().filter_map(|event| {
                if let SimEvent::Radio { pilot, message, .. } = event {
                    Some((*pilot, *message))
                } else {
                    None
                }
            }));
        self.apply_events(&out.events);
        // `BldgAppearance::update` runs the flash every frame, in view or not (bdactor.cpp:1936-1951)
        self.flashes.retain(|_, f| {
            f.advance(frame_length);
            f.is_active()
        });
        FrameOutput {
            events: out.events,
            result: out.result,
        }
    }

    fn world(&self) -> &mc2_sim::World {
        &self.sim.world
    }

    fn scenario_time(&self) -> f32 {
        self.sim.world.clock.scenario_time
    }

    fn turn(&self) -> i64 {
        self.sim.world.clock.turn
    }

    fn pause_command(&self, paused: bool) -> Option<PlayerCommand> {
        Some(PlayerCommand::SetPaused(paused))
    }

    fn object_visibility(
        &mut self,
        scene: &SceneRenderer,
        camera: &GameCamera,
        terrain: &CameraVisibility,
    ) -> Option<ObjectVisibility> {
        Some(LiveSim::object_visibility(self, scene, camera, terrain))
    }

    fn append_forced_instances(
        &mut self,
        scene: &SceneRenderer,
        view: &MissionView,
        out: &mut Vec<ShapeInstance>,
    ) {
        self.bomber_instances(scene, view, out);
    }

    fn waypoint_paths(&self, selected: &[i32]) -> Vec<crate::waypoints::WaypointPath> {
        crate::waypoints::paths(&self.sim.world, selected)
    }

    fn append_instances(
        &mut self,
        scene: &mut SceneRenderer,
        view: &MissionView,
        alpha: f32,
        out: &mut Vec<ShapeInstance>,
    ) {
        for i in self.flash_hidden.drain(..) {
            self.hidden[i] = false;
        }
        let alpha = alpha.clamp(0.0, 1.0);
        let sun = view.mission.light.terrain_light_direction();
        for m in &self.movers {
            let w = &self.sim.world;
            let Some(o) = w.objects.get(m.handle) else {
                continue;
            };
            if !o.exists() {
                continue;
            }
            let cur = v3(o.c.position);
            let (position, yaw) = match self.previous.get(&m.handle) {
                Some(&(p, r)) if (cur - p).length() < MAX_INTERPOLATED_STEP => {
                    let mut d = o.c.rotation - r;
                    if d > 180.0 {
                        d -= 360.0;
                    } else if d < -180.0 {
                        d += 360.0;
                    }
                    (p + (cur - p) * alpha, r + d * alpha)
                }
                _ => (cur, o.c.rotation),
            };
            // `object_visibility` has already installed the simulation's exact appearance
            // selection (including a destroyed mech's forward/backward fall shape selected by
            // `mech.cpp:4786-4791`, or a vehicle damage shape after `gvehicl.cpp:576-581`).
            // The old path constructed a fresh one-LOD instance here, so every mover was drawn
            // intact even when its retained appearance was destroyed.
            let mut inst = m.visibility.clone();
            // A mech's gesture animations come from its appearance type (`Mech3DAppearanceType::
            // setAnimation`, `mech3d.cpp:655`): bind them to every LOD model once, so the
            // `state_pose` below samples the bound gesture at the simulation's frame.
            if let Some(Some(Appearance::Mech(a))) = w.appearance.by_handle.get(m.handle.0 as usize)
            {
                if let Some(t) = w.appearance.mech_types.get(a.type_index) {
                    for lod in inst.shapes.lods.iter().flatten() {
                        if self.clipped_models.insert(lod.0) {
                            let clips = t
                                .anims
                                .iter()
                                .map(|set| {
                                    set.as_ref().map(|set| {
                                        AnimationClip::from_agl(scene.model(*lod), &set.entries)
                                    })
                                })
                                .collect();
                            scene.set_gesture_clips(*lod, clips);
                        }
                    }
                }
            }
            let model = scene.model(inst.model);
            inst.pose = state_pose(model, &inst.state);
            // Mech torso twist: the sim appearance's `torsoRotation` turns `joint_torso` on top of the gesture
            // clip (`mech3d.cpp:729-733`); kept on the instance so a later LOD pick re-poses with it.
            if let Some(Some(Appearance::Mech(a))) = w.appearance.by_handle.get(m.handle.0 as usize)
            {
                inst.turret =
                    mc2_render::appearance::torso_twist(scene.model(inst.model), a.torso_rotation);
                if inst.turret.is_some() {
                    inst.pose = mc2_render::appearance::state_pose_with_node(
                        scene.model(inst.model),
                        &inst.state,
                        inst.turret.as_ref(),
                    );
                }
            }
            // Ground vehicle animation and turret yaw from the sim's appearance
            // (`gvactor.cpp:2402-2406`): the type's animation clip at `current_frame`, and the
            // rotational node turned about the shape's Y axis by `turret_rotation` degrees.
            if let Some(Some(Appearance::Vehicle(a))) =
                w.appearance.by_handle.get(m.handle.0 as usize)
            {
                let ty = w.appearance.vehicle_types.get(a.type_index);
                // The simulation's vehicle appearance is not yet fed `setMoverParameters`
                // (`gvehicl.cpp:3288`; its `turret_rotation` just follows the body), so the
                // turret state is read from the vehicle object itself.
                let turret = match &o.kind {
                    mc2_sim::object::Kind::Vehicle(v) => v.turret_rotation,
                    _ => 0.0,
                };
                let mut base = HashMap::new();
                if let Some(idx) = ty
                    .map(|t| t.rotational_node_id.as_str())
                    .filter(|n| !n.is_empty() && *n != "NONE")
                    .and_then(|n| model.node_index(n))
                {
                    let q = Quat::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), turret.to_radians());
                    base.insert(idx, q);
                    // kept on the instance: the final LOD selection re-poses its own model with it
                    inst.turret = Some((model.nodes[idx].id.clone(), q));
                }
                // Each LOD has its own node order; retain the type's clips on every model
                // so final LOD selection can re-pose it (gvactor.cpp:1759-1770).
                if let Some(t) = ty {
                    for lod in inst.shapes.lods.iter().flatten() {
                        if self.clipped_models.insert(lod.0) {
                            let clips = t
                                .anims
                                .iter()
                                .map(|set| {
                                    set.as_ref().map(|set| {
                                        AnimationClip::from_agl(scene.model(*lod), &set.entries)
                                    })
                                })
                                .collect();
                            scene.set_gesture_clips(*lod, clips);
                        }
                    }
                }
                let model = scene.model(inst.model);
                let clip = usize::try_from(a.current_gesture)
                    .ok()
                    .and_then(|g| model.gestures.get(g))
                    .and_then(Option::as_ref);
                if clip.is_some() || !base.is_empty() {
                    let frame = if clip.is_some() { a.current_frame } else { 0.0 };
                    inst.pose = model.pose(clip, frame, &base);
                    inst.state.gesture = Some(a.current_gesture.max(0) as usize);
                    inst.state.frame = frame;
                }
            }
            inst.pose_key = Some((inst.model, inst.state));
            let intensity = view
                .mission
                .terrain
                .light_intensity_at(position.x, position.y, sun);
            let light = view.mission.light.ramp(intensity);
            inst.position = position;
            inst.yaw_degrees = yaw;
            inst.infinite_light = light;
            inst.in_view = true;
            out.push(inst);
        }
        // Structures the simulation destroyed: render the damaged shape if available.
        // The damage shape is selected by select_shapes via apply_status (bdactor.cpp:759-820).
        // The binding's visibility instance was already processed through select_shapes in the
        // visibility computation pass (lines 552-620), so we use it directly to preserve the
        // correct shape choice for destroyed/disabled structures.
        self.drawn_buildings.clear();
        // Animated survivors are appended after the destroyed shapes, so `drawn_buildings` indexes
        // the tail of `out`.
        let mut animated: Vec<ShapeInstance> = Vec::new();
        let mut flashing_static: Vec<ShapeInstance> = Vec::new();
        for s in &self.structures {
            // `drawFlash`: the shape is drawn with `SetARGBHighLight(flashColor)` (bdactor.cpp:1528-1531)
            let tint = self
                .flashes
                .get(&s.handle)
                .and_then(BuildingFlash::tint)
                .unwrap_or(0);
            let status = self.sim.world.objects.get(s.handle).map(|o| o.c.status);
            let is_destroyed =
                status == Some(ObjectStatus::Destroyed) || status == Some(ObjectStatus::Disabled);
            if is_destroyed {
                // Use the binding's visibility instance which already has the correct status
                // and shape choice from the visibility computation pass.
                let mut base = s.visibility.clone();
                base.highlight = tint;
                // Set the building's rotation from the sim (bdactor.cpp:1031).
                if let Some(o) = self.sim.world.objects.get(s.handle) {
                    base.yaw_degrees = o.c.rotation;
                }
                // The intact static instance is replaced by the damage shape, not drawn under it.
                self.hidden[s.static_index] = true;
                out.push(base);
                continue;
            }
            // Surviving structures: the sim's `BldgAppearance` animation (gates, doors, radar)
            // replaces the static pose; a structure with no running animation is the static
            // instance; a turreted structure also draws its turret yaw/pitch.
            let w = &self.sim.world;
            let Some(Some(Appearance::Building(a))) =
                w.appearance.by_handle.get(s.handle.0 as usize)
            else {
                continue;
            };
            let ty = w.appearance.building_types.get(a.type_index);
            // no data for the gesture: the type has no animation, the static pose is right
            let set = if a.current_gesture >= 0 {
                ty.and_then(|t| t.anims.get(a.current_gesture as usize))
                    .and_then(|s| s.as_ref())
            } else {
                None
            };
            let Some(base) = view.instances.get(s.static_index) else {
                continue;
            };
            let model = scene.model(base.model);
            // Turret: `SetNodeRotation` with Euler(pitch, yaw, 0) (bdactor.cpp:2036), pitch about
            // X then yaw about Y, like the vehicle turret.  The node is the type's
            // `AnimationNodeId` found by case-insensitive name (`TG_MultiShape::SetNodeRotation`,
            // msl.cpp:965); there is no fallback.  A type with no `[AnimationNode]` block ("NONE",
            // e.g. the `guntower` shape, whose node is GunPivot but whose type does not name it) or
            // a shape without that node gets index -1, which the C++ ignores (msl.h:318): the
            // shape is drawn unrotated.  That is the faithful behaviour, not an unavailable feature.
            let node = ty
                .map(|t| t.rotational_node_id.as_str())
                .filter(|n| !n.is_empty() && *n != "NONE")
                .and_then(|n| model.node_index(n));
            let mut base_rotations: HashMap<usize, Quat> = HashMap::new();
            let mut turret_pose = None;
            if let Some(idx) = node {
                let yaw =
                    Quat::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), a.turret_yaw.to_radians());
                let pitch =
                    Quat::from_axis_angle(Vec3::new(1.0, 0.0, 0.0), a.turret_pitch.to_radians());
                base_rotations.insert(idx, yaw * pitch);
                turret_pose = Some((model.nodes[idx].id.clone(), yaw * pitch));
            }
            if set.is_none() && base_rotations.is_empty() {
                if tint != 0 {
                    // a static building that is flashing: its tinted copy replaces the static instance this frame
                    let mut inst = base.clone();
                    inst.highlight = tint;
                    self.hidden[s.static_index] = true;
                    self.flash_hidden.push(s.static_index);
                    flashing_static.push(inst);
                }
                continue;
            }
            // The clip must be keyed by building type, not just model and gesture, because
            // different buildings of the same type may have different animation sets for
            // the same gesture index.
            let clip_key = (
                base.model.0,
                a.type_index.try_into().unwrap(),
                a.current_gesture,
            );
            let clip = match set {
                Some(set) => self
                    .clips
                    .entry(clip_key)
                    .or_insert_with(|| Some(AnimationClip::from_agl(model, &set.entries)))
                    .as_ref(),
                None => None,
            };
            let frame = if clip.is_some() { a.current_frame } else { 0.0 };
            let mut inst = base.clone();
            inst.highlight = tint;
            inst.state.gesture = clip.map(|_| a.current_gesture as usize);
            inst.turret = turret_pose;
            inst.state.frame = frame;
            inst.pose = model.pose(clip, frame, &base_rotations);
            // pose_key stops update_pose recomputing a pose built from the clip and turret.
            inst.pose_key = Some((inst.model, inst.state));
            if let Some(o) = w.objects.get(s.handle) {
                inst.yaw_degrees = o.c.rotation;
            }
            self.hidden[s.static_index] = true;
            // The building pose depends on both the object's rotation (base rotation) and turret yaw.
            // Include both in the state so that changes are detected.
            let object_rotation = w.objects.get(s.handle).map(|o| o.c.rotation).unwrap_or(0.0);
            let state = (
                a.current_gesture,
                frame,
                a.turret_yaw + a.turret_pitch * 1000.0,
                object_rotation,
            );
            self.drawn_buildings.push(state);
            animated.push(inst);
        }
        out.extend(flashing_static);
        out.extend(animated);
    }

    fn hides_static_instance(&self, index: usize) -> bool {
        self.hidden.get(index).copied().unwrap_or(false)
    }

    fn camera_target(&self, part_id: i32) -> Option<Vec3> {
        // `getCamObject(partId, true)` then `GameCamera::update`'s follow test: an existing object
        // of the home commander.  Other objects need `teamLineOfSight` / contact quality, which
        // the simulation does not expose yet: no target then (the camera stays where it is).
        let w = &self.sim.world;
        let o = w
            .objects
            .movers
            .iter()
            .filter_map(|&h| w.objects.get(h))
            .find(|o| o.c.part_id.0 == part_id)?;
        (o.exists() && o.commander_id() == mc2_sim::combat::HOME_COMMANDER)
            .then(|| v3(o.c.position))
    }

    fn camera_requests(&mut self) -> Vec<CameraRequest> {
        std::mem::take(&mut self.camera_requests)
    }

    fn hud_snapshot(&mut self) -> Option<crate::hud_data::HudSnapshot> {
        let mut snapshot = crate::hud_data::snapshot(&self.sim.world, &self.pick_boxes);
        let targets: Vec<_> = snapshot
            .picks
            .iter_mut()
            .filter(|pick| pick.captureable)
            .map(|pick| (pick.watch_id, pick))
            .collect();
        let home = self.sim.world.teams.home as i32;
        let movers = self.sim.world.objects.movers.clone();
        for (watch_id, pick) in targets {
            let Some(target) = self
                .sim
                .world
                .objects
                .by_watch_id(mc2_sim::ids::WatchId(watch_id))
            else {
                continue;
            };
            for &mover in &movers {
                let Some(unit) = self.sim.world.objects.get(mover) else {
                    continue;
                };
                if unit.team_id() != home || !matches!(&unit.kind, mc2_sim::object::Kind::Mech(_)) {
                    continue;
                }
                let part = unit.c.part_id.0;
                let blocked =
                    !mc2_sim::ai::capture::blockers(&mut self.sim.world, target, mover).is_empty();
                pick.capture_blockers.insert(part, blocked);
            }
        }
        Some(snapshot)
    }

    fn presentation_requests(&self, events: &[SimEvent]) -> Vec<SimEvent> {
        events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    SimEvent::PlayVideo { .. } | SimEvent::TutorialText { .. }
                )
            })
            .cloned()
            .collect()
    }

    fn sim_presentation(
        &self,
        world: &mc2_sim::World,
    ) -> Option<mc2_sim::mission::natives_presentation::Presentation> {
        Some(world.presentation.clone())
    }

    fn cursor_world_facts(&mut self, pos: mc2_sim::math::Vec3) -> Option<CursorWorldFacts> {
        let w = &mut self.sim.world;
        let in_play = w.terrain.is_game_select_position(&pos);
        let passable = in_play && w.terrain.passable_at(&pos);
        let line_of_sight = if in_play {
            let home = w.teams.home as i32;
            mc2_sim::object::hooks::team_line_of_sight(w, home, pos, 0.0)
        } else {
            false
        };
        Some(CursorWorldFacts {
            line_of_sight,
            passable,
        })
    }

    fn hud_messages(&mut self) -> Vec<crate::hud_data::HudMessage> {
        std::mem::take(&mut self.hud_messages)
    }

    fn results_screen_done(&mut self) {
        self.sim.results_screen_done();
    }

    fn mission_outcome(&mut self) -> Option<mc2_campaign::interface::Outcome> {
        Some(crate::outcome::to_campaign(
            mc2_sim::mission::outcome::build(&mut self.sim.world),
        ))
    }

    fn save_mission(&self) -> Option<Vec<u8>> {
        Some(mc2_sim::save::save_world(&self.sim.world))
    }

    fn load_mission(&mut self, game: &mut GameData, bytes: &[u8]) -> Result<(), String> {
        let base = mc2_sim::mission::init::load_world(&mut game.vfs, &self.spec)
            .map_err(|e| format!("mission re-initialisation: {e}"))?;
        // the base is prepared as `load` prepares one: the save's mech types and pilots come from that preparation
        let mut fresh = mc2_sim::Sim::new(base);
        std::mem::swap(&mut self.sim, &mut fresh);
        let launch = self.launch.take();
        self.prepare_base(game, launch.as_ref());
        self.launch = launch;
        let mut prepared = std::mem::replace(&mut self.sim, fresh);
        // the player's preference lives in the world and is not saved: the effects the load restarts need it
        prepared.world.objects.use_non_weapon_effects =
            self.sim.world.objects.use_non_weapon_effects;
        let world = mc2_sim::save::load_world(prepared.world, Some(&mut game.vfs), bytes)
            .map_err(|e| e.to_string())?;
        self.sim.world = world;
        // the late bindings (dropped support vehicles) belong to the replaced world: a restored handle may
        // now hold another unit, so they are dropped and `bind_new_movers` rebinds from the restored world
        self.movers.retain(|m| m.static_index.is_some());
        self.late_unbound.clear();
        self.bind_new_movers();
        // the interpolation baseline belongs to the replaced state; the bindings are by object handle
        self.previous.clear();
        self.snapshot_transforms();
        Ok(())
    }

    fn unavailable_summary(&self) -> Vec<String> {
        self.unavailable
            .iter()
            .map(|(k, n)| format!("{k} (x{n})"))
            .collect()
    }

    fn visibility_note(&self) -> String {
        visibility_trace_note(self.object_appearances_complete)
    }
}

#[cfg(test)]
mod tests {
    use super::{appearances_complete, terrain_object_recalculates, visibility_trace_note};

    #[test]
    fn terrain_object_warmup_uses_the_turn_being_stepped() {
        assert!(terrain_object_recalculates(2, false));
        assert!(!terrain_object_recalculates(3, false));
        assert!(!terrain_object_recalculates(4, false));
    }

    #[test]
    fn missing_binding_is_incomplete() {
        assert!(!appearances_complete(1, 0));
        assert!(visibility_trace_note(false).starts_with("object-visibility=incomplete"));
    }
}
