// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! A loaded mission as the viewer draws it: terrain, placed objects and mission parts uploaded to
//! the scene renderer, the camera start, and the static instances of every frame.

use crate::camera::{MissionCameraStart, MissionGround};
use mc2_render::game::GameData;
use mc2_render::math::Vec3;
use mc2_render::mission::{prefs_from_options, Mission, PaintSource};
use mc2_render::populate::{populate_with_home_paint, PopulateReport};
use mc2_render::scene::{SceneRenderer, ShapeInstance};
use mc2_render::terrain_tex::TerrainTextures;
use mc2_render::RenderError;
use mc2_sim::mission::init::{LoadSpec, LoadType};

pub struct MissionView {
    pub mission: Mission,
    /// Placed terrain objects and mission parts (static until the sim moves them in P2).
    pub instances: Vec<ShapeInstance>,
    pub report: PopulateReport,
    /// Renderer provenance for placed terrain objects, used by the live simulation to bind the
    /// exact appearance selected for the frame.
    pub terrain_instances: Vec<mc2_render::populate::TerrainInstance>,
    /// Indices into [`Self::instances`] of the mission parts (mechs and vehicles), in part order.
    pub part_instances: Vec<usize>,
    pub camera_start: MissionCameraStart,
    /// Where `GameCamera::activate` points the camera: the first player mover.  P1 takes the first
    /// `[PartN]` of team 0 (the player in single player); P2 takes it from the sim's mover list
    /// (first mover of the home commander that is on the GUI).
    pub first_player_mover: Option<Vec3>,
    /// The home commander's paint this view was built with: the campaign's when one was given,
    /// else `options.cfg`'s (`mission.cpp:1077-1081`).
    pub home_paint: PaintSource,
    /// Unplaced, home-painted instances of the support vehicles a VTOL drops after the load
    /// (`vehicleIDs[]`, `controlgui.cpp:96`), by object number: `LiveSim` binds a dropped mover to its copy.
    pub support_templates: Vec<(i32, mc2_render::scene::ShapeInstance)>,
    /// The airstrike bomber (`Artillery::bomber`, the "Shilone" appearance), `None` when the data has no such appearance.
    pub bomber: Option<std::rc::Rc<crate::bomber::BomberRig>>,
}

impl MissionView {
    /// Loads `data/missions/<name>.fit/.pak` and uploads it to `scene`.  The home commander's paint
    /// is `campaign` when a campaign launch supplies one (`ForceMech::colors`,
    /// `mc2-campaign/src/interface.rs`: base, then highlight), else `options.cfg`
    /// ([`prefs_from_options`]).  The `[PartN]` colours that are absent take `options.cfg`'s, never
    /// the campaign's.
    ///
    /// `launch` is the simulation's load spec; a logistics launch (`LoadType::SpLogistics`) replaces the
    /// mission file's commander-0 parts by the force the simulation spawns (see [`replace_home_parts`]),
    /// so every simulation mover has a view model.
    ///
    /// Fails as unavailable when a mech or vehicle has no paint source (see
    /// `PopulateReport::ensure_painted`): the launch is not drawn with raw masks.  A campaign launch
    /// that gives `Some(PaintSource::Unavailable)` claims campaign colours it does not have; it
    /// fails here, before any load.  The UI host's campaign launch supplies the colours; direct
    /// launches pass `None` and take `options.cfg`'s.
    pub fn load(
        game: &mut GameData,
        scene: &mut SceneRenderer,
        name: &str,
        campaign: Option<PaintSource>,
        launch: Option<&LoadSpec>,
    ) -> mc2_render::Result<Self> {
        if campaign == Some(PaintSource::Unavailable) {
            log::warn!("{name}: the campaign launch carries no colours for its home units");
            return Err(RenderError::Unavailable(format!(
                "paint unavailable for {name}: the campaign launch claims colours for its home units \
                 but supplies none (no campaign colour producer yet)"
            )));
        }
        let mut mission = Mission::load(game, name)?;
        if let Some(spec) = launch.filter(|s| s.load_type == LoadType::SpLogistics) {
            replace_home_parts(game, &mut mission, spec)?;
        }
        let textures = TerrainTextures::load(game, &mission)?;
        scene.clear_shapes();
        scene.set_terrain(&mission, &textures);
        let prefs = prefs_from_options(game);
        let home = campaign.unwrap_or(prefs);
        let populated = populate_with_home_paint(game, &mission, scene, 0, home, prefs)?;
        populated.report.ensure_painted()?;
        scene.set_terrain_lights(
            &mission,
            &populated.terrain_lights,
            mission.light.night_factor,
        );
        let mut support_templates = mc2_render::populate::populate_templates(
            game,
            scene,
            &crate::orders::SUPPORT_VEHICLE_IDS,
            home,
        )?;
        // The crew a destroyed vehicle bails out as (`createVehiclePilot`, gvehicl.cpp:2707): object 638, painted with its own
        // `resetPaintScheme(0x00ffffff, 0x00ffffff, 0x00ff8000)` (gvehicl.cpp:2803), not the home colours.
        support_templates.extend(mc2_render::populate::populate_templates(
            game,
            scene,
            &[mc2_sim::mission::spawn::VEHICLEPILOT_ID as i32],
            PaintSource::Scheme {
                base: 0x00ff_8000,
                highlight: 0x00ff_ffff,
            },
        )?);
        let bomber = match crate::bomber::BomberRig::load(game, scene) {
            Ok(b) => Some(std::rc::Rc::new(b)),
            Err(e) => {
                log::warn!("airstrike bomber unavailable: {e}");
                None
            }
        };
        let camera_start = MissionCameraStart::from_fit(&mut mission.fit);
        let first_player_mover = mission
            .parts
            .iter()
            .find(|p| p.team_id == 0 && p.exists)
            // `Mission::init` creates the mover at its raw `Part::position`; its Z is the
            // -1 sentinel until the first mover update snaps it to terrain (`mission.cpp` /
            // oracle init dump). GameCamera activation eases from that value.
            .map(|p| Vec3::new(p.x, p.y, -1.0));
        Ok(Self {
            instances: populated.instances,
            report: populated.report,
            terrain_instances: populated.terrain_instances,
            part_instances: populated.part_instances,
            camera_start,
            first_player_mover,
            home_paint: home,
            support_templates,
            bomber,
            mission,
        })
    }

