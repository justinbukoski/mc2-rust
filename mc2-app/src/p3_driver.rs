//! The P3 acceptance player: an outcome-driven generator of *physical* player input.
//!
//! The checked-in `p3_mc2_01_win.input` is a fixed schedule: it gives each patrol target one attack
//! window and never retargets, so a surviving bandit leaves "Destroy Bandit Patrol" undetermined.
//! A player does not play like that: they watch the screen and keep shooting until the objective
//! list says the job is done.  [`PlayerDriver`] does the same.  Every frame it looks only at what a
//! player can see on the HUD snapshot (the objectives panel, the tactical-map blips, the units on the
//! unit bar, the selection boxes of what is on screen) and produces only the events a player can
//! make: a tactical-map click to centre the camera, the Select Visible (E) and Default Attack (D)
//! hotkeys, a right-click on the tactical map to move, and a right-click on an enemy on screen to
//! attack.  It never reads or writes simulation state, and nothing is sent to the sim except
//! through the app's own input path ([`crate::headless::HeadlessRun::event`]).

use crate::headless::{self, HeadlessOptions, HeadlessReport, HeadlessRun, ProductionDriver};
use crate::hud_data::{HudSnapshot, HudUnit, PickTarget};
use crate::script::{Action, InputScript};
use crate::sim::SimDriver;
use crate::state::GameState;
use mc2_campaign::interface::Outcome;
use mc2_campaign::LoadSpec;
use mc2_platform::input::InputEvent;
use mc2_ui::objectives::ObjectiveStatus;
use mc2_ui::tacmap::{TacMap, TacTeam, TerrainBounds};
use std::collections::VecDeque;

/// The shipped 800x600 HUD's tactical-map rectangle (`buttonlayout800.fit`).
const TAC_MAP: (i32, i32, i32, i32) = (4, 435, 165, 596);
/// The opening ABL camera/movie sequence runs before normal mission input (90 s at 30 Hz).
const OPENING_TURNS: i64 = 2_700;
/// Retry the extraction order before the campaign acceptance frame budget expires.
const EXTRACTION_RETRY_TURNS: i64 = 300;
/// How long a lance keeps its attack order on one target before the player gives it again.
const REISSUE_TURNS: i64 = 600;
/// Frames between two looks at the screen while nothing is queued.
const THINK_TURNS: i64 = 30;
/// Where in the 800x600 frame the world view lies (above the HUD panel), for choosing a click point.
const WORLD_X: (f32, f32) = (24.0, 776.0);
const WORLD_Y: (f32, f32) = (24.0, 410.0);
/// On the way to the extraction point the lance fights what is within this reach of its centre.
const ENGAGE_RANGE: f32 = 1500.0;
/// The lance is spread out when a mech is this far from the others' centre; it regroups first.
const SPREAD_LIMIT: f32 = 350.0;
/// ... and is together again when every mech is this close.
const TOGETHER: f32 = 160.0;
/// The length of one leg of the walk to the extraction point.
const HOP: f32 = 900.0;

/// The mission's places, as the briefing and its objective markers show them (`mc2_01.fit`).
const AIRFIELD: (f32, f32) = (3136.0, -789.3);
const HANGAR: (f32, f32) = (3221.3, -277.3);
const PATROL: (f32, f32) = (3200.0, 2050.0);
const NORTH_ISLAND: (f32, f32) = (832.0, 2752.0);
const NORTH_BANDITS: (f32, f32) = (-1100.0, 3070.0);
const EXTRACTION: (f32, f32) = (-4117.3, 513.3);
/// Where a player scouts when no bandit shows on the tactical map: the briefing's patrol and north
/// island points, with intermediate points into the west approach where the patrol falls back.
const PATROL_SCOUTING: [(f32, f32); 18] = [
    (3264.0, 1984.0),
    (3434.7, 1984.0),
    (2837.3, 2112.0),
    (2922.7, 2112.0),
    (3349.3, 1984.0),
    (3562.7, 1984.0),
    (2922.7, 2197.3),
    (2566.0, 2554.0),
    (2200.0, 2300.0),
    (1900.0, 1900.0),
    (1500.0, 1500.0),
    (1200.0, 1100.0),
    // back south along the road the lance first came up, then west to where the bandits fall back
    (3320.0, 1100.0),
    (3300.0, 480.0),
    (3000.0, -800.0),
    (2000.0, -300.0),
    (1200.0, -60.0),
    (40.0, 20.0),
];
const NORTH_SCOUTING: [(f32, f32); 4] = [
    (-1045.3, 3093.3),
    (-1173.3, 3050.7),
    (-1500.0, 2900.0),
    (-700.0, 3000.0),
];

