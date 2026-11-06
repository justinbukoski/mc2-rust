// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Turns a mission's placed objects and parts into shape instances.
//!
//! Terrain objects (packet 1 of the mission pak) become buildings, trees and props; the fit's
//! `[PartN]` blocks become the mechs and vehicles.  Both resolve through `object2.pak` to an
//! appearance (`data/tgl/<name>.ini`) and from there to a `.tgl` shape
//! (`ObjectTypeManager::load`, `AppearanceType::init` in the C++).  Every appearance keeps its LODs
//! and damage shapes; [`select_shapes`] picks the one to draw from the camera each frame.

use crate::appearance::{state_pose, AppearanceKind, AppearanceState, ShapeChoice, ShapeSet};
use crate::camera::GameCamera;
use crate::error::optional_file;
use crate::error::RenderError;
use crate::game::GameData;
use crate::math::stuff_to_world;
use crate::math::Vec3;
use crate::mission::{prefs_from_options, Mission, PaintGap, PaintSource};
use crate::objects::{Appearance, ObjectClass, ObjectTypes, TerrainLightSpec};
use crate::scene::{ModelId, SceneRenderer, ShapeInstance, ShapeStyle};
use crate::shape::{AnimationClip, PaintRoutine, ShapeModel};
use crate::terrain_lights::TerrainLight;
use std::collections::{HashMap, HashSet};

/// A mech or vehicle drawn with its raw masks, because its paint is unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpainted {
    /// `Part::index` of the `[PartN]` block.
    pub part: usize,
    pub object_number: i32,
    pub class: ObjectClass,
    pub gap: PaintGap,
}

#[derive(Debug, Default, Clone)]
pub struct PopulateReport {
    pub terrain_objects: usize,
    pub placed: usize,
    pub trees: usize,
    pub buildings: usize,
    pub parts_placed: usize,
    pub skipped_no_type: usize,
    pub skipped_no_appearance: usize,
    pub skipped_no_shape: usize,
    /// One entry per mech or vehicle drawn with its raw masks, with the missing input (see
    /// [`PaintGap`]).  Empty when every mech and vehicle took its paint.
    pub unpainted: Vec<Unpainted>,
    pub unique_models: usize,
    pub vertex_count: usize,
    /// Declared files that are not in the data, each named once: an appearance's `data/tgl/<name>.ini`, or a shape
    /// of an appearance, which is then left out.  A file that is there but fails to load is an error of
    /// [`populate`], not an entry here.
    pub missing_files: Vec<String>,
}

pub struct Populated {
    pub instances: Vec<ShapeInstance>,
    pub report: PopulateReport,
    /// Source identity of each placed terrain-object instance.  Consumers that need the same
    /// appearance bounds as the renderer can bind a live object without guessing from a shared
    /// position alone.
    pub terrain_instances: Vec<TerrainInstance>,
    /// Indices into `instances` of mission parts (mechs / vehicles), in part order.
    pub part_instances: Vec<usize>,
    /// Night light pools of the placed buildings (`TerrainLightRGB` appearances); pass them to
    /// `SceneRenderer::set_terrain_lights` with the mission's night factor.
    pub terrain_lights: Vec<TerrainLight>,
}

/// Read-only provenance for a terrain-object [`ShapeInstance`].
#[derive(Clone, Copy, Debug)]
pub struct TerrainInstance {
    /// Index into [`Populated::instances`].
    pub index: usize,
    /// Mission/object-table number used to resolve the appearance.
    pub object_type: i32,
    /// Placed world position before any appearance animation.
    pub position: Vec3,
}

impl PopulateReport {
    /// `Ok` when every mech and vehicle took its paint.  Otherwise an [`RenderError::Unavailable`]
    /// that names each unit drawn with raw masks and the input it lacks: a production render path
    /// must not return a picture of those units as if it were painted.
    pub fn ensure_painted(&self) -> crate::Result<()> {
        if self.unpainted.is_empty() {
            return Ok(());
        }
        let units: Vec<String> = self
            .unpainted
            .iter()
            .map(|u| {
                format!(
                    "part {} (object {}, {:?}) needs {}",
                    u.part,
                    u.object_number,
                    u.class,
                    u.gap.missing_input()
                )
            })
            .collect();
        Err(RenderError::Unavailable(format!(
            "paint unavailable for {} mech(s) or vehicle(s): {}",
            self.unpainted.len(),
            units.join("; ")
        )))
    }
}

/// Advances the shape of every instance for the camera's frame, as the appearances' `recalcBounds` does each
/// frame (LOD and damage, see [`crate::appearance`]). An instance's LOD only changes while it is on screen: first
/// the class-specific C++ cone test, then the projected selection box of its drawn model. Each instance then takes its
/// pose from its state ([`state_pose`]): recomputed when its model changed or a gesture is bound, so the pose is a
/// function of the instance's state and model only.  Call it once per frame before drawing;
/// `high_object_detail` is the `HighObjectDetail` preference (`prefs.cpp`, default on).
pub fn select_shapes(
    scene: &SceneRenderer,
    instances: &mut [ShapeInstance],
    camera: &GameCamera,
    high_object_detail: bool,
) {
    for inst in instances {
        // Building::handleDestruction applies its status before `recalcBounds`, so its damaged
        // model supplies the transition frame's extent. Trees retain their existing ordering;
        // vehicles and mechs deliberately do too (their status handling is separately observed).
        if inst.shapes.kind == AppearanceKind::Building {
            inst.apply_status();
        }
        // The root-height term belongs to the current animation pose.  C++ computes it before
        // projecting the mech box, while the previous frame's model is still selected
        // (`mech3d.cpp:2153-2203`), so establish that same state before the visibility gates.
        update_pose(scene, inst);
        let model = scene.model(inst.model);
        // `OBBRadius` and the type box are fixed from the base (intact) shape when the building
        // is initialized and never change with damage or LOD (`bdactor.cpp:683-741`); only the
        // cone test reads the currently selected shape's `GetExtentRadius` (`bdactor.cpp:1103`).
        let rough_model = match inst.shapes.kind {
            AppearanceKind::Building => inst.shapes.lods[0].map_or(model, |base| scene.model(base)),
            _ => model,
        };
        let in_view = match inst.shapes.kind {
            AppearanceKind::Building => {
                camera.structure_sphere_in_view(inst.position, model.cone_radius())
                    && camera.rough_screen_bounds_in_view(
                        inst.position,
                        rough_screen_bounds(inst, rough_model, camera),
                        rough_screen_lower_right(inst, rough_model, camera),
                    )
                    && camera.selection_box_in_view(inst.position, selection_box_world(scene, inst))
            }
            // `TreeAppearance::recalcBounds` goes directly from its cone/far gate to the type
            // box (`bdactor.cpp:3595-3793`).  Unlike `BldgAppearance`, it has no preliminary
            // OBB-radius screen rectangle, so applying the building gate here can incorrectly
            // cull a tree whose final selection box is visible.
            AppearanceKind::Tree => {
                camera.structure_sphere_in_view(inst.position, model.cone_radius())
                    && camera.selection_box_in_view(inst.position, selection_box_world(scene, inst))
            }
            AppearanceKind::Vehicle | AppearanceKind::Mech => {
                camera.mover_centre_in_view(inst.position)
                    && camera.rough_screen_bounds_in_view(
                        inst.position,
                        rough_screen_bounds(inst, model, camera),
                        rough_screen_lower_right(inst, model, camera),
                    )
                    && camera.selection_box_in_view(inst.position, selection_box_world(scene, inst))
            }
        };
        inst.select_shape(camera.position_world(), in_view, high_object_detail);
        update_pose(scene, inst);
    }
}

/// Pixel bounds of the same final projected selection box that [`select_shapes`] uses for an
/// appearance's `inView` decision.  Replay tests use this only to prove an accepted oracle
/// classification disagreement is confined to the viewport boundary.
pub fn selection_box_screen_bounds(
    scene: &SceneRenderer,
    inst: &mut ShapeInstance,
    camera: &GameCamera,
) -> [f32; 4] {
    // Match `select_shapes`: the final type box is based on the pose that exists before its
    // visibility/LOD decision, not the replacement shape chosen after that decision.
    update_pose(scene, inst);
    let mut left = f32::INFINITY;
    let mut top = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for corner in selection_box_world(scene, inst) {
        let (screen, _) = camera.project_z(corner);
        left = left.min(screen[0]);
        top = top.min(screen[1]);
        right = right.max(screen[0]);
        bottom = bottom.max(screen[1]);
    }
    [left, top, right, bottom]
}

