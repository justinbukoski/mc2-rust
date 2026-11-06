// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The screen-sequence controller (`code/missionbegin.cpp`, single player): the logistics screens
//! form a grid, and the status a screen reports moves the player across it.
//!
//! ```text
//!          (2,0) purchase
//! (0,1) -> (1,1) -> (2,1) mech bay -> (3,1) pilot ready -> (4,1) loading
//!  select   brief    |                                         |
//!                   (2,2) mech lab                         done: launch
//! ```
//!
//! `Next` moves right, `Previous` left, `Up`/`Down` vertically, `SkipOneNext`/`SkipOnePrevious`
//! by two. A move onto a screen calls its `begin`; leaving one calls its `end`. The grid is
//! `singlePlayerScreens` in the C++; `active` is the copy the controller moves through.
//!
//! The controller does not launch the mission: it reports [`Flow::Launch`] and the app asks
//! `Logistics::load_spec`. The main menu is another lane's: [`Flow::MainMenu`] hands over to it,
//! and the app calls [`Sequence::restart`] or [`Sequence::skip_to_briefing`] when it returns.

use super::callout::Callout;
use super::movie::MovieRequest;
use super::script::{BrainBindings, LogisticsBrain};
use super::Page;
use crate::ctx::{Loader, RenderCtx, UiOutput, UpdateCtx};
use crate::input::UiInput;
use crate::screen::Status;
use mc2_campaign::Logistics;

/// The tutorial's screen-visit state of `MissionBegin::getCurrentScreenId`: the file-scope
/// `inPurchase` and `inMechLab` flags. The first-time flags it reads and clears are the campaign's
/// (`Logistics::first_time_purchase`, `first_time_mech_lab`), saved with the campaign as the C++
/// comment asks.
#[derive(Debug, Default)]
pub struct ScreenVisits {
    in_purchase: bool,
    in_mech_lab: bool,
}

impl ScreenVisits {
    /// `getCurrentScreenId` for the grid cell `(x, y)`: `10 * x + y`, except that the purchase
    /// screen (20) and the mech lab (22) report 0 once their first visit is over. Leaving the
    /// screen after its first visit clears the campaign's first-time flag.
    pub fn screen_id(&mut self, cell: (usize, usize), lg: &mut Logistics) -> i32 {
        let mut id = 10 * cell.0 as i32 + cell.1 as i32;
        if id == 20 && lg.first_time_purchase() {
            self.in_purchase = true;
        } else if id == 22 && lg.first_time_mech_lab() {
            self.in_mech_lab = true;
        }
        if id != 20 && self.in_purchase {
            lg.set_first_time_purchase(false);
        } else if id != 22 && self.in_mech_lab {
            lg.set_first_time_mech_lab(false);
        }
        if (id == 20 && !lg.first_time_purchase()) || (id == 22 && !lg.first_time_mech_lab()) {
            id = 0;
        }
        id
    }
}

/// `DIM_SCREEN_X`, `DIM_SCREEN_Y`.
pub const DIM_X: usize = 5;
pub const DIM_Y: usize = 3;

/// Indices into the pages the controller is built with.
pub mod slot {
    pub const SELECT: usize = 0;
    pub const BRIEFING: usize = 1;
    pub const MECH_BAY: usize = 2;
    pub const PILOT_READY: usize = 3;
    pub const PURCHASE: usize = 4;
    pub const MECH_LAB: usize = 5;
    pub const LOADING: usize = 6;
    /// The options screen. It is not on the grid: [`Sequence::open_options`] shows it over the
    /// current screen (`optionsScreenWrapper`, reached from the main menu in the C++).
    pub const OPTIONS: usize = 7;
    pub const COUNT: usize = 8;
}

/// The name of each slot, in the reports [`Flow::Unavailable`] and [`Flow::Failed`] give.
const SLOT_NAMES: [&str; slot::COUNT] = [
    "mission selection",
    "briefing",
    "mech bay",
    "pilot ready",
    "purchase",
    "mech lab",
    "loading",
    "options",
];

/// What a frame's update asks of the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Stay in the logistics sequence.
    Running,
    /// The loading screen was passed: launch the mission (`bDone`).
    Launch,
    /// The main menu (or the splash) takes over.
    MainMenu,
    /// The sequence needs a screen, or a part of one, that is not wired up yet (its name). It stays
    /// where it is; the app should say so to the player. The purchase, mech lab, loading and options
    /// screens are other lanes' and report this until they land.
    Unavailable(&'static str),
    /// A screen or script failed (see [`Sequence::fault`]). The sequence stops: nothing after the
    /// failure reports [`Flow::Launch`].
    Failed,
}

type Grid = [[Option<usize>; DIM_Y]; DIM_X];

