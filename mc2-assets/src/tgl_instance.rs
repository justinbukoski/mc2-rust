// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Headless TGL multi-shape instances: the node hierarchy, animation binding and node transforms of `TG_MultiShape`
//! (`mclib/msl.cpp`), without vertices, lighting, clipping or rendering.
//!
//! The simulation reads node positions (weapon muzzles, hit nodes, the root node height) from the matrices that
//! `TG_MultiShape::TransformMultiShape` (msl.cpp:1287) leaves in each `TG_ShapeRec::shapeToWorld`. This module ports
//! exactly that matrix part:
//! * [`ShapeType`] is a `TG_TypeMultiShape` reduced to node ids, centres and the parent links `CreateFrom` builds
//!   (msl.cpp:118: exact `strcmp` of parent id against node id, first match);
//! * [`AnimSet`] is a `TG_AnimateShape` (one gesture of animation data), including its quirk of caching the node index of
//!   every animated node on the *first* `SetAnimationState` call and reusing it for every shape afterwards (msl.cpp:2512);
//! * [`ShapeInstance`] is a `TG_MultiShape`: per-node matrices, the `calcedThisFrame` turn stamp (a node's matrices are
//!   computed at most once per turn, so a second transform in the same turn changes nothing), current animation, base
//!   (torso/turret) rotation and frame number.
//!
//! Stuff math comes from [`crate::stuff`]; results are bit-identical to the C++ on the same platform.

use crate::stuff::{quat_multiply, quat_normalize, Mat, Quat};
use crate::tgl::{Animation, MultiShape};

/// Case-insensitive name compare (`S_stricmp`).
fn stricmp_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// One node of a [`ShapeType`].
#[derive(Clone, Debug, PartialEq)]
pub struct TypeNode {
    /// `nodeId`.
    pub id: String,
    /// `parentId`.
    pub parent_id: String,
    /// `GetNodeCenter()` (used for an unanimated or position-less root).
    pub center: [f32; 3],
    /// `GetRelativeNodeCenter()` (used for an unanimated or position-less child).
    pub relative_center: [f32; 3],
    /// `CreateFrom` parent link: first node whose id equals `parent_id` byte for byte.
    pub parent: Option<usize>,
    /// `listOfTypeVertices[].position` (empty for a `TYPE_NODE`); used by the building footprint/LOS marking.
    pub vertices: Vec<[f32; 3]>,
}

/// `TG_TypeMultiShape` as far as node transforms need it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeType {
    /// Nodes in file order (`listOfTypeShapes`).
    pub nodes: Vec<TypeNode>,
}

impl ShapeType {
    /// Builds the node table from a decoded `.tgl` file.
    pub fn from_multishape(shape: &MultiShape) -> ShapeType {
        let mut nodes: Vec<TypeNode> = shape
            .nodes
            .iter()
            .map(|n| TypeNode {
                id: n.node_id.clone(),
                parent_id: n.parent_id.clone(),
                center: n.node_center,
                relative_center: n.relative_node_center,
                parent: None,
                vertices: n
                    .shape
                    .as_ref()
                    .map(|sh| sh.vertices.iter().map(|v| v.position).collect())
                    .unwrap_or_default(),
            })
            .collect();
        for i in 0..nodes.len() {
            let parent = nodes.iter().position(|m| m.id == nodes[i].parent_id);
            nodes[i].parent = parent;
        }
        ShapeType { nodes }
    }

    /// `GetNumShapes()`.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when the shape has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// `TG_TypeMultiShape::minBox` / `maxBox` (msl.cpp:240-275, computed by `LoadBinaryCopy`): the extent of every vertex
    /// plus its node's relative centre, over nodes whose id does not start with `_PAB`, `LOS_` or `SpotLight`
    /// (case-insensitive). Starts at +-100000.
    pub fn bounding_box(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [100000.0f32; 3];
        let mut max = [-100000.0f32; 3];
        let skip = |id: &str, p: &str| {
            id.len() >= p.len() && id.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
        };
        for n in &self.nodes {
            if skip(&n.id, "_PAB") || skip(&n.id, "LOS_") || skip(&n.id, "SpotLight") {
                continue;
            }
            let rel = n.relative_center;
            for v in &n.vertices {
                for k in 0..3 {
                    if min[k] > v[k] + rel[k] {
                        min[k] = v[k] + rel[k];
                    }
                    if max[k] < v[k] + rel[k] {
                        max[k] = v[k] + rel[k];
                    }
                }
            }
        }
        (min, max)
    }