/// The screen triangles `PerPolySelect` tests (`mclib/tgl.cpp:2763`): every front-facing triangle of the shape
/// drawn now (`listOfVisibleFaces`, the faces the GPU keeps with `FrontFace::Ccw` and back-face culling), its
/// corners projected to viewport pixels like [`selection_box_screen_bounds`].  Triangles with a corner behind
/// the eye are left out.
pub fn visible_screen_triangles(
    scene: &SceneRenderer,
    inst: &mut ShapeInstance,
    camera: &GameCamera,
) -> Vec<[f32; 6]> {
    update_pose(scene, inst);
    let model = scene.model(inst.model);
    let mats = crate::shape::instance_node_matrices(&inst.pose, inst.position, inst.yaw_degrees);
    let view_proj = camera.gpu_view_proj();
    let (vw, vh) = camera.viewport;
    let mut out = Vec::new();
    for batch in &model.batches {
        let first = batch.first_vertex as usize;
        let verts = &model.vertices[first..first + batch.vertex_count as usize];
        for tri in verts.chunks_exact(3) {
            let mut ndc = [[0.0f32; 2]; 3];
            let mut in_front = true;
            for (corner, v) in ndc.iter_mut().zip(tri) {
                let Some(m) = mats.get(v.node as usize) else {
                    in_front = false;
                    break;
                };
                let world =
                    m.transform_point(Vec3::new(v.position[0], v.position[1], v.position[2]));
                let c = view_proj.transform4([world.x, world.y, world.z, 1.0]);
                if c[3] <= 0.0 {
                    in_front = false;
                    break;
                }
                *corner = [c[0] / c[3], c[1] / c[3]];
            }
            if !in_front {
                continue;
            }
            let area = (ndc[1][0] - ndc[0][0]) * (ndc[2][1] - ndc[0][1])
                - (ndc[2][0] - ndc[0][0]) * (ndc[1][1] - ndc[0][1]);
            if area <= 0.0 {
                continue;
            }
            let px = |p: [f32; 2]| [(p[0] + 1.0) * 0.5 * vw, (1.0 - p[1]) * 0.5 * vh];
            let (a, b, c) = (px(ndc[0]), px(ndc[1]), px(ndc[2]));
            out.push([a[0], a[1], b[0], b[1], c[0], c[1]]);
        }
    }
    out
}

/// The C++ first-stage rectangle is intentionally not the projected type bounds.  It is based
/// on the appearance OBB radius and the controller's scale/tilt values, and can reject a large
/// type box which would otherwise overlap the screen (`bdactor.cpp:1163-1182`,
/// `gvactor.cpp:1621-1641`, `mech3d.cpp:2125-2145`).
fn rough_screen_bounds(
    inst: &ShapeInstance,
    model: &ShapeModel,
    _camera: &GameCamera,
) -> (f32, f32) {
    let r = match inst.shapes.kind {
        AppearanceKind::Building => model.building_obb_radius(),
        _ => model.extent_radius(),
    };
    match inst.shapes.kind {
        AppearanceKind::Mech => (-r, -2.0 * r),
        AppearanceKind::Building | AppearanceKind::Tree | AppearanceKind::Vehicle => {
            (-2.0 * r, -2.0 * r)
        }
    }
}

fn rough_screen_lower_right(
    inst: &ShapeInstance,
    model: &ShapeModel,
    camera: &GameCamera,
) -> (f32, f32) {
    let r = match inst.shapes.kind {
        AppearanceKind::Building => model.building_obb_radius(),
        _ => model.extent_radius(),
    };
    match inst.shapes.kind {
        AppearanceKind::Mech => (r, r),
        AppearanceKind::Building | AppearanceKind::Tree | AppearanceKind::Vehicle => {
            // `Camera::getTiltFactor` (camera.h:636-645), used by bdactor/gvactor just before
            // this gate.  The renderer's projection angle is the C++ `projectionAngle` here.
            let tilt = 0.5 + (camera.projection_angle - 28.0) * (1.0 / 60.0) * 0.5;
            (2.0 * r, r * tilt * 2.0)
        }
    }
}

/// The projected type box follows each C++ appearance class, rather than the draw transform.
/// Buildings, trees and vehicles rotate their Stuff-frame boxes; mechs project an unrotated,
/// world-axis box (`bdactor.cpp:1187-1261`, `gvactor.cpp:1647-1692`,
/// `mech3d.cpp:2173-2203`).
fn selection_box_world(scene: &SceneRenderer, inst: &mut ShapeInstance) -> [Vec3; 8] {
    let model = scene.model(inst.model);
    if inst.shapes.kind == AppearanceKind::Mech {
        let root_height_difference = mech_root_height_difference(inst, model);
        inst.root_height_difference = root_height_difference;
        return match inst.shapes.selection_bounds {
            Some(bounds) => ShapeModel::mech_designer_selection_box_world_at_height(
                bounds,
                inst.position,
                root_height_difference,
            ),
            None => model.mech_selection_box_world_at_height(inst.position, root_height_difference),
        };
    }
    if inst.shapes.kind == AppearanceKind::Vehicle {
        // GVAppearance rotates its two stored type endpoints and combines them for *every*
        // vehicle.  Mesh-derived bounds are expanded at init; authored `[3DBounds]` are already
        // type bounds and stay verbatim (`gvactor.cpp:968-980`, `1646-1692`).
        let bounds = inst
            .shapes
            .selection_bounds
            .unwrap_or_else(|| ShapeModel::expanded_vehicle_bounds(model.bounds));
        return model.selection_box_world_from_rotated_endpoints(
            bounds,
            inst.position,
            inst.yaw_degrees,
        );
    }
    if inst.shapes.kind == AppearanceKind::Tree {
        return model.selection_box_world_from_rotated_endpoints(
            inst.shapes.selection_bounds.unwrap_or(model.bounds),
            inst.position,
            inst.yaw_degrees,
        );
    }
    // The building type box is the one fixed at init from the base shape (or the authored
    // `[3DBounds]`); a damaged or LOD shape never replaces it (`bdactor.cpp:736-741`, `1187-1190`).
    let bounds = inst.shapes.building_type_bounds.unwrap_or_else(|| {
        inst.shapes.lods[0].map_or(model.building_bounds, |base| {
            scene.model(base).building_bounds
        })
    });
    model.selection_box_world_with_bounds(bounds, inst.position, inst.yaw_degrees)
}

/// The `baseRootNodeDifference` input to the mech selection box.  The current root comes from
/// the pure state/frame pose; its baseline comes from lane F's first update rather than the bind
/// pose (`mech3d.cpp:2153`, `4160`).  The previous `in_view` bit is the C++ `wasInView` latch:
/// out-of-view frames retain the last difference unless the jump gesture refreshes it.
fn mech_root_height_difference(inst: &ShapeInstance, model: &ShapeModel) -> f32 {
    let previous = inst.root_height_difference;
    if !inst.in_view && inst.state.gesture != Some(20) {
        return previous;
    }
    let Some(index) = model.node_index("joint_root") else {
        return previous;
    };
    let Some(current) = inst.pose.get(index) else {
        return previous;
    };
    let current_z = stuff_to_world()
        .transform_vector(current.transform_point(Vec3::ZERO))
        .z;
    let base_z = inst.state.base_root_node_height.unwrap_or_else(|| {
        let bind = model.bind_pose();
        bind.get(index)
            .map(|base| {
                stuff_to_world()
                    .transform_vector(base.transform_point(Vec3::ZERO))
                    .z
            })
            .unwrap_or(0.0)
    });
    let mut difference = current_z - base_z;
    if inst.in_view && inst.state.is_helicopter {
        difference -= 25.0;
    }
    difference
}

/// As [`select_shapes`], for an eye at `eye` (game world coordinates) and a caller-provided
/// `in_view(centre, radius)` test.  This remains the narrow visibility-hook API for tests and
/// other callers; the renderer itself uses [`select_shapes_from_instance`] so it can project the
/// C++ type selection box rather than a sphere.
pub fn select_shapes_from(
    scene: &SceneRenderer,
    instances: &mut [ShapeInstance],
    eye: Vec3,
    in_view: impl Fn(Vec3, f32) -> bool,
    high_object_detail: bool,
) {
    select_shapes_from_instance(
        scene,
        instances,
        |inst| in_view(inst.position, scene.model(inst.model).cone_radius()),
        eye,
        high_object_detail,
    );
}

fn select_shapes_from_instance(
    scene: &SceneRenderer,
    instances: &mut [ShapeInstance],
    in_view: impl Fn(&ShapeInstance) -> bool,
    eye: Vec3,
    high_object_detail: bool,
) {
    for inst in instances {
        let in_view = in_view(inst);
        inst.select_shape(eye, in_view, high_object_detail);
        update_pose(scene, inst);
    }
}

fn update_pose(scene: &SceneRenderer, inst: &mut ShapeInstance) {
    let key = (inst.model, inst.state);
    if inst.pose_key != Some(key) {
        let model = scene.model(inst.model);
        let node = inst
            .turret
            .as_ref()
            .and_then(|(id, q)| model.node_index(id).map(|i| (i, *q)));
        inst.pose = match node {
            Some((i, q)) => {
                // the same clip and frame `state_pose` would sample, plus the turret rotation
                let clip = inst
                    .state
                    .gesture
                    .and_then(|g| model.gestures.get(g))
                    .and_then(Option::as_ref);
                let frame = if clip.is_some() {
                    inst.state.frame.round_ties_even().max(0.0)
                } else {
                    0.0
                };
                model.pose(clip, frame, &HashMap::from([(i, q)]))
            }
            None => state_pose(model, &inst.state),
        };
        inst.pose_key = Some(key);
    }
}

