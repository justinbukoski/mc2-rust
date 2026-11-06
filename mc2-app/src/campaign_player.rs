// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! A deterministic, screen-driven campaign mission player for headless playthrough sweeps.
//!
//! This is deliberately separate from the mission-one P3 driver.  It only observes the HUD
//! snapshot a player can see (objective rows, tac-map contacts, the unit bar, and projected
//! targets) and emits the same mouse/keyboard events a player can make.  It never reads the
//! simulation while deciding what to do; the run report is collected only after play stops.

use crate::headless::{self, HeadlessOptions, HeadlessReport, HeadlessRun, ProductionDriver};
use crate::hud_data::{HudSnapshot, PickTarget};
use crate::script::{Action, InputScript};
use mc2_platform::input::InputEvent;
use mc2_ui::objectives::{ObjectiveStatus, ObjectiveText, ObjectiveView};
use mc2_ui::tacmap::{TacMap, TacTeam, TerrainBounds};
use std::collections::VecDeque;

const TAC_MAP: (i32, i32, i32, i32) = (4, 435, 165, 596);
const WORLD_X: (f32, f32) = (24.0, 776.0);
const WORLD_Y: (f32, f32) = (24.0, 410.0);
const THINK_TURNS: i64 = 30;
// Mission movies and ABL camera work commonly own the opening.  Waiting makes the first player
// gesture match the P3 player's physical-input contract rather than fighting the briefing.
const OPENING_TURNS: i64 = 2_700;
const REISSUE_TURNS: i64 = 360;
/// The viewport centre after `N` has taken the player to the next visible nav marker.
const NAV_VIEW_CENTRE: (f32, f32) = (400.0, 220.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectiveOrder {
    Move,
    ForceFire,
    Guard,
}

/// Player-visible opening objective locations, from `data/missions/mc2_01.fit` and its briefing
/// markers (also cited by `p3_driver.rs`). Other missions use the visible nav-marker fallback.
const OBJECTIVE_HINTS: &[(&str, usize, (f32, f32))] = &[
    ("mc2_01", 0, (3136.0, -789.3)),
    ("mc2_01", 1, (3221.3, -277.3)),
];

/// A message the player could see during the run, grouped at collection time by the sweep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: String,
}

/// The observable result of one campaign-player attempt.
#[derive(Clone, Debug)]
pub struct CampaignPlayerRun {
    pub report: HeadlessReport,
    pub objectives_done: usize,
    pub objectives_total: usize,
    pub notices: Vec<Notice>,
    pub orders_issued: u32,
    pub movement_orders: u32,
    pub force_moved: bool,
    /// The mission ended because the player's whole force was destroyed or disabled
    /// (`Mission::update`, mission.cpp:547-563), whatever the objectives say.
    pub force_lost: bool,
}

/// The generic player state.  All decisions live here, so tests can prove it issues gestures
/// without asking the simulation what those gestures did.
#[derive(Default)]
pub struct CampaignPlayer {
    mission: String,
    queue: VecDeque<(i64, InputEvent)>,
    next_look: i64,
    patrol_index: usize,
    last_attack: Option<(u32, i64)>,
    last_move: Option<((i32, i32), i64)>,
    orders_issued: u32,
    movement_orders: u32,
    observed_start: Option<Vec<(i32, (f32, f32))>>,
    force_moved: bool,
    objective_orders: u32,
}

fn gesture(text: &str) -> Vec<(i64, InputEvent)> {
    InputScript::parse(text)
        .expect("campaign player gesture parses")
        .actions
        .into_iter()
        .filter_map(|(frame, action)| match action {
            Action::Event(event) => Some((frame as i64, event)),
            Action::Resize(_, _) | Action::Screenshot(_) => None,
        })
        .collect()
}

fn tac_px(world: (f32, f32), vertices: i32) -> (f64, f64) {
    let mut map = TacMap::new(TerrainBounds::from_vertices(vertices.max(1)));
    map.set_pos(mc2_ui::draw::GuiRect::new(
        TAC_MAP.0, TAC_MAP.1, TAC_MAP.2, TAC_MAP.3,
    ));
    let (x, y) = map.world_to_tac_map(world.0, world.1);
    (
        f64::from(x).clamp(f64::from(TAC_MAP.0) + 1.0, f64::from(TAC_MAP.2) - 1.0),
        f64::from(y).clamp(f64::from(TAC_MAP.1) + 1.0, f64::from(TAC_MAP.3) - 1.0),
    )
}