    pub fn ground(&self) -> MissionGround<'_> {
        MissionGround {
            terrain: &self.mission.terrain,
            water: self.mission.water.elevation,
        }
    }
}

/// `Logistics::beginMission` (`logistics.cpp:733-776`): with a logistics launch the mission file's
/// commander-0 parts are not loaded (`commandersToLoad[0] = -1`, `logistics.cpp:556`); the player's
/// force is added around the first drop zone instead.  The view's home parts become that force, at
/// the positions the simulation gives its movers (`mc2_sim::mission::init::add_force`), so
/// `LiveSim::bind` finds a model for each.  The drop zone is read from the simulation's own load
/// plan (`prepare`), not re-derived.
fn replace_home_parts(
    game: &mut GameData,
    mission: &mut Mission,
    spec: &LoadSpec,
) -> mc2_render::Result<()> {
    let mut world = mc2_sim::World::new(spec.rng_mode, spec.seed);
    mc2_sim::mission::init::stuff_random_init(&mut world);
    mc2_sim::mission::init::prepare(&mut mc2_sim::mission::init::LoadCtx {
        world: &mut world,
        vfs: &mut game.vfs,
        spec,
    })
    .map_err(|e| {
        RenderError::Unavailable(format!("{}: load plan for the force: {e}", spec.mission))
    })?;
    let origin = world
        .mission
        .drop_zone_positions
        .first()
        .copied()
        .ok_or_else(|| {
            RenderError::Unavailable(format!("{}: no drop zone for the force", spec.mission))
        })?;
    // `plan_parts` (mission.cpp:2745-2780) skips the file's commander-0 parts under a logistics
    // launch except the destroyed ones (`Damage >= 1.0`), which are still created: keep exactly those
    mission
        .parts
        .retain(|p| p.commander_id != Some(0) || p.damage >= 1.0);
    let first_free = mission.parts.iter().map(|p| p.index + 1).max().unwrap_or(0);
    // pilotless mechs are not loaded and do not turn the ring (`logistics.cpp:759`)
    let crew: Vec<_> = spec
        .force
        .iter()
        .filter(|m| m.pilot_file.is_some())
        .collect();
    let step = 360.0 / spec.force.len().max(1) as f32;
    let mut vector = mc2_sim::math::Vec3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    let mut home = Vec::new();
    for (n, m) in crew.into_iter().enumerate() {
        home.push(mc2_render::mission::Part {
            index: first_free + n,
            object_number: m.object_number,
            x: origin.x + vector.x * 128.0,
            y: origin.y + vector.y * 128.0,
            rotation: 0.0,
            team_id: 0,
            commander_id: Some(0),
            active: true,
            exists: true,
            damage: 0.0,
            csv_file: Some(m.chassis_file.clone()),
            base_color: Some(m.colors[0]),
            highlight_color1: Some(m.colors[1]),
            highlight_color2: Some(m.colors[2]),
        });
        mc2_sim::mission::init::rotate_xy(&mut vector, step);
    }
    // the force leads the part list, as the first player part does for `first_player_mover`
    home.append(&mut mission.parts);
    mission.parts = home;
    Ok(())
}

/// Mission names in the data: every `data/missions/<name>.fit` that has a `<name>.pak`, from the
/// fast files and the loose `data/missions` folder, sorted.
pub fn list_missions(game: &GameData) -> Vec<String> {
    use std::collections::BTreeSet;
    let mut fits = BTreeSet::new();
    let mut paks = BTreeSet::new();
    let mut note = |name: &str| {
        let n = mc2_formats::vfs::normalize_name(name);
        if let Some(rest) = n.strip_prefix("data/missions/") {
            if rest.contains('/') {
                return;
            }
            if let Some(m) = rest.strip_suffix(".fit") {
                fits.insert(m.to_string());
            } else if let Some(m) = rest.strip_suffix(".pak") {
                paks.insert(m.to_string());
            }
        }
    };
    for path in game.vfs.archive_paths() {
        if let Ok(ff) = mc2_formats::fst::FastFile::open(path) {
            for e in ff.entries() {
                note(&e.name);
            }
        }
    }
    if let Ok(dir) = std::fs::read_dir(game.vfs.root().join("data").join("missions")) {
        for e in dir.flatten() {
            if let Some(n) = e.file_name().to_str() {
                note(&format!("data/missions/{n}"));
            }
        }
    }
    fits.intersection(&paks).cloned().collect()
}