/// Builds the instances for every placed terrain object and mission part.  The paint comes from
/// `options.cfg` ([`prefs_from_options`]): the home commander's unit and any `[PartN]` colour that
/// is absent.  A mech or vehicle without a paint source keeps its raw masks and is listed in
/// `PopulateReport::unpainted`.  Use [`populate_with_home_paint`] to supply another home source,
/// such as a campaign's colours.
///
/// `max_objects` limits the terrain objects (0 = all).  Each instance starts on its full detail
/// shape and bind pose; call [`select_shapes`] before drawing to apply the LOD and damage rules.
/// An error is a file that is in the data and fails to load; a file that is not in the data is left out
/// and named in [`PopulateReport::missing_files`].
pub fn populate(
    game: &mut GameData,
    mission: &Mission,
    scene: &mut SceneRenderer,
    max_objects: usize,
) -> crate::Result<Populated> {
    let prefs = prefs_from_options(game);
    populate_inner(game, mission, scene, max_objects, &[], prefs, prefs)
}

/// As [`populate`], and every mech also loads the animation of each gesture.  `gesture_suffixes[g]` is the
/// file suffix of gesture `g`: the animation of gesture `g` of appearance `name` is `<name><suffix>.agl`
/// (`MechAnimationNames`, `mech3d.cpp:142`, the table `mc2_sim::appearance::tables::MECH_ANIMATION_NAMES`).
/// Without suffixes the mechs keep the bind pose.
pub fn populate_with_animations(
    game: &mut GameData,
    mission: &Mission,
    scene: &mut SceneRenderer,
    max_objects: usize,
    gesture_suffixes: &[&str],
) -> crate::Result<Populated> {
    let prefs = prefs_from_options(game);
    populate_inner(
        game,
        mission,
        scene,
        max_objects,
        gesture_suffixes,
        prefs,
        prefs,
    )
}

/// [`populate`] with the home commander's paint `home` (`mission.cpp:1077-1081`, see
/// [`PaintSource`]) and the prefs colours `prefs` that stand in for absent `[PartN]` colours
/// (`mission.cpp:2644-2657`).  Every other unit keeps its own `[PartN]` colours.
pub fn populate_with_home_paint(
    game: &mut GameData,
    mission: &Mission,
    scene: &mut SceneRenderer,
    max_objects: usize,
    home: PaintSource,
    prefs: PaintSource,
) -> crate::Result<Populated> {
    populate_inner(game, mission, scene, max_objects, &[], home, prefs)
}

fn populate_inner(
    game: &mut GameData,
    mission: &Mission,
    scene: &mut SceneRenderer,
    max_objects: usize,
    gesture_suffixes: &[&str],
    home: PaintSource,
    prefs: PaintSource,
) -> crate::Result<Populated> {
    let mut resolver = Resolver {
        types: ObjectTypes::open(game)?,
        appearances: HashMap::new(),
        shapes: ShapeLoader {
            missing: HashSet::new(),
            gesture_suffixes: gesture_suffixes.iter().map(|s| s.to_string()).collect(),
            animated: HashSet::new(),
        },
    };
    let mut report = PopulateReport {
        terrain_objects: mission.objects.len(),
        ..Default::default()
    };
    let mut instances = Vec::new();
    let mut terrain_instances = Vec::new();
    let mut part_instances = Vec::new();
    let mut terrain_lights = Vec::new();
    let light = &mission.light;
    let sun = light.terrain_light_direction();

    for (n, obj) in mission.objects.iter().enumerate() {
        if max_objects != 0 && n >= max_objects {
            break;
        }
        let Some(res) = resolver.resolve(game, scene, obj.object_type, None, &mut report)? else {
            continue;
        };
        let (x, y) = (obj.position[0], obj.position[1]);
        let z = mission.terrain.elevation_at(x, y);
        let intensity = mission.terrain.light_intensity_at(x, y, sun);
        let ramp = light.ramp(intensity);
        let tree = matches!(res.class, ObjectClass::Tree | ObjectClass::TreeBuilding);
        let position = Vec3::new(x, y, z);
        terrain_instances.push(TerrainInstance {
            index: instances.len(),
            object_type: obj.object_type,
            position,
        });
        instances.push(new_instance(
            scene,
            &res,
            position,
            obj.rotation,
            ramp,
            tree.then_some(ramp),
        ));
        // `BldgAppearance::update`: a ground light while it is night-ish (`nightFactor > 0`).
        if let Some(spec) = res.terrain_light {
            terrain_lights.push(TerrainLight {
                position: Vec3::new(x, y, z),
                rgb: spec.rgb,
                intensity: spec.intensity,
                inner_radius: spec.inner_radius,
                outer_radius: spec.outer_radius,
            });
        }
        report.placed += 1;
        if tree {
            report.trees += 1;
        } else {
            report.buildings += 1;
        }
    }

    for part in &mission.parts {
        if !part.exists {
            continue;
        }
        let paint = part.paint(home, prefs);
        let Some(res) =
            resolver.resolve(game, scene, part.object_number, paint.ok(), &mut report)?
        else {
            continue;
        };
        if let Err(gap) = paint {
            if matches!(res.class, ObjectClass::BattleMech | ObjectClass::Vehicle) {
                report.unpainted.push(Unpainted {
                    part: part.index,
                    object_number: part.object_number,
                    class: res.class,
                    gap,
                });
            }
        }
        let z = mission.terrain.elevation_at(part.x, part.y);
        let intensity = mission.terrain.light_intensity_at(part.x, part.y, sun);
        let ramp = light.ramp(intensity);
        part_instances.push(instances.len());
        instances.push(new_instance(
            scene,
            &res,
            Vec3::new(part.x, part.y, z),
            part.rotation,
            ramp,
            None,
        ));
        report.parts_placed += 1;
    }
    report.unique_models = {
        let mut ids: Vec<usize> = instances.iter().map(|i| i.model.0).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    };
    report.vertex_count = instances
        .iter()
        .map(|i| scene.model(i.model).vertices.len())
        .sum();
    Ok(Populated {
        instances,
        report,
        terrain_instances,
        part_instances,
        terrain_lights,
    })
}

/// One unplaced instance per object type in `object_numbers`, painted for the home commander
/// (`PaintSource::Scheme`), for objects the simulation creates after the mission loads (the support
/// vehicles a VTOL drops).  A type with no shape or no home paint is left out.
pub fn populate_templates(
    game: &mut GameData,
    scene: &mut SceneRenderer,
    object_numbers: &[i32],
    home: PaintSource,
) -> crate::Result<Vec<(i32, ShapeInstance)>> {
    let PaintSource::Scheme { base, highlight } = home else {
        return Ok(Vec::new());
    };
    let mut resolver = Resolver {
        types: ObjectTypes::open(game)?,
        appearances: HashMap::new(),
        shapes: ShapeLoader {
            missing: HashSet::new(),
            gesture_suffixes: Vec::new(),
            animated: HashSet::new(),
        },
    };
    let mut report = PopulateReport::default();
    let mut out = Vec::new();
    for &n in object_numbers {
        if let Some(res) = resolver.resolve(
            game,
            scene,
            n,
            Some((highlight, highlight, base)),
            &mut report,
        )? {
            out.push((
                n,
                new_instance(scene, &res, Vec3::new(0.0, 0.0, 0.0), 0.0, [1.0; 3], None),
            ));
        }
    }
    Ok(out)
}

fn new_instance(
    scene: &SceneRenderer,
    res: &Resolved,
    position: Vec3,
    yaw_degrees: f32,
    infinite_light: [f32; 3],
    ambient_override: Option<[f32; 3]>,
) -> ShapeInstance {
    ShapeInstance {
        model: res.base,
        shapes: res.shapes,
        state: AppearanceState::default(),
        choice: ShapeChoice::default(),
        pose: scene.model(res.base).bind_pose(),
        pose_key: None,
        turret: None,
        position,
        yaw_degrees,
        infinite_light,
        ambient_override,
        casts_shadow: true,
        in_view: true,
        root_height_difference: 0.0,
        highlight: 0,
    }
}

/// An object type resolved to its shapes.
struct Resolved {
    /// `shapes.lods[0]`: the full detail model.
    base: ModelId,
    shapes: ShapeSet,
    class: ObjectClass,
    terrain_light: Option<TerrainLightSpec>,
}

/// Resolves object type numbers to shape sets, loading each appearance and shape once.
struct Resolver {
    types: ObjectTypes,
    /// Appearances by lowercase name; `None` where the `.ini` is not in the data.
    appearances: HashMap<String, Option<Appearance>>,
    shapes: ShapeLoader,
}

