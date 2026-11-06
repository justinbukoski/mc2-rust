// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Queued waypoint markers and path lines of the selected units.
//!
//! `MissionInterfaceManager::render` calls `MechWarrior::drawWaypointPath` for every selected
//! home mover while the waypoint key (or the mine toggle) is held (`missiongui.cpp:2977-2990`).
//! That draws a red line from the mover to its current order's point, a marker building
//! (`WalkWayPoint` / `RunWayPoint` / `JumpWayPoint`, chosen by the queued move's travel mode,
//! `warrior.cpp:1584-1592`) at each queued point and a red line from each point to the previous
//! one (`warrior.cpp:7407-7473`). A looping queue starts its chain from the last queued point
//! instead of the mover and draws no line to the current order.
//!
//! This module reads the queue from the simulation ([`paths`]), projects the red lines into the
//! overlay ([`draw_lines`]) and builds the marker shapes ([`marker_instance`]).

use crate::overlay::OverlayBatch;
use crate::world_view::argb;
use mc2_render::camera::GameCamera;
use mc2_render::math::Vec3;
use mc2_render::populate::building_instance;
use mc2_render::scene::{SceneRenderer, ShapeInstance};
use mc2_sim::World;

/// Pixel width of a path line: the C++ line is one pixel wide under line rasterisation rules; a triangle strip needs two to stay unbroken.
const LINE_THICKNESS: f32 = 2.0;

/// `LineElement( .., 0xffff0000, ..)` (`warrior.cpp:7436`, `7468`).
pub const LINE_COLOR: u32 = 0xffff_0000;

/// The marker art of a queued move (`TravelModeType`, `warrior.cpp:1584-1592`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MarkerKind {
    /// `TRAVEL_MODE_SLOW`: `WalkWayPoint`.
    Walk,
    /// `TRAVEL_MODE_FAST`: `RunWayPoint`.
    Run,
    /// `TRAVEL_MODE_JUMP`: `JumpWayPoint`.
    Jump,
}

impl MarkerKind {
    /// The appearance (`data/tgl/<name>.ini`) `getWayPointMarker` loads.
    pub fn appearance(self) -> &'static str {
        match self {
            Self::Walk => "WalkWayPoint",
            Self::Run => "RunWayPoint",
            Self::Jump => "JumpWayPoint",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Walk => 0,
            Self::Run => 1,
            Self::Jump => 2,
        }
    }
}

/// What `drawWaypointPath` draws for one selected mover.
#[derive(Clone, Debug, PartialEq)]
pub struct WaypointPath {
    /// The mover's position (`tmpPos`, the chain's start).
    pub start: Vec3,
    /// The current order's point, joined to `start` by a line (absent for a looping queue, and
    /// when the point is unset: `pos.x != 0 && pos.y != 0`).
    pub current: Option<Vec3>,
    /// The queued orders, in order.
    pub queue: Vec<(MarkerKind, Vec3)>,
    /// `tacOrderQueueLooping`.
    pub looping: bool,
}

impl WaypointPath {
    /// The red segments in drawing order (`warrior.cpp:7413-7471`).
    pub fn segments(&self) -> Vec<(Vec3, Vec3)> {
        let mut out = Vec::new();
        let mut prev = self.start;
        if self.looping {
            if let Some((_, last)) = self.queue.last() {
                prev = *last;
            }
        } else if let Some(cur) = self.current {
            out.push((self.start, cur));
            prev = cur;
        }
        for (_, point) in &self.queue {
            out.push((*point, prev));
            prev = *point;
        }
        out
    }
}

/// The marker kind of a queued order from its packed fields (`QueuedOrder::data`, the fields
/// `packed_order` writes): a jump order is `TRAVEL_MODE_JUMP`, otherwise the run bit picks
/// `TRAVEL_MODE_FAST` over `TRAVEL_MODE_SLOW`.
fn marker_kind(data: [u32; 2]) -> MarkerKind {
    const JUMP_TO_POINT: u32 = 4;
    const JUMP_TO_OBJECT: u32 = 5;
    const RUN_BIT: u32 = 1 << 8;
    match data[0] & 0x1f {
        JUMP_TO_POINT | JUMP_TO_OBJECT => MarkerKind::Jump,
        _ if data[1] & RUN_BIT != 0 => MarkerKind::Run,
        _ => MarkerKind::Walk,
    }
}