/// A line of what the driver did, kept for the trace and the porting log.
#[derive(Clone, Debug)]
pub struct DriverNote {
    pub turn: i64,
    pub text: String,
}

/// A leg of the walk to the extraction point: where to, when it was ordered, where the lance was.
type Hop = ((f32, f32), i64, (f32, f32));

#[derive(Default)]
pub struct PlayerDriver {
    queue: VecDeque<(i64, InputEvent)>,
    /// The next turn at which the driver looks at the screen.
    next_look: i64,
    /// The last attack: target watch id, the turn it was given, and how often it was tried.
    attack: Option<(u32, i64, u32)>,
    /// The last move order and the stage that gave it.
    moved: Option<(usize, i64)>,
    /// The last place the lance was sent to find a target, and when.
    advanced: Option<((f32, f32), i64)>,
    /// The scouting spot the lance is heading for.
    scout: usize,
    /// When the lance reached the current scouting spot, so the camera can settle before moving on.
    scout_arrival: Option<((f32, f32), i64)>,
    /// Targets given up on (the lance would not move toward them) until the given turn.
    given_up: Vec<(u32, i64)>,
    /// A moving contact the lance could not close on; pause contact pursuit while it scouts again.
    ignore_contacts_until: i64,
    lance_at_attack: Option<(f32, f32)>,
    lance_at_advance: Option<(f32, f32)>,
    /// The extraction leg: the turn a regroup was ordered and the last hop (point, turn, lance
    /// centre when it was given); `hop_direct` once a hop did not move the lance.
    regroup: Option<i64>,
    hop: Option<Hop>,
    hop_direct: bool,
    /// The earlier agent: the extraction leg is one move order given again every 300 turns, no
    /// fighting, no regrouping (kept to A/B the sim against the original game, see `VISUAL-QA.md`).
    pub walk_only: bool,
    stage: usize,
    last_status: Vec<ObjectiveStatus>,
    pub notes: Vec<DriverNote>,
}

/// Whether every unit still able to act (`(part id, destroyed or disabled)`) is selected; a disabled
/// unit is greyed on the unit bar and Select Visible skips it, so it is not waited for.
fn lance_selected(units: impl Iterator<Item = (i32, bool)>, selected: &[i32]) -> bool {
    units
        .filter(|(_, out)| !out)
        .all(|(part, _)| selected.contains(&part))
}