pub struct Sequence {
    pages: Vec<Box<dyn Page>>,
    /// `singlePlayerScreens`.
    canonical: Grid,
    /// `screens`: what the player moves through.
    active: Grid,
    /// `curScreenX`, `curScreenY`; `None` before the first start (`curScreenX = -1`).
    cur: Option<(usize, usize)>,
    /// `bReadyToLoad`: the loading screen was reached.
    ready_to_load: bool,
    /// `bSplash`: the main menu is showing.
    splash: bool,
    /// `logisticsBrain`: the tutorial's script, for the mission the sequence started on.
    brain: Option<LogisticsBrain>,
    /// Whether the brain was loaded and has not faulted since. The C++ loads it in
    /// `MissionBegin::begin` only, so a restart or a skip keeps the running instance (its module
    /// variables and state); a brain that never loaded, or that faulted, is loaded again.
    brain_loaded: bool,
    /// The screen-visit flags the brain's `logisticsScreenId` reads.
    visits: ScreenVisits,
    /// The tutorial's mouse callout (`animationRunning` and its target).
    callout: Callout,
    /// The grid cell the options screen was opened over. While it is set, the options page takes
    /// the updates and the render; the grid page stays as it was (suspended, not ended).
    options_return: Option<(usize, usize)>,
    /// The error that stopped the sequence (a failed script, or a screen that could not load).
    fault: Option<String>,
}

impl Sequence {
    /// The single-player sequence over `pages`, indexed by [`slot`].
    pub fn single_player(pages: Vec<Box<dyn Page>>) -> Sequence {
        assert_eq!(pages.len(), slot::COUNT, "one page per slot");
        let mut canonical: Grid = [[None; DIM_Y]; DIM_X];
        canonical[0][1] = Some(slot::SELECT);
        canonical[1][1] = Some(slot::BRIEFING);
        canonical[2][1] = Some(slot::MECH_BAY);
        canonical[3][1] = Some(slot::PILOT_READY);
        canonical[2][0] = Some(slot::PURCHASE);
        canonical[2][2] = Some(slot::MECH_LAB);
        canonical[4][1] = Some(slot::LOADING);
        Sequence {
            pages,
            canonical,
            active: canonical,
            cur: None,
            ready_to_load: false,
            splash: false,
            brain: None,
            brain_loaded: false,
            visits: ScreenVisits::default(),
            callout: Callout::default(),
            options_return: None,
            fault: None,
        }
    }

    /// Shows the options screen over the current screen (the C++ options wrapper, opened from the
    /// main menu). Errs, with the reason, when there is no current screen, when the options are
    /// already open, when the options screen is not wired up yet, or when it fails to begin (the
    /// failure is also [`Sequence::fault`], and the sequence stops). The current screen is not
    /// ended: it resumes unchanged when the options close.
    pub fn open_options(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) -> Result<(), String> {
        let Some(cell) = self.cur else {
            return Err("there is no screen to open the options over".into());
        };
        if self.options_return.is_some() {
            return Err("the options are already open".into());
        }
        if !self.pages[slot::OPTIONS].is_available() {
            return Err("the options screen is not available yet".into());
        }
        self.options_return = Some(cell);
        self.pages[slot::OPTIONS].begin_fade_in(1.0);
        self.pages[slot::OPTIONS].begin(ld, lg);
        self.note_fault(slot::OPTIONS);
        if let Some(e) = self.fault.clone() {
            self.options_return = None;
            return Err(e);
        }
        Ok(())
    }

    /// The error that stopped the sequence, if one did.
    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    /// Whether the options screen is showing over the sequence.
    pub fn in_options(&self) -> bool {
        self.options_return.is_some()
    }

    /// `MissionBegin::begin` with the main menu already set up: the mission selection, or the
    /// choose-mission screen when the campaign skips the logistics down to it.
    pub fn begin(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.active = self.canonical;
        self.ready_to_load = false;
        self.callout.reset();
        self.cur = Some((0, 1));
        if lg.skip_logistics() && lg.show_choose_mission() {
            self.cur = Some((3, 1));
            self.active[3][1] = self.canonical[0][1];
        }
        self.fault = None;
        self.load_brain(ld, lg);
        self.enter(ld, lg);
    }

    /// `MissionBegin::update` with the main menu's restart status: a new run of the sequence,
    /// as the main menu's "start" does. The C++ restart switches screens without `begin()`, so the
    /// brain keeps running (see [`Sequence::ensure_brain`]).
    pub fn restart(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.end_current(lg);
        self.active = self.canonical;
        self.splash = false;
        self.ready_to_load = false;
        if lg.skip_logistics() {
            if lg.show_choose_mission() {
                self.cur = Some((3, 1));
                self.active[3][1] = self.canonical[0][1];
            } else {
                self.cur = Some((4, 1));
            }
        } else {
            self.cur = Some((0, 1));
        }
        self.fault = None;
        self.ensure_brain(ld, lg);
        self.enter(ld, lg);
    }

