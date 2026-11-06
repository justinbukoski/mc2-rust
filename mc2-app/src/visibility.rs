// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The camera-derived simulation input (`rust/mc2-sim/DESIGN.md` section 5).
//!
//! What the camera sees changes simulation results, so the application computes it the way the
//! original renderer did and hands it to the sim each frame:
//! * `Terrain::objBlockInfo[b].active` / `Terrain::objVertexActive[v]`: ported from
//!   `MapData::update` (the vertex window around the camera, `mclib/mapdata.cpp`),
//!   `MapData::makeLists` (window vertices, blank vertices outside the map, block numbers) and the
//!   vertex loop of `Terrain::geometry` (`mclib/terrain.cpp`: the sphere-clip test in the
//!   `cameraFrame`, `CLIP_THRESHOLD_DISTANCE`, `VERTEX_EXTENT_RADIUS`) with the clip constants of
//!   `Camera::calculateProjectionConstants`;
//! * mover `inView`: the camera half of `Mech3DAppearance::recalcBounds` (`mclib/mech3d.cpp`: far
//!   clip, the `cosHalfFOV` view cone, the type's screen bounds and the projected selection box).
//!   The appearance data it reads come with each mover ([`MoverBounds`], from the appearance
//!   lane through the sim driver).

use crate::camera::{CameraRig, DEGREES_TO_RADS, FOV_FACTOR};
use crate::sim::{MoverBounds, MoverPosition, StructurePosition};
use mc2_render::camera::GameCamera;
use mc2_render::math::Vec3;
use mc2_render::terrain::{TerrainMap, VERTICES_BLOCK_SIDE, WORLD_UNITS_PER_VERTEX};
use std::collections::{BTreeMap, BTreeSet};

/// `CLIP_THRESHOLD_DISTANCE`: vertices closer than this (in the camera frame's x/y plane) are
/// always active.
pub const CLIP_THRESHOLD_DISTANCE: f32 = 768.0;
/// `VERTEX_EXTENT_RADIUS`: a full triangle.
pub const VERTEX_EXTENT_RADIUS: f32 = 384.0;
/// Elevation of `MapData::blankVertex` (vertices of the window outside the map).
pub const BLANK_VERTEX_ELEVATION: f32 = 33.0;

/// The camera state a frame's visibility was computed from.
#[derive(Clone, Debug)]
pub struct ViewSnapshot {
    /// The view (matrices, viewport in native pixels).
    pub camera: GameCamera,
    /// `Camera::position`: centre of the terrain vertex window.
    pub position: Vec3,
    /// `projectionAngle` (the player's tilt; the `cameraFrame` uses it, not the corrected pitch).
    pub projection_angle: f32,
    /// `cameraRotation`.
    pub rotation: f32,
    /// `camera_fov`.
    pub fov: f32,
    /// `cosHalfFOV`.
    pub cos_half_fov: f32,
    /// `newScaleFactor`.
    pub scale_factor: f32,
    /// Eye position (`getCameraOrigin`) and `lookVector`, world frame.
    pub eye: Vec3,
    pub look: Vec3,
    /// `screenResolution` the sphere clip constants were computed from, when it is not the
    /// viewport: the C++ first `Camera::update` runs before `viewMulX/Y` exist (0 x 0, constants
    /// `tan(0)`), which the oracle's turn-1 vertex set shows.
    pub clip_resolution: Option<(f32, f32)>,
}

impl ViewSnapshot {
    pub fn from_rig(rig: &CameraRig) -> Self {
        Self {
            camera: rig.view(),
            position: rig.position,
            projection_angle: rig.projection_angle,
            rotation: rig.rotation,
            fov: rig.fov,
            cos_half_fov: rig.cos_half_fov,
            scale_factor: rig.scale_factor,
            eye: rig.eye_position(),
            look: rig.look_vector(),
            clip_resolution: None,
        }
    }