fn focus(px: (f64, f64)) -> String {
    format!(
        "0 move {} {}\n1 button left down\n2 button left up\n",
        px.0.round(),
        px.1.round()
    )
}

fn map_move(px: (f64, f64)) -> String {
    format!(
        "0 move {} {}\n1 button right down\n2 button right up\n",
        px.0.round(),
        px.1.round()
    )
}

fn attack_at(point: (f32, f32)) -> String {
    format!(
        "0 tap D\n2 move {} {}\n3 button right down\n4 button right up\n",
        point.0.round(),
        point.1.round()
    )
}

fn objective_text(text: &ObjectiveText) -> Option<&str> {
    match text {
        ObjectiveText::Literal(text) => Some(text),
        ObjectiveText::Resource(_) => None,
    }
}

/// F9 wording tells the player whether the objective needs a forced shot or a guard order.
fn objective_order(objective: &ObjectiveView) -> ObjectiveOrder {
    let text = objective_text(&objective.description)
        .or_else(|| objective_text(&objective.title))
        .unwrap_or_default()
        .to_ascii_lowercase();
    if text.contains("destroy") || text.contains("eliminate") || text.contains("demolish") {
        ObjectiveOrder::ForceFire
    } else if text.contains("defend") || text.contains("protect") || text.contains("guard") {
        ObjectiveOrder::Guard
    } else {
        ObjectiveOrder::Move
    }
}

fn hostile(team: TacTeam) -> bool {
    matches!(team, TacTeam::Enemy | TacTeam::Neutral)
}

fn click_point(target: &PickTarget) -> Option<(f32, f32)> {
    let [left, top, right, bottom] = target.screen_box;
    let point = if target.mover || target.triangles.is_empty() {
        ((left + right) / 2.0, (top + bottom) / 2.0)
    } else {
        let triangle = target.triangles[0];
        (
            (triangle[0] + triangle[2] + triangle[4]) / 3.0,
            (triangle[1] + triangle[3] + triangle[5]) / 3.0,
        )
    };
    (WORLD_X.0 <= point.0 && point.0 <= WORLD_X.1 && WORLD_Y.0 <= point.1 && point.1 <= WORLD_Y.1)
        .then_some(point)
}