    /// Returns from the main menu to the logistics page that opened it. `mainMenu->NEXT` keeps
    /// `curScreenX/Y`; it does not restart the campaign at mission selection.
    pub fn return_from_main_menu(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.splash = false;
        self.ready_to_load = false;
        self.fault = None;
        self.ensure_brain(ld, lg);
        self.enter(ld, lg);
    }

    /// The main menu's "skip one" restart: the briefing is the first screen. Like the restart, it
    /// does not reload the brain (`setToMissionBriefing` only switches screens in the C++).
    pub fn skip_to_briefing(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.end_current(lg);
        self.active = self.canonical;
        self.splash = false;
        self.ready_to_load = false;
        self.cur = Some((1, 1));
        self.fault = None;
        self.ensure_brain(ld, lg);
        self.enter(ld, lg);
    }

    /// `MissionBegin::begin`'s brain load: the mission's `getCurrentABLScript`, if the file exists.
    /// A script that does not load stops the sequence (the C++ asserts), so the tutorial's
    /// behaviour is never silently left out.
    fn load_brain(&mut self, ld: &mut Loader<'_>, lg: &Logistics) {
        let script = lg.mission_info().current_abl_script_name();
        match LogisticsBrain::load(ld.vfs, script) {
            Ok(brain) => {
                self.brain = brain;
                self.brain_loaded = true;
            }
            Err(e) => {
                log::error!("{e}");
                self.brain = None;
                self.brain_loaded = false;
                self.fault = Some(e);
            }
        }
    }

    /// Loads the brain unless a loaded one is still running. The restart and the skip paths of the
    /// C++ never call `MissionBegin::begin`, so a running brain keeps its state across them. A
    /// brain that failed to load or to run is loaded afresh (the restart is the retry).
    fn ensure_brain(&mut self, ld: &mut Loader<'_>, lg: &Logistics) {
        if !self.brain_loaded {
            self.load_brain(ld, lg);
        }
    }

    /// Takes the fault a page reports after it began or updated.
    fn note_fault(&mut self, idx: usize) {
        if let Some(e) = self.pages[idx].fault() {
            log::error!("logistics: {e}");
            self.fault = Some(e.to_string());
        }
    }

    /// Whether a logistics script is running (`logisticsBrain != NULL`).
    pub fn has_brain(&self) -> bool {
        self.brain.is_some()
    }

    /// Whether the loading screen was reached (`bReadyToLoad`).
    pub fn ready_to_load(&self) -> bool {
        self.ready_to_load
    }

    /// Connects the app's movie host to the pages' movie controls. Call it once, before the first
    /// [`Sequence::begin`]: from then on the pages send movie requests, and the host must drain
    /// [`Sequence::take_movie_requests`] after each frame. Until then the movie is reported
    /// unavailable.
    pub fn connect_movie_host(&mut self) {
        for p in self.pages.iter_mut() {
            p.connect_movie_host();
        }
    }

    /// The movie requests every page made since the last call, in slot order. A page's own
    /// requests keep their order (its `end` comes after its earlier requests, its `begin` after its
    /// `end`); the app's movie host applies them after each frame.
    pub fn take_movie_requests(&mut self) -> Vec<MovieRequest> {
        self.pages
            .iter_mut()
            .flat_map(|p| p.take_movie_requests())
            .collect()
    }

    /// The page on the current grid cell.
    pub fn current_page(&self) -> Option<usize> {
        let (x, y) = self.cur?;
        self.active[x][y]
    }

    fn at(&self, x: i64, y: i64) -> Option<usize> {
        let x = usize::try_from(x).ok().filter(|&x| x < DIM_X)?;
        let y = usize::try_from(y).ok().filter(|&y| y < DIM_Y)?;
        self.active[x][y]
    }

    /// `screens[x][y]->beginFadeIn(1); begin()`: the first screen of a visit fades in. A screen
    /// that is not wired up is not begun; the update reports it.
    fn enter(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        let Some(idx) = self.current_page() else {
            return;
        };
        if !self.pages[idx].is_available() {
            return;
        }
        self.pages[idx].begin_fade_in(1.0);
        self.pages[idx].begin(ld, lg);
        self.note_fault(idx);
    }

    fn end_current(&mut self, lg: &mut Logistics) {
        if let Some(idx) = self.current_page() {
            self.pages[idx].end(lg);
        }
    }