    /// `(verticalSphereClipConstant, horizontalSphereClipConstant)`.
    pub fn sphere_clip_constants(&self) -> (f32, f32) {
        let (w, h) = self.clip_resolution.unwrap_or(self.camera.viewport);
        let mag = (w * w + h * h).sqrt();
        let inv = if mag > 1e-4 { 1.0 / mag } else { mag };
        (
            (h * inv * self.fov * FOV_FACTOR * DEGREES_TO_RADS).tan(),
            (w * inv * self.fov * FOV_FACTOR * DEGREES_TO_RADS).tan(),
        )
    }

    /// `Camera::cameraFrame` after `reset_to_world_frame` and `rotate((-projectionAngle, 0,
    /// cameraRotation))`: the rows `(i, j, k)` that `trans_to_frame` dots with.
    pub fn camera_frame(&self) -> [Vec3; 3] {
        let mut i = Vec3::new(1.0, 0.0, 0.0);
        let mut j = Vec3::new(0.0, 1.0, 0.0);
        let mut k = Vec3::new(0.0, 0.0, 1.0);
        // rotate_about_k(cameraRotation)
        let (s, c) = (self.rotation * DEGREES_TO_RADS).sin_cos();
        let (oi, oj) = (i, j);
        i = i * c + oj * s;
        j = j * c - oi * s;
        // rotate_about_i: the oracle's vertex sets (mc2_01 turns 2-8, exact) are reproduced with
        // +projectionAngle here, not the -projectionAngle `Camera::calculateProjectionConstants` passes
        let (s, c) = (self.projection_angle * DEGREES_TO_RADS).sin_cos();
        let (oj, ok) = (j, k);
        j = j * c + ok * s;
        k = k * c - oj * s;
        // rotate_about_j(0) leaves the frame as it is
        [i, j, k]
    }

    /// The camera test of `Mech3DAppearance::recalcBounds`: inside the far clip distance and
    /// inside the `cosHalfFOV` cone around the look vector.
    pub fn in_view_cone(&self, world: Vec3) -> bool {
        let d = world - self.eye;
        if d.length() > self.camera.settings.max_clip_distance {
            return false;
        }
        d.normalized().dot(self.look) > self.cos_half_fov
    }

    /// `Camera::projectZ`: screen position (native pixels) and whether it is on screen.
    pub fn project(&self, world: Vec3) -> ([f32; 4], bool) {
        self.camera.project_z(world)
    }
}

/// Per-frame camera visibility, in the shape `mc2_sim::visibility::Visibility` reads it.
#[derive(Clone, Debug, Default)]
pub struct CameraVisibility {
    /// `Terrain::objBlockInfo[b].active`, indexed by block (`blocksMapSide^2`).
    pub blocks: Vec<bool>,
    /// `Terrain::objVertexActive[v]`, indexed by `row * side + col`.
    pub vertices: Vec<bool>,
    /// Part ids of movers whose appearance is `inView`.
    pub movers_in_view: BTreeSet<i32>,
    /// Handles of structures whose appearances are in view.
    pub structures_in_view: BTreeSet<u32>,
    /// Camera-selected LOD per mover part id when the driver supplies appearance thresholds.
    pub mover_lods: BTreeMap<i32, usize>,
    /// Camera-selected LOD per structure handle when the driver supplies appearance thresholds.
    pub structure_lods: BTreeMap<u32, usize>,
    /// Final projected type-box bounds in viewport pixels, recorded for the replay oracle's
    /// explicit viewport-edge tolerance check.
    pub mover_selection_boxes: BTreeMap<i32, [f32; 4]>,
    /// Final projected type-box bounds in viewport pixels, keyed by structure handle.
    pub structure_selection_boxes: BTreeMap<u32, [f32; 4]>,
    /// The camera it was computed from (for appearance-level checks).
    pub view: Option<ViewSnapshot>,
    /// A live mover or structure has no bound production appearance.  Handed to the sim as
    /// [`mc2_sim::visibility::Visibility::object_in_view_available`] `= false`, rather than
    /// substituting a projected-point test for C++ `recalcBounds`.
    pub object_in_view_unavailable: bool,
}

