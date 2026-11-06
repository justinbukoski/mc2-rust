// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! TGL shapes: node hierarchy, geometry batching and AGL animation playback.
//!
//! Ported from `mclib/msl.cpp` (`TG_MultiShape::TransformMultiShape` hierarchy walk,
//! `TG_TypeMultiShape::CreateFrom` parent lookup, `TG_AnimateShape::SetAnimationState` node
//! matching) and `mclib/tgl.cpp` (`TG_TypeNode::CreateFrom`: `SpotLight_`/`LitWin_` node kinds;
//! `TG_Shape::Render`: textures flagged alpha are drawn blended).  The binary layouts are decoded
//! by `mc2-assets`.
//!
//! The original transforms each vertex on the CPU every frame.  Here the pose is a list of node
//! matrices; the vertex shader applies `node_matrix[vertex.node]` so an instance costs one
//! matrix per node.

use crate::game::GameData;
use crate::math::{stuff_to_world, Mat4, Quat, Vec3};
use mc2_assets::tgl::{self, Animation, MultiShape};
use std::collections::HashMap;

/// What kind of node a shape part is (decided by its name, `TG_TypeNode::CreateFrom`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// Ordinary geometry (or a helper without geometry).
    Normal,
    /// `SpotLight_*`: the lit cone shown at night only.
    Spotlight,
    /// `LitWin_*`: window quads, dark by day and lit at night.
    Window,
}

/// Whether a node's geometry counts towards the C++ selection box (`TG_TypeMultiShape`, `mclib/msl.cpp:240-245`,
/// case-insensitive prefixes).
pub fn counts_towards_bounds(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !(lower.starts_with("_pab") || lower.starts_with("los_") || lower.starts_with("spotlight"))
}

pub fn node_kind(name: &str) -> NodeKind {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("spotlight_") {
        NodeKind::Spotlight
    } else if lower.starts_with("litwin_") {
        NodeKind::Window
    } else {
        NodeKind::Normal
    }
}

/// Vertex of the merged shape mesh.  One per triangle corner (the UVs are per face).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShapeVertex {
    /// Position in the owning node's local frame (Stuff axes).
    pub position: [f32; 3],
    /// Index of the owning node; selects the node matrix.
    pub node: u32,
    pub normal: [f32; 3],
    /// `aRGBLight`: per-vertex emissive / marker colour (0xAARRGGBB).
    pub argb: u32,
    pub uv: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    /// Index into [`ShapeModel::texture_names`].
    pub texture: usize,
    /// Texture is flagged as having alpha: drawn blended.
    pub alpha: bool,
    pub kind: NodeKind,
    pub first_vertex: u32,
    pub vertex_count: u32,
}

#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub id: String,
    pub parent: Option<usize>,
    /// Absolute pivot (used by the root).
    pub center: Vec3,
    /// Pivot relative to the parent.
    pub relative_center: Vec3,
    pub kind: NodeKind,
}

#[derive(Clone, Debug)]
pub struct ShapeModel {
    pub name: String,
    pub nodes: Vec<NodeInfo>,
    pub texture_names: Vec<String>,
    pub texture_alpha: Vec<bool>,
    pub vertices: Vec<ShapeVertex>,
    pub batches: Vec<Batch>,
    /// Bounds of the geometry in the bind pose, Stuff frame (min, max), over the nodes the C++ box takes: every node
    /// but the `_PAB`, `LOS_` and `SpotLight` ones (`TG_TypeMultiShape`, `mclib/msl.cpp:236-280`).
    pub bounds: (Vec3, Vec3),
    /// `TG_TypeMultiShape::GetExtentRadius` (`msl.cpp:236-280`): the longer of the min/max box endpoints, where the
    /// box is every type vertex of the same nodes plus its node's *relative* centre.  Unlike `bounds` it leaves
    /// out the root node centre and counts vertices no triangle uses.
    pub cone_extent: f32,
    /// The building base box of `BldgAppearanceType::init`/`BldgAppearance::init` (`bdactor.cpp:683-741`): the
    /// `GetMinBox`/`GetMaxBox` endpoints (type vertices plus their own node's *relative* centre, `msl.cpp:245-270`)
    /// plus `GetRootNodeCenter` once.  Unlike `bounds` it does not accumulate parent offsets down the hierarchy.
    pub building_bounds: (Vec3, Vec3),
    /// Animation bound to each gesture (`Mech3DAppearanceType::setAnimation`, `mech3d.cpp:655`), indexed by
    /// gesture id; `None` where the appearance has no animation file for it.  Empty until loaded.  Each LOD is a
    /// separate model with its own node order, so each has its own table.
    pub gestures: Vec<Option<AnimationClip>>,
}

/// Which C++ recolouring routine a mask goes through.  The mech's
/// (`Mech3DAppearance::setPaintScheme`, `mech3d.cpp:1619`) writes alpha 255 on every painted pixel.
/// The vehicle's (`gvactor.cpp:1143`, `1168`) writes the source pixel's own alpha back, so a
/// translucent vehicle mask stays translucent.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaintRoutine {
    #[default]
    Mech,
    Vehicle,
}