    /// `MissionBegin::update`. The current screen updates first; then, when it reports a status
    /// other than running, the sequence moves. A fault stops the sequence until it is begun again.
    pub fn update(
        &mut self,
        ld: &mut Loader<'_>,
        input: &mut UiInput,
        out: &mut UiOutput,
        dt: f32,
        lg: &mut Logistics,
    ) -> Flow {
        if self.splash {
            return Flow::MainMenu;
        }
        if self.fault.is_some() {
            return Flow::Failed;
        }
        if self.options_return.is_some() {
            {
                let assets = &*ld.assets;
                let mut ctx = UpdateCtx {
                    input,
                    assets,
                    dt,
                    out,
                };
                self.pages[slot::OPTIONS].update(&mut ctx, lg);
            }
            self.note_fault(slot::OPTIONS);
            if self.fault.is_some() {
                self.options_return = None;
                return Flow::Failed;
            }
            // the options close on Done or Previous; the grid page resumes where it was
            if matches!(
                self.pages[slot::OPTIONS].status(),
                Status::Done | Status::Previous
            ) {
                self.pages[slot::OPTIONS].end(lg);
                self.options_return = None;
            }
            return Flow::Running;
        }
        let Some((x, y)) = self.cur else {
            return Flow::Running;
        };
        let Some(idx) = self.current_page() else {
            return Flow::Running;
        };
        if !self.pages[idx].is_available() {
            return Flow::Unavailable(SLOT_NAMES[idx]);
        }
        if let Some(brain) = self.brain.as_mut() {
            let bindings = BrainBindings {
                cell: (x, y),
                visits: &mut self.visits,
                lg,
                callout: &mut self.callout,
            };
            if let Err(e) = brain.execute(bindings) {
                log::error!("{e}");
                self.brain = None;
                self.brain_loaded = false;
                self.fault = Some(e);
                return Flow::Failed;
            }
        }
        self.callout.update(self.pages[idx].base_mut(), input, dt);
        {
            let assets = &*ld.assets;
            let mut ctx = UpdateCtx {
                input,
                assets,
                dt,
                out,
            };
            self.pages[idx].update(&mut ctx, lg);
        }
        self.note_fault(idx);
        if self.fault.is_some() {
            return Flow::Failed;
        }
        // a part of the screen the player used is not wired up: reported, and the screen's status
        // is kept, so a move it asked for still happens on the next frame
        if let Some(what) = self.pages[idx].take_unavailable() {
            return Flow::Unavailable(what);
        }
        let status = self.pages[idx].status();
        if status == Status::GotoSplash {
            self.pages[idx].end(lg);
            self.splash = true;
            return Flow::MainMenu;
        }
        if status == Status::ReadyToLoad && x == 4 {
            self.ready_to_load = true;
        }
        let (xi, yi) = (x as i64, y as i64);
        let target = match status {
            Status::Running => return Flow::Running,
            Status::Next if x < 4 => (xi + 1, yi),
            Status::Next => return Flow::Launch,
            Status::Previous => (xi - 1, yi),
            Status::Down => (xi, yi + 1),
            Status::Up => (xi, yi - 1),
            Status::SkipOneNext if x < 2 => (xi + 2, yi),
            Status::SkipOneNext => return Flow::Launch,
            Status::SkipOnePrevious => (xi - 2, yi),
            Status::MainMenu => {
                self.splash = true;
                return Flow::MainMenu;
            }
            _ => return Flow::Running,
        };
        // a move onto a screen that is not wired up is refused: this screen stays, and its status
        // is cleared so the move is not asked for again every frame
        if let Some(next) = self.at(target.0, target.1) {
            if !self.pages[next].is_available() {
                self.pages[idx].set_status(Status::Running);
                return Flow::Unavailable(SLOT_NAMES[next]);
            }
        }
        self.pages[idx].end(lg);
        self.go(target.0, target.1, ld, lg, out);
        if self.fault.is_some() {
            return Flow::Failed;
        }
        Flow::Running
    }

    /// Moves to the cell `(x, y)`: its screen begins and the navigation sound plays. A cell
    /// outside the grid, or an empty one, is still moved to (as the C++ does), with nothing begun.
    fn go(&mut self, x: i64, y: i64, ld: &mut Loader<'_>, lg: &mut Logistics, out: &mut UiOutput) {
        let (Ok(nx), Ok(ny)) = (usize::try_from(x), usize::try_from(y)) else {
            return;
        };
        if let Some(next) = self.at(x, y) {
            self.pages[next].begin(ld, lg);
            self.note_fault(next);
        }
        self.cur = Some((nx, ny));
        out.sounds.push(super::LOG_NEXTBACKBUTTONS);
    }

    /// `render`: the current screen, at rest.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if self.splash {
            return;
        }
        if self.options_return.is_some() {
            self.pages[slot::OPTIONS].render(ctx);
            return;
        }
        if let Some(idx) = self.current_page() {
            self.pages[idx].render(ctx);
        }
    }

    /// Borrows a page (the app wires its screens through this, and tests drive them).
    pub fn page_mut(&mut self, idx: usize) -> &mut dyn Page {
        self.pages[idx].as_mut()
    }
}