fn v3(v: mc2_sim::math::Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

/// `pos.z = land->getTerrainElevation(pos)` (`warrior.cpp:7426`): the C++ lays the current order's point on the terrain; the
/// queued points are laid there too, because the Rust move goals carry no height (the C++ goals keep the clicked terrain height).
fn on_terrain(world: &World, mut p: mc2_sim::math::Vec3) -> mc2_sim::math::Vec3 {
    p.z = world.terrain.elevation(&p);
    p
}

/// The paths of the selected home movers (`isSelected() && getCommander() == Commander::home`).
pub fn paths(world: &World, selected: &[i32]) -> Vec<WaypointPath> {
    let mut out = Vec::new();
    for &handle in &world.objects.movers {
        let Some(obj) = world.objects.get(handle) else {
            continue;
        };
        if !selected.contains(&obj.c.part_id.0) {
            continue;
        }
        let Some(slot) = mc2_sim::ai::warrior::slot_of(world, handle) else {
            continue;
        };
        if !mc2_sim::ai::warrior::commander_is_home(world, slot) {
            continue;
        }
        let Some(pilot) = world.pilots.get(slot) else {
            continue;
        };
        let current = pilot
            .cur_order
            .move_params
            .way_path
            .points
            .first()
            .filter(|p| p.x != 0.0 && p.y != 0.0)
            .map(|p| v3(on_terrain(world, *p)));
        out.push(WaypointPath {
            start: v3(obj.c.position),
            current,
            queue: pilot
                .queue
                .iter()
                .map(|q| (marker_kind(q.data), v3(on_terrain(world, q.point))))
                .collect(),
            looping: pilot.queue_looping,
        });
    }
    out
}

/// A `thickness`-pixel line between two pixel positions as two triangles.
fn line(batch: &mut OverlayBatch, a: [f32; 2], b: [f32; 2], thickness: f32, color: [f32; 4]) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0e-3 {
        return;
    }
    let (nx, ny) = (-dy / len * thickness * 0.5, dx / len * thickness * 0.5);
    let p = [
        [a[0] + nx, a[1] + ny],
        [a[0] - nx, a[1] - ny],
        [b[0] - nx, b[1] - ny],
        [b[0] + nx, b[1] + ny],
    ];
    batch.triangle(p[0], p[1], p[2], color);
    batch.triangle(p[0], p[2], p[3], color);
}

/// Draws the red path lines through `camera` (whose viewport is the target's pixels).
pub fn draw_lines(batch: &mut OverlayBatch, camera: &GameCamera, paths: &[WaypointPath]) {
    let color = argb(LINE_COLOR);
    for path in paths {
        for (from, to) in path.segments() {
            let (a, b) = (camera.project_z(from).0, camera.project_z(to).0);
            if [a[0], a[1], b[0], b[1]].iter().all(|v| v.is_finite()) {
                line(batch, [a[0], a[1]], [b[0], b[1]], LINE_THICKNESS, color);
            }
        }
    }
}

/// A marker building placed at `position` (`getWayPointMarker`: rotation 0), from a template
/// cache indexed by kind. `None` when the shipped data has no such appearance.
pub fn marker_instance(
    game: &mut mc2_render::game::GameData,
    scene: &mut SceneRenderer,
    cache: &mut [Option<Option<ShapeInstance>>; 3],
    kind: MarkerKind,
    position: Vec3,
) -> Option<ShapeInstance> {
    let slot = &mut cache[kind.index()];
    let template = slot.get_or_insert_with(|| {
        building_instance(game, scene, kind.appearance(), Vec3::ZERO)
            .ok()
            .flatten()
    });
    let mut inst = template.clone()?;
    inst.position = position;
    Some(inst)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32) -> Vec3 {
        Vec3::new(x, x, 0.0)
    }

    #[test]
    fn segments_chain_from_the_mover_through_the_current_order_and_the_queue() {
        let path = WaypointPath {
            start: p(0.0),
            current: Some(p(1.0)),
            queue: vec![(MarkerKind::Walk, p(2.0)), (MarkerKind::Run, p(3.0))],
            looping: false,
        };
        assert_eq!(
            path.segments(),
            [(p(0.0), p(1.0)), (p(2.0), p(1.0)), (p(3.0), p(2.0))]
        );
    }

    #[test]
    fn a_looping_queue_starts_at_its_last_point_and_skips_the_current_order() {
        let path = WaypointPath {
            start: p(0.0),
            current: Some(p(1.0)),
            queue: vec![(MarkerKind::Walk, p(2.0)), (MarkerKind::Walk, p(3.0))],
            looping: true,
        };
        assert_eq!(path.segments(), [(p(2.0), p(3.0)), (p(3.0), p(2.0))]);
    }

    #[test]
    fn marker_kind_follows_jump_then_run() {
        assert_eq!(marker_kind([2, 0]), MarkerKind::Walk);
        assert_eq!(marker_kind([2, 1 << 8]), MarkerKind::Run);
        assert_eq!(marker_kind([4, 1 << 8]), MarkerKind::Jump);
    }
}