/// The object half of a camera frame, selected by the same R1 appearance path that will draw the
/// frame.  Terrain activation is deliberately kept outside this type: it belongs to
/// `Terrain::geometry`, whereas these are `*Appearance::recalcBounds` results.
#[derive(Clone, Debug, Default)]
pub struct ObjectVisibility {
    pub movers_in_view: BTreeSet<i32>,
    pub structures_in_view: BTreeSet<u32>,
    pub mover_lods: BTreeMap<i32, usize>,
    pub structure_lods: BTreeMap<u32, usize>,
    pub mover_selection_boxes: BTreeMap<i32, [f32; 4]>,
    pub structure_selection_boxes: BTreeMap<u32, [f32; 4]>,
    /// False means a live object did not have a bound appearance, so pretending that its centre
    /// was its box would make the simulation evidence invalid.
    pub complete: bool,
}

/// Installs the production appearance-selection result in a terrain visibility frame.
pub fn with_objects(mut terrain: CameraVisibility, objects: ObjectVisibility) -> CameraVisibility {
    terrain.movers_in_view = objects.movers_in_view;
    terrain.structures_in_view = objects.structures_in_view;
    terrain.mover_lods = objects.mover_lods;
    terrain.structure_lods = objects.structure_lods;
    terrain.mover_selection_boxes = objects.mover_selection_boxes;
    terrain.structure_selection_boxes = objects.structure_selection_boxes;
    terrain.object_in_view_unavailable = !objects.complete;
    terrain
}

impl CameraVisibility {
    pub fn block_active(&self, block: u32) -> bool {
        self.blocks.get(block as usize).copied().unwrap_or(false)
    }
    pub fn vertex_active(&self, vertex: u32) -> bool {
        self.vertices.get(vertex as usize).copied().unwrap_or(false)
    }
    pub fn mover_in_view(&self, part_id: i32) -> bool {
        self.movers_in_view.contains(&part_id)
    }
    pub fn structure_in_view(&self, handle: u32) -> bool {
        self.structures_in_view.contains(&handle)
    }
    pub fn active_vertex_count(&self) -> usize {
        self.vertices.iter().filter(|v| **v).count()
    }
    pub fn active_block_count(&self) -> usize {
        self.blocks.iter().filter(|v| **v).count()
    }
}

/// The terrain-vertex window of `MapData::update`: absolute `(col, row)` of its top-left vertex.
pub fn vertex_window_origin(
    terrain: &TerrainMap,
    position: Vec3,
    visible_per_side: i32,
) -> (i32, i32) {
    let (tlx, tly) = terrain.top_left();
    let one_over = 1.0 / WORLD_UNITS_PER_VERTEX;
    let fx = (position.x - tlx) * one_over;
    let fy = (tly - position.y) * one_over;
    // float2long truncates
    let pvx = (fx + 0.5) as i32;
    let pvy = (fy + 0.5) as i32;
    let half = visible_per_side >> 1;
    (
        (pvx as f32 - half as f32) as i32,
        (pvy as f32 - half as f32) as i32,
    )
}