fn tac_px(world: (f32, f32)) -> (f64, f64) {
    let mut map = TacMap::new(TerrainBounds::from_vertices(100));
    map.set_pos(mc2_ui::draw::GuiRect::new(
        TAC_MAP.0, TAC_MAP.1, TAC_MAP.2, TAC_MAP.3,
    ));
    let (x, y) = map.world_to_tac_map(world.0, world.1);
    (
        f64::from(x).clamp(f64::from(TAC_MAP.0) + 1.0, f64::from(TAC_MAP.2) - 1.0),
        f64::from(y).clamp(f64::from(TAC_MAP.1) + 1.0, f64::from(TAC_MAP.3) - 1.0),
    )
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// The point `len` from `from` on the straight line to `to` (`to` itself when it is nearer).
fn hop_toward(from: (f32, f32), to: (f32, f32), len: f32) -> (f32, f32) {
    let d = dist(from, to);
    if d <= len {
        return to;
    }
    let k = len / d;
    (from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k)
}

/// A gesture as `InputScript` text with frames relative to now.
fn gesture(text: &str) -> Vec<(i64, InputEvent)> {
    InputScript::parse(text)
        .expect("driver gesture parses")
        .actions
        .into_iter()
        .filter_map(|(f, a)| match a {
            Action::Event(e) => Some((f as i64, e)),
            _ => None,
        })
        .collect()
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

fn attack_at(pt: (f32, f32)) -> String {
    format!(
        "0 tap D\n2 move {} {}\n3 button right down\n4 button right up\n",
        pt.0.round(),
        pt.1.round()
    )
}

/// The point a right-click should land on to hit `pick` on screen: the middle of its box for a
/// mover, the middle of a visible face for a structure; `None` when it is not on the world view.
fn click_point(pick: &PickTarget, attempt: u32) -> Option<(f32, f32)> {
    let [l, t, r, b] = pick.screen_box;
    let wobble = (attempt % 5) as f32 * 4.0;
    let pt = if pick.mover || pick.triangles.is_empty() {
        ((l + r) / 2.0 + wobble, (t + b) / 2.0 + wobble)
    } else {
        let tri = pick.triangles[attempt as usize % pick.triangles.len()];
        (
            (tri[0] + tri[2] + tri[4]) / 3.0,
            (tri[1] + tri[3] + tri[5]) / 3.0,
        )
    };
    (WORLD_X.0 <= pt.0 && pt.0 <= WORLD_X.1 && WORLD_Y.0 <= pt.1 && pt.1 <= WORLD_Y.1).then_some(pt)
}

impl PlayerDriver {
    pub fn new() -> Self {
        Self::default()
    }

    fn note(&mut self, turn: i64, text: String) {
        if std::env::var_os("MC2_P3_TRACE").is_some_and(|v| v != "0") {
            eprintln!("p3-driver turn {turn}: {text}");
        }
        self.notes.push(DriverNote { turn, text });
    }

    fn push(&mut self, now: i64, events: Vec<(i64, InputEvent)>) {
        let mut last = now;
        for (offset, e) in events {
            last = last.max(now + offset);
            self.queue.push_back((now + offset, e));
        }
        self.next_look = last + THINK_TURNS;
    }

    /// Delivers the events due at `turn`, then (when nothing is queued) looks at the screen.
    pub fn tick(&mut self, run: &mut HeadlessRun<ProductionDriver>) {
        let Some(turn) = run.core.mission.as_ref().map(|m| m.turn) else {
            return;
        };
        let events = self.step(turn, || {
            let hud = run.core.mission.as_ref()?.hud.as_ref()?;
            Some((hud.snapshot().clone(), hud.selection.selected.clone()))
        });
        for e in events {
            run.event(e);
        }
    }

    /// One driver frame at mission `turn`: the input events due now, and, when nothing is queued and
    /// it is time to look, a fresh look at the screen (`view`: the HUD snapshot and the selection).
    pub fn step(
        &mut self,
        turn: i64,
        view: impl FnOnce() -> Option<(HudSnapshot, Vec<i32>)>,
    ) -> Vec<InputEvent> {
        let mut due = Vec::new();
        while self.queue.front().is_some_and(|(at, _)| *at <= turn) {
            due.push(self.queue.pop_front().expect("front checked").1);
        }
        if !self.queue.is_empty() || turn < OPENING_TURNS || turn < self.next_look {
            return due;
        }
        let Some((snap, selected)) = view() else {
            return due;
        };
        self.next_look = turn + THINK_TURNS;
        self.look(turn, &snap, &selected);
        due
    }

    fn look(&mut self, turn: i64, snap: &HudSnapshot, selected: &[i32]) {
        let statuses: Vec<ObjectiveStatus> = snap
            .objectives
            .iter()
            .filter(|o| o.priority == 1 && !o.is_hidden_trigger)
            .map(|o| o.status)
            .collect();
        if statuses != self.last_status {
            self.note(turn, format!("primary objectives {statuses:?}"));
            self.last_status = statuses.clone();
        }
        if turn % 300 < THINK_TURNS {
            let positions: Vec<(i32, i32)> = snap
                .units
                .iter()
                .map(|u| (u.position.0 as i32, u.position.1 as i32))
                .collect();
            self.note(
                turn,
                format!("lance {positions:?}, {} selected", selected.len()),
            );
        }
        // The unit bar greys out a destroyed or disabled unit and Select Visible skips it: only
        // the units still able to act are the lance.
        let lance_units: Vec<&HudUnit> = snap.units.iter().filter(|u| !u.bar.destroyed).collect();
        if statuses.len() < 6 || lance_units.is_empty() {
            return;
        }
        // The first primary objective the objectives panel does not show as done.
        let Some(stage) = statuses
            .iter()
            .position(|s| *s != ObjectiveStatus::Successful)
        else {
            return;
        };
        if stage != self.stage {
            self.stage = stage;
            self.attack = None;
            self.moved = None;
            self.advanced = None;
            self.scout = 0;
            self.scout_arrival = None;
            self.ignore_contacts_until = 0;
            self.regroup = None;
            self.hop = None;
            self.hop_direct = false;
        }
        // Every living unit must be selected before an order: refocus on the lance and Select Visible.
        if !lance_selected(
            snap.units.iter().map(|u| (u.part_id, u.bar.destroyed)),
            selected,
        ) {
            let n = lance_units.len() as f32;
            let c = lance_units.iter().fold((0.0, 0.0), |a, u| {
                (a.0 + u.position.0 / n, a.1 + u.position.1 / n)
            });
            self.note(turn, format!("select visible around {c:?}"));
            let mut g = gesture(&focus(tac_px(c)));
            g.extend(gesture("30 tap E\n"));
            self.push(turn, g);
            return;
        }
        match stage {
            0 => self.go(turn, stage, AIRFIELD),
            3 => self.go(turn, stage, NORTH_ISLAND),
            5 if self.walk_only => self.go(turn, stage, EXTRACTION),
            5 => self.extract(turn, snap),
            1 => self.fight(turn, snap, HANGAR, 300.0, false, &[HANGAR]),
            2 => self.fight(turn, snap, PATROL, f32::MAX, true, &PATROL_SCOUTING),
            _ => self.fight(turn, snap, NORTH_BANDITS, f32::MAX, true, &NORTH_SCOUTING),
        }
    }

    /// The walk to the extraction point as a player makes it: shoot what shows up, keep the lance
    /// together (regroup when it strings out) and walk on in short legs, so a mech that is shot at
    /// is not left alone and a slow mech is waited for.  The objective wants every 'mech there.
    fn extract(&mut self, turn: i64, snap: &HudSnapshot) {
        let hostile = |t: TacTeam| matches!(t, TacTeam::Enemy | TacTeam::Neutral);
        let living: Vec<&HudUnit> = snap.units.iter().filter(|u| !u.bar.destroyed).collect();
        let n = living.len().max(1) as f32;
        let c = living.iter().fold((0.0, 0.0), |a, u| {
            (a.0 + u.position.0 / n, a.1 + u.position.1 / n)
        });
        let spread = living
            .iter()
            .map(|u| dist((u.position.0, u.position.1), c))
            .fold(0.0, f32::max);
        let engaged = snap
            .contacts
            .iter()
            .any(|ct| hostile(ct.team) && dist(ct.pos, c) <= ENGAGE_RANGE);
        if engaged {
            self.moved = None;
            self.regroup = None;
            self.hop = None;
            self.fight(turn, snap, c, ENGAGE_RANGE, true, &[EXTRACTION]);
            return;
        }
        // nothing to shoot at: whatever the fight left behind is over
        self.attack = None;
        self.advanced = None;
        if let Some(at) = self.regroup {
            if spread <= TOGETHER || turn - at >= 900 {
                self.note(turn, format!("lance together (spread {spread:.0})"));
                self.regroup = None;
            } else {
                return;
            }
        } else if spread > SPREAD_LIMIT && dist(c, EXTRACTION) > 400.0 {
            self.note(
                turn,
                format!("lance strung out (spread {spread:.0}): regroup at {c:?}"),
            );
            self.regroup = Some(turn);
            self.hop = None;
            self.push(turn, gesture(&map_move(tac_px(c))));
            return;
        }
        let left = dist(c, EXTRACTION);
        if self.hop_direct || left <= HOP + 300.0 {
            self.go(turn, 5, EXTRACTION);
            return;
        }
        if let Some((to, at, from)) = self.hop {
            let arrived = dist(c, to) < 250.0;
            if !arrived && turn - at < 900 {
                return;
            }
            if !arrived && dist(c, from) < 60.0 {
                self.note(
                    turn,
                    format!("hop to {to:?} did not move the lance: going straight"),
                );
                self.hop_direct = true;
                return;
            }
        }
        let to = hop_toward(c, EXTRACTION, HOP);
        self.note(
            turn,
            format!("stage 5: hop to {to:?} (lance at {c:?}, {left:.0} to go)"),
        );
        self.hop = Some((to, turn, c));
        self.push(turn, gesture(&map_move(tac_px(to))));
    }

    /// A tactical-map move, given again every so often until the objectives panel ticks it off.
    fn go(&mut self, turn: i64, stage: usize, to: (f32, f32)) {
        let retry_after = if stage == 5 {
            EXTRACTION_RETRY_TURNS
        } else {
            900
        };
        if self
            .moved
            .is_some_and(|(s, t)| s == stage && turn - t < retry_after)
        {
            return;
        }
        self.note(turn, format!("stage {stage}: move to {to:?}"));
        self.moved = Some((stage, turn));
        self.push(turn, gesture(&map_move(tac_px(to))));
    }

    /// Keeps shooting at what is left near `place` until the objective is done.
    fn fight(
        &mut self,
        turn: i64,
        snap: &HudSnapshot,
        place: (f32, f32),
        radius: f32,
        movers: bool,
        scouting: &[(f32, f32)],
    ) {
        let hostile = |t: TacTeam| matches!(t, TacTeam::Enemy | TacTeam::Neutral);
        let living: Vec<&HudUnit> = snap.units.iter().filter(|u| !u.bar.destroyed).collect();
        let n = living.len().max(1) as f32;
        let lance = living.iter().fold((0.0, 0.0), |a, u| {
            (a.0 + u.position.0 / n, a.1 + u.position.1 / n)
        });
        // A target the lance has been ordered after for 300 turns without moving is out of reach.
        if let (Some((id, t, _)), Some(at)) = (self.attack, self.lance_at_attack) {
            if turn - t >= 300 && dist(at, lance) < 60.0 {
                self.note(
                    turn,
                    format!("lance does not close on watch {id}; giving it up"),
                );
                self.given_up.push((id, turn + 3000));
                self.attack = None;
                self.lance_at_attack = None;
            }
        }
        self.given_up.retain(|(_, until)| *until > turn);
        // What is on screen to click, and in reach of the lance.
        let mut on_screen: Vec<&PickTarget> = snap
            .picks
            .iter()
            .filter(|p| {
                hostile(p.team)
                    && !p.disabled
                    && p.mover == movers
                    && !self.given_up.iter().any(|(id, _)| *id == p.watch_id)
                    && (!movers || dist((p.position.0, p.position.1), lance) <= 1000.0)
                    && dist((p.position.0, p.position.1), place) <= radius
            })
            .collect();
        let from = if movers { lance } else { place };
        on_screen.sort_by(|a, b| {
            dist((a.position.0, a.position.1), from)
                .total_cmp(&dist((b.position.0, b.position.1), from))
        });
        let tried = self.attack.map_or(0, |(_, _, n)| n);
        let fresh = self
            .attack
            .is_none_or(|(_, t, _)| turn - t >= REISSUE_TURNS);
        for pick in on_screen {
            // A target already under an order stays under it until the order is stale.
            if self.attack.is_some_and(|(id, _, _)| id == pick.watch_id) && !fresh {
                return;
            }
            let Some(pt) = click_point(pick, tried) else {
                continue;
            };
            self.note(
                turn,
                format!(
                    "attack watch {} at {:?} via click {pt:?} (try {tried})",
                    pick.watch_id, pick.position
                ),
            );
            let same = self.attack.is_some_and(|(id, _, _)| id == pick.watch_id);
            self.attack = Some((pick.watch_id, turn, if same { tried + 1 } else { 0 }));
            self.lance_at_attack = Some(lance);
            self.push(turn, gesture(&attack_at(pt)));
            return;
        }
        // Nothing shootable on screen.  Look at the nearest blip on the tactical map (or, for the
        // hangar, at the building) and, when the screen is already there, walk toward it.
        // A move that has not shifted the lance in 300 turns found no way through: the next spot.
        if let Some((spot, t)) = self.advanced {
            let stalled = turn - t >= 300
                && self.lance_at_advance.is_some_and(|p| dist(p, lance) < 50.0)
                && dist(lance, spot) > 200.0;
            if stalled {
                self.note(
                    turn,
                    format!("lance did not move toward {spot:?}; scouting elsewhere"),
                );
                self.scout += 1;
                self.advanced = None;
                self.lance_at_advance = None;
                self.ignore_contacts_until = turn + 1200;
            }
        }
        // Hold at a reached scouting spot long enough to inspect the HUD again. The camera can
        // still be travelling to the focus point when the lance arrives; immediately advancing
        // here skips targets that only become pickable after the view settles.
        let scout_spot = scouting[self.scout % scouting.len()];
        let reached_scout_spot = dist(lance, scout_spot) < 200.0;
        let arrived_at = match self.scout_arrival {
            Some((spot, arrived)) if dist(spot, scout_spot) < 1.0 && reached_scout_spot => {
                Some(arrived)
            }
            _ if reached_scout_spot => Some(turn),
            _ => None,
        };
        self.scout_arrival = arrived_at.map(|arrived| (scout_spot, arrived));
        if arrived_at.is_some_and(|arrived| turn - arrived >= 180) {
            self.scout += 1;
            self.scout_arrival = None;
        }
        let target = snap
            .contacts
            .iter()
            .filter(|c| {
                turn >= self.ignore_contacts_until
                    && movers
                    && hostile(c.team)
                    && dist(c.pos, place) <= radius
            })
            .map(|c| c.pos)
            .min_by(|a, b| dist(*a, from).total_cmp(&dist(*b, from)))
            .unwrap_or_else(|| scouting[self.scout % scouting.len()]);
        let seen = snap.contacts.iter().any(|c| {
            turn >= self.ignore_contacts_until
                && movers
                && hostile(c.team)
                && dist(c.pos, place) <= radius
        });
        if seen && self.advanced.is_some_and(|(_, t)| turn - t < 300) {
            self.note(turn, format!("focus on {target:?}"));
            self.push(turn, gesture(&focus(tac_px(target))));
            return;
        }
        if self
            .advanced
            .is_some_and(|(p, t)| p == target && turn - t < 1200)
        {
            return;
        }
        self.note(
            turn,
            format!(
                "stage {}: advance to {target:?} (lance at {from:?}, {} contacts, {} on screen)",
                self.stage,
                snap.contacts.len(),
                snap.picks.len()
            ),
        );
        self.advanced = Some((target, turn));
        self.lance_at_advance = Some(lance);
        let mut g = gesture(&map_move(tac_px(target)));
        g.extend(
            gesture(&focus(tac_px(target)))
                .into_iter()
                .map(|(f, e)| (f + 60, e)),
        );
        self.push(turn, g);
    }
}

/// The mission the driver knows the places of (its map constants are `mc2_01`'s).
pub const DRIVEN_MISSION: &str = "mc2_01";

/// One home mech read straight from the simulation's object table (not through `Outcome`).
#[derive(Clone, Debug, PartialEq)]
pub struct HomeMech {
    pub variant_name: String,
    /// The pilot's profile stem, as loaded for the mech.
    pub pilot_file: Option<String>,
    pub destroyed: bool,
    pub disabled: bool,
    /// The paint the mech was launched with.
    pub paint: [u32; 3],
}

/// One mech of any commander read straight from the simulation's object table.
#[derive(Clone, Debug, PartialEq)]
pub struct SimMech {
    pub variant_name: String,
    /// The owning commander's id (0 = the home commander).
    pub commander: i32,
    pub tonnage: f32,
    pub destroyed: bool,
    pub disabled: bool,
    pub move_level: i32,
}

/// A mission played by [`play_campaign_mission`]: what the run reported and what the mission hands
/// to the campaign's results flow.
#[derive(Clone, Debug)]
pub struct CampaignMission {
    pub report: HeadlessReport,
    /// `Logistics::setMissionCompleted`'s input: the result code, the home team's objectives, every
    /// home mover's survival and pilot report (the survivors) and every mech's destroyed/disabled
    /// state (the salvage candidates), read from the simulation the frame the mission ended.
    pub outcome: Outcome,
    /// Each of the home commander's mechs as the simulation's object table holds it when the mission
    /// ended, in object-table order; an independent read (by identity) of what `outcome.movers` reports.
    pub home_mechs: Vec<HomeMech>,
    /// Every mech of the mission (any commander) as the object table holds it when the mission
    /// ended: an independent read of what `outcome.mechs` (enemies, salvage) reports.
    pub sim_mechs: Vec<SimMech>,
    /// The paint the loaded mission view gave the home force (`MissionView::home_paint`).
    pub view_home_paint: mc2_render::mission::PaintSource,
    /// `true` when every simulation mover was bound to a model of the live view for the whole run, so
    /// mover visibility (which feeds the simulation) was complete; `false` makes the result explicitly
    /// unavailable for parity (`LiveSim::visibility_note`).
    pub mover_visibility_complete: bool,
    /// What the driver did, for the trace.
    pub notes: Vec<DriverNote>,
}

/// Plays a campaign mission with the physical-input player and returns the mission's `Outcome`.
///
/// `options` carries the data directory, the screen size and the game config; `launch` is the
/// campaign's `LoadSpec` (`Logistics::beginMission`: the force the player actually has, difficulty,
/// ammo option, resource points); it replaces `options.game.campaign_launch` and its mission replaces
/// `options.mission`.  The run goes through the production app route, ends on the first frame that
/// composes the Results screen, and takes at most `options.frames` frames.  The driver sends only
/// the events a player can make ([`PlayerDriver`]); the `Outcome` is read afterwards, never to steer.
///
/// Errors: the driver only knows [`DRIVEN_MISSION`]'s places (any other mission is reported
/// unavailable, not attempted); the mission is not over within the frame budget; the run fails.
pub fn play_campaign_mission(
    options: &HeadlessOptions,
    launch: LoadSpec,
) -> Result<CampaignMission, String> {
    let played = play_campaign_mission_unverified(options, launch)?;
    if played.mover_visibility_complete {
        Ok(played)
    } else {
        Err(format!(
            "unavailable: the live view cannot bind every simulation mover of this campaign force \
             (mover visibility feeds the simulation), so the mission's result ({:?}) is not offered \
             as campaign evidence; see `play_campaign_mission_unverified` for the diagnostic run",
            played.outcome.result
        ))
    }
}

/// Like [`play_campaign_mission`] but also returns a run whose mover visibility was incomplete
/// (`CampaignMission::mover_visibility_complete == false`).  A diagnostic: such an `Outcome` must
/// not be applied to the campaign.
pub fn play_campaign_mission_unverified(
    options: &HeadlessOptions,
    launch: LoadSpec,
) -> Result<CampaignMission, String> {
    if launch.mission != DRIVEN_MISSION {
        return Err(format!(
            "the P3 player driver is unavailable for {:?}: it knows only the places of {DRIVEN_MISSION}",
            launch.mission
        ));
    }
    let mut options = options.clone();
    options.mission = launch.mission.clone();
    // the rendered home paint is the launch's, as the UI host hands it to the game
    // (`UiHost::launch_campaign_paint`): the first force mech's colours
    if let Some(force) = launch.force.first() {
        options.game.campaign_paint = Some(mc2_render::mission::PaintSource::Scheme {
            base: force.colors[0],
            highlight: force.colors[1],
        });
    }
    options.game.campaign_launch = Some(launch);
    options.script = InputScript::default();
    let mut driver = PlayerDriver::new();
    let (mut run, report) =
        headless::run_driven_keep(&options, |run| driver.tick(run), true, |_| {})?;
    let result = match run.core.state() {
        GameState::Results(r) => *r,
        other => {
            let tail: Vec<String> = driver
                .notes
                .iter()
                .rev()
                .take(8)
                .map(|n| format!("turn {}: {}", n.turn, n.text))
                .collect();
            return Err(format!(
                "the mission was not over after {} frames (state {}); the driver's last notes: {tail:?}",
                report.frames,
                other.name()
            ));
        }
    };
    let rt = run
        .core
        .mission
        .as_mut()
        .ok_or("no mission at the end of the run")?;
    let outcome = rt
        .driver
        .mission_outcome()
        .ok_or("the simulation reported no outcome")?;
    debug_assert_eq!(outcome.result as u32, result);
    let view_home_paint = rt.view.home_paint;
    let mover_visibility_complete = rt
        .driver
        .visibility_note()
        .starts_with("object-visibility=production");
    let world = &rt.driver.sim().world;
    let sim_mechs: Vec<SimMech> = world
        .objects
        .movers
        .iter()
        .filter_map(|&h| world.objects.get(h))
        .filter(|o| o.exists())
        .filter_map(|o| match &o.kind {
            mc2_sim::object::Kind::Mech(m) => Some(SimMech {
                variant_name: m.variant_name.clone(),
                commander: i32::from(m.m.commander.0),
                tonnage: o.c.tonnage,
                destroyed: o.c.is_destroyed(),
                disabled: o.c.is_disabled(),
                move_level: i32::from(m.m.move_level),
            }),
            _ => None,
        })
        .collect();
    let home_mechs: Vec<HomeMech> = world
        .objects
        .movers
        .iter()
        .filter(|&&h| world.objects.get(h).is_some_and(|o| o.exists()))
        .filter_map(|&h| world.objects.get(h))
        .filter_map(|o| match &o.kind {
            mc2_sim::object::Kind::Mech(m) if m.m.commander.0 == 0 => Some(HomeMech {
                variant_name: m.variant_name.clone(),
                pilot_file: world
                    .mission
                    .pilot_loads
                    .get(m.m.pilot_slot as usize)
                    .and_then(|l| l.profile.as_ref())
                    .map(|p| p.file_name.clone()),
                destroyed: o.c.is_destroyed(),
                disabled: o.c.is_disabled(),
                paint: world
                    .mission
                    .paint_schemes
                    .iter()
                    .find(|(wid, _)| *wid == o.c.watch_id)
                    .map_or([0; 3], |(_, c)| *c),
            }),
            _ => None,
        })
        .collect();
    Ok(CampaignMission {
        report,
        outcome,
        home_mechs,
        sim_mechs,
        view_home_paint,
        mover_visibility_complete,
        notes: driver.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_to_extraction_goes_in_legs_of_a_fixed_length() {
        let to = (-4117.3, 513.3);
        let from = (-442.8, 2738.7);
        let leg = hop_toward(from, to, HOP);
        assert!((dist(from, leg) - HOP).abs() < 0.5);
        // on the line from the lance to the extraction point
        let along = dist(from, leg) + dist(leg, to);
        assert!((along - dist(from, to)).abs() < 0.5);
        assert_eq!(hop_toward((-4000.0, 500.0), to, HOP), to);
    }

    #[test]
    fn a_disabled_lance_member_is_not_waited_for() {
        // Select Visible picked the two able units; the disabled third stays off the selection.
        let units = [(1, false), (2, false), (3, true)];
        assert!(lance_selected(units.into_iter(), &[1, 2]));
        assert!(!lance_selected(units.into_iter(), &[1]));
        assert!(lance_selected([(3, true)].into_iter(), &[]));
    }
}