/// `Mech3DAppearance::setPaintScheme(DWORD, DWORD, DWORD)`: mechs and vehicles ship their texture
/// as a mask.  Pure red, pure green and pure blue pixels are replaced by the three paint colours,
/// scaled by `1 - (1 - v/256)^2` so the artists' shading survives; anything else is kept.
///
/// `red`, `green`, `blue` are the colours for the pure red / green / blue regions, as the C++
/// callers pass them.  The mission passes `highlightColor1`, `highlightColor2` and `baseColor` in
/// that order (`resetPaintScheme(parts[i].highlightColor1, ...highlightColor2, ...baseColor)`,
/// `mission.cpp:1077-1085`).  Each colour is `0x00RRGGBB` and is used as passed: the Windows build
/// defines no `BGR` (`mclib/MCLib.vcproj:46`), so `setPaintScheme` takes the `#else` branch
/// (`mech3d.cpp:1738-1741`) and writes `(r << 16) | (g << 8) | b` into the A8R8G8B8 texel
/// (`mech3d.cpp:1665`).  The alariq Linux build's `-DBGR` (`mclib/CMakeLists.txt:89`) swaps R and
/// B first (`mech3d.cpp:1731-1737`); that is a reference-build artefact (DIFFERENCES D-107,
/// `docs/evidence/rc-paint-windows.md`).  `routine` decides the alpha of a painted pixel (see
/// [`PaintRoutine`]).
pub fn apply_paint_scheme(rgba: &mut [u8], red: u32, green: u32, blue: u32, routine: PaintRoutine) {
    let scaled = |v: u8| {
        let b = v as f32 * 0.003_906_25;
        let b = 1.0 - b;
        1.0 - b * b
    };
    let mix = |c: u32, k: f32| -> [u8; 3] {
        [
            (((c >> 16) & 0xff) as f32 * k) as u8,
            (((c >> 8) & 0xff) as f32 * k) as u8,
            ((c & 0xff) as f32 * k) as u8,
        ]
    };
    for px in rgba.chunks_exact_mut(4) {
        let (r, g, b) = (px[0], px[1], px[2]);
        let new = if g == 0 && b == 0 {
            Some(mix(red, scaled(r)))
        } else if r == 0 && b == 0 {
            Some(mix(green, scaled(g)))
        } else if r == 0 && g == 0 {
            Some(mix(blue, scaled(b)))
        } else {
            None
        };
        if let Some(n) = new {
            px[0] = n[0];
            px[1] = n[1];
            px[2] = n[2];
            if routine == PaintRoutine::Mech {
                px[3] = 255;
            }
        }
    }
}

/// `BldgAppearance::init` / `TreeAppearance::init`: textures whose file name starts with `a_` are
/// loaded with their alpha channel (`gos_Texture_Alpha`, flagged `textureAlpha`); all others are
/// opaque (`gos_Texture_Solid`).  The flag stored inside the TGL is overwritten by this rule.
pub fn texture_has_alpha(name: &str) -> bool {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    base.len() >= 2 && base[..2].eq_ignore_ascii_case("a_")
}

/// File name of a texture inside the 128 pixel texture folder.
pub fn texture_path(name: &str) -> String {
    let base = name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    format!("data/tgl/128/{base}")
}

/// `GetMinBox`/`GetMaxBox` from the raw type data (`msl.cpp:236-270`): min/max over all type vertices of the
/// non-`_PAB`/`LOS_`/`SpotLight` nodes, each offset by its own relative node centre only (no root centre).  The box
/// starts at the C++ seeds, `+/-100000` (`msl.h:107-113`).
fn type_box(shape: &MultiShape) -> (Vec3, Vec3) {
    let mut mn = [100_000.0_f32; 3];
    let mut mx = [-100_000.0_f32; 3];
    for n in &shape.nodes {
        if !counts_towards_bounds(&n.node_id) {
            continue;
        }
        let Some(s) = &n.shape else { continue };
        for v in &s.vertices {
            for a in 0..3 {
                let p = v.position[a] + n.relative_node_center[a];
                mn[a] = mn[a].min(p);
                mx[a] = mx[a].max(p);
            }
        }
    }
    (Vec3::from_array(mn), Vec3::from_array(mx))
}

/// `GetExtentRadius` (`msl.cpp:274-280`): the larger length of the two [`type_box`] endpoints.
fn type_extent_radius(shape: &MultiShape) -> f32 {
    let (mn, mx) = type_box(shape);
    mn.length().max(mx.length())
}

/// `GetRootNodeCenter` (`msl.h:423-435`): the node centre of the last node without a parent.
fn root_node_center(shape: &MultiShape) -> Vec3 {
    shape
        .nodes
        .iter()
        .rfind(|n| n.parent_index.is_none())
        .map_or(Vec3::ZERO, |n| Vec3::from_array(n.node_center))
}

impl ShapeModel {
    pub fn from_multishape(name: &str, shape: &MultiShape) -> Self {
        let nodes: Vec<NodeInfo> = shape
            .nodes
            .iter()
            .map(|n| NodeInfo {
                id: n.node_id.clone(),
                parent: n.parent_index,
                center: Vec3::from_array(n.node_center),
                relative_center: Vec3::from_array(n.relative_node_center),
                kind: node_kind(&n.node_id),
            })
            .collect();
        let texture_names: Vec<String> = shape.textures.iter().map(|t| t.name.clone()).collect();
        let texture_alpha: Vec<bool> = shape
            .textures
            .iter()
            .map(|t| texture_has_alpha(&t.name))
            .collect();

        // Triangles grouped by (kind, texture) so each batch is one draw.
        struct Tri {
            node: usize,
            tri: usize,
        }
        let mut groups: HashMap<(u8, usize), Vec<Tri>> = HashMap::new();
        for (ni, n) in shape.nodes.iter().enumerate() {
            let Some(s) = &n.shape else { continue };
            for (ti, t) in s.triangles.iter().enumerate() {
                let kind = match nodes[ni].kind {
                    NodeKind::Normal => 0u8,
                    NodeKind::Spotlight => 1,
                    NodeKind::Window => 2,
                };
                groups
                    .entry((kind, t.texture as usize))
                    .or_default()
                    .push(Tri { node: ni, tri: ti });
            }
        }
        let mut keys: Vec<_> = groups.keys().copied().collect();
        keys.sort_unstable();

        let mut vertices = Vec::new();
        let mut batches = Vec::new();
        let mut mn = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut mx = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
        for key in keys {
            let first = vertices.len() as u32;
            for t in &groups[&key] {
                let s = shape.nodes[t.node].shape.as_ref().unwrap();
                let tri = &s.triangles[t.tri];
                for (corner, &vi) in tri.vertices.iter().enumerate() {
                    let v = &s.vertices[vi as usize];
                    vertices.push(ShapeVertex {
                        position: v.position,
                        node: t.node as u32,
                        normal: v.normal,
                        argb: v.argb_light,
                        uv: tri.uv[corner],
                    });
                }
            }
            let kind = match key.0 {
                0 => NodeKind::Normal,
                1 => NodeKind::Spotlight,
                _ => NodeKind::Window,
            };
            batches.push(Batch {
                texture: key.1,
                alpha: texture_alpha.get(key.1).copied().unwrap_or(false),
                kind,
                first_vertex: first,
                vertex_count: vertices.len() as u32 - first,
            });
        }

        let mut model = ShapeModel {
            name: name.to_string(),
            nodes,
            texture_names,
            texture_alpha,
            vertices,
            batches,
            bounds: (Vec3::ZERO, Vec3::ZERO),
            cone_extent: 0.0,
            building_bounds: (Vec3::ZERO, Vec3::ZERO),
            gestures: Vec::new(),
        };
        model.cone_extent = type_extent_radius(shape);
        let has_vertex = shape.nodes.iter().any(|n| {
            counts_towards_bounds(&n.node_id)
                && n.shape.as_ref().is_some_and(|s| !s.vertices.is_empty())
        });
        if has_vertex {
            let (lo, hi) = type_box(shape);
            let root = root_node_center(shape);
            model.building_bounds = (lo + root, hi + root);
        }
        // Bind-pose bounds from the bind pose node matrices.
        let pose = model.bind_pose();
        let mut any = false;
        // C++ scans every type vertex, used by a triangle or not (`msl.cpp:245-270`).
        for (ni, n) in shape.nodes.iter().enumerate() {
            // The C++ box leaves out the spotlight, `_PAB` and `LOS_` nodes (`msl.cpp:240-245`); window quads count.
            if !counts_towards_bounds(&n.node_id) {
                continue;
            }
            let Some(s) = &n.shape else { continue };
            for v in &s.vertices {
                any = true;
                let p = pose[ni].transform_point(Vec3::from_array(v.position));
                mn = Vec3::new(mn.x.min(p.x), mn.y.min(p.y), mn.z.min(p.z));
                mx = Vec3::new(mx.x.max(p.x), mx.y.max(p.y), mx.z.max(p.z));
            }
        }
        if !any {
            mn = Vec3::ZERO;
            mx = Vec3::ZERO;
        }
        model.bounds = (mn, mx);
        model
    }