/// `clearObjBlocksActive` + `MapData::update/makeLists` + the activity half of
/// `Terrain::geometry`, plus mover `inView`.
pub fn compute(
    view: &ViewSnapshot,
    terrain: &TerrainMap,
    visible_per_side: i32,
    movers: &[MoverPosition],
    structures: &[StructurePosition],
) -> CameraVisibility {
    let side = terrain.side() as i32;
    let blocks_side = side / VERTICES_BLOCK_SIDE as i32;
    let mut blocks = vec![false; (blocks_side * blocks_side).max(0) as usize];
    let mut vertices = vec![false; (side * side) as usize];
    let (vclip, hclip) = view.sphere_clip_constants();
    let [fi, fj, fk] = view.camera_frame();
    let cam = view.eye;
    let half = side / 2;
    let (ox, oy) = vertex_window_origin(terrain, view.position, visible_per_side);
    let n = visible_per_side.max(0);
    for wy in 0..n {
        for wx in 0..n {
            let (ax, ay) = (ox + wx, oy + wy);
            let inside = ax >= 0 && ax < side && ay >= 0 && ay < side;
            let (vertex_num, elevation) = if inside {
                (
                    ay * side + ax,
                    terrain.vertex(ax as usize, ay as usize).elevation,
                )
            } else {
                (-1, BLANK_VERTEX_ELEVATION)
            };
            // C integer division truncates towards zero (out-of-map vertices left of / above the
            // map land in the first block column / row).
            let block =
                ax / VERTICES_BLOCK_SIDE as i32 + (ay / VERTICES_BLOCK_SIDE as i32) * blocks_side;
            let v = Vec3::new(
                (ax - half) as f32 * WORLD_UNITS_PER_VERTEX,
                (half - ay) as f32 * WORLD_UNITS_PER_VERTEX,
                elevation,
            );
            let d = v - cam;
            let oc = Vec3::new(fi.dot(d), fj.dot(d), fk.dot(d));
            let distance_to_eye = oc.length();
            let distance_to_clip = (oc.x * oc.x + oc.y * oc.y).sqrt();
            let clip_distance = (1.0 / oc.y).abs();
            let mut on_screen = true;
            if distance_to_clip > CLIP_THRESHOLD_DISTANCE {
                let extent_angle = VERTEX_EXTENT_RADIUS / distance_to_eye;
                if oc.z.abs() * clip_distance > vclip + extent_angle
                    || oc.x.abs() * clip_distance > hclip + extent_angle
                {
                    on_screen = false;
                }
            }
            if on_screen {
                if block >= 0 && (block as usize) < blocks.len() {
                    blocks[block as usize] = true;
                }
                if vertex_num >= 0 {
                    vertices[vertex_num as usize] = true;
                }
            }
        }
    }
    let movers_in_view: BTreeSet<i32> = movers
        .iter()
        .filter(|m| mover_in_view(view, m.position, m.bounds.as_ref()))
        .map(|m| m.part_id)
        .collect();
    let structures_in_view: BTreeSet<u32> = structures
        .iter()
        .filter(|s| {
            structure_in_view(
                view,
                s.position,
                s.extent_radius,
                s.rotation,
                s.bounds.as_ref(),
            )
        })
        .map(|s| s.handle)
        .collect();
    let mover_lods = movers
        .iter()
        .map(|m| {
            let lod = if movers_in_view.contains(&m.part_id) && m.lod_active {
                m.lod_profile
                    .map(|p| p.select((m.position - view.eye).length()))
                    .unwrap_or(m.current_lod)
            } else {
                m.current_lod
            };
            (m.part_id, lod)
        })
        .collect();
    let structure_lods = structures
        .iter()
        .map(|s| {
            let lod = if structures_in_view.contains(&s.handle) && s.lod_active {
                s.lod_profile
                    .map(|p| p.select((s.position - view.eye).length()))
                    .unwrap_or(s.current_lod)
            } else {
                s.current_lod
            };
            (s.handle, lod)
        })
        .collect();
    CameraVisibility {
        blocks,
        vertices,
        movers_in_view,
        structures_in_view,
        mover_lods,
        structure_lods,
        mover_selection_boxes: BTreeMap::new(),
        structure_selection_boxes: BTreeMap::new(),
        view: Some(view.clone()),
        object_in_view_unavailable: movers.iter().any(|m| m.bounds.is_none())
            || structures.iter().any(|s| s.bounds.is_none()),
    }
}