    /// `TG_MultiShape::GetRootNodeCenter()` (msl.h:461): the centre of the *last* node without a parent.
    pub fn root_node_center(&self) -> [f32; 3] {
        let mut r = [0.0; 3];
        for n in &self.nodes {
            if n.parent.is_none() {
                r = n.center;
            }
        }
        r
    }

    /// `TG_MultiShape::GetShapeVertexInEditor(shape, vertex, rotation)` (msl.cpp:983): the vertex relative to the
    /// object, from the node's *own* relative centre plus the root centre (no hierarchy), turned by `rotation` degrees.
    pub fn shape_vertex_in_editor(&self, shape: usize, vertex: usize, rotation: f32) -> [f32; 3] {
        let Some(n) = self.nodes.get(shape) else {
            return [0.0; 3];
        };
        let Some(v) = n.vertices.get(vertex) else {
            return [0.0; 3];
        };
        let root = self.root_node_center();
        let rel = n.relative_center;
        let mut p = [
            -(v[0] + rel[0] + root[0]),
            v[2] + rel[2] + root[2],
            v[1] + rel[1] + root[1],
        ];
        if rotation != 0.0 {
            crate::stuff::rotate_xy(&mut p, rotation);
        }
        p
    }

    /// `TG_MultiShape::GetNodeNameId` (msl.h:545): first node with this id (case-insensitive), or -1.
    pub fn node_name_id(&self, name: &str) -> i32 {
        self.nodes
            .iter()
            .position(|n| stricmp_eq(&n.id, name))
            .map_or(-1, |i| i as i32)
    }

    /// Chain from `i` up to its root (`childChain` in msl.cpp: index 0 is the node itself). The C++ asserts the depth
    /// stays below `MAX_NODES`; a parent cycle (impossible in shipped data) is cut at the node count.
    fn chain(&self, i: usize) -> Vec<usize> {
        let mut chain = vec![i];
        while let Some(p) = self.nodes[*chain.last().unwrap()].parent {
            if chain.len() > self.nodes.len() {
                break;
            }
            chain.push(p);
        }
        chain
    }

    /// `TG_MultiShape::isChildOf(nodeName, parentName)` (msl.cpp:1904): `nodeName` is `parentName` or one of its
    /// descendants.
    pub fn is_child_of(&self, node_name: &str, parent_name: &str) -> bool {
        (0..self.nodes.len()).any(|i| {
            stricmp_eq(&self.nodes[i].id, node_name)
                && self
                    .chain(i)
                    .iter()
                    .any(|&j| stricmp_eq(&self.nodes[j].id, parent_name))
        })
    }
}

/// `TG_AnimateShape`: the per-node animation of one gesture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimSet {
    /// `listOfAnimation`.
    pub entries: Vec<Animation>,
    /// `shapeIdsSet` plus every entry's `shapeId` (None = -1, node missing).
    pub shape_ids: Option<Vec<Option<usize>>>,
}

impl AnimSet {
    /// Wraps decoded `.agl` entries.
    pub fn new(entries: Vec<Animation>) -> AnimSet {
        AnimSet {
            entries,
            shape_ids: None,
        }
    }

    /// `GetFrameRate()` (msl.h:625): the first entry's rate, 30 when there are no entries.
    pub fn frame_rate(&self) -> f32 {
        self.entries.first().map_or(30.0, |a| a.frame_rate)
    }

    /// `ReverseFrameRate()` (msl.h:637): negates the first entry's rate.
    pub fn reverse_frame_rate(&mut self) {
        if let Some(a) = self.entries.first_mut() {
            a.frame_rate = -a.frame_rate;
        }
    }

    /// `GetNumFrames()` (msl.h:643): the first entry's frame count, 0 when there are no entries.
    pub fn num_frames(&self) -> i32 {
        self.entries.first().map_or(0, |a| a.frames as i32)
    }
}

/// One `TG_ShapeRec`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeRec {
    /// `localShapeToWorld`.
    pub local: Mat,
    /// `shapeToWorld`.
    pub shape_to_world: Mat,
    /// `calcedThisFrame` (an `int32_t` turn stamp; -1 after `CreateFrom`).
    pub calced: i32,
    /// `currentAnimation`: (animation set index, entry index) in the owner's animation table.
    pub anim: Option<(usize, usize)>,
    /// `baseRotation`.
    pub base_rotation: Quat,
    /// `processMe` (false for nodes cut off by `StopUsing`; transforms still run, only drawing stops).
    pub process_me: bool,
}