    pub fn load(game: &mut GameData, name: &str) -> crate::Result<Self> {
        let bytes = game.read(&format!("data/tgl/{name}.tgl"))?;
        let shape = tgl::decode_shape(&bytes)
            .map_err(|e| crate::RenderError::Other(format!("{name}.tgl: {e}")))?;
        Ok(Self::from_multishape(name, &shape))
    }

    /// The farthest corner of the bind-pose `bounds` from the shape origin: the OBB radius that sizes the
    /// rough screen rectangle of vehicles, mechs and trees.  Buildings use [`Self::building_obb_radius`].
    /// Invariant under the axis swap between the Stuff and game frames.  This is *not* `GetExtentRadius`;
    /// see [`Self::cone_radius`].
    pub fn extent_radius(&self) -> f32 {
        let (lo, hi) = self.bounds;
        Vec3::new(
            lo.x.abs().max(hi.x.abs()),
            lo.y.abs().max(hi.y.abs()),
            lo.z.abs().max(hi.z.abs()),
        )
        .length()
    }

    /// The farthest corner of `building_bounds`: the building `OBBRadius`, fixed from the base shape (`bdactor.cpp:710-724`).
    pub fn building_obb_radius(&self) -> f32 {
        let (lo, hi) = self.building_bounds;
        Vec3::new(
            lo.x.abs().max(hi.x.abs()),
            lo.y.abs().max(hi.y.abs()),
            lo.z.abs().max(hi.z.abs()),
        )
        .length()
    }

    /// Fixture helper for a shape whose nodes all sit at the origin: `bounds` is then also the type box the
    /// C++ extent is taken from, so both fields follow from one pair of endpoints.
    pub fn set_zero_root_bounds(&mut self, bounds: (Vec3, Vec3)) {
        self.bounds = bounds;
        self.building_bounds = bounds;
        self.cone_extent = bounds.0.length().max(bounds.1.length());
    }

    /// `GetExtentRadius`: the larger length of the minimum and maximum bounds endpoints
    /// (`msl.cpp:274-280`), not the farthest corner.  `BldgAppearance::recalcBounds` and the tree
    /// equivalent use it for the horizontal/vertical cone test (`bdactor.cpp:1104`, `3625`).
    pub fn cone_radius(&self) -> f32 {
        self.cone_extent
    }

    /// The eight corners of this type's selection box in game world coordinates.  The C++
    /// appearances project these corners before deciding `inView` and changing LOD
    /// (`bdactor.cpp:1183-1302`, `gvactor.cpp:1642-1732`, `mech3d.cpp:2146-2244`).
    ///
    /// `bounds` is in the Stuff frame, so this deliberately uses the same yaw and basis
    /// conversion as [`instance_node_matrices`].  It is a type box, not a posed mesh box:
    /// that is what the C++ code tests (apart from the mech root-height adjustment, which
    /// is simulation state and is not yet exposed by the render input).
    pub fn selection_box_world(&self, world_position: Vec3, yaw_degrees: f32) -> [Vec3; 8] {
        self.selection_box_world_with_bounds(self.bounds, world_position, yaw_degrees)
    }

    /// The non-designer ground-vehicle type box. `GVAppearance::init` expands the mesh box by
    /// `EXPAND_FACTOR` before it is rotated and projected (`gvactor.cpp:968-980`).
    pub fn vehicle_selection_box_world(&self, world_position: Vec3, yaw_degrees: f32) -> [Vec3; 8] {
        self.selection_box_world_from_rotated_endpoints(
            Self::expanded_vehicle_bounds(self.bounds),
            world_position,
            yaw_degrees,
        )
    }