/// `BldgAppearance::recalcBounds` (bdactor.cpp): clip the structure's extent sphere, then apply
/// its rough projected bounds and projected eight-corner selection box when appearance bounds
/// are available.
pub fn structure_in_view(
    view: &ViewSnapshot,
    position: Vec3,
    extent_radius: f32,
    rotation: f32,
    bounds: Option<&MoverBounds>,
) -> bool {
    let (vclip, hclip) = view.sphere_clip_constants();
    let [fi, fj, fk] = view.camera_frame();
    let d = position - view.eye;
    let frame = Vec3::new(fi.dot(d), fj.dot(d), fk.dot(d));
    let distance = frame.length();
    if distance > view.camera.settings.max_clip_distance {
        return false;
    }
    let clip_distance = (1.0 / frame.y).abs();
    let extent_angle = extent_radius / distance;
    if frame.z.abs() * clip_distance > vclip + extent_angle
        || frame.x.abs() * clip_distance > hclip + extent_angle
    {
        return false;
    }
    let Some(bounds) = bounds else {
        return true;
    };
    let (width, height) = view.camera.viewport;
    let (screen, _) = view.project(position);
    let scale = view.scale_factor;
    let rough_upper = (
        screen[0] + bounds.screen_upper_left.0 * scale,
        screen[1] + bounds.screen_upper_left.1 * scale,
    );
    let rough_lower = (
        screen[0] + bounds.screen_lower_right.0 * scale,
        screen[1] + bounds.screen_lower_right.1 * scale,
    );
    if rough_lower.0 < 0.0 || rough_lower.1 < 0.0 || rough_upper.0 > width || rough_upper.1 > height
    {
        return false;
    }
    let upper = bounds.box_upper_left;
    let lower = bounds.box_lower_right;
    let (sin, cos) = (-rotation * DEGREES_TO_RADS).sin_cos();
    let corners = [
        (upper.x, upper.y, upper.z),
        (upper.x, upper.y, lower.z),
        (upper.x, lower.y, upper.z),
        (upper.x, lower.y, lower.z),
        (lower.x, upper.y, upper.z),
        (lower.x, upper.y, lower.z),
        (lower.x, lower.y, upper.z),
        (lower.x, lower.y, lower.z),
    ];
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y, z) in corners {
        let corner = rotated_structure_corner(
            position,
            Vec3::new(x, y, z),
            sin,
            cos,
            bounds.base_root_node_difference,
        );
        let (projected, _) = view.project(corner);
        min_x = min_x.min(projected[0]);
        max_x = max_x.max(projected[0]);
        min_y = min_y.min(projected[1]);
        max_y = max_y.max(projected[1]);
    }
    max_x >= 0.0 && max_y >= 0.0 && min_x <= width && min_y <= height
}

/// C++ `BldgAppearance::recalcBounds` converts selection-box corners to Stuff coordinates before
/// calling `Rotate` (`mclib/bdactor.cpp:1187-1261`). `Rotate` mixes Stuff X/Y, which map to game
/// X/Z (`mclib/mathfunc.cpp:34-44`).
fn rotated_structure_corner(
    position: Vec3,
    offset_game: Vec3,
    sin: f32,
    cos: f32,
    root_height: f32,
) -> Vec3 {
    let stuff_x = -offset_game.x;
    let stuff_y = offset_game.z;
    let stuff_z = offset_game.y;
    let rotated_stuff_x = stuff_x * cos + stuff_y * sin;
    let rotated_stuff_y = stuff_y * cos - stuff_x * sin;
    Vec3::new(
        position.x - rotated_stuff_x,
        position.y + stuff_z,
        position.z + rotated_stuff_y + root_height,
    )
}

impl mc2_sim::visibility::Visibility for CameraVisibility {
    fn block_active(&self, block: u32) -> bool {
        CameraVisibility::block_active(self, block)
    }

    fn vertex_active(&self, vertex: u32) -> bool {
        CameraVisibility::vertex_active(self, vertex)
    }

    fn mover_in_view(&self, part: mc2_sim::PartId) -> bool {
        CameraVisibility::mover_in_view(self, part.0)
    }

    fn structure_in_view(&self, handle: mc2_sim::ObjectHandle) -> bool {
        CameraVisibility::structure_in_view(self, handle.0)
    }

    fn object_in_view_available(&self) -> bool {
        !self.object_in_view_unavailable
    }

    fn mover_lod(&self, part: mc2_sim::PartId) -> usize {
        self.mover_lods.get(&part.0).copied().unwrap_or(0)
    }

    fn structure_lod(&self, handle: mc2_sim::ObjectHandle) -> Option<usize> {
        // `None` for a structure the producer did not report (no appearance type): unavailable,
        // never a silent LOD 0.
        self.structure_lods.get(&handle.0).copied()
    }
}