/// `TG_MultiShape` (node state only).
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeInstance {
    /// `listOfShapes`.
    pub recs: Vec<ShapeRec>,
    /// `frameNum` (`SetFrameNum`).
    pub frame_num: f32,
}

impl ShapeInstance {
    /// `TG_TypeMultiShape::CreateFrom()` (msl.cpp:118): identity matrices, no animation, identity base rotation.
    pub fn create_from(ty: &ShapeType) -> ShapeInstance {
        ShapeInstance {
            recs: (0..ty.len())
                .map(|_| ShapeRec {
                    local: Mat::IDENTITY,
                    shape_to_world: Mat::IDENTITY,
                    calced: -1,
                    anim: None,
                    base_rotation: Quat::IDENTITY,
                    process_me: true,
                })
                .collect(),
            frame_num: 0.0,
        }
    }

    /// `ClearAnimation()`.
    pub fn clear_animation(&mut self) {
        for r in &mut self.recs {
            r.anim = None;
        }
    }

    /// `TG_AnimateShape::SetAnimationState(shape)` (msl.cpp:2512) for animation set `set_index` of the owner's table.
    /// The first call ever on `set` resolves node ids against `ty` and caches them in `set`; later calls (on any shape,
    /// also another LOD with a different node order) reuse the cached indices.
    pub fn set_animation_state(&mut self, ty: &ShapeType, set: &mut AnimSet, set_index: usize) {
        self.clear_animation();
        if set.shape_ids.is_none() {
            let mut ids = Vec::with_capacity(set.entries.len());
            for (j, a) in set.entries.iter().enumerate() {
                let found = (0..ty.len()).find(|&i| stricmp_eq(&a.node_id, &ty.nodes[i].id));
                if let Some(i) = found {
                    self.set_current_animation(i, set_index, j);
                }
                ids.push(found);
            }
            set.shape_ids = Some(ids);
        } else if let Some(ids) = &set.shape_ids {
            for (j, id) in ids.iter().enumerate() {
                if let Some(i) = *id {
                    self.set_current_animation(i, set_index, j);
                }
            }
        }
    }

    /// `SetCurrentAnimation(shapeNum, anim)` (msl.h:478): ignored when out of range.
    fn set_current_animation(&mut self, shape: usize, set_index: usize, entry: usize) {
        if let Some(r) = self.recs.get_mut(shape) {
            r.anim = Some((set_index, entry));
        }
    }

    /// `SetNodeRotation(const char*, rot)` (msl.cpp:965): first node with this id gets the base rotation; returns its
    /// index or -1.
    pub fn set_node_rotation_by_name(&mut self, ty: &ShapeType, name: &str, rot: &Quat) -> i32 {
        for i in 0..self.recs.len().min(ty.len()) {
            if stricmp_eq(&ty.nodes[i].id, name) {
                self.recs[i].base_rotation = *rot;
                return i as i32;
            }
        }
        -1
    }

    /// `SetNodeRotation(long, rot)` (msl.h:302): ignored when out of range.
    pub fn set_node_rotation(&mut self, index: i32, rot: &Quat) {
        if index >= 0 {
            if let Some(r) = self.recs.get_mut(index as usize) {
                r.base_rotation = *rot;
            }
        }
    }

    /// `GetTransformedNodePosition(pos, rot, long nodeId)` (msl.cpp:1108): the node's last computed translation in
    /// simulation coordinates, (0, 0, 0) when the index is out of range.
    pub fn node_position(&self, index: i32) -> [f32; 3] {
        if index >= 0 {
            if let Some(r) = self.recs.get(index as usize) {
                let e = &r.shape_to_world.0;
                return [-e[3], e[11], e[7]];
            }
        }
        [0.0, 0.0, 0.0]
    }

    /// `GetTransformedNodePosition(pos, rot, const char *nodeId)` (msl.cpp:1071): by name (case-insensitive).
    pub fn node_position_by_name(&self, ty: &ShapeType, name: &str) -> [f32; 3] {
        self.node_position(ty.node_name_id(name))
    }