    /// As [`selection_box_world`](Self::selection_box_world), with a designer-provided
    /// `[3DBounds]` override when one exists.
    pub fn selection_box_world_with_bounds(
        &self,
        bounds: (Vec3, Vec3),
        world_position: Vec3,
        yaw_degrees: f32,
    ) -> [Vec3; 8] {
        let (lo, hi) = bounds;
        let yaw = yaw_degrees.to_radians();
        let (sin, cos) = yaw.sin_cos();
        let corner = |p: Vec3| {
            let rotated = Vec3::new(p.x * cos + p.z * sin, p.y, -p.x * sin + p.z * cos);
            world_position + crate::math::stuff_to_world().transform_vector(rotated)
        };
        [
            corner(Vec3::new(lo.x, lo.y, lo.z)),
            corner(Vec3::new(lo.x, lo.y, hi.z)),
            corner(Vec3::new(lo.x, hi.y, lo.z)),
            corner(Vec3::new(lo.x, hi.y, hi.z)),
            corner(Vec3::new(hi.x, lo.y, lo.z)),
            corner(Vec3::new(hi.x, lo.y, hi.z)),
            corner(Vec3::new(hi.x, hi.y, lo.z)),
            corner(Vec3::new(hi.x, hi.y, hi.z)),
        ]
    }

    /// The vehicle/tree form of the C++ type box.  Unlike the building path, the vehicle and
    /// tree code rotates the stored minimum and maximum vectors once and then combines their
    /// components into eight corners; it does *not* rotate every original box corner
    /// (`gvactor.cpp:1646-1692`, `bdactor.cpp:3693-3729`).  Keeping that slightly unusual
    /// sequence matters at a nonzero yaw near a viewport edge.
    pub fn selection_box_world_from_rotated_endpoints(
        &self,
        bounds: (Vec3, Vec3),
        world_position: Vec3,
        yaw_degrees: f32,
    ) -> [Vec3; 8] {
        let (lo, hi) = bounds;
        let yaw = yaw_degrees.to_radians();
        let (sin, cos) = yaw.sin_cos();
        let endpoint = |p: Vec3| {
            let rotated = Vec3::new(p.x * cos + p.z * sin, p.y, -p.x * sin + p.z * cos);
            world_position + crate::math::stuff_to_world().transform_vector(rotated)
        };
        let lo = endpoint(lo) - world_position;
        let hi = endpoint(hi) - world_position;
        Self::selection_box_corners(lo, hi, |p| world_position + p)
    }

    /// The non-designer mech type box in game-world axes. Unlike buildings and ground
    /// vehicles, `Mech3DAppearance::recalcBounds` does not rotate this box: it stores the
    /// shape bounds as `(x, z, y)`, expands them by `EXPAND_FACTOR`, and adds them directly to
    /// the object position (`mech3d.cpp:1282-1299`, `2173-2203`).
    pub fn mech_selection_box_world(&self, world_position: Vec3) -> [Vec3; 8] {
        self.mech_selection_box_world_at_height(world_position, 0.0)
    }

    /// As [`Self::mech_selection_box_world`], with the latched displacement of `joint_root`.
    /// `Mech3DAppearance::recalcBounds` adds `baseRootNodeDifference` to every type-box corner
    /// before its screen test (`mech3d.cpp:2153-2203`).
    pub fn mech_selection_box_world_at_height(
        &self,
        world_position: Vec3,
        root_height_difference: f32,
    ) -> [Vec3; 8] {
        let (lo, hi) = Self::expanded_type_bounds(self.bounds);
        Self::selection_box_corners(lo, hi, |p| {
            world_position + Vec3::new(p.x, p.z, p.y + root_height_difference)
        })
    }

    /// A `[3DBounds]` mech box is already in the C++ world axes: `AppearanceType::init`
    /// stores the six values verbatim, and `Mech3DAppearance::recalcBounds` adds them directly
    /// to `position` (`apprtype.cpp:80-121`, `mech3d.cpp:2173-2203`).
    pub fn mech_designer_selection_box_world(
        bounds: (Vec3, Vec3),
        world_position: Vec3,
    ) -> [Vec3; 8] {
        Self::mech_designer_selection_box_world_at_height(bounds, world_position, 0.0)
    }

    /// The designer-authored counterpart of [`Self::mech_selection_box_world_at_height`].
    pub fn mech_designer_selection_box_world_at_height(
        bounds: (Vec3, Vec3),
        world_position: Vec3,
        root_height_difference: f32,
    ) -> [Vec3; 8] {
        Self::selection_box_corners(bounds.0, bounds.1, |p| {
            world_position + Vec3::new(p.x, p.y, p.z + root_height_difference)
        })
    }

    fn selection_box_corners(lo: Vec3, hi: Vec3, map: impl Fn(Vec3) -> Vec3) -> [Vec3; 8] {
        [
            map(Vec3::new(lo.x, lo.y, lo.z)),
            map(Vec3::new(lo.x, lo.y, hi.z)),
            map(Vec3::new(lo.x, hi.y, lo.z)),
            map(Vec3::new(lo.x, hi.y, hi.z)),
            map(Vec3::new(hi.x, lo.y, lo.z)),
            map(Vec3::new(hi.x, lo.y, hi.z)),
            map(Vec3::new(hi.x, hi.y, lo.z)),
            map(Vec3::new(hi.x, hi.y, hi.z)),
        ]
    }

    /// `EXPAND_FACTOR` in `gvactor.cpp:89` and `mech3d.cpp:100`. It applies only to a box
    /// derived from a mesh; `[3DBounds]` values are already type boxes and stay verbatim.
    fn expanded_type_bounds((lo, hi): (Vec3, Vec3)) -> (Vec3, Vec3) {
        (lo * 1.25, hi * 1.25)
    }

    pub(crate) fn expanded_vehicle_bounds(bounds: (Vec3, Vec3)) -> (Vec3, Vec3) {
        Self::expanded_type_bounds(bounds)
    }