impl Resolver {
    fn resolve(
        &mut self,
        game: &mut GameData,
        scene: &mut SceneRenderer,
        type_num: i32,
        paint: Option<(u32, u32, u32)>,
        report: &mut PopulateReport,
    ) -> crate::Result<Option<Resolved>> {
        let Some(info) = self.types.get(type_num).cloned() else {
            report.skipped_no_type += 1;
            return Ok(None);
        };
        let key = info.appearance.to_ascii_lowercase();
        if !self.appearances.contains_key(&key) {
            let app = Appearance::load(game, &info.appearance)?;
            if app.is_none() {
                report
                    .missing_files
                    .push(format!("data/tgl/{}.ini", info.appearance));
            }
            self.appearances.insert(key.clone(), app);
        }
        let Some(app) = self.appearances[&key].clone() else {
            report.skipped_no_appearance += 1;
            return Ok(None);
        };
        self.shapes
            .resolve_appearance(game, scene, info.class, &app, paint, report)
    }
}

/// The building type box `BldgAppearance::init` fixes once (`bdactor.cpp:736-741`): the authored `[3DBounds]`
/// endpoints verbatim (`getDesignerTypeBounds`), otherwise `GetMinBox`/`GetMaxBox` plus the root node centre of the
/// base (intact) shape.  A damaged or LOD shape never replaces it.
pub fn building_type_bounds(authored: Option<(Vec3, Vec3)>, base: &ShapeModel) -> (Vec3, Vec3) {
    authored.unwrap_or(base.building_bounds)
}

/// A building instance of the appearance `data/tgl/<name>.ini` at `position`, made by the same resolver and
/// instance constructor `populate` uses (so the type box is the one production assigns).  `None` where the
/// appearance or its base shape is not in the data.
pub fn building_instance(
    game: &mut GameData,
    scene: &mut SceneRenderer,
    name: &str,
    position: Vec3,
) -> crate::Result<Option<ShapeInstance>> {
    let Some(app) = Appearance::load(game, name)? else {
        return Ok(None);
    };
    let mut loader = ShapeLoader {
        missing: HashSet::new(),
        gesture_suffixes: Vec::new(),
        animated: HashSet::new(),
    };
    let res = loader.resolve_appearance(
        game,
        scene,
        ObjectClass::Building,
        &app,
        None,
        &mut PopulateReport::default(),
    )?;
    Ok(res.map(|res| new_instance(scene, &res, position, 0.0, [1.0; 3], None)))
}

/// Loads the shapes of appearances, each file once.
struct ShapeLoader {
    /// Lowercase names of the files that are not in the data, so each is looked up and reported once.
    missing: HashSet<String>,
    /// File suffix per gesture (see [`populate_with_animations`]).
    gesture_suffixes: Vec<String>,
    /// Mech models whose gesture animations are loaded.
    animated: HashSet<ModelId>,
}

impl ShapeLoader {
    /// The shapes of `app`, an appearance of an object of `class`; `None` when it has no base shape in the data.
    fn resolve_appearance(
        &mut self,
        game: &mut GameData,
        scene: &mut SceneRenderer,
        class: ObjectClass,
        app: &Appearance,
        paint: Option<(u32, u32, u32)>,
        report: &mut PopulateReport,
    ) -> crate::Result<Option<Resolved>> {
        let paintable = matches!(class, ObjectClass::BattleMech | ObjectClass::Vehicle);
        let style = ShapeStyle {
            glow: app.glow,
            paint: if paintable { paint } else { None },
            paint_routine: if class == ObjectClass::Vehicle {
                PaintRoutine::Vehicle
            } else {
                PaintRoutine::Mech
            },
        };
        let mech = class == ObjectClass::BattleMech;
        // Animated appearances own their model clips, even when they share a mesh.
        let owner = ((mech && !self.gesture_suffixes.is_empty()) || class == ObjectClass::Vehicle)
            .then_some(app.name.as_str());
        let kind = match class {
            ObjectClass::BattleMech => AppearanceKind::Mech,
            ObjectClass::Vehicle => AppearanceKind::Vehicle,
            // `TreeAppearance` for the trees (populate groups them as `resolve` does).
            ObjectClass::Tree | ObjectClass::TreeBuilding => AppearanceKind::Tree,
            _ => AppearanceKind::Building,
        };
        let mut shapes = ShapeSet {
            kind,
            selection_bounds: app.selection_bounds,
            ..Default::default()
        };
        // A LOD whose file is not in the data is left out (the C++ would stop the game there).
        for (i, lod) in app.lods.iter().enumerate() {
            let Some(lod) = lod else { continue };
            shapes.lods[i] = self.load_shape(game, scene, &lod.file, style, owner, report)?;
            shapes.lod_distance[i] = lod.distance;
        }
        let Some(base) = shapes.lods[0] else {
            report.skipped_no_shape += 1;
            return Ok(None);
        };
        if kind == AppearanceKind::Building {
            // `BldgAppearanceType::init` stores authored `[3DBounds]` verbatim, or
            // derives the type box from the base shape before any damage transition.
            shapes.building_type_bounds = Some(building_type_bounds(
                app.selection_bounds,
                scene.model(base),
            ));
        }
        if let Some(file) = &app.damaged {
            shapes.damaged = self.load_shape(game, scene, file, style, None, report)?;
        }
        if mech {
            // `mech3d.cpp:327-331`: both fall damage shapes are read from the FallForwardDam file.  Optional: a
            // mech without it simply keeps its LOD shape.
            let file = format!("{}FallForwardDam", app.name);
            let dmg = optional_file(scene.load_shape(game, &file, style))?;
            shapes.fall_damage = [dmg, dmg];
            // Each LOD has its own node order, so each gets its own gesture clips.
            for lod in shapes.lods.iter().flatten() {
                if !self.gesture_suffixes.is_empty() && self.animated.insert(*lod) {
                    let clips =
                        gesture_clips(game, scene.model(*lod), &app.name, &self.gesture_suffixes)?;
                    scene.set_gesture_clips(*lod, clips);
                }
            }
        }
        Ok(Some(Resolved {
            base,
            shapes,
            class,
            terrain_light: app.terrain_light.clone(),
        }))
    }

    /// Loads one shape of an appearance.  A file that is not in the data is reported once and left out; a file
    /// that is there and fails to load is an error.  `owner` gives the model its own gesture clips.
    fn load_shape(
        &mut self,
        game: &mut GameData,
        scene: &mut SceneRenderer,
        file: &str,
        style: ShapeStyle,
        owner: Option<&str>,
        report: &mut PopulateReport,
    ) -> crate::Result<Option<ModelId>> {
        let key = file.to_ascii_lowercase();
        if self.missing.contains(&key) {
            return Ok(None);
        }
        let loaded = match owner {
            Some(owner) => scene.load_shape_owned(game, file, style, owner),
            None => scene.load_shape(game, file, style),
        };
        match optional_file(loaded)? {
            Some(id) => Ok(Some(id)),
            None => {
                self.missing.insert(key);
                report.missing_files.push(file.to_string());
                Ok(None)
            }
        }
    }
}

/// The animation of every gesture of a mech appearance: `<appearance><suffix>.agl` for each suffix, or
/// `None` where the file is not in the data (`Mech3DAppearanceType::init`, `mech3d.cpp:394-416`).  A file that
/// is there and fails to parse is an error.
fn gesture_clips(
    game: &mut GameData,
    model: &ShapeModel,
    appearance: &str,
    suffixes: &[String],
) -> crate::Result<Vec<Option<AnimationClip>>> {
    suffixes
        .iter()
        .map(|suffix| {
            let name = format!("{appearance}{suffix}").to_ascii_lowercase();
            optional_file(AnimationClip::load(game, model, &name))
        })
        .collect()
}