    /// The matrix half of `TG_MultiShape::TransformMultiShape(pos, rot)` (msl.cpp:1287-1590). `pos`/`rot` are the
    /// renderer-space origin (`x = -position.x, y = position.z, z = position.y`) and orientation; `anims` is the owner's
    /// animation table that [`ShapeRec::anim`] indexes. Nodes already computed in `turn` are left alone.
    pub fn transform(
        &mut self,
        ty: &ShapeType,
        anims: &[Option<AnimSet>],
        pos: [f32; 3],
        rot: &Quat,
        turn: i64,
    ) {
        let mut shape_origin = Mat::IDENTITY;
        shape_origin.build_rotation_quat(rot);
        shape_origin.build_translation(pos);
        let f_num = self.frame_num as i32;
        let n = self.recs.len().min(ty.len());
        for i in 0..n {
            let chain = ty.chain(i);
            let cur_child = chain.len() - 1;
            for j in (0..=cur_child).rev() {
                let node = chain[j];
                if i64::from(self.recs[node].calced) == turn {
                    continue;
                }
                let anim = self.recs[node].anim.and_then(|(s, e)| {
                    anims
                        .get(s)
                        .and_then(|a| a.as_ref())
                        .and_then(|a| a.entries.get(e))
                });
                let base = self.recs[node].base_rotation;
                let mut local = Mat::IDENTITY;
                let is_root = j == cur_child;
                match anim {
                    Some(a) => {
                        let slerp = frame_quat(a, f_num).unwrap_or(Quat::IDENTITY);
                        let mut total = slerp;
                        if !base.is_identity() {
                            total = quat_multiply(&slerp, &base);
                            quat_normalize(&mut total);
                        }
                        local.build_rotation_quat(&total);
                        let t = match frame_pos(a, f_num) {
                            Some(p) => p,
                            None if is_root => ty.nodes[node].center,
                            None => ty.nodes[node].relative_center,
                        };
                        local.build_translation(t);
                    }
                    None if is_root => {
                        local.build_rotation_euler(0.0, 0.0, 0.0);
                        local.build_translation(ty.nodes[node].center);
                    }
                    None => {
                        local.build_rotation_quat(&base);
                        local.build_translation(ty.nodes[node].relative_center);
                    }
                }
                let local_to_world = if is_root {
                    local
                } else {
                    Mat::multiply(&local, &self.recs[chain[j + 1]].local)
                };
                let r = &mut self.recs[node];
                r.local = local_to_world;
                r.shape_to_world = Mat::multiply(&local_to_world, &shape_origin);
                r.calced = turn as i32;
            }
        }
    }

    /// `TG_MultiShape::GetShapeVertexInWorld(shape, vertex, rotation)` (msl.cpp:1016): the vertex through the node's
    /// current `shapeToWorld` (identity until the first transform), in simulation axes, turned by `rotation` degrees.
    pub fn shape_vertex_in_world(
        &self,
        ty: &ShapeType,
        shape: usize,
        vertex: usize,
        rotation: f32,
    ) -> [f32; 3] {
        let (Some(n), Some(rec)) = (ty.nodes.get(shape), self.recs.get(shape)) else {
            return [0.0; 3];
        };
        let Some(v) = n.vertices.get(vertex) else {
            return [0.0; 3];
        };
        let m = &rec.shape_to_world;
        let x = v[0] * m.at(0, 0) + v[1] * m.at(1, 0) + v[2] * m.at(2, 0) + m.at(3, 0);
        let y = v[0] * m.at(0, 1) + v[1] * m.at(1, 1) + v[2] * m.at(2, 1) + m.at(3, 1);
        let z = v[0] * m.at(0, 2) + v[1] * m.at(1, 2) + v[2] * m.at(2, 2) + m.at(3, 2);
        let mut p = [-x, z, y];
        if rotation != 0.0 {
            crate::stuff::rotate_xy(&mut p, rotation);
        }
        p
    }

    /// `StopUsing(nodeName)` (msl.cpp:1721): every descendant of `nodeName` (not the node itself) stops being processed.
    pub fn stop_using(&mut self, ty: &ShapeType, node_name: &str) {
        for i in 0..self.recs.len().min(ty.len()) {
            let chain = ty.chain(i);
            if chain
                .iter()
                .rev()
                .any(|&j| stricmp_eq(&ty.nodes[j].id, node_name))
                && !stricmp_eq(&ty.nodes[i].id, node_name)
            {
                self.recs[i].process_me = false;
            }
        }
    }
}