    pub fn node_index(&self, id: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.id.eq_ignore_ascii_case(id))
    }

    /// Node matrices of the rest pose (no animation, no extra rotations).
    pub fn bind_pose(&self) -> Vec<Mat4> {
        self.pose(None, 0.0, &HashMap::new())
    }

    /// Hierarchy walk of `TG_MultiShape::TransformMultiShape`.
    ///
    /// Each node's local transform is `Translate(t) * Rotate(q)` with `q` the animation
    /// quaternion (left-multiplied by `base_rotations[node]` if given, as `msl.cpp:1443`, `rotation.cpp:773-787`) and `t` the animation position, or the
    /// node's pivot when the clip has none (the root uses its absolute `center`, the others
    /// their `relative_center`).  The result is the accumulation parent-first, in the Stuff frame,
    /// relative to the shape origin.
    pub fn pose(
        &self,
        clip: Option<&AnimationClip>,
        frame: f32,
        base_rotations: &HashMap<usize, Quat>,
    ) -> Vec<Mat4> {
        let mut out: Vec<Option<Mat4>> = vec![None; self.nodes.len()];
        for i in 0..self.nodes.len() {
            self.pose_node(i, clip, frame, base_rotations, &mut out, 0);
        }
        out.into_iter()
            .map(|m| m.unwrap_or(Mat4::IDENTITY))
            .collect()
    }

    fn pose_node(
        &self,
        i: usize,
        clip: Option<&AnimationClip>,
        frame: f32,
        base: &HashMap<usize, Quat>,
        out: &mut Vec<Option<Mat4>>,
        depth: usize,
    ) -> Mat4 {
        if let Some(m) = out[i] {
            return m;
        }
        let node = &self.nodes[i];
        let track = clip.and_then(|c| c.tracks.get(i)).and_then(|t| t.as_ref());
        let mut rot = match track {
            Some(t) => t.rotation_at(frame),
            None => Quat::IDENTITY,
        };
        if let Some(b) = base.get(&i) {
            rot = (*b * rot).normalized();
        }
        let is_root = node.parent.is_none();
        let default_t = if is_root {
            node.center
        } else {
            node.relative_center
        };
        let trans = track
            .and_then(|t| t.position_at(frame))
            .unwrap_or(default_t);
        let local = Mat4::translation(trans).mul(&Mat4::from_quat(rot));
        // Parent chains in shipped data are shallow; the guard stops a malformed cycle.
        let accum = match node.parent {
            Some(p) if depth < 64 && p != i => self
                .pose_node(p, clip, frame, base, out, depth + 1)
                .mul(&local),
            _ => local,
        };
        out[i] = Some(accum);
        accum
    }
}

/// Keyframes of one node.
#[derive(Clone, Debug)]
pub struct Track {
    pub rotations: Vec<Quat>,
    pub positions: Vec<Vec3>,
}

impl Track {
    pub fn rotation_at(&self, frame: f32) -> Quat {
        sample(&self.rotations, frame, |a, b, t| a.slerp(b, t)).unwrap_or(Quat::IDENTITY)
    }
    pub fn position_at(&self, frame: f32) -> Option<Vec3> {
        sample(&self.positions, frame, |a, b, t| a.lerp(b, t))
    }
}

fn sample<T: Copy>(keys: &[T], frame: f32, mix: impl Fn(T, T, f32) -> T) -> Option<T> {
    if keys.is_empty() {
        return None;
    }
    let f = frame.max(0.0);
    let i = (f.floor() as usize).min(keys.len() - 1);
    let j = (i + 1).min(keys.len() - 1);
    Some(mix(keys[i], keys[j], f - f.floor()))
}

/// One AGL file bound to a model.
#[derive(Clone, Debug)]
pub struct AnimationClip {
    pub frames: usize,
    /// Playback rate in frames per second (`frameRate`).
    pub frame_rate: f32,
    pub tracks: Vec<Option<Track>>,
}

impl AnimationClip {
    /// `TG_AnimateShape::SetAnimationState`: tracks are matched to nodes by id; tracks whose node
    /// is missing (an arm shot off, a different LOD) are dropped.
    pub fn from_agl(model: &ShapeModel, anims: &[Animation]) -> Self {
        let mut tracks: Vec<Option<Track>> = vec![None; model.nodes.len()];
        let mut frames = 0;
        let mut frame_rate = 30.0;
        for a in anims {
            frames = frames.max(a.quaternions.len()).max(a.positions.len());
            if a.frame_rate > 0.0 {
                frame_rate = a.frame_rate;
            }
            if let Some(i) = model.node_index(&a.node_id) {
                tracks[i] = Some(Track {
                    rotations: a.quaternions.iter().map(|q| Quat::from_array(*q)).collect(),
                    positions: a.positions.iter().map(|p| Vec3::from_array(*p)).collect(),
                });
            }
        }
        AnimationClip {
            frames,
            frame_rate,
            tracks,
        }
    }

    pub fn load(game: &mut GameData, model: &ShapeModel, name: &str) -> crate::Result<Self> {
        let bytes = game.read(&format!("data/tgl/{name}.agl"))?;
        let anims = tgl::decode_animation(&bytes)
            .map_err(|e| crate::RenderError::Other(format!("{name}.agl: {e}")))?;
        Ok(Self::from_agl(model, &anims))
    }

    /// Frame number after `seconds` of looping playback.
    pub fn frame_at(&self, seconds: f32) -> f32 {
        if self.frames <= 1 {
            return 0.0;
        }
        (seconds * self.frame_rate).rem_euclid(self.frames as f32)
    }
}