/// Loads `data/tgl/<shape><suffix>.agl` for a model of the scene (e.g. `werewolf` + `walk`).  `None` when the
/// file is not in the data.
pub fn load_clip(
    game: &mut GameData,
    scene: &SceneRenderer,
    model: ModelId,
    animation_name: &str,
) -> crate::Result<Option<AnimationClip>> {
    optional_file(AnimationClip::load(
        game,
        scene.model(model),
        animation_name,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::CameraSettings;
    use crate::gpu::Gpu;
    use crate::objects::{GlowColors, LodFile};
    use crate::shape::{AnimationClip, NodeInfo, NodeKind, Track};
    use mc2_gfx::config::RenderConfig;
    use mc2_sim::appearance::tables::{gesture, MECH_ANIMATION_NAMES};
    use std::path::PathBuf;

    /// A scene on the headless adapter, or `None` only for an explicit GPU opt-out.
    fn scene() -> Option<SceneRenderer> {
        let cfg = RenderConfig {
            msaa: 1,
            ..RenderConfig::default()
        };
        match Gpu::headless(&cfg) {
            Ok(gpu) => Some(SceneRenderer::new(gpu).unwrap()),
            Err(e)
                if e.to_string().contains("adapter")
                    && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                        == Some(std::ffi::OsStr::new("1")) =>
            {
                eprintln!("SKIPPED: MC2_SKIP_GPU_TESTS=1 and no usable GPU adapter ({e})");
                None
            }
            Err(e) => panic!("headless render tests need a GPU adapter: {e}"),
        }
    }

    /// A data root holding only `files`, read through the same VFS as the shipped data.
    fn fixture(tag: &str, files: &[(&str, &[u8])]) -> (PathBuf, GameData) {
        let root = std::env::temp_dir().join(format!("mc2-render-{tag}-{}", std::process::id()));
        for (name, bytes) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let game = GameData {
            vfs: mc2_formats::vfs::Vfs::new(&root),
        };
        (root, game)
    }

    fn appearance(name: &str, lods: [Option<(&str, f32)>; 2]) -> Appearance {
        let lod = |l: Option<(&str, f32)>| {
            l.map(|(file, distance)| LodFile {
                file: file.into(),
                distance,
            })
        };
        Appearance {
            name: name.into(),
            lods: [lod(lods[0]), lod(lods[1]), None],
            shadow: None,
            damaged: None,
            selection_bounds: None,
            glow: GlowColors::default(),
            terrain_light: None,
        }
    }

    fn empty_model() -> ShapeModel {
        ShapeModel {
            name: "m".into(),
            nodes: vec![NodeInfo {
                id: "root".into(),
                parent: None,
                center: Vec3::ZERO,
                relative_center: Vec3::ZERO,
                kind: NodeKind::Normal,
            }],
            texture_names: Vec::new(),
            texture_alpha: Vec::new(),
            vertices: Vec::new(),
            batches: Vec::new(),
            bounds: (Vec3::ZERO, Vec3::ZERO),
            cone_extent: 0.0,
            building_bounds: (Vec3::ZERO, Vec3::ZERO),
            gestures: Vec::new(),
        }
    }

    fn edge_instance(
        model: ModelId,
        kind: AppearanceKind,
        position: Vec3,
        selection_bounds: (Vec3, Vec3),
        yaw_degrees: f32,
    ) -> ShapeInstance {
        let mut shapes = ShapeSet::default();
        shapes.kind = kind;
        shapes.lods[0] = Some(model);
        shapes.lods[1] = Some(model);
        shapes.lod_distance[1] = 100.0;
        shapes.selection_bounds = Some(selection_bounds);
        ShapeInstance {
            model,
            shapes,
            state: AppearanceState::default(),
            choice: ShapeChoice::default(),
            pose: Vec::new(),
            pose_key: None,
            turret: None,
            position,
            yaw_degrees,
            infinite_light: [1.0; 3],
            ambient_override: None,
            casts_shadow: false,
            in_view: true,
            root_height_difference: 0.0,
            highlight: 0,
        }
    }

    #[test]
    fn cxx_edge_gates_select_shapes_before_lod() {
        let Some(mut scene) = scene() else { return };
        let (root, mut game) = fixture("cxx-edge-gates", &[]);
        std::fs::create_dir_all(&root).unwrap();
        let model = scene.add_model(&mut game, empty_model(), ShapeStyle::default());
        let mut camera = GameCamera::new(CameraSettings::default(), (1024.0, 768.0));
        camera.target = Vec3::ZERO;
        camera.zoom_value(camera.settings.altitude_minimum);
        let eye = camera.position_world();
        let forward = (camera.target - eye).normalized();
        // At yaw zero the C++ Stuff-to-world basis puts world west (-x) at screen right
        // (camera.cpp projection; `camera.rs` has the matching handedness check).
        let right = Vec3::new(-1.0, 0.0, 0.0);
        let half_fov_tan = 20.0_f32.to_radians().tan();

        // gvactor.cpp:1601-1605 compares cosine > cosHalfFOV.  The object is 30 degrees
        // right of the look vector: cos(30) = 0.866025 < cos(40 * 0.7071) = 0.8809, so
        // inView=false and the otherwise eligible Distance1=100 LOD must remain 0.
        let cone = edge_instance(
            model,
            AppearanceKind::Vehicle,
            eye + (forward * 30.0_f32.to_radians().cos() + right * 30.0_f32.to_radians().sin())
                * 1200.0,
            (
                Vec3::new(-600.0, -600.0, -600.0),
                Vec3::new(600.0, 600.0, 600.0),
            ),
            0.0,
        );

        // gvactor.cpp:1632-1640 rejects its cheap rectangle first.  At the focal plane,
        // x = 512 + 1.02 * 512 = 1034.24.  This model's OBB radius is zero, so both
        // horizontal rectangle sides are 1034.24 > 1024: inView=false even though the
        // authored +/-600 selection box would overlap the screen.
        let rough = edge_instance(
            model,
            AppearanceKind::Vehicle,
            camera.target + right * (camera.altitude * half_fov_tan * 1.02),
            (
                Vec3::new(-600.0, -600.0, -600.0),
                Vec3::new(600.0, 600.0, 600.0),
            ),
            0.0,
        );

        // gvactor.cpp:1646-1729 rotates just the two `[3DBounds]` endpoints, then combines
        // their components.  At +45 degrees, (-100,0,0) and (10,0,20) become Stuff-space
        // (-70.711,0,70.711) and (21.213,0,7.071), so their world-y values are 70.711 and
        // 7.071.  With centre y=250, projectionAngle=20 and altitude=560, `projectZ` gives
        // screen y = 772.41 for y=257.071 and 980.61 for y=320.711: both are below 768.
        // The rough rectangle is centred at y=754.01 and therefore passes first.  Thus the
        // C++ final-box result is inView=false and the eligible Distance1=100 must retain LOD 0.
        let rotated_authored = edge_instance(
            model,
            AppearanceKind::Vehicle,
            Vec3::new(0.0, 250.0, 0.0),
            (Vec3::new(-100.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 20.0)),
            45.0,
        );

        let mut instances = [cone, rough, rotated_authored];
        select_shapes(&scene, &mut instances, &camera, true);
        for (name, inst) in [
            ("cone", &instances[0]),
            ("rough", &instances[1]),
            ("rotated", &instances[2]),
        ] {
            assert!(!inst.in_view, "{name} C++ edge gate should reject it");
            assert_eq!(inst.choice.current_lod, 0, "{name} should retain LOD 0");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    /// One building at `0.9` camera-forward units to the right of the target (altitude 560,
    /// projection angle 20), with `bounds` as both its shape and type box unless overridden.
    fn building_cone_edge(
        name: &str,
        bounds: (Vec3, Vec3),
        type_bounds: (Vec3, Vec3),
    ) -> Option<bool> {
        let mut m = empty_model();
        m.set_zero_root_bounds(bounds);
        building_cone_edge_model(name, m, type_bounds, 0.9)
    }

    /// As [`building_cone_edge`] for a prepared model, `ratio` camera-forward units to the right.
    fn building_cone_edge_model(
        name: &str,
        m: ShapeModel,
        type_bounds: (Vec3, Vec3),
        ratio: f32,
    ) -> Option<bool> {
        let mut scene = scene()?;
        let (root, mut game) = fixture(name, &[]);
        std::fs::create_dir_all(&root).unwrap();
        let base = scene.add_model(&mut game, m, ShapeStyle::default());
        let mut camera = GameCamera::new(CameraSettings::default(), (1024.0, 768.0));
        camera.target = Vec3::ZERO;
        camera.zoom_value(camera.settings.altitude_minimum);
        let position = camera.target + Vec3::new(-1.0, 0.0, 0.0) * (camera.altitude * ratio);
        let mut instances = [ShapeInstance {
            model: base,
            shapes: ShapeSet {
                kind: AppearanceKind::Building,
                lods: [Some(base), None, None],
                building_type_bounds: Some(type_bounds),
                ..Default::default()
            },
            state: AppearanceState::default(),
            choice: ShapeChoice::default(),
            pose: Vec::new(),
            pose_key: None,
            turret: None,
            position,
            yaw_degrees: 0.0,
            infinite_light: [1.0; 3],
            ambient_override: None,
            casts_shadow: false,
            in_view: false,
            root_height_difference: 0.0,
            highlight: 0,
        }];
        select_shapes(&scene, &mut instances, &camera, true);
        std::fs::remove_dir_all(root).unwrap();
        Some(instances[0].in_view)
    }

    const ASYM: (Vec3, Vec3) = (Vec3::new(-300.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 300.0));
    const SYM: (Vec3, Vec3) = (Vec3::new(-300.0, 0.0, -300.0), Vec3::new(300.0, 0.0, 300.0));
    const TYPE_400: (Vec3, Vec3) = (
        Vec3::new(-400.0, -400.0, -400.0),
        Vec3::new(400.0, 400.0, 400.0),
    );

    #[test]
    fn building_cone_uses_endpoint_lengths_not_the_farthest_corner() {
        // GetExtentRadius = max(|min|, |max|) (msl.cpp:274-280).  ASYM: |(-300,0,0)| = |(0,0,300)|
        // = 300, while the farthest-corner OBBRadius is sqrt(300^2+300^2) = 424.26.  At x/forward
        // = 0.9 and distance 560*sqrt(1.81) = 753.4 the cone limit is 0.4167 + 300/753.4 = 0.815
        // < 0.9 (C++ rejects, bdactor.cpp:1104-1118); with 424.26 it would be 0.980 (accept).  The
        // rough rect (centre x ~ 1778, half width 848.5, bdactor.cpp:1168-1178) and the 400-unit
        // type box (x ~ 773..2783 minus, left edge < 1024) both pass, isolating the cone.
        // SYM has endpoint lengths 424.26 and so is accepted by the same gates.
        let Some(asym) = building_cone_edge("cone-asym", ASYM, TYPE_400) else {
            return;
        };
        let sym = building_cone_edge("cone-sym", SYM, TYPE_400).unwrap();
        assert!(!asym, "cone radius 300 must reject");
        assert!(sym, "cone radius 424.26 must accept");
    }

    /// One node of a raw TGL file: `verts`/`tris` empty means a node without a shape.
    struct RawNode {
        id: &'static str,
        parent: &'static str,
        center: [f32; 3],
        rel: [f32; 3],
        verts: Vec<[f32; 3]>,
        tris: Vec<[u32; 3]>,
    }

    fn raw(
        id: &'static str,
        parent: &'static str,
        center: [f32; 3],
        rel: [f32; 3],
        verts: &[[f32; 3]],
        tris: &[[u32; 3]],
    ) -> RawNode {
        RawNode {
            id,
            parent,
            center,
            rel,
            verts: verts.to_vec(),
            tris: tris.to_vec(),
        }
    }

    /// The bytes of a `.tgl` file (the layout `mc2_assets::tgl::decode_shape` reads), so the fixtures enter
    /// through the production file -> `decode_shape` -> `ShapeModel::from_multishape` -> `ShapeLoader` path.
    fn tgl_bytes(nodes: &[RawNode]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(mc2_assets::tgl::CURRENT_SHAPE_VERSION.to_le_bytes());
        b.extend((nodes.len() as u32).to_le_bytes());
        b.extend(0u32.to_le_bytes()); // no textures
        let id = |s: &str| {
            let mut f = [0u8; 25];
            f[..s.len()].copy_from_slice(s.as_bytes());
            f
        };
        for n in nodes {
            if n.verts.is_empty() {
                b.extend(0u32.to_le_bytes());
            } else {
                b.extend(1u32.to_le_bytes());
                b.extend((n.verts.len() as u32).to_le_bytes());
                for p in &n.verts {
                    for c in p.iter().chain(&[0.0, 1.0, 0.0]) {
                        b.extend(c.to_le_bytes());
                    }
                    b.extend(0u32.to_le_bytes());
                }
                b.extend((n.tris.len() as u32).to_le_bytes());
                for t in &n.tris {
                    for i in t {
                        b.extend(i.to_le_bytes());
                    }
                    b.extend(0u32.to_le_bytes()); // texture
                    b.extend(0u32.to_le_bytes()); // render flags
                    for _ in 0..6 {
                        b.extend(0.0f32.to_le_bytes());
                    }
                    for c in [0.0f32, 1.0, 0.0] {
                        b.extend(c.to_le_bytes());
                    }
                }
                b.extend(0u32.to_le_bytes()); // shape textures
            }
            for c in n.center.iter().chain(&n.rel) {
                b.extend(c.to_le_bytes());
            }
            b.extend(id(n.id));
            b.extend(id(n.parent));
        }
        b
    }

    /// A building appearance resolved by the production `ShapeLoader::resolve_appearance` from raw `.tgl` files
    /// (`base`, and `damaged` when given) and the appearance's authored `[3DBounds]`.  Nothing of the type box is
    /// supplied by the test.
    fn resolve_raw_building(
        scene: &mut SceneRenderer,
        tag: &str,
        base: &[RawNode],
        damaged: Option<&[RawNode]>,
        authored: Option<(Vec3, Vec3)>,
    ) -> (PathBuf, Resolved) {
        let base_bytes = tgl_bytes(base);
        let damaged_bytes = damaged.map(tgl_bytes);
        let mut files: Vec<(&str, &[u8])> = vec![("data/tgl/rawbase.tgl", &base_bytes)];
        if let Some(d) = &damaged_bytes {
            files.push(("data/tgl/rawdamaged.tgl", d));
        }
        let (root, mut game) = fixture(tag, &files);
        let mut app = appearance("raw", [Some(("rawbase", 0.0)), None]);
        app.selection_bounds = authored;
        app.damaged = damaged.map(|_| "rawdamaged".to_string());
        let mut loader = ShapeLoader {
            missing: HashSet::new(),
            gesture_suffixes: Vec::new(),
            animated: HashSet::new(),
        };
        let mut report = PopulateReport::default();
        let res = loader
            .resolve_appearance(
                &mut game,
                scene,
                ObjectClass::Building,
                &app,
                None,
                &mut report,
            )
            .unwrap()
            .expect("the base shape loads");
        (root, res)
    }

    /// The camera of every building fixture: altitude 560, projection angle 20, scale factor 1.
    fn edge_camera() -> GameCamera {
        let mut camera = GameCamera::new(CameraSettings::default(), (1024.0, 768.0));
        camera.target = Vec3::ZERO;
        camera.zoom_value(camera.settings.altitude_minimum);
        camera
    }

    /// `inView` of a raw-file building `ratio` camera-forward units right of centre (screen x =
    /// 512 + 512 * ratio / tan 20 = 512 + 1406.7 * ratio), intact then destroyed, with the model shown after
    /// destruction.  `None` where there is no GPU adapter.
    fn raw_building_frames(
        tag: &str,
        base: &[RawNode],
        damaged: Option<&[RawNode]>,
        authored: Option<(Vec3, Vec3)>,
        ratio: f32,
    ) -> Option<(bool, bool, bool, (Vec3, Vec3))> {
        let mut scene = scene()?;
        let (root, res) = resolve_raw_building(&mut scene, tag, base, damaged, authored);
        let type_box = res.shapes.building_type_bounds.expect("a building box");
        let camera = edge_camera();
        let position = camera.target + Vec3::new(-1.0, 0.0, 0.0) * (camera.altitude * ratio);
        let mut instances = [new_instance(&scene, &res, position, 0.0, [1.0; 3], None)];
        instances[0].in_view = false;
        select_shapes(&scene, &mut instances, &camera, true);
        let before = instances[0].in_view;
        instances[0].state.status = crate::appearance::ObjectStatus::Destroyed;
        select_shapes(&scene, &mut instances, &camera, true);
        let shown_damaged = res.shapes.damaged == Some(instances[0].model);
        let after = instances[0].in_view;
        std::fs::remove_dir_all(root).unwrap();
        Some((before, after, shown_damaged, type_box))
    }

    /// The rough-rect/type-box offset of the edge fixtures: a building 1.02 focal-plane half-widths right of
    /// centre has screen x = 512 * 2.02 = 1034.24 (`0.3712` forward units at tan 20 = 0.36397).
    const EDGE_RATIO: f32 = 1.02 * 0.363_970_23;

    /// A box of `h` around the origin from two triangles' vertices: Stuff min `(-h,-h,-h)`, max `(h,h,h)`.
    fn cube_node(h: f32) -> Vec<RawNode> {
        vec![raw(
            "root",
            "",
            [0.0; 3],
            [0.0; 3],
            &[[-h, -h, -h], [h, h, h], [0.0, 0.0, 0.0]],
            &[[0, 1, 2]],
        )]
    }

    #[test]
    fn building_nested_nodes_use_each_nodes_relative_centre_not_the_accumulated_pose() {
        // root (centre 0) -> mid (relative -100, no shape) -> leaf (relative -10, triangle (0,0,0) (1,0,0)
        // (0,0,1)).  C++ box (msl.cpp:245-270): each vertex plus ITS node's relative centre only, so
        // min (-10,0,0), max (-9,0,1); the root centre 0 is added once (bdactor.cpp:683-741).
        //   OBBRadius = |(-10,0,1)| = sqrt(101) = 10.050; GetExtentRadius = max(|min| = 10, |max| =
        //   sqrt(82) = 9.055) = 10.
        // The accumulated bind pose would put the leaf at x -110..-109 (radius ~110).
        // Building at x/forward 0.40: screen x = 512 + 1406.7*0.40 = 1074.7 px.  Cone: distance 560*
        // sqrt(1.16) = 603.1, limit 0.4167 + 10/603.1 = 0.433 > 0.40: passes (bdactor.cpp:1088-1118).
        // Rough rect left = 1074.7 - 2*10.05 = 1054.6 > 1024: C++ rejects.  The pose-accumulated radius
        // 110 would give left 854.7 and a type box reaching 110 world units (276 px) toward the
        // centre: accept.  Control at 0.30 (934 px): on screen, accepted.
        let nested = || {
            vec![
                raw("root", "", [0.0; 3], [0.0; 3], &[], &[]),
                raw("mid", "root", [0.0; 3], [-100.0, 0.0, 0.0], &[], &[]),
                raw(
                    "leaf",
                    "mid",
                    [0.0; 3],
                    [-10.0, 0.0, 0.0],
                    &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                    &[[0, 1, 2]],
                ),
            ]
        };
        let Some((before, after, _, type_box)) =
            raw_building_frames("nested-edge", &nested(), Some(&nested()), None, 0.40)
        else {
            return;
        };
        assert_eq!(
            type_box,
            (Vec3::new(-10.0, 0.0, 0.0), Vec3::new(-9.0, 0.0, 1.0))
        );
        assert!(
            !before,
            "C++ rejects: base rect and type box are off screen"
        );
        assert!(!after);
        let (control, ..) =
            raw_building_frames("nested-control", &nested(), None, None, 0.30).unwrap();
        assert!(control, "control: the same building at 934 px is on screen");

        let mut scene = scene().unwrap();
        let (root, res) = resolve_raw_building(&mut scene, "nested-numbers", &nested(), None, None);
        let model = scene.model(res.base);
        assert!((model.building_obb_radius() - 10.049_876).abs() < 1e-3);
        assert!((model.cone_radius() - 10.0).abs() < 1e-4);
        assert!(
            model.bounds.0.x < -109.0,
            "the pose-accumulated box differs"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn building_cone_excludes_the_root_offset_and_counts_unused_vertices() {
        // Root node centre (-100,0,0); vertices (-10,0,0), (10,0,0), (0,0,1) in a triangle and one no
        // triangle uses, (0,0,-10).  C++ box over all four (msl.cpp:245-270): min (-10,0,-10), max
        // (10,0,1), GetExtentRadius = max(14.142, 10.050) = 14.142.  Type box = box + root centre
        // (bdactor.cpp:738-741) = (-110,0,-10)..(-90,0,1); OBBRadius = |(-110,0,-10)| = 110.45.
        // At x/forward 0.45 (screen 1145 px, distance 560*sqrt(1.2025) = 614.1) the cone limit is
        // 0.4167 + 14.142/614.1 = 0.4397 < 0.45: C++ rejects.  The render-space radius (~110) would pass
        // it (0.596).  Control at 0.40: cone limit 0.4167 + 14.142/603.1 = 0.440 > 0.40, rough rect left
        // 1074.7 - 220.9 = 853.8 and the type box (90..110 world units toward the centre) both overlap.
        let building = || {
            vec![raw(
                "root",
                "",
                [-100.0, 0.0, 0.0],
                [0.0; 3],
                &[
                    [-10.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [0.0, 0.0, -10.0],
                ],
                &[[0, 1, 2]],
            )]
        };
        let Some((before, _, _, type_box)) =
            raw_building_frames("cone-root", &building(), None, None, 0.45)
        else {
            return;
        };
        assert_eq!(
            type_box,
            (Vec3::new(-110.0, 0.0, -10.0), Vec3::new(-90.0, 0.0, 1.0))
        );
        assert!(!before, "radius 14.142 must reject");
        let (control, ..) =
            raw_building_frames("cone-root-ok", &building(), None, None, 0.40).unwrap();
        assert!(control, "control: every gate passes at 0.40");
    }

    #[test]
    fn building_base_bounds_count_vertices_no_triangle_uses() {
        // One triangle (0,0,0) (1,0,0) (0,0,1) plus a vertex no triangle uses at Stuff (-100,0,0).  C++
        // scans every type vertex (msl.cpp:245-270): box (-100,0,0)..(1,0,1), OBBRadius = sqrt(100^2 +
        // 1^2) = 100.005 (bdactor.cpp:710-724), type box reaching 100 Stuff units (world +x, toward
        // the centre).  Building at x/forward 0.40 (cone limit 0.4167 + 100/603 > 0.40), screen
        // 1074.7 px: rough rect left 1074.7 - 200 = 874.7 and the type box left overlap: C++ accepts.
        // The triangle-only shape (radius 1.414) has rect left 1071.9 > 1024: rejects.
        let tri = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let with_unused = vec![raw(
            "root",
            "",
            [0.0; 3],
            [0.0; 3],
            &[tri[0], tri[1], tri[2], [-100.0, 0.0, 0.0]],
            &[[0, 1, 2]],
        )];
        let triangle_only = vec![raw("root", "", [0.0; 3], [0.0; 3], &tri, &[[0, 1, 2]])];
        let Some((seen, _, _, type_box)) =
            raw_building_frames("unused-vertex", &with_unused, None, None, 0.40)
        else {
            return;
        };
        assert_eq!(
            type_box,
            (Vec3::new(-100.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 1.0))
        );
        assert!(seen, "C++ accepts with the unused vertex in the box");
        let (control, ..) =
            raw_building_frames("unused-vertex-control", &triangle_only, None, None, 0.40).unwrap();
        assert!(!control, "control: the triangle-only box rejects");
    }

    #[test]
    fn building_zero_radius_base_rejects_despite_a_larger_damaged_shape() {
        // OBBRadius and the type box come from the BASE shape once (bdactor.cpp:683-741).  Base all
        // vertices at the origin: r = 0, the rough rect is the column x = 1034.24 (bdactor.cpp:1168-1173)
        // and 1034.24 > 1024 fails `upperLeft.x <= resX` (bdactor.cpp:1175-1178).  The damaged r =
        // sqrt(3)*100 shape selected by `setObjStatus` (bldng.cpp:1497-1499) is read by neither the
        // rect nor the type box, so C++ rejects on the damage frame as well.
        let Some((before, after, shown_damaged, _)) = raw_building_frames(
            "building-zero-base",
            &cube_node(0.0),
            Some(&cube_node(100.0)),
            None,
            EDGE_RATIO,
        ) else {
            return;
        };
        assert!(!before);
        assert!(shown_damaged);
        assert!(!after);
    }

    #[test]
    fn building_base_type_box_rejects_despite_a_wider_damaged_shape() {
        // Base r = 173.2: rough rect x in 1034.24 -/+ 346.4 = [687.8, 1380.6] overlaps [0, 1024]
        // (bdactor.cpp:1168-1178).  The authored zero `[3DBounds]` type box projects to x = 1034.24 >
        // 1024, so the type-box test rejects (bdactor.cpp:1187-1190, 1293-1299).  The damaged +/-100
        // box (x 783..1285) is never used for it.
        let zero = (Vec3::ZERO, Vec3::ZERO);
        let Some((before, after, shown_damaged, type_box)) = raw_building_frames(
            "building-zero-type",
            &cube_node(100.0),
            Some(&cube_node(100.0)),
            Some(zero),
            EDGE_RATIO,
        ) else {
            return;
        };
        assert_eq!(type_box, zero);
        assert!(!before);
        assert!(shown_damaged);
        assert!(!after);
    }

    #[test]
    fn building_damage_transition_on_the_edge_is_accepted_by_the_base_bounds() {
        // Base r = 173.2 and derived type box +/-100 (x about 783..1285) both overlap the screen, so
        // C++ accepts; the zero damaged shape (extent 0, cone 0.371 < 0.4167) supplies neither rect
        // nor box, so it cannot reject on the transition frame.
        let Some((before, after, shown_damaged, _)) = raw_building_frames(
            "building-accepted-edge",
            &cube_node(100.0),
            Some(&cube_node(0.0)),
            None,
            EDGE_RATIO,
        ) else {
            return;
        };
        assert!(before);
        assert!(shown_damaged);
        assert!(after);
    }

    #[test]
    fn building_damage_uses_its_new_extent_for_the_transition_frame_cone() {
        let Some(mut scene) = scene() else { return };
        let (root, mut game) = fixture("building-damage-fixed-box-edge", &[]);
        std::fs::create_dir_all(&root).unwrap();
        let mut intact_model = empty_model();
        intact_model.set_zero_root_bounds((
            Vec3::new(-100.0, -100.0, -100.0),
            Vec3::new(100.0, 100.0, 100.0),
        ));
        let intact = scene.add_model(&mut game, intact_model, ShapeStyle::default());
        let damaged_model = empty_model();
        let damaged = scene.add_model(&mut game, damaged_model, ShapeStyle::default());
        let mut camera = GameCamera::new(CameraSettings::default(), (1024.0, 768.0));
        camera.target = Vec3::ZERO;
        camera.zoom_value(camera.settings.altitude_minimum);
        // `cameraFrame` sees x / forward = 0.45 here. The C++ horizontal clip is
        // tan(40 * 0.7071 * 1024 / 1280) = 0.4167 (`bdactor.cpp:1088-1118`). The
        // intact r=sqrt(3)*100 adds 0.309 at this 560-unit focal distance, so it
        // passes; the damaged r=0 does not. The base OBB rectangle and type box still
        // overlap, so this isolates the current `bldgShape` cone decision.
        let position = camera.target + Vec3::new(-1.0, 0.0, 0.0) * (camera.altitude * 0.45);
        let shapes = ShapeSet {
            kind: AppearanceKind::Building,
            lods: [Some(intact), None, None],
            damaged: Some(damaged),
            ..Default::default()
        };
        let mut instances = [ShapeInstance {
            model: intact,
            shapes,
            state: AppearanceState::default(),
            choice: ShapeChoice::default(),
            pose: Vec::new(),
            pose_key: None,
            turret: None,
            position,
            yaw_degrees: 0.0,
            infinite_light: [1.0; 3],
            ambient_override: None,
            casts_shadow: false,
            in_view: false,
            root_height_difference: 0.0,
            highlight: 0,
        }];
        select_shapes(&scene, &mut instances, &camera, true);
        assert!(
            instances[0].in_view,
            "the intact extent admits the edge centre"
        );
        instances[0].state.status = crate::appearance::ObjectStatus::Destroyed;
        select_shapes(&scene, &mut instances, &camera, true);
        assert_eq!(instances[0].model, damaged);
        assert!(
            !instances[0].in_view,
            "the damaged extent must decide the destruction frame's cone gate"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selection_uses_lane_f_root_baseline_and_helicopter_state() {
        // `Mech3DAppearance::recalcBounds` uses the first-update baseRootNodeHeight, retains
        // baseRootNodeDifference between out-of-view frames, and lowers an in-view helicopter
        // by 25 (mech3d.cpp:2153-2160, 4160). Exercise those inputs through select_shapes,
        // the production LOD/visibility path, rather than testing the box helper alone.
        let Some(mut scene) = scene() else { return };
        let (root, mut game) = fixture("root-selection", &[]);
        std::fs::create_dir_all(&root).unwrap();
        let mut model = empty_model();
        model.nodes[0].id = "joint_root".into();
        model.gestures = vec![Some(AnimationClip {
            frames: 1,
            frame_rate: 30.0,
            tracks: vec![Some(Track {
                rotations: Vec::new(),
                positions: vec![Vec3::new(0.0, 100.0, 0.0)],
            })],
        })];
        let model_id = scene.add_model(&mut game, model, ShapeStyle::default());
        let mut shapes = ShapeSet::default();
        shapes.kind = AppearanceKind::Mech;
        shapes.lods[0] = Some(model_id);
        let mut instances = [ShapeInstance {
            model: model_id,
            shapes,
            state: AppearanceState {
                gesture: Some(0),
                base_root_node_height: Some(25.0),
                ..Default::default()
            },
            choice: ShapeChoice::default(),
            pose: scene.model(model_id).bind_pose(),
            pose_key: None,
            turret: None,
            position: Vec3::ZERO,
            yaw_degrees: 0.0,
            infinite_light: [1.0; 3],
            ambient_override: None,
            casts_shadow: false,
            in_view: true,
            root_height_difference: 0.0,
            highlight: 0,
        }];
        let mut camera = GameCamera::new(CameraSettings::default(), (1024.0, 768.0));
        camera.settings.max_clip_distance = 1.0e9;
        camera.target = Vec3::ZERO;
        select_shapes(&scene, &mut instances, &camera, true);
        assert!((instances[0].root_height_difference - 75.0).abs() < 1e-6);
        instances[0].state.is_helicopter = true;
        select_shapes(&scene, &mut instances, &camera, true);
        assert!((instances[0].root_height_difference - 50.0).abs() < 1e-6);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_absent_file_is_none_and_a_broken_one_is_an_error() {
        // The same rule for shapes and animations: absent from the data is left out, present and unreadable is an
        // error (no adapter needed).
        let (root, mut game) = fixture(
            "optional-files",
            &[
                ("data/tgl/garbage.tgl", b"not a shape"),
                ("data/tgl/garbage_walk.agl", b"not an animation"),
            ],
        );
        assert!(optional_file(ShapeModel::load(&mut game, "absent"))
            .unwrap()
            .is_none());
        assert!(optional_file(ShapeModel::load(&mut game, "garbage")).is_err());
        let model = empty_model();
        let no_clip = gesture_clips(&mut game, &model, "absent", &["walk".into()]).unwrap();
        assert_eq!(no_clip.len(), 1);
        assert!(no_clip[0].is_none());
        assert!(gesture_clips(&mut game, &model, "garbage", &["_walk".into()]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_present_broken_lod_fails_the_appearance_and_an_absent_base_is_reported() {
        let Some(mut scene) = scene() else { return };
        let (root, mut game) = fixture("broken-shape", &[("data/tgl/garbage.tgl", b"not a shape")]);
        let mut loader = ShapeLoader {
            missing: HashSet::new(),
            gesture_suffixes: Vec::new(),
            animated: HashSet::new(),
        };
        let mut report = PopulateReport::default();
        // An absent base shape: the object is skipped and the file is named.
        let absent = appearance("fixture", [Some(("absent", 0.0)), None]);
        assert!(loader
            .resolve_appearance(
                &mut game,
                &mut scene,
                ObjectClass::Building,
                &absent,
                None,
                &mut report
            )
            .unwrap()
            .is_none());
        assert_eq!(report.missing_files, vec!["absent".to_string()]);
        // A present LOD that does not parse is an error, even beside an absent base.
        let broken_lod = appearance("fixture", [Some(("absent", 0.0)), Some(("garbage", 10.0))]);
        assert!(loader
            .resolve_appearance(
                &mut game,
                &mut scene,
                ObjectClass::Building,
                &broken_lod,
                None,
                &mut report
            )
            .is_err());
        // A present base shape that does not parse is an error, not a skipped object.
        let broken_base = appearance("fixture", [Some(("garbage", 0.0)), None]);
        assert!(loader
            .resolve_appearance(
                &mut game,
                &mut scene,
                ObjectClass::Building,
                &broken_base,
                None,
                &mut report
            )
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn appearances_that_share_a_mesh_keep_their_own_animations() {
        // Two mech appearances name the same shape file and different animation files (`<appearance><suffix>.agl`).
        // Each poses from its own clips, whichever is loaded first (shipped data).
        let Some(root) = std::env::var_os("MC2_DATA") else {
            eprintln!("MC2_DATA not set; skipping shipped-data test");
            return;
        };
        let walk = gesture::WALK as usize;
        let suffixes: Vec<String> = MECH_ANIMATION_NAMES.iter().map(|s| s.to_string()).collect();
        for order in [["Werewolf", "Flea"], ["Flea", "Werewolf"]] {
            let Some(mut scene) = scene() else { return };
            let mut game = GameData::open(&root).unwrap();
            let mut loader = ShapeLoader {
                missing: HashSet::new(),
                gesture_suffixes: suffixes.clone(),
                animated: HashSet::new(),
            };
            let mut report = PopulateReport::default();
            let mut bases = HashMap::new();
            for name in order {
                let app = appearance(name, [Some(("werewolf", 0.0)), None]);
                let res = loader
                    .resolve_appearance(
                        &mut game,
                        &mut scene,
                        ObjectClass::BattleMech,
                        &app,
                        None,
                        &mut report,
                    )
                    .unwrap()
                    .expect("werewolf has a shape");
                bases.insert(name, res.base);
            }
            assert_ne!(
                bases["Werewolf"], bases["Flea"],
                "one model would share both clip sets"
            );
            for name in ["Werewolf", "Flea"] {
                let model = bases[name];
                // The clip the appearance names (`<appearance>walk.agl`), at frame 3.
                let clip = optional_file(AnimationClip::load(
                    &mut game,
                    scene.model(model),
                    &format!("{name}{}", MECH_ANIMATION_NAMES[walk]).to_ascii_lowercase(),
                ))
                .unwrap();
                let expected = match clip {
                    Some(clip) => scene.model(model).pose(Some(&clip), 3.0, &HashMap::new()),
                    None => scene.model(model).bind_pose(),
                };
                let actual = state_pose(
                    scene.model(model),
                    &AppearanceState {
                        status: crate::appearance::ObjectStatus::Normal,
                        gesture: Some(walk),
                        frame: 3.0,
                        fall_shape: None,
                        ..Default::default()
                    },
                );
                assert_eq!(actual, expected, "{name} after {order:?}");
            }
        }
    }

    #[test]
    fn unavailable_names_each_unit_and_its_missing_input() {
        assert!(PopulateReport::default().ensure_painted().is_ok());
        let report = PopulateReport {
            unpainted: vec![
                Unpainted {
                    part: 3,
                    object_number: 45,
                    class: ObjectClass::BattleMech,
                    gap: PaintGap::NoHomeColours,
                },
                Unpainted {
                    part: 7,
                    object_number: 874,
                    class: ObjectClass::Vehicle,
                    gap: PaintGap::NoCommander,
                },
            ],
            ..Default::default()
        };
        let msg = report.ensure_painted().unwrap_err().to_string();
        assert!(
            msg.starts_with("paint unavailable for 2 mech(s) or vehicle(s): "),
            "{msg}"
        );
        assert!(
            msg.contains(
                "part 3 (object 45, BattleMech) needs the home commander's colours (options.cfg"
            ),
            "{msg}"
        );
        assert!(
            msg.contains("part 7 (object 874, Vehicle) needs a CommanderId"),
            "{msg}"
        );
    }
}