/// `currentAnimation->quat[fNum]`. The C++ indexes without a bound; a frame past the node's own data (never in shipped
/// data: every entry of an `.agl` has the same frame count) reads nothing here.
fn frame_quat(a: &Animation, f: i32) -> Option<Quat> {
    if f < 0 {
        return None;
    }
    a.quaternions.get(f as usize).map(|q| Quat {
        x: q[0],
        y: q[1],
        z: q[2],
        w: q[3],
    })
}

/// `currentAnimation->pos[fNum]` (None when the entry has no position track).
fn frame_pos(a: &Animation, f: i32) -> Option<[f32; 3]> {
    if a.positions.is_empty() || f < 0 {
        return None;
    }
    a.positions.get(f as usize).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stuff::quat_from_euler;

    fn ty() -> ShapeType {
        let node = |id: &str, parent: &str, c: [f32; 3], rc: [f32; 3]| TypeNode {
            id: id.into(),
            parent_id: parent.into(),
            center: c,
            relative_center: rc,
            parent: None,
            vertices: vec![],
        };
        let mut t = ShapeType {
            nodes: vec![
                node("joint_root", "", [0.0, 10.0, 0.0], [0.0, 10.0, 0.0]),
                node(
                    "joint_torso",
                    "joint_root",
                    [0.0, 15.0, 0.0],
                    [0.0, 5.0, 0.0],
                ),
                node("weapon_a", "joint_torso", [2.0, 15.0, 1.0], [2.0, 0.0, 1.0]),
            ],
        };
        for i in 0..3 {
            let p = t.nodes.iter().position(|m| m.id == t.nodes[i].parent_id);
            t.nodes[i].parent = p;
        }
        t
    }

    #[test]
    fn rest_pose_and_turn_stamp() {
        let t = ty();
        let mut s = ShapeInstance::create_from(&t);
        let rot = quat_from_euler(0.0, 0.0, 0.0);
        s.transform(&t, &[], [-100.0, 5.0, 200.0], &rot, 1);
        assert_eq!(s.node_position(2), [98.0, 201.0, 20.0]);
        // Same turn: nothing recomputed even though the origin moved.
        s.transform(&t, &[], [0.0, 0.0, 0.0], &rot, 1);
        assert_eq!(s.node_position(2), [98.0, 201.0, 20.0]);
        s.transform(&t, &[], [0.0, 0.0, 0.0], &rot, 2);
        assert_eq!(s.node_position(2), [-2.0, 1.0, 15.0]);
        assert_eq!(s.node_position(7), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn torso_rotation_turns_children() {
        let t = ty();
        let mut s = ShapeInstance::create_from(&t);
        let half_turn = quat_from_euler(0.0, std::f32::consts::PI, 0.0);
        assert_eq!(
            s.set_node_rotation_by_name(&t, "JOINT_TORSO", &half_turn),
            1
        );
        s.transform(&t, &[], [0.0, 0.0, 0.0], &Quat::IDENTITY, 1);
        let p = s.node_position(2);
        assert!(
            (p[0] - 2.0).abs() < 1e-5 && (p[1] + 1.0).abs() < 1e-5,
            "{p:?}"
        );
        assert!(t.is_child_of("weapon_a", "joint_torso"));
        assert!(!t.is_child_of("joint_root", "joint_torso"));
    }

    #[test]
    fn animation_ids_are_cached_on_first_use() {
        let t = ty();
        let a = Animation {
            node_id: "WEAPON_A".into(),
            shape_id: -1,
            frame_rate: 30.0,
            tick_rate: 4800.0,
            quaternions: vec![],
            positions: vec![[1.0, 2.0, 3.0]],
            frames: 1,
        };
        let mut set = AnimSet::new(vec![a]);
        let mut s = ShapeInstance::create_from(&t);
        s.set_animation_state(&t, &mut set, 0);
        assert_eq!(s.recs[2].anim, Some((0, 0)));
        // A shape with another node order still gets the cached index 2.
        let mut other = t.clone();
        other.nodes.swap(1, 2);
        let mut s2 = ShapeInstance::create_from(&other);
        s2.set_animation_state(&other, &mut set, 0);
        assert_eq!(s2.recs[2].anim, Some((0, 0)));
        assert_eq!(set.num_frames(), 1);
    }
}