impl CampaignPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_mission(mission: &str) -> Self {
        Self {
            mission: mission.to_ascii_lowercase(),
            ..Self::default()
        }
    }

    pub fn orders_issued(&self) -> u32 {
        self.orders_issued
    }

    pub fn movement_orders(&self) -> u32 {
        self.movement_orders
    }

    /// Set when unit-bar positions visibly change by more than one world unit after the player
    /// begins observing. This is a HUD fact, never a simulation inspection.
    pub fn force_moved(&self) -> bool {
        self.force_moved
    }

    fn push(&mut self, now: i64, events: Vec<(i64, InputEvent)>) {
        let mut last = now;
        for (offset, event) in events {
            last = last.max(now + offset);
            self.queue.push_back((now + offset, event));
        }
        self.next_look = last + THINK_TURNS;
    }

    /// Feeds only physical input to the production headless app route.
    pub fn tick(&mut self, run: &mut HeadlessRun<ProductionDriver>) {
        let Some(turn) = run.core.mission.as_ref().map(|mission| mission.turn) else {
            return;
        };
        let events = self.step(turn, || {
            let hud = run.core.mission.as_ref()?.hud.as_ref()?;
            Some((hud.snapshot().clone(), hud.selection.selected.clone()))
        });
        for event in events {
            run.event(event);
        }
    }

    /// One player think tick, exposed for deterministic unit testing of the input policy.
    pub fn step(
        &mut self,
        turn: i64,
        view: impl FnOnce() -> Option<(HudSnapshot, Vec<i32>)>,
    ) -> Vec<InputEvent> {
        let mut due = Vec::new();
        while self.queue.front().is_some_and(|(at, _)| *at <= turn) {
            due.push(self.queue.pop_front().expect("queue front checked").1);
        }
        if !self.queue.is_empty() || turn < OPENING_TURNS || turn < self.next_look {
            return due;
        }
        let Some((snapshot, selected)) = view() else {
            return due;
        };
        self.next_look = turn + THINK_TURNS;
        self.look(turn, &snapshot, &selected);
        due
    }

    fn look(&mut self, turn: i64, snapshot: &HudSnapshot, selected: &[i32]) {
        let now_positions: Vec<_> = snapshot
            .units
            .iter()
            .filter(|unit| !unit.bar.destroyed)
            .map(|unit| (unit.part_id, (unit.position.0, unit.position.1)))
            .collect();
        if let Some(start) = &self.observed_start {
            self.force_moved |= now_positions.iter().any(|(part, now)| {
                start.iter().any(|(first_part, first)| {
                    part == first_part && (now.0 - first.0).hypot(now.1 - first.1) > 1.0
                })
            });
        } else if !now_positions.is_empty() {
            self.observed_start = Some(now_positions);
        }
        let primary: Vec<_> = snapshot
            .objectives
            .iter()
            .filter(|objective| objective.priority == 1 && !objective.is_hidden_trigger)
            .collect();
        if !primary.is_empty()
            && primary
                .iter()
                .all(|objective| objective.status == ObjectiveStatus::Successful)
        {
            return;
        }

        let living: Vec<_> = snapshot
            .units
            .iter()
            .filter(|unit| !unit.bar.destroyed)
            .collect();
        if living.is_empty() {
            return;
        }
        if living.iter().any(|unit| !selected.contains(&unit.part_id)) {
            let centre = living.iter().fold((0.0, 0.0), |sum, unit| {
                (sum.0 + unit.position.0, sum.1 + unit.position.1)
            });
            let count = living.len() as f32;
            let mut select = gesture(&focus(tac_px(
                (centre.0 / count, centre.1 / count),
                snapshot.vertices_per_side,
            )));
            select.extend(gesture("30 tap E\n"));
            self.push(turn, select);
            return;
        }

        // Work the visible primary objectives in F9 display order.  This is deliberately derived
        // only from the HUD: the objective's status and activation are not read from the sim.
        let active_objective = primary.iter().enumerate().find_map(|(index, objective)| {
            (objective.is_active && objective.status == ObjectiveStatus::Undetermined)
                .then_some((index, *objective))
        });

        let pick = snapshot
            .picks
            .iter()
            .filter(|target| hostile(target.team) && !target.disabled)
            .filter_map(|target| click_point(target).map(|point| (target, point)))
            .min_by_key(|(target, _)| (!target.mover, target.watch_id));
        if let Some((target, point)) = pick {
            if self
                .last_attack
                .is_none_or(|(id, then)| id != target.watch_id || turn - then >= REISSUE_TURNS)
            {
                self.last_attack = Some((target.watch_id, turn));
                self.orders_issued += 1;
                self.push(turn, gesture(&attack_at(point)));
            }
            return;
        }

        let hinted_target = active_objective.and_then(|(index, _)| {
            OBJECTIVE_HINTS
                .iter()
                .find(|(mission, objective, _)| {
                    mission.eq_ignore_ascii_case(&self.mission) && *objective == index
                })
                .map(|(_, _, point)| *point)
        });
        let target = snapshot
            .contacts
            .iter()
            .filter(|contact| hostile(contact.team))
            .map(|contact| contact.pos)
            .next()
            .or(hinted_target)
            .unwrap_or_else(|| self.next_patrol(snapshot.vertices_per_side));
        let bucket = (
            (target.0 / 128.0).round() as i32,
            (target.1 / 128.0).round() as i32,
        );
        if self
            .last_move
            .is_some_and(|(last, then)| last == bucket && turn - then < REISSUE_TURNS)
        {
            return;
        }
        self.last_move = Some((bucket, turn));
        self.movement_orders += 1;
        self.orders_issued += 1;
        let (point, mut move_and_focus) = match active_objective {
            Some((_, objective)) if hinted_target.is_some() => {
                let point = tac_px(target, snapshot.vertices_per_side);
                self.objective_orders += 1;
                (
                    point,
                    objective_gesture(objective_order(objective), point, false),
                )
            }
            Some((_, objective)) => {
                self.objective_orders += 1;
                // `N` cycles the nav markers a player can see.  The following viewport click
                // orders the selected force to the marker that the production camera has framed.
                let point = (f64::from(NAV_VIEW_CENTRE.0), f64::from(NAV_VIEW_CENTRE.1));
                (
                    point,
                    objective_gesture(objective_order(objective), point, true),
                )
            }
            None => {
                let point = tac_px(target, snapshot.vertices_per_side);
                (point, gesture(&map_move(point)))
            }
        };
        move_and_focus.extend(
            gesture(&focus(point))
                .into_iter()
                .map(|(offset, event)| (offset + 60, event)),
        );
        self.push(turn, move_and_focus);
    }

    /// A deterministic lawnmower search of the tac map when no enemy is currently visible.
    fn next_patrol(&mut self, vertices: i32) -> (f32, f32) {
        let bounds = TerrainBounds::from_vertices(vertices.max(1));
        let row = self.patrol_index / 5;
        let column = self.patrol_index % 5;
        self.patrol_index = self.patrol_index.wrapping_add(1);
        let x_column = if row.is_multiple_of(2) {
            column
        } else {
            4 - column
        };
        let x_fraction = x_column as f32 / 4.0;
        let y_fraction = row as f32 / 4.0;
        (
            bounds.top_left_x() + bounds.map_side * x_fraction,
            bounds.top_left_y() - bounds.map_side * y_fraction,
        )
    }
}