/// `Mech3DAppearance::recalcBounds`'s `inView`: inside the far clip distance and the `cosHalfFOV`
/// cone; then the rough screen test of the type's screen bounds (scaled by the zoom) around the
/// projected position; then the projected selection box (eight corners) must overlap the screen.
pub fn mover_in_view(view: &ViewSnapshot, position: Vec3, bounds: Option<&MoverBounds>) -> bool {
    if !view.in_view_cone(position) {
        return false;
    }
    let b = bounds.copied().unwrap_or(MoverBounds {
        screen_upper_left: (0.0, 0.0),
        screen_lower_right: (0.0, 0.0),
        box_upper_left: Vec3::ZERO,
        box_lower_right: Vec3::ZERO,
        base_root_node_difference: 0.0,
    });
    let (w, h) = view.camera.viewport;
    let on_screen =
        |ul: (f32, f32), lr: (f32, f32)| lr.0 >= 0.0 && lr.1 >= 0.0 && ul.0 <= w && ul.1 <= h;
    let (s, _) = view.project(position);
    let k = view.scale_factor;
    let ul = (
        s[0] + b.screen_upper_left.0 * k,
        s[1] + b.screen_upper_left.1 * k,
    );
    let lr = (
        s[0] + b.screen_lower_right.0 * k,
        s[1] + b.screen_lower_right.1 * k,
    );
    if !on_screen(ul, lr) {
        return false;
    }
    let (u, l) = (b.box_upper_left, b.box_lower_right);
    let z = |c: f32| position.z + c + b.base_root_node_difference;
    let corners = [
        Vec3::new(position.x + u.x, position.y + u.y, z(u.z)),
        Vec3::new(position.x + u.x, position.y + l.y, z(u.z)),
        Vec3::new(position.x + l.x, position.y + u.y, z(u.z)),
        Vec3::new(position.x + l.x, position.y + l.y, z(u.z)),
        Vec3::new(position.x + l.x, position.y + l.y, z(l.z)),
        Vec3::new(position.x + l.x, position.y + u.y, z(l.z)),
        Vec3::new(position.x + u.x, position.y + l.y, z(l.z)),
        Vec3::new(position.x + u.x, position.y + u.y, z(l.z)),
    ];
    let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for c in corners {
        let (p, _) = view.project(c);
        min = (min.0.min(p[0]), min.1.min(p[1]));
        max = (max.0.max(p[0]), max.1.max(p[1]));
    }
    on_screen(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{CameraData, FlatGround, MissionCameraStart};
    use crate::sim::LodProfile;
    use mc2_assets::map::TerrainVertex;
    use mc2_render::camera::CameraSettings;

    fn flat_map(side: usize) -> TerrainMap {
        let v = TerrainVertex {
            normal: [0.0, 0.0, 1.0],
            elevation: 400.0,
            texture_data: 0,
            local_rgb_light: 0,
            terrain_type: 0,
            selected: 0,
            water: 0,
            shadow: 0,
            highlighted: 0,
        };
        TerrainMap::new(side, vec![v; side * side])
    }

    fn rig_at(x: f32, y: f32, viewport: (f32, f32)) -> CameraRig {
        let g = FlatGround {
            side: 100.0 * 128.0,
            height: 400.0,
            water: 0.0,
        };
        let mut r = CameraRig::new(
            CameraSettings::default(),
            &MissionCameraStart::default(),
            CameraData::default(),
            [(1200.0, 35.0); 4],
            viewport,
        );
        r.set_position(Vec3::new(x, y, 400.0), false, &g, 1.0 / 30.0);
        r.all_normal();
        r.update(10, &g, 1.0 / 30.0);
        r
    }

    #[test]
    fn window_is_centred_on_the_camera() {
        let t = flat_map(100);
        // map centre: vertex 50,50 -> window starts 15 vertices up-left
        assert_eq!(vertex_window_origin(&t, Vec3::ZERO, 30), (35, 35));
        assert_eq!(
            vertex_window_origin(&t, Vec3::new(128.0 * 10.0, -128.0 * 5.0, 0.0), 30),
            (45, 40)
        );
    }

    #[test]
    fn the_vertices_around_the_target_are_active() {
        let t = flat_map(100);
        let r = rig_at(0.0, 0.0, (800.0, 600.0));
        let vis = compute(&ViewSnapshot::from_rig(&r), &t, 30, &[], &[]);
        // the target vertex (50,50) and its block are active
        assert!(vis.vertex_active(50 * 100 + 50));
        let blocks_side = 100 / 20;
        assert!(vis.block_active((50 / 20 + (50 / 20) * blocks_side) as u32));
        // nothing outside the 30x30 window
        assert!(!vis.vertex_active(0));
        assert!(!vis.vertex_active(99 * 100 + 99));
        let n = vis.active_vertex_count();
        assert!(n > 100 && n <= 900, "{n}");
    }

    #[test]
    fn clip_constants_follow_the_viewport_aspect() {
        let t = flat_map(100);
        let a = compute(
            &ViewSnapshot::from_rig(&rig_at(0.0, 0.0, (800.0, 600.0))),
            &t,
            30,
            &[],
            &[],
        );
        let b = compute(
            &ViewSnapshot::from_rig(&rig_at(0.0, 0.0, (1920.0, 800.0))),
            &t,
            30,
            &[],
            &[],
        );
        // vclip = tan(h/|res| * fov * .7071), hclip likewise with w: the aspect, not the size,
        // decides the sets (a wider screen narrows the vertical test), so only bounds hold
        for v in [&a, &b] {
            assert!(v.active_vertex_count() > 0 && v.active_vertex_count() <= 900);
        }
    }

    #[test]
    fn window_outside_the_map_marks_edge_blocks_only_in_range() {
        let t = flat_map(100);
        let r = rig_at(-6000.0, 6000.0, (800.0, 600.0));
        let vis = compute(&ViewSnapshot::from_rig(&r), &t, 30, &[], &[]);
        assert_eq!(vis.blocks.len(), 25);
        assert!(vis.block_active(0));
        assert!(!vis.block_active(24));
    }

    #[test]
    fn movers_in_front_are_in_view_movers_behind_are_not() {
        let r = rig_at(0.0, 0.0, (800.0, 600.0));
        let v = ViewSnapshot::from_rig(&r);
        assert!(mover_in_view(&v, Vec3::new(0.0, 0.0, 400.0), None));
        // far behind the eye
        let behind = v.eye - v.look * 500.0;
        assert!(!mover_in_view(&v, behind, None));
        // beyond the far clip distance
        let far = v.eye + v.look * 10_000.0;
        assert!(!mover_in_view(&v, far, None));
        // just off the left edge: out as a point, in with a box that reaches onto the screen
        let probe = (0..2000)
            .map(|i| Vec3::new(i as f32 * 2.0, 0.0, 400.0))
            .chain((0..2000).map(|i| Vec3::new(-(i as f32) * 2.0, 0.0, 400.0)))
            .find(|p| {
                let (s, _) = v.project(*p);
                s[0] < -5.0 && s[0] > -40.0 && v.in_view_cone(*p)
            })
            .expect("a point just off screen");
        assert!(!mover_in_view(&v, probe, None));
        let wide = MoverBounds {
            screen_upper_left: (-100.0, -150.0),
            screen_lower_right: (100.0, 10.0),
            box_upper_left: Vec3::new(-60.0, -60.0, 0.0),
            box_lower_right: Vec3::new(60.0, 60.0, 80.0),
            base_root_node_difference: 0.0,
        };
        assert!(mover_in_view(&v, probe, Some(&wide)));
        let vis = compute(
            &v,
            &flat_map(100),
            30,
            &[
                MoverPosition {
                    part_id: 512,
                    position: Vec3::new(0.0, 0.0, 400.0),
                    bounds: None,
                    lod_profile: Some(LodProfile {
                        distances: [0.0, 100.0, 200.0],
                        available: [true, true, true],
                        high_detail: true,
                    }),
                    lod_active: true,
                    current_lod: 0,
                },
                MoverPosition {
                    part_id: 513,
                    position: behind,
                    bounds: None,
                    lod_profile: None,
                    lod_active: true,
                    current_lod: 1,
                },
            ],
            &[
                StructurePosition {
                    handle: 42,
                    position: Vec3::new(0.0, 0.0, 400.0),
                    rotation: 0.0,
                    bounds: None,
                    extent_radius: 5000.0,
                    lod_profile: Some(LodProfile {
                        distances: [0.0, 100.0, 200.0],
                        available: [true, true, true],
                        high_detail: true,
                    }),
                    lod_active: true,
                    current_lod: 1,
                },
                StructurePosition {
                    handle: 43,
                    position: behind,
                    rotation: 0.0,
                    bounds: None,
                    extent_radius: 10.0,
                    lod_profile: None,
                    lod_active: true,
                    current_lod: 2,
                },
            ],
        );
        assert!(vis.mover_in_view(512) && !vis.mover_in_view(513));
        assert!(vis.structure_in_view(42));
        assert_eq!(vis.mover_lods[&512], 2);
        assert_eq!(vis.mover_lods[&513], 1);
        assert_eq!(vis.structure_lods[&42], 2);
        assert_eq!(vis.structure_lods[&43], 2);
    }

    #[test]
    fn production_object_selection_marks_the_visibility_input_available() {
        let terrain = CameraVisibility {
            object_in_view_unavailable: true,
            ..CameraVisibility::default()
        };
        let objects = ObjectVisibility {
            movers_in_view: [704].into_iter().collect(),
            structures_in_view: [42].into_iter().collect(),
            mover_lods: [(704, 1)].into_iter().collect(),
            structure_lods: [(42, 2)].into_iter().collect(),
            mover_selection_boxes: BTreeMap::new(),
            structure_selection_boxes: BTreeMap::new(),
            complete: true,
        };
        let selected = with_objects(terrain, objects);
        assert!(mc2_sim::visibility::Visibility::object_in_view_available(
            &selected
        ));
        assert!(selected.mover_in_view(704));
        assert!(selected.structure_in_view(42));
        assert_eq!(selected.mover_lods[&704], 1);
        assert_eq!(selected.structure_lods[&42], 2);
    }

    #[test]
    fn structure_screen_bounds_clip_and_retain_prior_lod_when_off_screen() {
        let r = rig_at(0.0, 0.0, (800.0, 600.0));
        let v = ViewSnapshot::from_rig(&r);
        let center = Vec3::new(0.0, 0.0, 400.0);
        let offscreen = MoverBounds {
            screen_upper_left: (1000.0, -20.0),
            screen_lower_right: (1100.0, 20.0),
            box_upper_left: Vec3::new(-20.0, -20.0, 0.0),
            box_lower_right: Vec3::new(20.0, 20.0, 50.0),
            base_root_node_difference: 0.0,
        };
        assert!(!structure_in_view(
            &v,
            center,
            5000.0,
            0.0,
            Some(&offscreen)
        ));
        assert!(structure_in_view(&v, center, 5000.0, 0.0, None));
    }

    #[test]
    fn structure_box_rotates_in_stuff_xz_plane() {
        // C++ BldgAppearance::recalcBounds builds Stuff corners at bdactor.cpp:1187-1261,
        // then Rotate mixes Stuff X/Y at mathfunc.cpp:34-44 (game X/Z after axis conversion).
        let rotation = 90.0 * DEGREES_TO_RADS;
        let (sin, cos) = (-rotation).sin_cos();
        let corner = rotated_structure_corner(
            Vec3::new(10.0, 20.0, 30.0),
            Vec3::new(1.0, 2.0, 3.0),
            sin,
            cos,
            0.0,
        );
        assert!((corner.x - 13.0).abs() < 1e-5);
        assert!((corner.y - 22.0).abs() < 1e-5);
        assert!((corner.z - 29.0).abs() < 1e-5);
    }
}