/// Instance matrices: the shape origin of `TG_MultiShape::TransformMultiShape`
/// (`BuildRotation(yaw)`, `BuildTranslation(position)` in the Stuff frame) applied on top of the
/// node pose, then converted to game world coordinates.
///
/// `yaw_degrees` is the object's `rotation`; `world_position` its game-world position.
pub fn instance_node_matrices(pose: &[Mat4], world_position: Vec3, yaw_degrees: f32) -> Vec<Mat4> {
    let yaw = yaw_degrees.to_radians();
    // `EulerAngles(0, yaw, 0)` is Stuff's row-vector matrix with (0,0)=cos, (0,2)=-sin,
    // (2,0)=sin: `x' = x cos + z sin`, `z' = -x sin + z cos`, which is exactly `Mat4::rotation_y(yaw)`
    // for column vectors.  See the test below for the world-space meaning.
    let origin = Mat4::translation(crate::math::world_to_stuff().transform_point(world_position))
        .mul(&Mat4::rotation_y(yaw));
    let to_world = stuff_to_world();
    pose.iter().map(|m| to_world.mul(&origin.mul(m))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_assets::tgl::{Triangle, TypeNode, TypeShape, Vertex};

    fn tri_shape(offset: [f32; 3]) -> TypeShape {
        let v = |p: [f32; 3]| Vertex {
            position: [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]],
            normal: [0.0, 1.0, 0.0],
            argb_light: 0,
        };
        TypeShape {
            vertices: vec![v([0.0, 0.0, 0.0]), v([1.0, 0.0, 0.0]), v([0.0, 0.0, 1.0])],
            triangles: vec![Triangle {
                vertices: [0, 1, 2],
                texture: 0,
                render_flags: 0,
                uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                face_normal: [0.0, 1.0, 0.0],
            }],
            textures: vec![],
        }
    }

    fn node(id: &str, parent: &str, rel: [f32; 3], shape: Option<TypeShape>) -> TypeNode {
        TypeNode {
            node_id: id.into(),
            parent_id: parent.into(),
            node_center: rel,
            relative_node_center: rel,
            shape,
            parent_index: None,
        }
    }

    fn two_node_model() -> ShapeModel {
        let mut nodes = vec![
            node("root", "", [0.0, 10.0, 0.0], Some(tri_shape([0.0; 3]))),
            node("arm", "root", [5.0, 0.0, 0.0], Some(tri_shape([0.0; 3]))),
        ];
        nodes[1].parent_index = Some(0);
        ShapeModel::from_multishape(
            "t",
            &MultiShape {
                textures: vec![tgl::NamedTexture {
                    name: "C:\\art\\Foo.tga".into(),
                    node_index: u32::MAX,
                    gos_handle: u32::MAX,
                    alpha: false,
                }],
                nodes,
            },
        )
    }

    #[test]
    fn texture_names_resolve_to_the_128_folder() {
        assert_eq!(
            texture_path("\\\\aas1\\MC2\\Art\\MC2_MECHS\\Werewolf\\WerewolfRGB.tga"),
            "data/tgl/128/werewolfrgb.tga"
        );
        assert_eq!(
            texture_path("A_Tinshack.tga"),
            "data/tgl/128/a_tinshack.tga"
        );
    }

    #[test]
    fn paint_scheme_recolours_pure_channels_only() {
        // pure red 255, pure green 128, pure blue 64, black, and a mixed pixel
        let mut px = vec![
            255, 0, 0, 255, //
            0, 128, 0, 255, //
            0, 0, 64, 255, //
            0, 0, 0, 255, //
            10, 20, 30, 255,
        ];
        apply_paint_scheme(
            &mut px,
            0xff80_4020,
            0xff00_ff00,
            0xffff_ffff,
            PaintRoutine::Mech,
        );
        // red 255: k = 1 - (1 - 255/256)^2 ~ 1.  The Windows build uses the colour as passed
        // (no `BGR`, mech3d.cpp:1738-1741): 0x804020 is R, G, B = 128, 64, 32.
        assert_eq!(&px[0..3], &[127, 63, 31]);
        // green 128: k = 1 - 0.25 = 0.75
        assert_eq!(&px[4..7], &[0, 191, 0]);
        // blue 64: k = 1 - 0.5625 = 0.4375
        assert_eq!(&px[8..11], &[111, 111, 111]);
        // black is claimed by the red branch with factor 0
        assert_eq!(&px[12..15], &[0, 0, 0]);
        // mixed colours are left alone
        assert_eq!(&px[16..19], &[10, 20, 30]);
    }

    #[test]
    fn home_scheme_masks_match_the_hand_derived_c_values() {
        // The home commander's scheme: prefs highlight 0x0007083e and base 0x00fae525 (the real
        // options.cfg values), laid out as `resetPaintScheme(highlight, highlight, base)`.
        // The Windows build uses them as passed (no `BGR`, mech3d.cpp:1738-1741): highlight
        // R,G,B = 7,8,62 (dark blue) and base R,G,B = 250,229,37 (yellow).  mech3d.cpp:1646-1662
        // gives k(v) = 1 - (1 - v/256)^2 and truncates each channel `c * k` to a byte.  k(255) = 1 - (1/256)^2 = 0.9999847412109375, so every
        // full-strength channel keeps its value minus one step:
        //   highlight (7, 8, 62) -> (6, 7, 61)
        //   base      (250, 229, 37) -> (249, 228, 36)
        let mut px = vec![
            255, 0, 0, 255, // pure red   -> highlight
            0, 255, 0, 255, // pure green -> highlight (highlight 2)
            0, 0, 255, 255, // pure blue  -> base
        ];
        apply_paint_scheme(
            &mut px,
            0x0007_083e,
            0x0007_083e,
            0x00fa_e525,
            PaintRoutine::Mech,
        );
        assert_eq!(&px[0..4], &[6, 7, 61, 255]);
        assert_eq!(&px[4..8], &[6, 7, 61, 255]);
        assert_eq!(&px[8..12], &[249, 228, 36, 255]);
    }

    #[test]
    fn vehicle_paint_keeps_the_source_alpha_mech_paint_is_opaque() {
        // A translucent mask: pure red, pure green (alpha 100 and 30), and a mixed pixel (50).
        let src = [255, 0, 0, 100, 0, 128, 0, 30, 10, 20, 30, 50];
        let mut mech = src.to_vec();
        apply_paint_scheme(
            &mut mech,
            0xffff_ffff,
            0xffff_ffff,
            0xffff_ffff,
            PaintRoutine::Mech,
        );
        // `setPaintScheme` writes 0xff over the painted pixels; the mixed pixel is kept as it was
        assert_eq!([mech[3], mech[7], mech[11]], [255, 255, 50]);
        let mut vehicle = src.to_vec();
        apply_paint_scheme(
            &mut vehicle,
            0xffff_ffff,
            0xffff_ffff,
            0xffff_ffff,
            PaintRoutine::Vehicle,
        );
        // `gvactor.cpp:1143,1168`: the painted pixels keep the alpha they had
        assert_eq!([vehicle[3], vehicle[7], vehicle[11]], [100, 30, 50]);
        // and the colour is painted all the same: white at full strength (255 -> 254), white at
        // green 128 (k = 0.75 -> 191), the mixed pixel untouched
        assert_eq!(&vehicle[0..3], &[254, 254, 254]);
        assert_eq!(&vehicle[4..7], &[191, 191, 191]);
        assert_eq!(&vehicle[8..11], &[10, 20, 30]);
    }

    #[test]
    fn vehicle_paint_takes_the_colours_unswapped_like_the_mech() {
        // `GVAppearance::setPaintScheme` (gvactor.cpp:1228-1240) takes the same `#else` branch on
        // Windows: the prefs paint a vehicle yellow and dark blue too, not the BGR build's teal.
        // The prefs default 0xffff7e00 (prefs.cpp:73) is orange, R,G,B = 255,126,0.
        let mut px = vec![255, 0, 0, 200, 0, 255, 0, 200, 0, 0, 255, 200];
        apply_paint_scheme(
            &mut px,
            0x0007_083e,
            0xffff_7e00,
            0x00fa_e525,
            PaintRoutine::Vehicle,
        );
        assert_eq!(&px[0..4], &[6, 7, 61, 200]);
        assert_eq!(&px[4..8], &[254, 125, 0, 200]);
        assert_eq!(&px[8..12], &[249, 228, 36, 200]);
    }

    #[test]
    fn alpha_textures_are_the_a_prefixed_ones() {
        assert!(texture_has_alpha(
            "C:\\Art\\Scenes\\TinShack\\A_Tinshack.tga"
        ));
        assert!(texture_has_alpha("a_palm.tga"));
        assert!(!texture_has_alpha(
            "\\\\aas1\\MC2\\Werewolf\\WerewolfRGB.tga"
        ));
        assert!(!texture_has_alpha("x"));
        // the shape built from a model flags its batches with this rule
        let m = two_node_model();
        assert_eq!(m.texture_alpha, vec![false]);
    }

    #[test]
    fn node_kinds_follow_the_name_prefix() {
        assert_eq!(node_kind("SpotLight_Building"), NodeKind::Spotlight);
        assert_eq!(node_kind("spotlight_x"), NodeKind::Spotlight);
        assert_eq!(node_kind("LitWin_TinShack"), NodeKind::Window);
        assert_eq!(node_kind("Spotlight_WW"), NodeKind::Spotlight);
        assert_eq!(node_kind("TinShack"), NodeKind::Normal);
        assert_eq!(node_kind("Site_Smoke1"), NodeKind::Normal);
    }

    #[test]
    fn mesh_is_unwelded_and_batched_per_texture() {
        let m = two_node_model();
        assert_eq!(m.vertices.len(), 6);
        assert_eq!(m.batches.len(), 1);
        assert_eq!(m.batches[0].vertex_count, 6);
        assert_eq!(m.vertices[3].node, 1);
        assert_eq!(m.vertices[1].uv, [1.0, 0.0]);
    }

    #[test]
    fn bind_pose_accumulates_pivots_down_the_hierarchy() {
        let m = two_node_model();
        let pose = m.bind_pose();
        // root pivot is its absolute centre, the arm sits 5 units further along x
        assert_eq!(
            pose[0].transform_point(Vec3::ZERO),
            Vec3::new(0.0, 10.0, 0.0)
        );
        assert_eq!(
            pose[1].transform_point(Vec3::ZERO),
            Vec3::new(5.0, 10.0, 0.0)
        );
        // bounds cover both triangles
        assert_eq!(m.bounds.0.y, 10.0);
        assert_eq!(m.bounds.1.x, 6.0);
    }

    #[test]
    fn rotating_the_parent_swings_the_child_around_its_pivot() {
        let m = two_node_model();
        let q = Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), std::f32::consts::FRAC_PI_2);
        let clip = AnimationClip {
            frames: 1,
            frame_rate: 30.0,
            tracks: vec![
                Some(Track {
                    rotations: vec![q],
                    positions: vec![],
                }),
                None,
            ],
        };
        let pose = m.pose(Some(&clip), 0.0, &HashMap::new());
        // The root keeps its pivot; the arm pivot (5,0,0) rotates a quarter turn about z => (0,5,0)
        let arm = pose[1].transform_point(Vec3::ZERO);
        assert!(
            (arm.x - 0.0).abs() < 1e-4 && (arm.y - 15.0).abs() < 1e-4,
            "{arm:?}"
        );
        // the vertex (1,0,0) of the root also turns to (0,1,0) around the pivot
        let v = pose[0].transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert!((v.x - 0.0).abs() < 1e-4 && (v.y - 11.0).abs() < 1e-4);
    }

    #[test]
    fn animation_frames_interpolate_between_keys() {
        let q0 = Quat::IDENTITY;
        let q1 = Quat::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), 1.0);
        let t = Track {
            rotations: vec![q0, q1],
            positions: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0)],
        };
        let mid = t.rotation_at(0.5);
        let want = Quat::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), 0.5);
        assert!((mid.y - want.y).abs() < 1e-4);
        assert_eq!(t.position_at(0.25).unwrap(), Vec3::new(2.5, 0.0, 0.0));
        // past the end clamps to the last key
        assert_eq!(t.position_at(9.0).unwrap(), Vec3::new(10.0, 0.0, 0.0));
        let empty = Track {
            rotations: vec![],
            positions: vec![],
        };
        assert_eq!(empty.rotation_at(3.0), Quat::IDENTITY);
        assert!(empty.position_at(3.0).is_none());
    }

    #[test]
    fn clips_match_nodes_by_name_ignoring_case_and_wrap() {
        let m = two_node_model();
        let anim = Animation {
            node_id: "ARM".into(),
            shape_id: -1,
            frame_rate: 30.0,
            tick_rate: 160.0,
            quaternions: vec![[0.0, 0.0, 0.0, 1.0]; 31],
            positions: vec![],
            frames: 31,
        };
        let missing = Animation {
            node_id: "gone".into(),
            ..anim.clone()
        };
        let clip = AnimationClip::from_agl(&m, &[anim, missing]);
        assert!(clip.tracks[1].is_some() && clip.tracks[0].is_none());
        assert_eq!(clip.frames, 31);
        assert!((clip.frame_at(1.0) - 30.0).abs() < 1e-4);
        assert!((clip.frame_at(31.0 / 30.0) - 0.0).abs() < 1e-3);
    }

    #[test]
    fn instance_yaw_turns_counter_clockwise_in_the_world() {
        // A point in front of the model (Stuff +z = world +y after the basis change) turns to
        // world (-sin yaw, cos yaw): counter-clockwise from north, like the camera's yaw.
        let pose = vec![Mat4::IDENTITY];
        let m = instance_node_matrices(&pose, Vec3::new(100.0, 200.0, 30.0), 90.0);
        let p = m[0].transform_point(Vec3::new(0.0, 0.0, 1.0));
        // yaw 90: forward (0,0,1)_stuff -> (sin90,0,cos90)_stuff = (1,0,0)_stuff -> world (-1, 0)
        assert!(
            (p.x - 99.0).abs() < 1e-4 && (p.y - 200.0).abs() < 1e-4,
            "{p:?}"
        );
        assert!((p.z - 30.0).abs() < 1e-4);
        // zero yaw keeps forward along world +y
        let m = instance_node_matrices(&pose, Vec3::ZERO, 0.0);
        let p = m[0].transform_point(Vec3::new(0.0, 0.0, 1.0));
        assert!((p.y - 1.0).abs() < 1e-5 && p.x.abs() < 1e-5);
        // Stuff up stays world up
        let p = m[0].transform_point(Vec3::new(0.0, 1.0, 0.0));
        assert!((p.z - 1.0).abs() < 1e-5);
    }

    #[test]
    fn mech_selection_box_is_world_axis_aligned_and_ignores_draw_yaw() {
        // Mech3DAppearance stores a shape-derived box as `(x, z, y)` and projects it
        // directly from `position`; unlike the building/GV paths it has no `Rotate` call. The
        // derived box is expanded by 1.25 before that axis reorder (`mech3d.cpp:1282-1299`,
        // `2173-2203`). A `[3DBounds]` override is already in those world axes
        // (`apprtype.cpp:80-121`) and is not expanded.
        let mut model = two_node_model();
        model.bounds = (Vec3::new(-4.0, 2.0, -6.0), Vec3::new(8.0, 10.0, 12.0));
        let position = Vec3::new(100.0, 200.0, 30.0);
        let derived = model.mech_selection_box_world(position);
        assert!(derived.contains(&Vec3::new(95.0, 192.5, 32.5)));
        assert!(derived.contains(&Vec3::new(110.0, 215.0, 42.5)));

        let designer = ShapeModel::mech_designer_selection_box_world(
            (Vec3::new(-9.0, -8.0, -7.0), Vec3::new(6.0, 5.0, 4.0)),
            position,
        );
        assert!(designer.contains(&Vec3::new(91.0, 192.0, 23.0)));
        assert!(designer.contains(&Vec3::new(106.0, 205.0, 34.0)));
    }

    #[test]
    fn derived_vehicle_box_expands_but_designer_box_does_not() {
        // `GVAppearance::init` scales a mesh-derived type box by EXPAND_FACTOR 1.25
        // (`gvactor.cpp:968-980`), while `[3DBounds]` is a pre-authored replacement.
        let mut model = two_node_model();
        model.bounds = (Vec3::new(-4.0, -2.0, -6.0), Vec3::new(8.0, 10.0, 12.0));
        let position = Vec3::new(100.0, 200.0, 30.0);
        let derived = model.vehicle_selection_box_world(position, 0.0);
        assert!(derived.contains(&Vec3::new(90.0, 215.0, 42.5)));
        let designer = model.selection_box_world_with_bounds(model.bounds, position, 0.0);
        assert!(designer.contains(&Vec3::new(92.0, 212.0, 40.0)));
        assert!(!designer.contains(&Vec3::new(90.0, 215.0, 42.5)));
    }

    #[test]
    fn vehicle_and_tree_boxes_combine_rotated_endpoints() {
        // GV and tree appearance code rotates only minBox and maxBox before it combines their
        // coordinates, instead of rotating eight source corners (gvactor.cpp:1646-1692,
        // bdactor.cpp:3693-3729).  The two boxes visibly differ at 45 degrees.
        let mut model = two_node_model();
        model.bounds = (Vec3::new(-20.0, -3.0, -5.0), Vec3::new(10.0, 7.0, 30.0));
        let at = Vec3::new(100.0, 200.0, 30.0);
        let cxx = model.selection_box_world_from_rotated_endpoints(model.bounds, at, 45.0);
        let all_corners = model.selection_box_world_with_bounds(model.bounds, at, 45.0);
        assert_ne!(cxx, all_corners);
        // Every endpoint-combined corner is still based only on the two rotated endpoints.
        assert_eq!(cxx.len(), 8);
    }

    #[test]
    fn mech_box_applies_the_animated_root_height_to_every_corner() {
        // `baseRootNodeDifference` is added to every projected selection-box point before the
        // final screen test (mech3d.cpp:2153-2203), which is what keeps a jumping mech at an
        // edge from retaining its previous LOD.
        let mut model = two_node_model();
        model.bounds = (Vec3::new(-4.0, -2.0, -6.0), Vec3::new(8.0, 10.0, 12.0));
        let at = Vec3::new(100.0, 200.0, 30.0);
        let rest = model.mech_selection_box_world(at);
        let raised = model.mech_selection_box_world_at_height(at, 75.0);
        for (a, b) in rest.into_iter().zip(raised) {
            assert!((b.x - a.x).abs() < 1e-6 && (b.y - a.y).abs() < 1e-6);
            assert!((b.z - a.z - 75.0).abs() < 1e-6);
        }
    }
}