fn objective_gesture(
    order: ObjectiveOrder,
    point: (f64, f64),
    nav_marker: bool,
) -> Vec<(i64, InputEvent)> {
    let nav = if nav_marker { "0 tap N\n" } else { "" };
    let mode = match order {
        ObjectiveOrder::Move => "",
        ObjectiveOrder::ForceFire => "20 key F down\n",
        ObjectiveOrder::Guard => "20 key G down\n",
    };
    let release = match order {
        ObjectiveOrder::Move => "",
        ObjectiveOrder::ForceFire => "24 key F up\n",
        ObjectiveOrder::Guard => "24 key G up\n",
    };
    gesture(&format!(
        "{nav}{mode}21 move {} {}\n22 button right down\n23 button right up\n{release}",
        point.0.round(),
        point.1.round()
    ))
}

/// Runs one mission through the production headless app and collects its outcome *after* the
/// player has finished steering.  The player decision loop above has no reference to this data.
pub fn play_mission(options: &HeadlessOptions) -> Result<CampaignPlayerRun, String> {
    let mut player = CampaignPlayer::for_mission(&options.mission);
    let (mut run, report) =
        headless::run_driven_keep(options, |run| player.tick(run), true, |_| {})?;
    let (objectives_done, objectives_total) = run
        .core
        .mission
        .as_ref()
        .and_then(|mission| mission.hud.as_ref())
        .map(|hud| {
            let objectives: Vec<_> = hud
                .snapshot()
                .objectives
                .iter()
                .filter(|objective| objective.priority == 1 && !objective.is_hidden_trigger)
                .collect();
            (
                objectives
                    .iter()
                    .filter(|objective| objective.status == ObjectiveStatus::Successful)
                    .count(),
                objectives.len(),
            )
        })
        .unwrap_or_default();
    let force_lost = run
        .core
        .mission
        .as_ref()
        .is_some_and(|mission| mission.driver.sim().world.mission.term.force_lost);
    let mut notices: Vec<Notice> = report
        .sim_unavailable
        .iter()
        .cloned()
        .map(|kind| Notice { kind })
        .collect();
    if let Some(host) = run.core.ui_host_mut() {
        notices.extend(host.notices().iter().cloned().map(|kind| Notice { kind }));
    }
    Ok(CampaignPlayerRun {
        report,
        objectives_done,
        objectives_total,
        notices,
        orders_issued: player.orders_issued(),
        movement_orders: player.movement_orders(),
        force_moved: player.force_moved(),
        force_lost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_platform::keys::{KEY_F, KEY_G, KEY_N};

    #[test]
    fn patrol_visits_distinct_tac_map_places() {
        let mut player = CampaignPlayer::new();
        let first = player.next_patrol(100);
        let second = player.next_patrol(100);
        assert_ne!(first, second);
    }

    #[test]
    fn visible_objective_wording_generates_nav_force_fire_and_guard_inputs() {
        let destroy = ObjectiveView {
            description: ObjectiveText::Literal("Destroy the radar installation".into()),
            ..Default::default()
        };
        let destroy_events = objective_gesture(objective_order(&destroy), (400.0, 220.0), true);
        assert!(destroy_events.iter().any(|(_, event)| {
            matches!(event, InputEvent::Key { key, pressed: true, .. } if *key == KEY_N)
        }));
        assert!(destroy_events.iter().any(|(_, event)| {
            matches!(event, InputEvent::Key { key, pressed: true, .. } if *key == KEY_F)
        }));

        let defend = ObjectiveView {
            description: ObjectiveText::Literal("Defend the convoy".into()),
            ..Default::default()
        };
        let defend_events = objective_gesture(objective_order(&defend), (400.0, 220.0), true);
        assert!(defend_events.iter().any(|(_, event)| {
            matches!(event, InputEvent::Key { key, pressed: true, .. } if *key == KEY_G)
        }));
        assert!(defend_events.iter().any(|(_, event)| {
            matches!(
                event,
                InputEvent::Button {
                    button: mc2_platform::input::MouseButton::Right,
                    pressed: true
                }
            )
        }));
    }
}
