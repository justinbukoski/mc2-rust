// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The production GUI: the `mc2-ui` screens in the order the game shows them, over the campaign
//! state of `mc2-campaign`.
//!
//! ```text
//! main menu --NEW CAMPAIGN--> mission selection -> briefing -> mech bay -> pilot ready -> loading
//!     |  ^                 purchase / mech lab / campaign options                           |
//!     |  +--- results -> salvage -> pilot review -> next mission or menu      mission <-------+
//!     +--OPTIONS--> options (over the menu)
//! ```
//!
//! [`UiHost`] is the [`UiLayer`] [`crate::game::GameCore`] installs.  It owns the screens, the
//! campaign ([`Logistics`]), the GUI input ([`UiInput`]) and the 2D renderer, and acts on the game
//! states the core is in: it shows a screen in `MainMenu`, `Logistics` and `Results`, and hands
//! over with a [`StateRequest`] (`Logistics` for a new campaign, `StartMission` when the loading
//! screen finishes, `MainMenu`, `Exit`).  Nothing is drawn or consumed in a mission.
//!
//! The screens are `code/mainmenu.cpp`, `code/missionbegin.cpp` (the sequence controller) and the
//! screens it moves between, `code/loadscreen.cpp` and `code/optionsscreenwrapper.cpp`; see
//! `mc2-ui`.  Input comes from the core in game pixels and is converted to the screens' 800x600
//! space; the draw list is fitted into the frame by [`UiLayout`].
//!
//! **Sound.** What the screens ask for ([`mc2_ui::UiOutput`] sounds and Betty lines, the main
//! menu's tune, the options screen's volumes) goes out as [`UiCue`]s: to a [`UiCueSink`] when one
//! is set, otherwise to the core ([`UiLayer::take_audio`]), which plays them on its `AppAudio`.
//! Audio counts as available when either exists ([`UiLayer::set_audio_available`] carries the
//! core's side); without both, volume edits are refused and the player is told.
//!
//! **Not here (explicitly unavailable).** Multiplayer remains unavailable.
//! Every option of the options screen applies on YES and is written to `options.cfg` ([`AppliedSettings`]; D-90).
//! The mission loads in one step while the loading
//! screen is up (the core has no incremental load): the bar sits half full until the core is in
//! the mission, then it fills and the doors open.  A failed load shows the reason and returns to
//! the main menu.

use crate::campaign_save::{CampaignSaveService, SaveListing, SaveSlot};
use crate::layers::{AppliedSettings, DrawContext, UiFrame, UiLayer};
use crate::movie_control::{MovieControl, MovieEnd};
use crate::movie_render::{MoviePlacement, MovieRenderer};
use crate::pages::{self, OptionsSetup};
use crate::state::{GameState, StateRequest};
use mc2_campaign::interface::Outcome;
use mc2_campaign::{GameData, LoadSpec, Logistics, MissionOptions};
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;
use mc2_gfx::config::RenderConfig;
use mc2_platform::audio::{PlayMode, QueueSource, SharedMixer, MOVIE_AUDIO_CHANNEL};
use mc2_platform::input::{InputEvent, KeyStatus, MouseButton};
use mc2_platform::keys;
use mc2_platform::movie::{Movie, MoviePlayer, VideoFrame};
use mc2_platform::paths::UserPaths;
use mc2_ui::cursor::Cursors;
use mc2_ui::draw::{DrawList, GuiRect};
use mc2_ui::font::text::{self, TextAttrs};
use mc2_ui::font::FontRef;
use mc2_ui::input::UiInput;
use mc2_ui::logistics::briefing::Briefing;
use mc2_ui::logistics::mech_bay::MechBay;
use mc2_ui::logistics::mission_select::MissionSelect;
use mc2_ui::logistics::movie::{final_video_request, MovieRequest};
use mc2_ui::logistics::pilot_ready::PilotReady;
use mc2_ui::logistics::sequence::{slot, Flow, Sequence};
use mc2_ui::logistics::{Page, Placeholder};
use mc2_ui::logistics_dialog::{CampaignDialog, CampaignRow};
use mc2_ui::mechlopedia::Mechlopedia;
use mc2_ui::mission_results::MissionResults;
use mc2_ui::preview::ModelPreviews;
use mc2_ui::render::{ScaleMode, UiLayout, UiRenderer};
use mc2_ui::screen::{LogisticsScreen, Status};
use mc2_ui::screens::{
    LoadingScreen, MainMenu, MainMenuAction, OptionsEnv, OptionsResult, OptionsScreen, Prefs,
};
use mc2_ui::screens::{SaveLoadDialog, SaveLoadEvent, SaveRowInfo};
use mc2_ui::solo_missions::SoloMissions;
use mc2_ui::{Loader, RenderCtx, UiAssets, UiOutput, UpdateCtx};
use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;

/// The size of the screens' coordinate space (`Environment.screenWidth/Height` for the menus).
const UI_SIZE: (u32, u32) = (800, 600);

/// `IDS_LAWYER_BABBLE_FONT`: the font of the message banner.
const BANNER_FONT: u32 = 60018;
/// How long a message stays on screen, in seconds.
const BANNER_SECONDS: f32 = 6.0;

/// One thing the screens ask the audio to do (`sndSystem` calls).
#[derive(Clone, Debug, PartialEq)]
pub enum UiCue {
    /// `playDigitalSample(id)`.
    Sound(i32),
    /// `playBettySample(id)`.
    Betty(i32),
    /// `playDigitalMusic(id)`.
    Music(i64),
    /// `stopDigitalMusic()` before a full-screen cutscene.
    StopMusic,
    /// The options screen's audio tab: master, music, sfx, radio, Betty.
    Volumes([i32; 5]),
}

/// Where the host sends its [`UiCue`]s (the app audio module).
pub trait UiCueSink {
    fn cues(&mut self, cues: &[UiCue]);
}

/// Which screen the player sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveScreen {
    /// None: not started yet, or the game is in a mission / loading it.
    None,
    MainMenu,
    Options,
    /// A page of the logistics sequence ([`mc2_ui::logistics::sequence::slot`]).
    Logistics(usize),
    /// The mission results screens (salvage, pilot review) after a won mission.
    Results,
    /// The Mechlopedia screen.
    Mechlopedia,
    /// The list of solo missions (main menu "SINGLE MISSION").
    SoloMissions,
    /// The numbered campaign save/load slot picker.
    SaveLoad,
    /// The campaign list New Campaign opens (`LogisticsSaveDialog::beginCampaign`): the campaign
    /// and the tutorial.
    CampaignPick,
}

/// One button of the active screen, for the visual QA harness ([`UiHost::button_regions`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonRegion {
    pub id: i32,
    /// The button's label string id; 0 when it has none (an icon button).
    pub text_id: i32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub enabled: bool,
}

/// Observable timing for a movie currently shown by the production UI host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoviePlaybackProgress {
    /// Video presentation clock, in seconds from the first picture.
    pub clock: f64,
    /// Timestamp of the frame most recently supplied to the renderer.
    pub displayed_frame_time: Option<f64>,
    /// Stereo sample frames waiting in the app audio mixer.
    pub queued_audio_frames: usize,
}

/// `LoadScreen` as a page of the sequence (slot [`slot::LOADING`]).
///
/// It runs its doors in, reports ready to load, and (once the host has finished the load) runs the
/// doors out and reports done, which launches the mission.
pub struct LoadingPage {
    pub loading: LoadingScreen,
    status: Status,
    outro: bool,
    /// Door sounds not yet handed to the cue sink.
    sounds: Vec<i32>,
}

/// What opening the New Campaign list came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CampaignPick {
    Opened,
    /// No campaign script: the original starts `campaign` at once.
    NoCampaigns,
    /// The list could not be built (its layout is missing or unreadable).
    Failed,
}

impl LoadingPage {
    pub fn load(ld: &mut Loader<'_>) -> Result<LoadingPage, String> {
        Ok(LoadingPage {
            loading: LoadingScreen::load(ld)?,
            status: Status::Running,
            outro: false,
            sounds: Vec::new(),
        })
    }

    /// The mission is loaded: fill the bar and close the doors.
    fn finish(&mut self) {
        if !self.outro {
            self.outro = true;
            self.loading.set_progress(100.0);
            self.loading.begin_outro();
            // `LoadScreen::update`: the opening sound once progress passes 99
            self.sounds.push(mc2_ui::ctx::sfx::LOAD_DOORS_OPENING);
        }
    }
}

impl Page for LoadingPage {
    fn begin(&mut self, _ld: &mut Loader<'_>, _lg: &mut Logistics) {
        self.outro = false;
        self.status = Status::Running;
        self.loading.begin();
        // `LoadScreen::begin`: the doors close on entry
        self.sounds.push(mc2_ui::ctx::sfx::LOAD_DOORS_CLOSING);
    }
    fn end(&mut self, _lg: &mut Logistics) {}
    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _lg: &mut Logistics) {
        self.status = self.loading.update(ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.loading.render(ctx);
    }
    fn begin_fade_in(&mut self, t: f32) {
        self.loading.screen.begin_fade_in(t);
    }
    fn status(&self) -> Status {
        self.status
    }
    fn set_status(&mut self, status: Status) {
        self.status = status;
    }
    fn base(&self) -> &LogisticsScreen {
        &self.loading.screen
    }
    fn base_mut(&mut self) -> &mut LogisticsScreen {
        &mut self.loading.screen
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

enum Mode {
    Menu,
    SaveLoad(SaveLoadScreen),
    /// The campaign choice over the main menu (`LogisticsSaveDialog::beginCampaign`).
    CampaignPick(Box<CampaignDialog>),
    Options(Box<OptionsScreen>),
    Sequence,
    /// The salvage and pilot review screens after a won mission (`MissionResults`).
    Results(Box<MissionResults>),
    Mechlopedia(Box<mc2_ui::mechlopedia::Mechlopedia>),
    /// The solo mission list over the main menu (`MPLoadMap::beginSingleMission`).
    Solo(Box<SoloMissions>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveLoadAction {
    Save,
    Load,
}

/// Why the dialog is sliding out.
enum SaveLoadClosing {
    Cancel,
    Act(SaveLoadAction, SaveSlot),
}

/// The GAME LIST rows and their info boxes for `action` (the autosave is listed for Load only).
fn save_rows(
    listing: SaveListing,
    action: SaveLoadAction,
) -> (Vec<(SaveSlot, String)>, Vec<SaveRowInfo>) {
    let table = mc2_ui::strings::StringTable::builtin();
    let (rows, infos): (Vec<(SaveSlot, String)>, Vec<SaveRowInfo>) = listing
        .saves
        .into_iter()
        .filter(|row| action == SaveLoadAction::Load || row.slot != SaveSlot::Autosave)
        .map(|row| {
            let name = match &row.slot {
                SaveSlot::Autosave => "Autosave".to_string(),
                SaveSlot::Numbered(number) => format!("Slot {number:02}"),
                SaveSlot::Named(name) => name.clone(),
            };
            // `IDS_DIALOG_CBILLS`: "C-BILLS : %ld"
            let cbills = table
                .get(27678)
                .unwrap_or("C-BILLS : %ld")
                .replace("%ld", &row.c_bills.to_string());
            // `updateMissionInfo`: campaign, mission, C-Bills and the time of the save
            let mission = match (&row.current_mission_name, row.current_mission_name_id) {
                (Some(name), _) => name.clone(),
                (None, Some(id)) => table.get(id).unwrap_or_default().to_string(),
                (None, None) => String::new(),
            };
            let (date, time) = format_save_time(row.modified);
            let when = table
                .get(27679)
                .unwrap_or("TIME : %s  %s")
                .replacen("%s", &date, 1)
                .replacen("%s", &time, 1);
            let mut text = format!("{}\n", row.campaign_name);
            if !mission.is_empty() {
                text.push_str(&format!("{mission}\n"));
            }
            text.push_str(&format!("{cbills}\n{when}"));
            let info = SaveRowInfo {
                name: name.clone(),
                text,
                mission_file: row.current_mission_file.clone(),
            };
            ((row.slot, name), info)
        })
        .unzip();
    (rows, infos)
}

struct SaveLoadScreen {
    action: SaveLoadAction,
    rows: Vec<(SaveSlot, String)>,
    selected: usize,
    /// The panel is leaving; the host moves on once `OutAnim` has played.
    closing: Option<SaveLoadClosing>,
    filename: String,
    pending_overwrite: Option<SaveSlot>,
    /// The delete prompt is up for this save (`bDeletePrompt`).
    pending_delete: Option<SaveSlot>,
    return_to_logistics: bool,
    /// The original dialog art (`mcl_sm.fit`); the slot rules stay in this host.
    dialog: Box<SaveLoadDialog>,
}

/// `mis_PLAYER_WIN_BIG` (`code/mission.h:46`): the only result that goes on to the results screens
/// (`code/logistics.cpp:130`).
use crate::audio::MIS_PLAYER_WIN_BIG;

/// Everything that needs the game data to exist.
struct Inner {
    assets: UiAssets,
    input: UiInput,
    cursors: Option<Cursors>,
    banner_font: FontRef,
    lg: Logistics,
    menu: MainMenu,
    seq: Option<Sequence>,
    mode: Mode,
    /// The New Campaign list after it has answered No: its panel slides out over the menu.
    campaign_closing: Option<Box<CampaignDialog>>,
    /// The campaign the player confirmed; it starts once the panel has slid out.
    campaign_pending: Option<String>,
    prefs: Prefs,
    original: Prefs,
    /// The `options.cfg` an accepted options edit is written to (`prefs.save()`).
    options_path: Option<std::path::PathBuf>,
    /// A new campaign was started from the menu.
    campaign_started: bool,
    /// The campaign main menu was opened from the logistics sequence; Save/Cancel returns there.
    menu_return_to_logistics: bool,
    /// The Mechlopedia screen (shown from the main menu).
    mechlopedia: Option<Box<Mechlopedia>>,
    /// A separate offscreen renderer for the Mechlopedia's selected chassis model. It produces
    /// CPU RGBA pictures which the production UI renderer uploads on its next frame.
    mechlopedia_previews: Option<ModelPreviews>,
    /// The offscreen renderer for the 3D mech pictures of the logistics pages (`mechCamera`),
    /// made on the first draw of a page. `Err` once it could not be made: the pages show their
    /// fallback and nothing is retried.
    logistics_previews: Option<Result<ModelPreviews, String>>,
    /// The data root, for the preview renderer's own game data.
    data_root: std::path::PathBuf,
}

pub struct UiHost {
    inner: Option<Inner>,
    cue_sink: Option<Box<dyn UiCueSink>>,
    /// Cues for the core's audio ([`UiLayer::take_audio`]) while no sink is set.
    audio_out: Vec<UiCue>,
    /// The core has a game sound installed (told every frame).
    core_audio: bool,
    /// Events since the last frame, in game pixels.
    pending: Vec<InputEvent>,
    game_size: (f32, f32),
    /// A UI state is current (set by the last update).
    shown: bool,
    error: Option<String>,
    failed: bool,
    notices: Vec<String>,
    /// The message on screen (over any screen) and the seconds it has left.
    banner: Option<(String, f32)>,
    banner_is_movie_failure: bool,
    /// Distinct messages waiting for their own visible banner interval.
    pending_banners: VecDeque<String>,
    launch: Option<LoadSpec>,
    single_mission_on_result: Option<String>,
    /// Test seam: start the campaign with `Sequence::begin` (mission selection first).
    full_logistics_start: bool,
    /// The mission just ended: its `scenarioResult` and what it reports to the campaign, until the
    /// first Results update begins the results flow.
    results_pending: Option<(u32, Option<Outcome>)>,
    /// `lastMissionResult` (`code/logistics.cpp:131`).
    last_result: u32,
    /// Accepted options the core has not applied yet.
    applied: Option<AppliedSettings>,
    /// The start-up `options.cfg` has been handed to the core (see `update`).
    startup_options_read: bool,
    /// The loading screen asked the core to load the mission; its doors stay until the load is
    /// done (a failed load drops it with the sequence).
    holding: bool,
    /// The pause menu's OPTIONS opened the options screen over the mission (`bInvokeOptionsScreenFlag`,
    /// `code/mechcmd2.cpp:2125`): the mission does not run until the screen is done.
    mission_overlay: bool,
    /// The options screen over the mission is to be built on the next update.
    mission_options_requested: bool,
    open_mission_options_on_load: bool,
    renderer: Option<UiRenderer>,
    movie_mixer: Option<SharedMixer>,
    movie_control: Option<MovieControl>,
    final_video_active: bool,
    save_service: Option<CampaignSaveService>,
    config_dir: Option<std::path::PathBuf>,
    movie_player: Option<MoviePlayer>,
    movie_frame: Option<VideoFrame>,
    movie_placement: MoviePlacement,
    movie_skip_key: Option<u32>,
    resume_logistics_music: bool,
    movie_renderer: Option<MovieRenderer>,
    intro_movie_attempted: bool,
    pending_movie: Option<String>,
    pending_movie_requests: Vec<MovieRequest>,
    dt: f32,
}

impl Default for UiHost {
    fn default() -> Self {
        UiHost::new()
    }
}

/// `holding`: the loading screen asked the core to load the mission and is still on screen.
fn is_ui_state(s: &GameState, holding: bool) -> bool {
    match s {
        GameState::MainMenu | GameState::Logistics | GameState::Results(_) => true,
        GameState::MissionLoading(_) | GameState::InMission => holding,
        _ => false,
    }
}

fn centre(b: &mc2_ui::widgets::AnimButton) -> (f32, f32) {
    (
        b.obj.global_x() as f32 + b.obj.width() / 2.0,
        b.obj.global_y() as f32 + b.obj.height() / 2.0,
    )
}

/// What the core takes from accepted options (`CPrefs::save` copies them into the globals the game reads).
fn applied_from(p: &Prefs, resolution: Option<(u32, u32)>) -> AppliedSettings {
    AppliedSettings {
        shadows: p.use_shadows,
        left_right_mouse_profile: p.use_left_right_mouse_profile,
        use_non_weapon_effects: p.use_non_weapon_effects,
        terrain_detail: p.use_water_interest_texture,
        high_object_detail: p.use_high_object_detail,
        local_shadows: p.use_local_shadows,
        pilot_videos: p.pilot_videos,
        async_mouse: p.async_mouse,
        renderer: p.renderer,
        hot_keys: p.keys.clone(),
        resolution,
        saved_resolution: None,
    }
}

/// The player's saved options (`CPrefs::load`, `code/prefs.cpp:118`): the user's `options.cfg` in
/// `config_dir`, else the game's own, else the defaults.
fn saved_prefs(vfs: &mut Vfs, config_dir: Option<&std::path::Path>) -> Prefs {
    config_dir
        .and_then(|dir| std::fs::read(mc2_platform::paths::options_read_path(dir)).ok())
        .and_then(|bytes| FitIni::from_bytes(bytes).ok())
        .map(|mut fit| Prefs::from_fit(&mut fit))
        .or_else(|| load_prefs(vfs, "options.cfg"))
        .unwrap_or_default()
}

fn load_prefs(vfs: &mut Vfs, name: &str) -> Option<Prefs> {
    let mut fit = vfs.read_fit(name).ok().flatten()?;
    Some(Prefs::from_fit(&mut fit))
}

/// The date and time of a save (`gos_GetFormattedDate` / `gos_GetFormattedTime`) in local time,
/// as `LogisticsSaveDialog::updateMissionInfo` shows them (`localtime`).
fn format_save_time(t: std::time::SystemTime) -> (String, String) {
    format_time_in(chrono::DateTime::<chrono::Local>::from(t))
}

/// `(MM/DD/YYYY, HH:MM:SS)` of `t` in whatever zone it carries; production passes `Local`.
fn format_time_in<Tz: chrono::TimeZone>(t: chrono::DateTime<Tz>) -> (String, String)
where
    Tz::Offset: std::fmt::Display,
{
    (
        t.format("%m/%d/%Y").to_string(),
        t.format("%H:%M:%S").to_string(),
    )
}

fn push_output(cues: &mut Vec<UiCue>, out: &UiOutput) {
    cues.extend(out.sounds.iter().map(|&s| UiCue::Sound(s)));
    cues.extend(out.betty.iter().map(|&b| UiCue::Betty(b)));
}

/// What one frame of the active screen asks of the game.
enum Step {
    None,
    Request(StateRequest),
}

impl Inner {
    /// Show the campaign-aware main menu after leaving a campaign screen.
    fn begin_main_menu(&mut self) {
        self.menu.can_save = self.campaign_started && !self.lg.mission_info().is_single_mission();
        if self.campaign_started {
            // The startup splash keeps Save disabled. Once a campaign has been entered, the
            // main-menu save/load flow is available even when returning from logistics.
            self.menu.set_draw_background(false);
        }
        self.menu.begin();
    }

    fn load(vfs: &mut Vfs, config_dir: Option<&std::path::Path>) -> Result<Inner, String> {
        let data = Arc::new(GameData::load(vfs).map_err(|e| format!("campaign data: {e}"))?);
        let lg = Logistics::new(Arc::clone(&data)).map_err(|e| format!("campaign: {e}"))?;
        let mut assets = UiAssets::default();
        let menu = MainMenu::load(&mut Loader::new(vfs, &mut assets))?;
        let cursors = match Cursors::load(vfs, &mut assets.textures, "cursors", UI_SIZE.0) {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("ui: no cursors: {e}");
                None
            }
        };
        let banner_font = Loader::new(vfs, &mut assets).font(BANNER_FONT);
        let config_dir = config_dir
            .map(std::path::Path::to_path_buf)
            .or_else(|| UserPaths::resolve().map(|p| p.config));
        let options_path = config_dir.as_deref().map(|dir| dir.join("options.cfg"));
        let prefs = saved_prefs(vfs, config_dir.as_deref());
        let original = load_prefs(vfs, "orgprefs.cfg").unwrap_or_default();
        assets.set_player_paint(prefs.base_color, prefs.highlight_color);
        Ok(Inner {
            assets,
            input: UiInput::new(UI_SIZE.0, UI_SIZE.1),
            cursors,
            banner_font,
            lg,
            menu,
            seq: None,
            mode: Mode::Menu,
            campaign_closing: None,
            campaign_pending: None,
            prefs,
            original,
            options_path,
            campaign_started: false,
            menu_return_to_logistics: false,
            mechlopedia: None,
            mechlopedia_previews: None,
            logistics_previews: None,
            data_root: vfs.root().to_path_buf(),
        })
    }

    /// The pages of a new run of the sequence, with production campaign adapters installed.
    fn build_sequence(
        vfs: &mut Vfs,
        assets: &mut UiAssets,
        prefs: Prefs,
        original: Prefs,
        volumes_live: bool,
        config_dir: Option<&std::path::Path>,
    ) -> Result<Sequence, String> {
        let mut ld = Loader::new(vfs, assets);
        let mut pages: Vec<Box<dyn Page>> = vec![
            Box::new(MissionSelect::load(&mut ld)?),
            Box::new(Briefing::load(&mut ld)?),
            Box::new(MechBay::load(&mut ld)?),
            Box::new(PilotReady::load(&mut ld)?),
            Box::new(Placeholder::unavailable("purchase")),
            Box::new(Placeholder::unavailable("mech lab")),
            Box::new(LoadingPage::load(&mut ld)?),
            Box::new(Placeholder::unavailable("options")),
        ];
        let config_path = config_dir
            .map(std::path::Path::to_path_buf)
            .or_else(|| UserPaths::resolve().map(|p| p.config))
            .ok_or_else(|| "no per-user config directory is available".to_string())?
            .join("options.cfg");
        std::fs::create_dir_all(config_path.parent().expect("config path has a parent"))
            .map_err(|e| format!("create user config directory: {e}"))?;
        pages::install(
            vfs,
            assets,
            &mut pages,
            OptionsSetup {
                prefs,
                original,
                env: OptionsEnv::default(),
                volumes_live,
                persist_to: Some(config_path),
            },
        )?;
        Ok(Sequence::single_player(pages))
    }
}

impl UiHost {
    pub fn new() -> UiHost {
        UiHost {
            inner: None,
            cue_sink: None,
            audio_out: Vec::new(),
            core_audio: false,
            pending: Vec::new(),
            game_size: (UI_SIZE.0 as f32, UI_SIZE.1 as f32),
            shown: false,
            error: None,
            failed: false,
            notices: Vec::new(),
            banner: None,
            banner_is_movie_failure: false,
            pending_banners: VecDeque::new(),
            launch: None,
            single_mission_on_result: None,
            full_logistics_start: false,
            results_pending: None,
            last_result: 0,
            applied: None,
            startup_options_read: false,
            holding: false,
            mission_overlay: false,
            mission_options_requested: false,
            open_mission_options_on_load: false,
            renderer: None,
            movie_mixer: None,
            movie_control: None,
            final_video_active: false,
            save_service: CampaignSaveService::new_default().ok(),
            config_dir: None,
            movie_player: None,
            movie_frame: None,
            movie_placement: MoviePlacement::FullScreen,
            movie_skip_key: None,
            resume_logistics_music: false,
            movie_renderer: None,
            intro_movie_attempted: false,
            pending_movie: None,
            pending_movie_requests: Vec::new(),
            dt: 1.0 / 30.0,
        }
    }

    /// Sends the screens' sounds and music to `sink` from now on.
    pub fn set_cue_sink(&mut self, sink: Box<dyn UiCueSink>) {
        self.cue_sink = Some(sink);
    }

    fn start_movie(
        &mut self,
        vfs: &mut Vfs,
        name: &str,
        placement: MoviePlacement,
        stop_music: bool,
    ) {
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(name);
        let vfs_name = format!("data/movies/{stem}.mpg");
        match Movie::open_vfs(vfs, &vfs_name) {
            Ok(movie) => {
                let mut writer = None;
                if movie.info().has_audio {
                    if let Some(mixer) = &self.movie_mixer {
                        let (source, queue) = QueueSource::new(2, movie.info().audio_sample_rate);
                        let mut mixer = mixer.lock();
                        mixer.assign(MOVIE_AUDIO_CHANNEL, Box::new(source));
                        mixer.set_mode(MOVIE_AUDIO_CHANNEL, PlayMode::PlayOnce);
                        writer = Some(queue);
                    }
                }
                self.movie_player = Some(MoviePlayer::new(movie, writer));
                self.movie_placement = placement;
                self.resume_logistics_music = stop_music;
                if stop_music {
                    self.send(&[UiCue::StopMusic]);
                }
            }
            Err(error) => {
                self.say_movie_failure(format!("movie unavailable ({name}): {error}"));
            }
        }
    }

    fn advance_movie(&mut self, vfs: &mut Vfs, state: &GameState, dt: f32) -> bool {
        if self.movie_player.is_none() {
            if matches!(state, GameState::MainMenu) && !self.intro_movie_attempted {
                self.intro_movie_attempted = true;
                self.start_movie(vfs, "msft", MoviePlacement::FullScreen, false);
            } else if matches!(state, GameState::Logistics) {
                if let Some(name) = self.pending_movie.take() {
                    self.start_movie(vfs, &name, MoviePlacement::LogisticsCutscene, true);
                }
            }
        }
        let Some(player) = self.movie_player.as_mut() else {
            return false;
        };
        if let Some(frame) = player.advance(f64::from(dt.max(0.0))) {
            self.movie_frame = Some(frame);
        }
        if let Some(error) = player.take_error() {
            let message = format!("movie playback failed: {error}");
            self.say_movie_failure(message);
            self.finish_movie();
            return false;
        }
        if player.is_finished() {
            self.finish_movie();
            false
        } else {
            true
        }
    }

    fn consume_movie_requests(&mut self, vfs: &mut Vfs, requests: &[MovieRequest]) {
        let selection_movie_rect = self
            .inner
            .as_mut()
            .and_then(|inner| inner.seq.as_mut())
            .and_then(|seq| {
                seq.page_mut(slot::SELECT)
                    .as_any_mut()
                    .downcast_mut::<MissionSelect>()
            })
            .and_then(|select| select.movie_rect());
        for request in requests {
            if matches!(request, MovieRequest::FinalVideo(_)) {
                self.final_video_active = true;
                self.movie_placement = MoviePlacement::FullScreen;
            } else if matches!(
                request,
                MovieRequest::Open(_)
                    | MovieRequest::Play
                    | MovieRequest::Pause(_)
                    | MovieRequest::Stop
            ) {
                if let Some(rect) = selection_movie_rect {
                    self.movie_placement = MoviePlacement::MissionSelectVidCom { rect };
                }
            }
            let (result, cues) = {
                let control = self
                    .movie_control
                    .get_or_insert_with(|| MovieControl::new(self.movie_mixer.clone()));
                let result = control.apply(vfs, request);
                (result, control.take_cues())
            };
            if let Err(error) = result {
                if error.starts_with("movie unavailable")
                    || error.starts_with("movie playback failed")
                {
                    self.say_movie_failure(error);
                } else {
                    self.say(error);
                }
            }
            let cues = cues
                .into_iter()
                .map(|cue| match cue {
                    crate::audio::Cue::Sound(id) => UiCue::Sound(id),
                    crate::audio::Cue::Betty(id) => UiCue::Betty(id),
                    crate::audio::Cue::Music(id) => UiCue::Music(id),
                    crate::audio::Cue::StopMusic => UiCue::StopMusic,
                    crate::audio::Cue::Volumes(volumes) => UiCue::Volumes(volumes),
                })
                .collect::<Vec<_>>();
            self.send(&cues);
        }
    }

    fn drain_pending_movie_requests(&mut self, vfs: &mut Vfs) {
        let requests = std::mem::take(&mut self.pending_movie_requests);
        self.consume_movie_requests(vfs, &requests);
    }

    fn advance_movie_control(&mut self, vfs: &mut Vfs, dt: f32) -> Option<StateRequest> {
        let (result, cues, end) = {
            let control = self.movie_control.as_mut()?;
            let result = control.advance(vfs, dt);
            (result, control.take_cues(), control.take_end())
        };
        if let Err(error) = result {
            self.say(error);
        }
        let cues = cues
            .into_iter()
            .map(|cue| match cue {
                crate::audio::Cue::Sound(id) => UiCue::Sound(id),
                crate::audio::Cue::Betty(id) => UiCue::Betty(id),
                crate::audio::Cue::Music(id) => UiCue::Music(id),
                crate::audio::Cue::StopMusic => UiCue::StopMusic,
                crate::audio::Cue::Volumes(volumes) => UiCue::Volumes(volumes),
            })
            .collect::<Vec<_>>();
        self.send(&cues);
        if end.is_some_and(|end| end == MovieEnd::MainMenu) {
            self.final_video_active = false;
            if let Some(inner) = self.inner.as_mut() {
                inner.mode = Mode::Menu;
                inner.begin_main_menu();
            }
            return Some(StateRequest::MainMenu);
        }
        None
    }

    fn stop_movie(&mut self) {
        if let Some(player) = self.movie_player.as_mut() {
            player.skip();
        }
        self.finish_movie();
    }

    fn finish_movie(&mut self) {
        self.release_movie_audio();
        self.movie_player = None;
        self.movie_frame = None;
        if self.resume_logistics_music {
            self.resume_logistics_music = false;
            let tune = self
                .inner
                .as_ref()
                .map(|inner| inner.lg.mission_info().current_logistics_tune_id())
                .unwrap_or(-1);
            if tune >= 0 {
                self.send(&[UiCue::Music(tune)]);
            }
        }
    }

    fn release_movie_audio(&mut self) {
        if let Some(mixer) = &self.movie_mixer {
            mixer.lock().release(MOVIE_AUDIO_CHANNEL);
        }
    }

    /// The screen the player sees.
    pub fn screen(&self) -> ActiveScreen {
        let Some(inner) = self.inner.as_ref().filter(|_| self.shown) else {
            return ActiveScreen::None;
        };
        match &inner.mode {
            Mode::Menu => ActiveScreen::MainMenu,
            Mode::Options(_) => ActiveScreen::Options,
            Mode::Results(_) => ActiveScreen::Results,
            Mode::Solo(_) => ActiveScreen::SoloMissions,
            Mode::SaveLoad(_) => ActiveScreen::SaveLoad,
            Mode::CampaignPick(_) => ActiveScreen::CampaignPick,
            Mode::Sequence => inner
                .seq
                .as_ref()
                .and_then(|seq| {
                    if seq.in_options() {
                        Some(slot::OPTIONS)
                    } else {
                        seq.current_page()
                    }
                })
                .map_or(ActiveScreen::None, ActiveScreen::Logistics),
            Mode::Mechlopedia(_) => ActiveScreen::Mechlopedia,
        }
    }

    /// The campaign state the screens read and change (`LogisticsData::instance`).
    /// `None` until a new campaign has been started from the menu.
    pub fn campaign(&self) -> Option<&Logistics> {
        self.inner
            .as_ref()
            .filter(|i| i.campaign_started)
            .map(|i| &i.lg)
    }

    pub fn campaign_mut(&mut self) -> Option<&mut Logistics> {
        self.inner
            .as_mut()
            .filter(|i| i.campaign_started)
            .map(|i| &mut i.lg)
    }

    /// `lastMissionResult`: the `scenarioResult` of the mission whose results this host last began.
    pub fn last_mission_result(&self) -> u32 {
        self.last_result
    }

    /// The running sequence (a new campaign has begun one).
    pub fn sequence_mut(&mut self) -> Option<&mut Sequence> {
        self.inner.as_mut()?.seq.as_mut()
    }

    /// The `options.cfg` an accepted options edit is written to.
    pub fn options_file(&self) -> Option<&std::path::Path> {
        self.inner.as_ref()?.options_path.as_deref()
    }

    /// The options the player has accepted (`options.cfg` at start-up until then).
    pub fn prefs(&self) -> Option<&Prefs> {
        self.inner.as_ref().map(|i| &i.prefs)
    }

    /// Set the options prefs (for testing). This replaces the prefs that will be used for the next
    /// campaign launch's `load_spec`.
    pub fn set_prefs(&mut self, prefs: Prefs) {
        if let Some(inner) = self.inner.as_mut() {
            inner
                .assets
                .set_player_paint(prefs.base_color, prefs.highlight_color);
            inner.prefs = prefs;
        }
    }

    /// Set the save directory used by the production menu (tests may use `$TMPDIR`).
    pub fn set_campaign_save_service(&mut self, service: CampaignSaveService) {
        self.save_service = Some(service);
    }

    /// Isolates the host's persistent options file (headless runs use a directory under `$TMPDIR`).
    pub fn set_config_directory(&mut self, directory: std::path::PathBuf) {
        self.config_dir = Some(directory);
    }

    /// The directory campaign saves go to, if any.
    pub fn campaign_save_directory(&self) -> Option<&std::path::Path> {
        self.save_service.as_ref().map(|s| s.directory())
    }

    /// Isolates everything the host persists: options in `config_dir`, campaign saves in its sibling
    /// `saves` directory (unless `MC2_SAVE_DIR` names one explicitly).
    pub fn set_private_user_directory(&mut self, config_dir: std::path::PathBuf) {
        if std::env::var_os("MC2_SAVE_DIR").is_none() {
            let saves = config_dir
                .parent()
                .map(|p| p.join("saves"))
                .unwrap_or_else(|| config_dir.join("saves"));
            self.save_service = Some(CampaignSaveService::new(saves));
        }
        self.config_dir = Some(config_dir);
    }

    /// Screens or parts of screens the player used that are not wired up, in order.
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    /// Current playback timing, for the app's headless playback evidence and diagnostics.
    pub fn movie_progress(&self) -> Option<MoviePlaybackProgress> {
        let player = self.movie_player.as_ref()?;
        Some(MoviePlaybackProgress {
            clock: player.clock(),
            displayed_frame_time: self.movie_frame.as_ref().map(|frame| frame.time),
            queued_audio_frames: player.queued_audio_frames(),
        })
    }

    /// The clock of the movie selected in mission-selection (the screen's Play/Pause/Stop controls).
    pub fn selection_movie_clock(&self) -> Option<f64> {
        self.movie_control.as_ref()?.clock()
    }

    /// Injects a movie request into the production consumer for headless integration tests.
    #[doc(hidden)]
    pub fn request_movie_for_test(&mut self, request: MovieRequest) {
        self.pending_movie_requests.push(request);
    }

    /// The hand-over of the mission the loading screen launched (`Logistics::beginMission`), for
    /// the simulation to build its force from.  Returned once.
    pub fn take_launch(&mut self) -> Option<LoadSpec> {
        self.launch.take()
    }

    /// Starts the next New Campaign at mission selection (`MissionBegin::begin`) instead of the
    /// RESTART route, so tests can walk the logistics pages of a `skipLogistics()` mission.
    #[doc(hidden)]
    pub fn start_campaign_at_selection(&mut self) {
        self.full_logistics_start = true;
    }

    /// Makes the next mission end see a single-mission campaign (`LogisticsData::setSingleMission`):
    /// its win skips the results screens.  For the tests of that path.
    #[doc(hidden)]
    pub fn set_single_mission_on_next_result(&mut self, mission: &str) {
        self.single_mission_on_result = Some(mission.to_string());
    }

    /// Replaces campaign state before the next real GameCore result dispatch (test seam).
    #[doc(hidden)]
    pub fn replace_campaign_for_result_test(&mut self, campaign: Logistics) {
        if let Some(inner) = self.inner.as_mut() {
            inner.lg = campaign;
            inner.campaign_started = true;
        }
    }

    /// The current launch without consuming it (the simulation load reads it).
    pub fn launch_spec(&self) -> Option<&LoadSpec> {
        self.launch.as_ref()
    }

    /// The home commander's paint from the current launch (`LoadSpec::force[0].colors`), or
    /// from the mission options (`prefs.base_color`, `prefs.highlight_color`) if the force is
    /// empty (quickstart launch).  Returns `None` if there is no launch.  The caller can retrieve
    /// it without consuming the spec (use this before `take_launch()` if both are needed).
    pub fn launch_campaign_paint(&self) -> Option<mc2_render::mission::PaintSource> {
        let spec = self.launch.as_ref()?;
        // If the force is not empty, use the first mech's colors (they come from the campaign's
        // home commander's prefs in `load_spec`).
        if let Some(force) = spec.force.first() {
            let [base, highlight, _] = force.colors;
            return Some(mc2_render::mission::PaintSource::Scheme { base, highlight });
        }
        // For quickstart launches (empty force), the colors come from the mission options that
        // were passed to `load_spec`.  The UI host stores the options in `prefs` when it creates
        // the launch spec (ui_host.rs:839-844).
        let prefs = self.inner.as_ref()?.prefs.clone();
        Some(mc2_render::mission::PaintSource::Scheme {
            base: prefs.base_color,
            highlight: prefs.highlight_color,
        })
    }

    /// The buttons the active screen shows, in the screens' 800x600 space.  This is the visual QA
    /// harness's locator for label regions (`rust/tools/visual-qa`): it reads the same button
    /// objects the screen draws and hit-tests.
    pub fn button_regions(&mut self) -> Vec<ButtonRegion> {
        fn of(screen: &mc2_ui::screen::LogisticsScreen, out: &mut Vec<ButtonRegion>) {
            for b in &screen.buttons {
                if !b.is_showing() || b.state == mc2_ui::widgets::ButtonState::Hidden {
                    continue;
                }
                out.push(ButtonRegion {
                    id: b.id(),
                    text_id: b.data.text_id,
                    x: b.obj.global_x() as f32,
                    y: b.obj.global_y() as f32,
                    w: b.obj.width(),
                    h: b.obj.height(),
                    enabled: b.state != mc2_ui::widgets::ButtonState::Disabled,
                });
            }
        }
        let mut out = Vec::new();
        let Some(inner) = self.inner.as_mut() else {
            return out;
        };
        match &inner.mode {
            Mode::Menu => of(&inner.menu.screen, &mut out),
            Mode::Options(o) => of(&o.screen, &mut out),
            Mode::Mechlopedia(m) => of(&m.base, &mut out),
            Mode::Solo(d) => of(&d.screen, &mut out),
            Mode::Results(r) => match r.stage() {
                Some(mc2_campaign::results::ResultsStage::Salvage) => {
                    of(&r.salvage.screen, &mut out)
                }
                Some(mc2_campaign::results::ResultsStage::PilotReview) => {
                    of(&r.pilots.screen, &mut out)
                }
                _ => {}
            },
            Mode::Sequence => {
                if let Some(seq) = inner.seq.as_mut() {
                    if let Some(page) = seq.current_page() {
                        of(seq.page_mut(page).base(), &mut out);
                    }
                }
            }
            Mode::SaveLoad(_) | Mode::CampaignPick(_) => {}
        }
        out
    }

    /// The centre of a main menu button, in the screens' 800x600 space.
    pub fn menu_button(&self, id: i32) -> Option<(f32, f32)> {
        Some(centre(self.inner.as_ref()?.menu.screen.button(id)?))
    }

    /// The campaigns the New Campaign list offers, top to bottom: the script stem and the name shown
    /// (`None` when the list is not open).
    pub fn campaign_pick_rows(&self) -> Option<Vec<(String, String)>> {
        match &self.inner.as_ref()?.mode {
            Mode::CampaignPick(d) => Some(
                d.rows()
                    .iter()
                    .map(|r| (r.stem.clone(), r.name.clone()))
                    .collect(),
            ),
            _ => None,
        }
    }

    /// The selected row of the campaign list and its mission info (the mission name shown under the
    /// map picture); `None` when it is not open or no row is selected.
    pub fn campaign_pick_selection(&self) -> Option<(String, String)> {
        match &self.inner.as_ref()?.mode {
            Mode::CampaignPick(d) => d.selected().map(|r| (r.stem.clone(), d.mission_text())),
            _ => None,
        }
    }

    /// The centres of the campaign list's Load and Cancel buttons (`None` when it is not open).
    pub fn campaign_pick_buttons(&self) -> Option<((f32, f32), (f32, f32))> {
        match &self.inner.as_ref()?.mode {
            Mode::CampaignPick(d) => Some((d.button_centre(true)?, d.button_centre(false)?)),
            _ => None,
        }
    }

    /// The centre of row `i` of the campaign list (`None` when it is not open).
    pub fn campaign_pick_row_centre(&self, i: usize) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::CampaignPick(d) => d.row_centre(i),
            _ => None,
        }
    }

    /// Whether the New Campaign panel is still sliding out after a No.
    /// The centre of row `i` of the Save / Load list, in UI coordinates.
    pub fn save_load_row_centre(&self, i: usize) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => s.dialog.row_centre(i),
            _ => None,
        }
    }

    /// The Save / Load panel has slid in and takes input.
    pub fn save_load_ready(&self) -> bool {
        match self.inner.as_ref().map(|i| &i.mode) {
            Some(Mode::SaveLoad(s)) => s.dialog.entered() && s.closing.is_none(),
            _ => false,
        }
    }

    /// The info text under the Save / Load list (`textObjects[2]`) and whether the mission picture
    /// is showing.
    pub fn save_load_info(&self) -> Option<(String, bool)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => Some((s.dialog.info_text(), s.dialog.picture_shown())),
            _ => None,
        }
    }

    /// The name the Save dialog's box shows, and the name a save would use.
    pub fn save_load_names(&self) -> Option<(String, String)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => Some((s.dialog.entry(), s.filename.clone())),
            _ => None,
        }
    }

    /// The centre of the name box, in UI coordinates.
    pub fn save_load_edit_point(&self) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => s.dialog.edit_centre(),
            _ => None,
        }
    }

    /// A point inside the list but below its last row, if there is room.
    pub fn save_load_blank_point(&self) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => s.dialog.blank_point(),
            _ => None,
        }
    }

    /// The selected row and the Save dialog's filename.
    pub fn save_load_selected(&self) -> Option<usize> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => Some(s.selected),
            _ => None,
        }
    }

    /// The centre of the overwrite prompt's YES (or NO) button, in UI coordinates.
    pub fn save_load_prompt_button(&self, yes: bool) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => s.dialog.prompt_button_centre(yes),
            _ => None,
        }
    }

    /// The centre of the DELETE button, in UI coordinates, and whether it is enabled.
    pub fn save_load_delete_button(&self) -> Option<((f32, f32), bool)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => s.dialog.delete_button(),
            _ => None,
        }
    }

    /// The centre of the SAVE / LOAD button and whether it is enabled.
    pub fn save_load_confirm_button(&self) -> Option<((f32, f32), bool)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => s.dialog.confirm_button(),
            _ => None,
        }
    }

    /// The centre of the CANCEL button and whether it is enabled.
    pub fn save_load_cancel_button(&self) -> Option<((f32, f32), bool)> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => s.dialog.cancel_button(),
            _ => None,
        }
    }

    /// The file name a Save would use.
    pub fn save_load_filename(&self) -> Option<String> {
        match &self.inner.as_ref()?.mode {
            Mode::SaveLoad(s) => Some(s.filename.clone()),
            _ => None,
        }
    }

    pub fn campaign_pick_leaving(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|i| i.campaign_closing.is_some())
    }

    /// The menu pointer in UI pixels and whether the cursor is drawn (inside the window).
    pub fn menu_pointer(&self) -> Option<((i32, i32), bool)> {
        let i = self.inner.as_ref()?;
        Some((
            (i.input.mouse_x(), i.input.mouse_y()),
            i.input.draw_mouse && i.input.gos().cursor_inside_window(),
        ))
    }

    /// The names the solo mission list shows, top to bottom (`None` when it is not open).
    pub fn solo_mission_names(&self) -> Option<Vec<String>> {
        match &self.inner.as_ref()?.mode {
            Mode::Solo(d) => Some(d.maps.iter().map(|m| m.name.clone()).collect()),
            _ => None,
        }
    }

    /// The file stems behind [`Self::solo_mission_names`].
    pub fn solo_mission_files(&self) -> Option<Vec<String>> {
        match &self.inner.as_ref()?.mode {
            Mode::Solo(d) => Some(d.maps.iter().map(|m| m.file.clone()).collect()),
            _ => None,
        }
    }

    /// Whether the 3D picture `key` ([`mc2_ui::preview::PreviewRequest::key`]) has been rendered
    /// for a logistics page.
    pub fn picture_rendered(&self, key: &str) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|i| i.assets.textures.find(key).is_some())
    }

    /// The centre of row `row` of the solo mission list, in UI pixels.
    pub fn solo_mission_row(&self, row: usize) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Solo(d) => {
                let o = d.list.item(row)?.obj();
                Some((
                    o.global_x() as f32 + o.width() / 2.0,
                    o.global_y() as f32 + o.height() / 2.0,
                ))
            }
            _ => None,
        }
    }

    /// The centre of a button of the solo mission list (`7` Yes, `8` No), in UI pixels.
    pub fn solo_mission_button(&self, id: i32) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Solo(d) => d.screen.button(id).map(centre),
            _ => None,
        }
    }

    pub fn mechlopedia_button(&self, id: i32) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Mechlopedia(screen) => Some(centre(screen.base.button(id)?)),
            _ => None,
        }
    }

    /// The centre of a button of the options screen, while it is open.
    pub fn options_button(&self, id: i32) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => Some(centre(o.screen.button(id)?)),
            _ => None,
        }
    }

    /// The gameplay tab's paint swatches as `(centre, 0xAARRGGBB)`, while the options are open.
    pub fn options_swatches(&self) -> Vec<((f32, f32), u32)> {
        match self.inner.as_ref().map(|i| &i.mode) {
            Some(Mode::Options(o)) => o.gameplay_swatches(),
            _ => Vec::new(),
        }
    }

    /// The centre of a control (check box, radio button) of the shown options tab.
    pub fn options_tab_control(&self, id: i32) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => o.tab_control_point(id),
            _ => None,
        }
    }

    /// The centre of the right-step button of the audio tab's `slider`-th volume slider.
    pub fn options_volume_step(&self, slider: usize) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => o.volume_step_point(slider),
            _ => None,
        }
    }

    /// The centre of the left-step button of the audio tab's `slider`-th volume slider.
    pub fn options_volume_decrease(&self, slider: usize) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => o.volume_decrease_point(slider),
            _ => None,
        }
    }

    /// The centre of a button of page `page` of the sequence.
    pub fn page_button(&mut self, page: usize, id: i32) -> Option<(f32, f32)> {
        let seq = self.sequence_mut()?;
        Some(centre(seq.page_mut(page).base().button(id)?))
    }

    /// The centre of a Done or specialty button on the current results screen.
    pub fn results_button(&self, id: i32) -> Option<(f32, f32)> {
        let Some(Mode::Results(results)) = self.inner.as_ref().map(|i| &i.mode) else {
            return None;
        };
        match results.stage()? {
            mc2_campaign::results::ResultsStage::Salvage => {
                Some(centre(results.salvage.screen.button(id)?))
            }
            mc2_campaign::results::ResultsStage::PilotReview => {
                // the confirm button belongs to the promotion pop-up
                let button = results
                    .pilots
                    .screen
                    .button(id)
                    .or_else(|| results.pilots.choose_button().filter(|b| b.id() == id))?;
                Some(centre(button))
            }
            mc2_campaign::results::ResultsStage::Done => None,
        }
    }

    /// What the results flow holds: (salvage offers, active pilots in the review, whether the
    /// review's timed reveals have all run).  Test-support for the visual-qa scenario check.
    pub fn results_content(&self) -> Option<(usize, usize, bool)> {
        let Some(Mode::Results(results)) = self.inner.as_ref().map(|i| &i.mode) else {
            return None;
        };
        let flow = results.flow()?;
        Some((
            flow.offers().len(),
            flow.pilot_review().map_or(0, |r| r.active.len()),
            results.pilots.list.is_done(),
        ))
    }

    /// The first enabled specialty in the visible pilot-review picker.  This is a test-support
    /// locator only: callers still select it by the same pointer input as a player.
    pub fn results_specialty_option(&self) -> Option<(f32, f32)> {
        let Some(Mode::Results(results)) = self.inner.as_ref().map(|i| &i.mode) else {
            return None;
        };
        results.pilots.picker_pilot()?;
        let promotion = &results.pilots.promotion;
        if !promotion.entered() {
            return None;
        }
        promotion
            .list
            .items
            .iter()
            .zip(&promotion.rows)
            .find(|(_, skill)| skill.is_some())
            .map(|(item, _)| {
                let rect = item.obj().global_rect();
                (
                    (rect.left + rect.right) as f32 / 2.0,
                    (rect.top + rect.bottom) as f32 / 2.0,
                )
            })
    }

    /// The results screens while they run, for the tests that look at what they show.
    pub fn results(&self) -> Option<&MissionResults> {
        match self.inner.as_ref().map(|i| &i.mode) {
            Some(Mode::Results(r)) => Some(r),
            _ => None,
        }
    }

    /// The centre of an options control on the in-campaign Options page.
    pub fn campaign_options_control_point(&mut self, id: i32) -> Option<(f32, f32)> {
        let options = self
            .sequence_mut()?
            .page_mut(slot::OPTIONS)
            .as_any_mut()
            .downcast_mut::<pages::CampaignOptions>()?;
        options.options.tab_control_point(id)
    }

    /// Picks the `idx`-th display mode of the open options screen.
    pub fn select_options_resolution(&mut self, idx: usize) -> bool {
        match self.inner.as_mut().map(|i| &mut i.mode) {
            Some(Mode::Options(o)) => o.select_resolution(idx),
            _ => false,
        }
    }

    /// The resolution the open options screen currently has selected.
    pub fn options_edited_resolution(&self) -> Option<(i32, i32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => {
                let p = o.edited_prefs();
                Some((p.resolution_x, p.resolution_y))
            }
            _ => None,
        }
    }

    /// The centre of the open options screen's resolution drop list button.
    pub fn options_resolution_button(&self) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => Some(o.resolution_expand_point()),
            _ => None,
        }
    }

    /// The centre of the `i`-th entry of the open, expanded resolution list.
    pub fn options_resolution_item(&self, i: usize) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => o.resolution_item_point(i),
            _ => None,
        }
    }

    /// The centre of a row of the open options screen's hot key list.
    pub fn options_hotkey_row(&self, row: usize) -> Option<(f32, f32)> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => o.hotkey_row_point(row),
            _ => None,
        }
    }

    /// All the options the open options screen currently has, as edited.
    pub fn options_edited_prefs(&self) -> Option<Prefs> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => Some(o.edited_prefs()),
            _ => None,
        }
    }

    /// The hot keys the open options screen currently has, per command.
    pub fn options_edited_keys(&self) -> Option<Vec<u32>> {
        match &self.inner.as_ref()?.mode {
            Mode::Options(o) => Some(o.edited_prefs().keys),
            _ => None,
        }
    }

    /// Shows an options tab (a click on its button does the same).
    pub fn select_options_tab(&mut self, tab: usize) {
        if let Some(Inner {
            mode: Mode::Options(o),
            ..
        }) = self.inner.as_mut()
        {
            o.select_tab(tab);
        }
    }

    /// The message the player is shown, while it lasts.
    pub fn banner(&self) -> Option<&str> {
        self.banner.as_ref().map(|(t, _)| t.as_str())
    }

    /// Whether the main-menu splash is still showing behind the intro movie.
    pub fn menu_intro_showing(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|inner| !inner.menu.intro_over)
    }

    /// Records `line` and shows it over the screen.
    fn say(&mut self, line: String) {
        log::warn!("ui: {line}");
        match self.banner.as_ref() {
            Some((current, _)) if current == &line => {}
            Some(_) if self.banner_is_movie_failure => {
                if !self.pending_banners.contains(&line) {
                    self.pending_banners.push_back(line.clone());
                }
            }
            _ => {
                self.banner = Some((line.clone(), BANNER_SECONDS));
                self.banner_is_movie_failure = false;
                self.pending_banners.clear();
            }
        }
        if self.notices.last() != Some(&line) {
            self.notices.push(line);
        }
    }

    fn say_movie_failure(&mut self, line: String) {
        log::warn!("ui: {line}");
        self.banner = Some((line.clone(), BANNER_SECONDS));
        self.banner_is_movie_failure = true;
        self.pending_banners.clear();
        if self.notices.last() != Some(&line) {
            self.notices.push(line);
        }
    }

    fn notice(&mut self, what: &str) {
        self.say(format!("{what} is not available yet"));
    }

    fn send(&mut self, cues: &[UiCue]) {
        if cues.is_empty() {
            return;
        }
        if let Some(s) = self.cue_sink.as_mut() {
            s.cues(cues);
        } else {
            self.audio_out.extend_from_slice(cues);
        }
    }

    fn layout(&self) -> UiLayout {
        let (w, h) = self.game_size;
        UiLayout::new(
            UI_SIZE,
            (w.round().max(1.0) as u32, h.round().max(1.0) as u32),
            ScaleMode::Fit,
        )
    }

    /// Feeds the events of the frame in 800x600 coordinates and starts the frame.
    fn begin_input(&mut self) {
        let layout = self.layout();
        let events = std::mem::take(&mut self.pending);
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        for e in events {
            let e = match e {
                InputEvent::CursorMoved { x, y } => {
                    let (ux, uy) = layout.to_ui(x as f32, y as f32);
                    InputEvent::CursorMoved {
                        x: f64::from(ux),
                        y: f64::from(uy),
                    }
                }
                e => e,
            };
            inner.input.handle(&e);
        }
        inner.input.begin_frame(self.dt);
    }

    /// New Campaign: `LogisticsSaveDialog::beginCampaign` lists the campaign scripts. With none
    /// the original starts `campaign` at once; otherwise the first is selected and the player
    /// confirms one.
    fn open_campaign_pick(&mut self, vfs: &mut Vfs) -> CampaignPick {
        let Some(inner) = self.inner.as_mut() else {
            return CampaignPick::Failed;
        };
        let rows = CampaignRow::from_data(inner.lg.data());
        if rows.is_empty() {
            return CampaignPick::NoCampaigns;
        }
        let mut ld = Loader::new(vfs, &mut inner.assets);
        let mut dialog = match CampaignDialog::load(&mut ld) {
            Ok(d) => Box::new(d),
            Err(e) => {
                log::error!("New Campaign list: {e}");
                return CampaignPick::Failed;
            }
        };
        dialog.begin(&mut ld, rows);
        inner.input.flush_keys();
        inner.campaign_closing = None;
        inner.campaign_pending = None;
        inner.mode = Mode::CampaignPick(dialog);
        CampaignPick::Opened
    }

    /// Runs the campaign list and starts the chosen campaign script.
    fn campaign_pick_frame(&mut self) -> Step {
        let dt = self.dt;
        let inner = self.inner.as_mut().expect("loaded");
        let Mode::CampaignPick(dialog) = &mut inner.mode else {
            return Step::None;
        };
        let mut out = UiOutput::default();
        let answer = {
            let mut ctx = UpdateCtx {
                input: &mut inner.input,
                assets: &inner.assets,
                dt,
                out: &mut out,
            };
            dialog.update(&mut ctx)
        };
        let mut cues = Vec::new();
        push_output(&mut cues, &out);
        match answer {
            None => {
                self.send(&cues);
                Step::None
            }
            Some(Status::Yes) => {
                let name = dialog.chosen.clone();
                // the panel slides out while the campaign loads
                if let Mode::CampaignPick(dialog) = std::mem::replace(&mut inner.mode, Mode::Menu) {
                    inner.campaign_closing = Some(dialog);
                }
                inner.campaign_pending = Some(name);
                self.send(&cues);
                Step::None
            }
            Some(_) => {
                // the panel slides out over the menu, which takes input again at once
                let Mode::CampaignPick(dialog) = std::mem::replace(&mut inner.mode, Mode::Menu)
                else {
                    return Step::None;
                };
                inner.campaign_closing = Some(dialog);
                inner.menu.resume();
                self.send(&cues);
                Step::None
            }
        }
    }

    /// The menu has been left for the campaign: start it and show mission selection.
    fn new_campaign(&mut self, vfs: &mut Vfs, campaign: &str) -> Option<StateRequest> {
        let volumes_live = self.core_audio || self.cue_sink.is_some();
        let inner = self.inner.as_mut()?;
        let full_start = self.full_logistics_start;
        let mut movie = None;
        let started = Inner::build_sequence(
            vfs,
            &mut inner.assets,
            inner.prefs.clone(),
            inner.original.clone(),
            volumes_live,
            self.config_dir.as_deref(),
        )
        .and_then(|mut seq| {
            seq.connect_movie_host();
            inner
                .lg
                .start_new_campaign(campaign)
                .map_err(|e| format!("new campaign: {e}"))?;
            movie = inner
                .lg
                .take_current_big_video()
                .filter(|name| !name.is_empty());
            // the main menu's New Campaign is `MissionBegin::update`'s RESTART status
            // (`code/missionbegin.cpp:478`): a `skipLogistics()` mission goes to Loading, or to
            // mission selection, never through briefing, mech bay and pilot ready
            let mut loader = Loader::new(vfs, &mut inner.assets);
            if full_start {
                seq.begin(&mut loader, &mut inner.lg);
            } else {
                seq.restart(&mut loader, &mut inner.lg);
            }
            Ok(seq)
        });
        match started {
            Ok(seq) => {
                self.pending_movie = movie;
                inner.seq = Some(seq);
                inner.campaign_started = true;
                inner.menu_return_to_logistics = false;
                inner.mode = Mode::Sequence;
                // `MissionSelectionScreen::update` plays the logistics tune on entering
                let tune = inner.lg.mission_info().current_logistics_tune_id();
                if tune >= 0 {
                    self.send(&[UiCue::Music(tune)]);
                }
                Some(StateRequest::Logistics)
            }
            Err(e) => {
                self.say(format!("The campaign could not be started ({e})"));
                self.error = Some(e);
                self.inner.as_mut()?.menu.resume();
                None
            }
        }
    }

    fn menu_frame(&mut self, vfs: &mut Vfs) -> Step {
        let dt = self.dt;
        if let Some(inner) = self.inner.as_mut() {
            if let Some(d) = inner.campaign_closing.as_mut() {
                d.anim.update(dt);
                if !d.anim.leaving() {
                    inner.campaign_closing = None;
                }
            }
            if inner.campaign_closing.is_none() {
                if let Some(name) = inner.campaign_pending.take() {
                    return self
                        .new_campaign(vfs, &name)
                        .map_or(Step::None, Step::Request);
                }
            } else if inner.campaign_pending.is_some() {
                // the confirmed dialog is still sliding out: it is modal, the menu takes no input
                // (mainmenu.cpp:509-587 returns after updating the dialog)
                // Pending presses are dropped but the pointer position and window-entry state
                // survive, so the cursor stays visible and in place (logisticsdialog.cpp:150-151).
                let (x, y) = inner.input.real_mouse_pos();
                let inside = inner.input.gos().cursor_inside_window();
                inner.input = UiInput::new(UI_SIZE.0, UI_SIZE.1);
                if inside {
                    inner.input.handle(&InputEvent::CursorMoved {
                        x: f64::from(x),
                        y: f64::from(y),
                    });
                }
                inner.input.begin_frame(dt);
                inner.input.end_frame();
                return Step::None;
            }
        }
        let mut cues = Vec::new();
        let mut out = UiOutput::default();
        let frame = {
            let inner = self.inner.as_mut().expect("loaded");
            let mut ctx = UpdateCtx {
                input: &mut inner.input,
                assets: &inner.assets,
                dt,
                out: &mut out,
            };
            inner.menu.update(&mut ctx)
        };
        push_output(&mut cues, &out);
        if let Some(t) = frame.music {
            cues.push(UiCue::Music(i64::from(t)));
        }
        self.send(&cues);
        let Some(action) = frame.action else {
            return Step::None;
        };
        match action {
            MainMenuAction::NewCampaign => {
                // `beginCampaign` finding no campaign file starts `campaign` at once
                match self.open_campaign_pick(vfs) {
                    CampaignPick::Opened => return Step::None,
                    CampaignPick::NoCampaigns => {
                        return self
                            .new_campaign(vfs, "campaign")
                            .map_or(Step::None, Step::Request);
                    }
                    CampaignPick::Failed => {
                        // the dialog could not be built: no campaign starts, the menu stays usable
                        if let Some(inner) = self.inner.as_mut() {
                            inner.menu.resume();
                        }
                    }
                }
            }
            MainMenuAction::Options => {
                if !self.open_options(vfs) {
                    self.inner.as_mut().expect("loaded").menu.resume();
                }
            }
            MainMenuAction::Quit => return Step::Request(StateRequest::Exit),
            MainMenuAction::SingleMission => self.open_solo_missions(vfs),
            MainMenuAction::ReturnToGame => {
                // `MM_MSG_RETURN_TO_GAME` (`code/mainmenu.cpp:320-331`): back to the logistics page that
                // opened the menu, with the campaign's tune
                let tune = self
                    .inner
                    .as_ref()
                    .map_or(-1, |i| i.lg.mission_info().current_logistics_tune_id());
                if tune >= 0 {
                    self.send(&[UiCue::Music(tune)]);
                }
                return self.resume_menu_to_logistics(vfs);
            }
            MainMenuAction::SaveGame => {
                self.open_save_load(vfs, SaveLoadAction::Save);
            }
            MainMenuAction::LoadGame => {
                self.open_save_load(vfs, SaveLoadAction::Load);
            }
            other => {
                let name = match other {
                    MainMenuAction::Multiplayer => "multiplayer",
                    MainMenuAction::Encyclopedia => "the Mechlopedia",
                    _ => "this menu entry",
                };

                // Handle Encyclopedia - create and show the screen
                if let MainMenuAction::Encyclopedia = other {
                    let inner = self.inner.as_mut().expect("loaded");
                    let ld = &mut Loader::new(vfs, &mut inner.assets);
                    match Mechlopedia::load(ld) {
                        Ok(mut m) => {
                            // Use the shipped campaign data, options, and art on the production
                            // path before the first selected row is drawn.
                            let data = inner.lg.data();
                            m.begin(&inner.assets, data, &inner.lg);
                            m.set_unlimited_ammo(inner.prefs.use_unlimited_ammo, &inner.assets);
                            m.set_preview_paint(
                                inner.prefs.base_color,
                                inner.prefs.highlight_color,
                                &inner.assets,
                            );
                            m.load_visuals(&mut Loader::new(vfs, &mut inner.assets));
                            let preview_result = (|| -> Result<ModelPreviews, String> {
                                let gpu = mc2_render::gpu::Gpu::headless(&RenderConfig::from_env())
                                    .map_err(|e| e.to_string())?;
                                let data = mc2_render::game::GameData::open(vfs.root())
                                    .map_err(|e| e.to_string())?;
                                ModelPreviews::new(Arc::new(gpu), data).map_err(|e| e.to_string())
                            })();
                            let (previews, preview_available) = match preview_result {
                                Ok(previews) => (Some(previews), true),
                                Err(e) => {
                                    // The 2D encyclopedia remains usable if a platform cannot
                                    // make the auxiliary offscreen renderer; do not turn that
                                    // recoverable presentation failure into an app-level error.
                                    log::warn!("Mechlopedia preview renderer: {e}");
                                    (None, false)
                                }
                            };
                            m.set_preview_available(preview_available, &inner.assets);
                            inner.mechlopedia_previews = previews;
                            inner.mechlopedia = Some(Box::new(m));
                            inner.mode = Mode::Mechlopedia(inner.mechlopedia.take().unwrap());
                        }
                        Err(e) => {
                            self.error = Some(format!("mechlopedia: {e}"));
                            inner.menu.resume();
                            return Step::None;
                        }
                    }
                } else {
                    self.notice(name);
                    self.inner.as_mut().expect("loaded").menu.resume();
                }
            }
        }
        Step::None
    }

    /// `MM_MSG_SINGLE_MISSION` (`code/mainmenu.cpp:364-381`): the list of solo missions opens over
    /// the menu.
    fn open_solo_missions(&mut self, vfs: &mut Vfs) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        let loaded = SoloMissions::load(&mut Loader::new(vfs, &mut inner.assets));
        match loaded {
            Ok(mut dialog) => {
                dialog.begin(&mut Loader::new(vfs, &mut inner.assets));
                if let Ok(row) = usize::try_from(dialog.list.selected_item()) {
                    dialog.show_picture(&mut Loader::new(vfs, &mut inner.assets), row);
                }
                inner.input.flush_keys();
                inner.mode = Mode::Solo(Box::new(dialog));
            }
            Err(e) => {
                self.error = Some(format!("solo missions: {e}"));
                inner.menu.resume();
            }
        }
    }

    /// One frame of the solo mission list: picking a row shows its picture; Yes starts the map
    /// (`LogisticsData::setSingleMission`, then the briefing: `SKIPONENEXT`), No returns to the menu.
    fn solo_frame(&mut self, vfs: &mut Vfs) -> Step {
        let dt = self.dt;
        let mut out = UiOutput::default();
        let inner = self.inner.as_mut().expect("loaded");
        let Mode::Solo(dialog) = &mut inner.mode else {
            return Step::None;
        };
        let update = {
            let mut ctx = UpdateCtx {
                input: &mut inner.input,
                assets: &inner.assets,
                dt,
                out: &mut out,
            };
            dialog.update(&mut ctx)
        };
        if let Some(row) = update.picked {
            dialog.show_picture(&mut Loader::new(vfs, &mut inner.assets), row);
        }
        let picked = dialog.picked_file().map(str::to_string);
        let mut cues = Vec::new();
        push_output(&mut cues, &out);
        match update.closed {
            Some(mc2_ui::screen::Status::Yes) => {
                self.send(&cues);
                return match picked {
                    Some(file) => self.start_solo_mission(vfs, &file),
                    None => Step::None,
                };
            }
            Some(_) => {
                // `beginAnim.begin()`: back to the menu
                inner.mode = Mode::Menu;
                inner.menu.resume();
            }
            None => {}
        }
        self.send(&cues);
        Step::None
    }

    /// The Yes of the solo mission list: `setSingleMission(file)` replaces the campaign with a
    /// one-mission game, and the sequence starts at the briefing.
    fn start_solo_mission(&mut self, vfs: &mut Vfs, file: &str) -> Step {
        let volumes_live = self.core_audio || self.cue_sink.is_some();
        let Some(inner) = self.inner.as_mut() else {
            return Step::None;
        };
        let path = format!("data/missions/{file}.fit");
        let started = (|| -> Result<Sequence, String> {
            let mut fit = vfs
                .read_fit(&path)
                .map_err(|e| format!("{path}: {e}"))?
                .ok_or_else(|| format!("{path}: no such mission"))?;
            let settings = mc2_sim_data::campaign::MissionSettings::from_fit(&mut fit)
                .map_err(|e| format!("{path}: {e}"))?;
            // `readIdFloat("DropWeightLimit")` on the block `readMissionInfo` left selected
            let _ = fit.seek_block("MissionSettings");
            let drop_weight = fit.read_f32("DropWeightLimit").ok();
            let mut seq = Inner::build_sequence(
                vfs,
                &mut inner.assets,
                inner.prefs.clone(),
                inner.original.clone(),
                volumes_live,
                self.config_dir.as_deref(),
            )?;
            seq.connect_movie_host();
            inner.lg.set_single_mission(file, &settings, drop_weight);
            seq.skip_to_briefing(&mut Loader::new(vfs, &mut inner.assets), &mut inner.lg);
            Ok(seq)
        })();
        match started {
            Ok(seq) => {
                inner.seq = Some(seq);
                inner.campaign_started = true;
                inner.menu_return_to_logistics = false;
                inner.mode = Mode::Sequence;
                Step::Request(StateRequest::Logistics)
            }
            Err(e) => {
                self.say(format!("The mission could not be started ({e})"));
                self.error = Some(e);
                if let Some(inner) = self.inner.as_mut() {
                    inner.mode = Mode::Menu;
                    inner.menu.resume();
                }
                Step::None
            }
        }
    }

    /// Builds the options screen (`OptionsScreenWrapper::begin`); `false` when it would not load.
    fn open_options(&mut self, vfs: &mut Vfs) -> bool {
        let inner = self.inner.as_mut().expect("loaded");
        let env = OptionsEnv::default();
        let screen = OptionsScreen::load(
            &mut Loader::new(vfs, &mut inner.assets),
            inner.prefs.clone(),
            inner.original.clone(),
            &env,
        );
        match screen {
            Ok(o) => {
                // keys pressed on the menu (the splash skip) are not hot key edits
                inner.input.flush_keys();
                inner.mode = Mode::Options(Box::new(o));
                true
            }
            Err(e) => {
                self.error = Some(format!("options: {e}"));
                false
            }
        }
    }

    fn open_save_load(&mut self, vfs: &mut Vfs, action: SaveLoadAction) {
        let Some(service) = self.save_service.as_ref() else {
            self.say("Campaign save storage is unavailable".to_string());
            if let Some(inner) = self.inner.as_mut() {
                inner.menu.resume();
            }
            return;
        };
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        if action == SaveLoadAction::Save && !inner.campaign_started {
            self.say("Start a campaign before saving".to_string());
            if let Some(inner) = self.inner.as_mut() {
                inner.menu.resume();
            }
            return;
        }
        let listing = match service.list(Arc::clone(inner.lg.data())) {
            Ok(listing) => listing,
            Err(error) => {
                self.say(format!("Campaign slots could not be read ({error})"));
                if let Some(inner) = self.inner.as_mut() {
                    inner.menu.resume();
                }
                return;
            }
        };
        let (rows, infos) = save_rows(listing, action);
        if rows.is_empty() && action == SaveLoadAction::Load {
            self.say("There are no campaign saves to load".to_string());
            if let Some(inner) = self.inner.as_mut() {
                inner.menu.resume();
            }
            return;
        }
        if let Some(inner) = self.inner.as_mut() {
            let mut dialog = match SaveLoadDialog::load(&mut Loader::new(vfs, &mut inner.assets)) {
                Ok(dialog) => Box::new(dialog),
                Err(error) => {
                    log::error!("Save / Load dialog: {error}");
                    inner.menu.resume();
                    return;
                }
            };
            dialog.begin(
                &mut Loader::new(vfs, &mut inner.assets),
                action == SaveLoadAction::Load,
                &infos,
            );
            inner.input.flush_keys();
            inner.mode = Mode::SaveLoad(SaveLoadScreen {
                action,
                rows,
                selected: 0,
                closing: None,
                filename: String::new(),
                pending_overwrite: None,
                pending_delete: None,
                return_to_logistics: inner.menu_return_to_logistics,
                dialog,
            });
        }
    }

    fn save_load_frame(&mut self, vfs: &mut Vfs) -> Step {
        let volumes_live = self.core_audio || self.cue_sink.is_some();
        let dt = self.dt;
        let mut cues = Vec::new();
        let mut notice: Option<String> = None;
        let (selected, cancelled_to_logistics) = {
            let inner = self.inner.as_mut().expect("loaded");
            let Mode::SaveLoad(screen) = &mut inner.mode else {
                return Step::None;
            };
            let mut out = UiOutput::default();
            let events = {
                let mut ctx = UpdateCtx {
                    input: &mut inner.input,
                    assets: &inner.assets,
                    dt,
                    out: &mut out,
                };
                screen.dialog.update(&mut ctx)
            };
            push_output(&mut cues, &out);
            let cancel_key = inner.input.key_status(keys::KEY_ESCAPE) == KeyStatus::Released;
            let mut from_closing = false;
            let result = if screen.closing.is_some() {
                // the panel is sliding out; nothing else takes input
                if screen.dialog.leaving() {
                    (None, false)
                } else {
                    from_closing = true;
                    match screen.closing.take() {
                        Some(SaveLoadClosing::Act(action, slot)) => (Some((action, slot)), false),
                        _ => {
                            let return_to_logistics = screen.return_to_logistics;
                            inner.mode = Mode::Menu;
                            inner.menu.resume();
                            (None, return_to_logistics)
                        }
                    }
                }
            } else if let Some(slot) = screen.pending_overwrite.clone() {
                let answer = events.iter().find_map(|e| match e {
                    SaveLoadEvent::Overwrite(yes) => Some(*yes),
                    _ => None,
                });
                if let Some(confirmed) = answer {
                    screen.pending_overwrite = None;
                    (confirmed.then_some((SaveLoadAction::Save, slot)), false)
                } else {
                    (None, false)
                }
            } else if let Some(slot) = screen.pending_delete.clone() {
                let answer = events.iter().find_map(|e| match e {
                    SaveLoadEvent::DeleteAnswer(yes) => Some(*yes),
                    _ => None,
                });
                if let Some(confirmed) = answer {
                    screen.pending_delete = None;
                    if confirmed {
                        notice = Self::delete_save(self.save_service.as_ref(), vfs, inner, slot);
                    }
                }
                (None, false)
            } else if cancel_key || events.contains(&SaveLoadEvent::Cancel) {
                screen.closing = Some(SaveLoadClosing::Cancel);
                screen.dialog.end();
                (None, false)
            } else {
                if inner.input.key_status(keys::KEY_UP) == KeyStatus::Released {
                    screen.selected = screen.selected.saturating_sub(1);
                }
                if inner.input.key_status(keys::KEY_DOWN) == KeyStatus::Released
                    && !screen.rows.is_empty()
                {
                    screen.selected = (screen.selected + 1).min(screen.rows.len() - 1);
                }
                // a clicked-into name box edits the name itself; otherwise the keys do
                let editing = screen.action == SaveLoadAction::Save && screen.dialog.edit_focused();
                if editing {
                    screen.filename = screen
                        .dialog
                        .entry()
                        .chars()
                        .filter(|ch| ch.is_ascii_alphanumeric() || " ._-".contains(*ch))
                        .collect();
                } else if inner.input.key_status(keys::KEY_BACK) == KeyStatus::Released {
                    screen.filename.pop();
                }
                if screen.action == SaveLoadAction::Save && !editing {
                    loop {
                        let key = inner.input.pop_key();
                        if key == 0 {
                            break;
                        }
                        let ch = (key & 0xff) as u8 as char;
                        if (ch.is_ascii_alphanumeric() || " ._-".contains(ch))
                            && screen.filename.len() < 255
                        {
                            screen.filename.push(ch);
                        }
                    }
                }
                if events.contains(&SaveLoadEvent::Delete) {
                    if let Some((slot, name)) = Self::delete_target(screen) {
                        screen.pending_delete = Some(slot);
                        screen.dialog.ask_delete(&inner.assets, &name);
                    }
                }
                let mut activate = events.contains(&SaveLoadEvent::Confirm);
                for event in &events {
                    let SaveLoadEvent::Row(row) = *event else {
                        continue;
                    };
                    if row < screen.rows.len() {
                        screen.selected = row;
                        if screen.action == SaveLoadAction::Save {
                            screen.filename = Self::row_filename(&screen.rows[row].0);
                        } else {
                            activate = true;
                        }
                    }
                }
                let selected = if activate
                    || (screen.dialog.entered()
                        && inner.input.key_status(keys::KEY_RETURN) == KeyStatus::Released)
                {
                    Self::activate_save_load(screen)
                } else {
                    None
                };
                let selected = if let Some((SaveLoadAction::Save, slot)) = selected {
                    if screen.rows.iter().any(|(existing, _)| existing == &slot) {
                        screen.pending_overwrite = Some(slot);
                        let name = screen.filename.clone();
                        screen.dialog.ask_overwrite(&inner.assets, &name);
                        None
                    } else {
                        Some((SaveLoadAction::Save, slot))
                    }
                } else {
                    selected
                };
                (selected, false)
            };
            let result = match (result, &mut inner.mode) {
                ((Some((action, slot)), _), Mode::SaveLoad(screen)) if !from_closing => {
                    screen.closing = Some(SaveLoadClosing::Act(action, slot));
                    screen.dialog.end();
                    (None, false)
                }
                (result, _) => result,
            };
            if let Mode::SaveLoad(screen) = &mut inner.mode {
                let name = match screen.action {
                    SaveLoadAction::Save => screen.filename.clone(),
                    SaveLoadAction::Load => screen
                        .rows
                        .get(screen.selected)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_default(),
                };
                // logisticsdialog.cpp:728-742: confirmation needs a file name
                let can_confirm = !name.is_empty();
                screen
                    .dialog
                    .show(&inner.assets, Some(screen.selected), &name);
                screen.dialog.set_confirm_enabled(can_confirm);
            }
            result
        };
        self.send(&cues);
        if let Some(message) = notice {
            self.say(message);
        }
        if cancelled_to_logistics {
            return self.resume_menu_to_logistics(vfs);
        }
        let Some((action, slot)) = selected else {
            return Step::None;
        };
        let Some(service) = self.save_service.as_ref() else {
            self.say("Campaign save storage is unavailable".to_string());
            return Step::None;
        };
        match action {
            SaveLoadAction::Save => {
                let Some(inner) = self.inner.as_ref() else {
                    return Step::None;
                };
                let save_result = match slot {
                    SaveSlot::Numbered(number) => service.save_slot(number, &inner.lg),
                    SaveSlot::Named(name) => service.save_named(&name, &inner.lg),
                    SaveSlot::Autosave => return Step::None,
                };
                match save_result {
                    Ok(()) => self.say("Campaign saved".to_string()),
                    Err(error) => self.say(format!("Campaign could not be saved ({error})")),
                }
                let return_to_logistics = self
                    .inner
                    .as_ref()
                    .and_then(|inner| match &inner.mode {
                        Mode::SaveLoad(screen) => Some(screen.return_to_logistics),
                        _ => None,
                    })
                    .unwrap_or(false);
                if return_to_logistics {
                    self.resume_menu_to_logistics(vfs)
                } else {
                    let inner = self.inner.as_mut().expect("loaded");
                    inner.mode = Mode::Menu;
                    inner.menu.resume();
                    Step::None
                }
            }
            SaveLoadAction::Load => {
                let Some(inner) = self.inner.as_mut() else {
                    return Step::None;
                };
                let restored = match service.load(slot, Arc::clone(inner.lg.data())) {
                    Ok(campaign) => campaign,
                    Err(error) => {
                        inner.mode = Mode::Menu;
                        inner.menu.resume();
                        let _ = inner;
                        self.say(format!("Campaign could not be loaded ({error})"));
                        return Step::None;
                    }
                };
                inner.lg = restored;
                inner.campaign_started = true;
                if inner.lg.campaign_over() {
                    // A completed campaign has no current operation for mission selection to
                    // load. Keep its restored state available from the main menu instead of
                    // restarting into an invalid selection page.
                    inner.mode = Mode::Menu;
                    inner.menu.resume();
                    return Step::Request(StateRequest::Logistics);
                }
                let resume_state = inner.lg.to_save();
                let campaign_data = Arc::clone(inner.lg.data());
                let mut sequence = match Inner::build_sequence(
                    vfs,
                    &mut inner.assets,
                    inner.prefs.clone(),
                    inner.original.clone(),
                    volumes_live,
                    self.config_dir.as_deref(),
                ) {
                    Ok(sequence) => sequence,
                    Err(error) => {
                        inner.mode = Mode::Menu;
                        inner.menu.resume();
                        let _ = inner;
                        self.say(format!("The campaign could not be resumed ({error})"));
                        return Step::None;
                    }
                };
                sequence.connect_movie_host();
                sequence.restart(&mut Loader::new(vfs, &mut inner.assets), &mut inner.lg);
                // Restart chooses the same page as the C++ RESTART route, including direct load
                // for campaigns that skip logistics. The page setup can refresh campaign data;
                // restore the saved state after it begins.
                inner.lg = match Logistics::from_save(campaign_data, &resume_state) {
                    Ok(loaded) => loaded.logistics,
                    Err(error) => {
                        self.say(format!("The campaign could not be resumed ({error})"));
                        return Step::None;
                    }
                };
                inner.seq = Some(sequence);
                inner.mode = Mode::Sequence;
                Step::Request(StateRequest::Logistics)
            }
        }
    }

    fn resume_menu_to_logistics(&mut self, vfs: &mut Vfs) -> Step {
        let Some(inner) = self.inner.as_mut() else {
            return Step::None;
        };
        let Some(sequence) = inner.seq.as_mut() else {
            inner.mode = Mode::Menu;
            inner.menu.resume();
            return Step::None;
        };
        sequence.return_from_main_menu(&mut Loader::new(vfs, &mut inner.assets), &mut inner.lg);
        inner.menu_return_to_logistics = false;
        inner.mode = Mode::Sequence;
        Step::Request(StateRequest::Logistics)
    }

    /// The name-box text a row puts in the file name field (the numbered slots' file stem).
    fn row_filename(slot: &SaveSlot) -> String {
        match slot {
            SaveSlot::Named(name) => name.clone(),
            SaveSlot::Numbered(number) => format!("slot-{number:02}"),
            SaveSlot::Autosave => String::new(),
        }
    }

    /// What DELETE would remove (`LogisticsSaveDialog::handleMessage`, `DELETE_BUTTON`): the row
    /// whose name is in the name box when saving, else the selected row; `None` when the list is
    /// empty.
    fn delete_target(screen: &SaveLoadScreen) -> Option<(SaveSlot, String)> {
        if screen.action == SaveLoadAction::Save && !screen.filename.is_empty() {
            let typed = screen.rows.iter().find(|(slot, name)| {
                name.eq_ignore_ascii_case(&screen.filename)
                    || matches!(slot, SaveSlot::Numbered(n)
                        if screen.filename.eq_ignore_ascii_case(&format!("slot-{n:02}")))
            });
            if let Some(row) = typed {
                return Some(row.clone());
            }
        }
        screen.rows.get(screen.selected).cloned()
    }

    /// `DeleteFile` then `initDialog` (logisticsdialog.cpp:765-796): the save is removed, the list
    /// is rebuilt and its first row is selected. Returns a message for the player when the
    /// removal or the re-listing failed.
    fn delete_save(
        service: Option<&CampaignSaveService>,
        vfs: &mut Vfs,
        inner: &mut Inner,
        slot: SaveSlot,
    ) -> Option<String> {
        let service = service?;
        let mut message = None;
        if let Err(error) = service.delete(slot) {
            message = Some(format!("Campaign save could not be deleted ({error})"));
        }
        let Mode::SaveLoad(screen) = &mut inner.mode else {
            return message;
        };
        match service.list(Arc::clone(inner.lg.data())) {
            Ok(listing) => {
                let (rows, infos) = save_rows(listing, screen.action);
                screen
                    .dialog
                    .fill_rows(&mut Loader::new(vfs, &mut inner.assets), &infos);
                screen.rows = rows;
                screen.selected = 0;
                screen.filename = screen
                    .rows
                    .first()
                    .map(|(slot, _)| Self::row_filename(slot))
                    .unwrap_or_default();
            }
            Err(error) => {
                message = Some(format!("Campaign slots could not be read ({error})"));
            }
        }
        message
    }

    fn activate_save_load(screen: &SaveLoadScreen) -> Option<(SaveLoadAction, SaveSlot)> {
        if screen.action == SaveLoadAction::Save {
            if screen.filename.is_empty() {
                return None;
            }
            let filename = screen
                .filename
                .strip_suffix(".ron")
                .or_else(|| screen.filename.strip_suffix(".RON"))
                .unwrap_or(&screen.filename);
            let slot = screen
                .rows
                .iter()
                .find(|(slot, _)| match slot {
                    SaveSlot::Named(name) => name.eq_ignore_ascii_case(filename),
                    SaveSlot::Numbered(number) => screen
                        .filename
                        .eq_ignore_ascii_case(&format!("slot-{number:02}")),
                    SaveSlot::Autosave => false,
                })
                .map(|(slot, _)| slot.clone())
                .unwrap_or_else(|| SaveSlot::Named(filename.to_string()));
            return Some((screen.action, slot));
        }
        screen
            .rows
            .get(screen.selected)
            .map(|(slot, _)| (screen.action, slot.clone()))
    }

    fn options_frame(&mut self) {
        let dt = self.dt;
        let mut out = UiOutput::default();
        let mut cues = Vec::new();
        let mut notices: Vec<String> = Vec::new();
        let inner = self.inner.as_mut().expect("loaded");
        let Mode::Options(o) = &mut inner.mode else {
            return;
        };
        let frame = {
            let mut ctx = UpdateCtx {
                input: &mut inner.input,
                assets: &inner.assets,
                dt,
                out: &mut out,
            };
            o.update(&mut ctx)
        };
        push_output(&mut cues, &out);
        if let Some(v) = frame.volumes {
            cues.push(UiCue::Volumes(v));
        }
        match frame.result {
            Some(OptionsResult::Accepted {
                prefs,
                resolution_changed,
            }) => {
                let mut prefs = *prefs;
                // a new resolution takes the game screen and the window now, so the screen's "next mission" dialog
                // is dropped; the mission HUD is laid out for the size it loads at, so a mission already running
                // keeps its layout until the next one
                if resolution_changed {
                    o.dismiss_warning();
                }
                let resolution = (resolution_changed
                    && (prefs.resolution_x, prefs.resolution_y)
                        != (inner.prefs.resolution_x, inner.prefs.resolution_y))
                    .then_some((prefs.resolution_x as u32, prefs.resolution_y as u32));
                // no audio output is installed, so a volume edit would change nothing audible
                if !(self.cue_sink.is_some() || self.core_audio) {
                    let kept = &inner.prefs;
                    let volumes = |p: &Prefs| {
                        [
                            p.digital_master_volume,
                            p.music_volume,
                            p.sfx_volume,
                            p.radio_volume,
                            p.betty_volume,
                        ]
                    };
                    if volumes(&prefs) != volumes(kept) {
                        prefs.digital_master_volume = kept.digital_master_volume;
                        prefs.music_volume = kept.music_volume;
                        prefs.sfx_volume = kept.sfx_volume;
                        prefs.radio_volume = kept.radio_volume;
                        prefs.betty_volume = kept.betty_volume;
                        notices.push(
                            "Changing Volumes is not available yet: the old setting stays"
                                .to_string(),
                        );
                    }
                }
                // `prefs.save()` (`code/optionsarea.cpp:270`)
                if let Some(path) = &inner.options_path {
                    let written = path
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| std::fs::write(path, prefs.to_fit()));
                    if let Err(e) = written {
                        notices.push(format!("The options could not be saved ({e})"));
                    }
                }
                notices.extend(deferred_option_notices(&inner.prefs, &prefs));
                self.applied = Some(applied_from(&prefs, resolution));
                inner
                    .assets
                    .set_player_paint(prefs.base_color, prefs.highlight_color);
                inner.prefs = prefs;
            }
            Some(OptionsResult::Cancelled) => {
                let p = &inner.prefs;
                cues.push(UiCue::Volumes([
                    p.digital_master_volume,
                    p.music_volume,
                    p.sfx_volume,
                    p.radio_volume,
                    p.betty_volume,
                ]));
            }
            None => {}
        }
        if o.is_done() {
            inner.mode = Mode::Menu;
            if self.mission_overlay {
                // `optionsScreenWrapper->end()`: back to the mission, which runs again
                self.mission_overlay = false;
            } else {
                inner.menu.resume();
            }
        }
        self.send(&cues);
        if !notices.is_empty() {
            self.say(notices.join(". "));
        }
    }

    /// `Logistics::start(log_RESULTS)` (`code/logistics.cpp:121-180`) for the mission that just ended
    /// with `result`: a win records the mission in the campaign and goes on to the results screens
    /// (`MissionResults::begin`); anything else skips them (`bDone`).  A mission started from the
    /// command line (`justStartMission`: no campaign was started from the menu) quits the game when
    /// it is over, as the original does.
    fn begin_results(&mut self, vfs: &mut Vfs, result: u32, outcome: Option<Outcome>) -> Step {
        let Some(inner) = self.inner.as_mut() else {
            return Step::None;
        };
        self.last_result = result;
        self.holding = false;
        let just_start = !inner.campaign_started;
        if let Some(mission) = self.single_mission_on_result.take() {
            inner.lg.set_single_mission(
                &mission,
                &mc2_sim_data::campaign::MissionSettings::default(),
                None,
            );
        }
        if result != MIS_PLAYER_WIN_BIG {
            // `missionResults->bDone = true` (a mission lost on the command line quits, :176-178)
            return if just_start {
                Step::Request(StateRequest::Exit)
            } else {
                self.leave_results(vfs, false)
            };
        }
        inner.lg.remove_mechs_in_force_group();
        let Some(outcome) = outcome else {
            self.say("The mission reported no outcome, so its results cannot be shown".to_string());
            return self.go_main_menu();
        };
        if let Err(e) = inner.lg.set_mission_completed(&outcome) {
            self.say(format!("The mission could not be recorded ({e})"));
            self.error = Some(format!("results: {e}"));
            return self.go_main_menu();
        }
        if inner.lg.campaign_over() && !inner.lg.mission_info().is_single_mission() {
            let autosave_result = self.save_service.as_ref().map_or_else(
                || Err("campaign save storage is unavailable".to_string()),
                |service| {
                    service
                        .save_campaign_completion(&inner.lg)
                        .map_err(|e| e.to_string())
                },
            );
            let request = final_video_request(&inner.lg);
            if request.is_some() {
                inner.mode = Mode::Menu;
            }
            let _ = inner;
            match autosave_result {
                Ok(()) => self.say("Campaign completed and autosaved".to_string()),
                Err(error) => self.say(format!("Campaign completion autosave failed ({error})")),
            }
            if let Some(request) = request {
                self.consume_movie_requests(vfs, &[request]);
                return Step::None;
            }
            // `playFullScreenVideo` returns straight to the splash for an empty name.
            return self.go_main_menu();
        }
        if inner.lg.mission_info().is_single_mission() {
            return self.go_main_menu();
        }
        let mut results = match MissionResults::load(&mut Loader::new(vfs, &mut inner.assets)) {
            Ok(r) => r,
            Err(e) => {
                self.say(format!("The results screens could not be shown ({e})"));
                self.error = Some(format!("results screens: {e}"));
                return self.go_main_menu();
            }
        };
        let mut out = UiOutput::default();
        {
            let mut ctx = UpdateCtx {
                input: &mut inner.input,
                assets: &inner.assets,
                dt: self.dt,
                out: &mut out,
            };
            results.begin(&mut ctx, &mut inner.lg, &outcome);
        }
        inner.mode = Mode::Results(Box::new(results));
        // `MissionResults::begin` plays the results loop (`code/missionresults.cpp:188-192`)
        let mut cues = vec![UiCue::Music(i64::from(mc2_audio::ids::music::RESULTS_LOOP))];
        push_output(&mut cues, &out);
        self.send(&cues);
        Step::None
    }

    /// One frame of the results screens; when they are done (`MissionResults::isDone`) the game goes on.
    fn results_frame(&mut self, vfs: &mut Vfs) -> Step {
        let dt = self.dt;
        let mut out = UiOutput::default();
        let (done, autosave_request) = {
            let inner = self.inner.as_mut().expect("loaded");
            let Mode::Results(r) = &mut inner.mode else {
                return Step::None;
            };
            // the pilot review's kill markers are coloured by their units' damage, which needs
            // the texture store the screens only read
            if r.kill_icons_pending() {
                r.compose_kill_icons(&mut inner.assets.textures);
            }
            let mut ctx = UpdateCtx {
                input: &mut inner.input,
                assets: &inner.assets,
                dt,
                out: &mut out,
            };
            r.update(&mut ctx, &mut inner.lg);
            let done = r.is_done();
            (done, done.then(|| r.autosave(&inner.lg)).flatten())
        };
        let mut cues = Vec::new();
        push_output(&mut cues, &out);
        self.send(&cues);
        if !done {
            return Step::None;
        }
        if let Some(request) = autosave_request.as_ref() {
            // `MissionResults::end` is the C++ decision point (`code/missionresults.cpp:113-129`):
            // only after salvage and pilot review are complete does it persist the campaign.
            let result = self.save_service.as_ref().map_or_else(
                || Err("campaign save storage is unavailable".to_string()),
                |service| {
                    let inner = self.inner.as_ref().expect("loaded");
                    service
                        .save_autosave_if_eligible(&inner.lg, Some(request), false)
                        .map_err(|e| e.to_string())
                },
            );
            match result {
                Ok(true) => self.say("Campaign autosaved after a mission".to_string()),
                Ok(false) => {}
                Err(error) => self.say(format!("Campaign autosave failed ({error})")),
            }
        }
        self.leave_results(vfs, true)
    }

    /// The results are over (`Logistics::update`, `code/logistics.cpp:325-385`): a command-line mission
    /// quits; a campaign mission goes back to the mission screens, after a lost mission to the
    /// briefing (or the splash screen when the campaign skips logistics), after a win to the next
    /// mission with its pre-operation video.
    fn leave_results(&mut self, vfs: &mut Vfs, won: bool) -> Step {
        let Some(inner) = self.inner.as_mut() else {
            return Step::None;
        };
        if !inner.campaign_started {
            return Step::Request(StateRequest::Exit);
        }
        let Some(seq) = inner.seq.as_mut() else {
            return self.go_main_menu();
        };
        let mut movie = None;
        seq.begin(&mut Loader::new(vfs, &mut inner.assets), &mut inner.lg);
        if !won {
            if inner.lg.skip_logistics() {
                if !inner.lg.show_choose_mission() {
                    // `beginSplash`
                    return self.go_main_menu();
                }
            } else {
                seq.skip_to_briefing(&mut Loader::new(vfs, &mut inner.assets), &mut inner.lg);
            }
        } else {
            movie = inner
                .lg
                .take_current_big_video()
                .filter(|name| !name.is_empty());
        }
        inner.mode = Mode::Sequence;
        self.pending_movie = movie;
        Step::Request(StateRequest::Logistics)
    }

    fn go_main_menu(&mut self) -> Step {
        if let Some(inner) = self.inner.as_mut() {
            inner.menu_return_to_logistics = false;
            inner.mode = Mode::Menu;
            inner.begin_main_menu();
        }
        Step::Request(StateRequest::MainMenu)
    }

    fn sequence_frame(&mut self, vfs: &mut Vfs, state: &GameState) -> Step {
        let dt = self.dt;
        let mut out = UiOutput::default();
        let mut options_error = None;
        let (flow, fault, ready, movie_requests, accepted_prefs, option_notices, option_volumes) = {
            let inner = self.inner.as_mut().expect("loaded");
            let Some(seq) = inner.seq.as_mut() else {
                return Step::None;
            };
            if inner.input.key_status(keys::KEY_ESCAPE) == KeyStatus::Released && !seq.in_options()
            {
                if let Err(error) =
                    seq.open_options(&mut Loader::new(vfs, &mut inner.assets), &mut inner.lg)
                {
                    options_error = Some(error);
                }
            }
            // the lab hides its ammo row under the accepted `UnlimitedAmmo` option
            if let Some(page) = seq
                .page_mut(slot::MECH_LAB)
                .as_any_mut()
                .downcast_mut::<pages::MechLabPage>()
            {
                page.lab.set_unlimited_ammo(inner.prefs.use_unlimited_ammo);
            }
            let flow = seq.update(
                &mut Loader::new(vfs, &mut inner.assets),
                &mut inner.input,
                &mut out,
                dt,
                &mut inner.lg,
            );
            let (accepted_prefs, option_notices, option_volumes) = {
                let options = seq
                    .page_mut(slot::OPTIONS)
                    .as_any_mut()
                    .downcast_mut::<pages::CampaignOptions>();
                options.map_or((None, Vec::new(), None), |options| {
                    (
                        options.take_accepted(),
                        options.take_notices(),
                        options.take_volumes(),
                    )
                })
            };
            // the loading screen reached: the core loads the mission now; the bar fills and the
            // doors close only once that has worked (the core is in the mission)
            if seq.ready_to_load() {
                if let Some(p) = seq
                    .page_mut(slot::LOADING)
                    .as_any_mut()
                    .downcast_mut::<LoadingPage>()
                {
                    if matches!(state, GameState::InMission) {
                        p.finish();
                    } else {
                        p.loading.set_progress(50.0);
                    }
                }
            }
            (
                flow,
                seq.fault().map(str::to_string),
                seq.ready_to_load(),
                seq.take_movie_requests(),
                accepted_prefs,
                option_notices,
                option_volumes,
            )
        };
        if let Some(error) = options_error {
            self.say(format!("Campaign options could not be opened ({error})"));
        }
        self.consume_movie_requests(vfs, &movie_requests);
        let mut cues = Vec::new();
        if let Some(prefs) = accepted_prefs {
            let inner = self.inner.as_mut().expect("loaded");
            let resolution = ((prefs.resolution_x, prefs.resolution_y)
                != (inner.prefs.resolution_x, inner.prefs.resolution_y))
                .then_some((prefs.resolution_x as u32, prefs.resolution_y as u32));
            self.applied = Some(applied_from(&prefs, resolution));
            inner
                .assets
                .set_player_paint(prefs.base_color, prefs.highlight_color);
            inner.prefs = prefs;
        }
        if let Some(volumes) = option_volumes {
            cues.push(UiCue::Volumes(volumes));
        }
        if !option_notices.is_empty() {
            self.say(option_notices.join(". "));
        }
        push_output(&mut cues, &out);
        if let Some(seq) = self.inner.as_mut().and_then(|i| i.seq.as_mut()) {
            if let Some(p) = seq
                .page_mut(slot::LOADING)
                .as_any_mut()
                .downcast_mut::<LoadingPage>()
            {
                cues.extend(p.sounds.drain(..).map(UiCue::Sound));
            }
        }
        self.send(&cues);
        if !self.holding && ready {
            self.holding = true;
            let inner = self.inner.as_ref().expect("loaded");
            let p = &inner.prefs;
            let spec = inner.lg.load_spec(MissionOptions {
                difficulty: p.game_difficulty,
                unlimited_ammo: p.use_unlimited_ammo,
                base_color: p.base_color,
                highlight_color: p.highlight_color,
            });
            let mission = spec.mission.clone();
            self.launch = Some(spec);
            return Step::Request(StateRequest::StartMission(mission));
        }
        match flow {
            Flow::Running => Step::None,
            Flow::Unavailable(what) => {
                self.notice(what);
                Step::None
            }
            Flow::Launch => {
                // the doors are out and the mission is running
                self.holding = false;
                Step::None
            }
            Flow::MainMenu | Flow::Failed => {
                if let Some(e) = fault {
                    self.say(format!("The campaign screens failed ({e})"));
                    self.error = Some(e);
                }
                self.holding = false;
                let inner = self.inner.as_mut().expect("loaded");
                inner.menu_return_to_logistics = flow == Flow::MainMenu;
                inner.mode = Mode::Menu;
                inner.begin_main_menu();
                Step::Request(StateRequest::MainMenu)
            }
        }
    }
}

impl UiLayer for UiHost {
    fn is_single_mission(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|i| i.lg.mission_info().is_single_mission())
    }

    fn mission_save_dir(&self) -> Option<std::path::PathBuf> {
        self.save_service
            .as_ref()
            .map(|s| s.directory().to_path_buf())
    }

    fn open_mission_options(&mut self) {
        self.mission_options_requested = true;
    }

    fn blocks_mission(&self) -> bool {
        self.mission_overlay || self.mission_options_requested
    }

    fn set_audio_available(&mut self, available: bool) {
        self.core_audio = available;
    }

    fn set_movie_mixer(&mut self, mixer: Option<SharedMixer>) {
        // GameCore publishes the mixer every frame. Keep the controller (and its open movie)
        // while audio availability is unchanged; replacing it here drops playback state.
        if self.movie_control.is_none() || self.movie_mixer.is_some() != mixer.is_some() {
            self.movie_control = Some(MovieControl::new(mixer.clone()));
            self.movie_mixer = mixer;
        }
    }
    fn take_audio(&mut self) -> Vec<crate::audio::Cue> {
        use crate::audio::Cue;
        std::mem::take(&mut self.audio_out)
            .into_iter()
            .map(|c| match c {
                UiCue::Sound(i) => Cue::Sound(i),
                UiCue::Betty(i) => Cue::Betty(i),
                UiCue::Music(i) => Cue::Music(i),
                UiCue::StopMusic => Cue::StopMusic,
                UiCue::Volumes(v) => Cue::Volumes(v),
            })
            .collect()
    }
    fn handle_event(&mut self, state: &GameState, event: &InputEvent) -> bool {
        if let Some(skip_key) = self.movie_skip_key {
            if matches!(event, InputEvent::Key { key, pressed: false, .. } if *key == skip_key) {
                self.movie_skip_key = None;
                return true;
            }
        }
        if self.movie_player.is_some() {
            let skip_key = match event {
                InputEvent::Key {
                    key, pressed: true, ..
                } if *key == keys::KEY_SPACE || *key == keys::KEY_ESCAPE => Some(*key),
                _ => None,
            };
            let skip = skip_key.is_some()
                || matches!(
                    event,
                    InputEvent::Button {
                        button: MouseButton::Left,
                        pressed: true
                    }
                );
            if skip {
                self.movie_skip_key = skip_key;
                self.stop_movie();
            }
            return true;
        }
        if self.final_video_active {
            let skip = matches!(event, InputEvent::Key { key, pressed: true, .. }
                if *key == keys::KEY_SPACE || *key == keys::KEY_ESCAPE)
                || matches!(
                    event,
                    InputEvent::Button {
                        button: MouseButton::Left,
                        pressed: true
                    }
                );
            if skip {
                self.pending_movie_requests.push(MovieRequest::Skip);
            }
            return true;
        }
        if !is_ui_state(state, self.holding || self.mission_overlay) {
            return false;
        }
        self.pending.push(event.clone());
        // the screens are modal
        true
    }

    fn update(&mut self, frame: &mut UiFrame<'_>) -> Option<StateRequest> {
        self.game_size = frame.game_size;
        self.dt = frame.dt;
        if !matches!(frame.state, GameState::InMission) {
            self.mission_overlay = false;
            self.mission_options_requested = false;
        } else if self.mission_options_requested {
            self.mission_options_requested = false;
            self.mission_overlay = true;
            self.pending.clear();
            self.open_mission_options_on_load = true;
        }
        self.shown = is_ui_state(frame.state, self.holding || self.mission_overlay);
        if let Some((_, left)) = self.banner.as_mut() {
            *left -= frame.dt;
            if *left <= 0.0 {
                self.banner = None;
                self.banner_is_movie_failure = false;
            }
        }
        if self.banner.is_none() {
            if let Some(line) = self.pending_banners.pop_front() {
                self.banner = Some((line, BANNER_SECONDS));
            }
        }
        // The game reads `options.cfg` at start-up whatever it opens first (`mechcmd2.cpp`), so a launch
        // straight into a mission (`--mission`) draws with the saved options too, without the menus.
        if !self.startup_options_read {
            self.startup_options_read = true;
            // only the player's own saved file: without one the command line's switches stand
            // (the windowed host never sets a directory: it resolves the normal user one, as `Inner::load` does)
            let dir = self
                .config_dir
                .clone()
                .or_else(|| UserPaths::resolve().map(|p| p.config));
            let saved = dir
                .as_deref()
                .is_some_and(|dir| mc2_platform::paths::options_read_path(dir).is_file());
            if saved && self.inner.is_none() && self.applied.is_none() {
                let prefs = saved_prefs(frame.vfs, dir.as_deref());
                self.applied = Some(applied_from(&prefs, None));
            }
        }
        if !self.shown {
            self.pending.clear();
            return None;
        }
        if self.inner.is_none() && !self.failed {
            match Inner::load(frame.vfs, self.config_dir.as_deref()) {
                Ok(i) => {
                    // the saved options are in force from the first mission on (`mechcmd2.cpp`
                    // reads them at start-up), not only once Options has been accepted
                    let mut a = applied_from(&i.prefs, None);
                    a.saved_resolution = (i.prefs.resolution_x > 0 && i.prefs.resolution_y > 0)
                        .then_some((i.prefs.resolution_x as u32, i.prefs.resolution_y as u32));
                    self.applied = Some(a);
                    self.inner = Some(i);
                    if !(self.cue_sink.is_some() || self.core_audio) {
                        self.say(
                            "Audio is not available yet: menu music and sounds stay silent"
                                .to_string(),
                        );
                    }
                }
                Err(e) => {
                    self.failed = true;
                    self.error = Some(e);
                }
            }
        }
        self.inner.as_ref()?;
        if self.open_mission_options_on_load {
            self.open_mission_options_on_load = false;
            if !self.open_options(frame.vfs) {
                self.mission_overlay = false;
            }
        }
        self.drain_pending_movie_requests(frame.vfs);
        // the state the core is in decides the screen
        match frame.state {
            GameState::Results(_) => {
                // `Logistics::start(log_RESULTS)`, once, for the mission that just ended
                if let Some((result, outcome)) = self.results_pending.take() {
                    self.pending.clear();
                    match self.begin_results(frame.vfs, result, outcome) {
                        Step::Request(r) => return Some(r),
                        Step::None => {}
                    }
                } else if !self.final_video_active
                    && !matches!(self.inner.as_ref().map(|i| &i.mode), Some(Mode::Results(_)))
                {
                    // Results with no mission behind them (nothing ended here): nothing to show
                    self.pending.clear();
                    self.say("There are no mission results to show".to_string());
                    let inner = self.inner.as_mut().expect("loaded");
                    inner.mode = Mode::Menu;
                    inner.begin_main_menu();
                    return Some(StateRequest::MainMenu);
                }
            }
            GameState::MainMenu => {
                let inner = self.inner.as_mut().expect("loaded");
                if matches!(inner.mode, Mode::Sequence) {
                    inner.mode = Mode::Menu;
                    inner.begin_main_menu();
                    // a failed load: `report` has told the player why
                    self.holding = false;
                }
            }
            _ => {}
        }
        self.begin_input();
        if matches!(frame.state, GameState::Logistics) || self.final_video_active {
            if let Some(request) = self.advance_movie_control(frame.vfs, frame.dt) {
                return Some(request);
            }
        }
        let movie_active = if matches!(frame.state, GameState::MainMenu | GameState::Logistics) {
            self.advance_movie(frame.vfs, frame.state, frame.dt)
        } else {
            false
        };
        let step = if movie_active || self.final_video_active {
            Step::None
        } else {
            match self.inner.as_ref().map(|i| &i.mode) {
                Some(Mode::Menu) => self.menu_frame(frame.vfs),
                Some(Mode::SaveLoad(_)) => self.save_load_frame(frame.vfs),
                Some(Mode::CampaignPick(_)) => self.campaign_pick_frame(),
                Some(Mode::Options(_)) => {
                    self.options_frame();
                    Step::None
                }
                Some(Mode::Sequence) => self.sequence_frame(frame.vfs, frame.state),
                Some(Mode::Results(_)) => self.results_frame(frame.vfs),
                Some(Mode::Solo(_)) => self.solo_frame(frame.vfs),
                Some(Mode::Mechlopedia(_)) => {
                    // Handle Mechlopedia frame
                    let dt = self.dt;
                    let mut out = UiOutput::default();
                    let inner = self.inner.as_mut().expect("loaded");

                    let mut ctx = UpdateCtx {
                        input: &mut inner.input,
                        assets: &inner.assets,
                        dt,
                        out: &mut out,
                    };

                    if let Mode::Mechlopedia(m) = &mut inner.mode {
                        m.update(&mut ctx);
                        // Check if the screen wants to exit
                        if m.base.status == mc2_ui::screen::Status::Next {
                            // Resume the menu after Mechlopedia exits
                            inner.menu.resume();
                            inner.mechlopedia = None;
                            inner.mechlopedia_previews = None;
                            inner.mode = Mode::Menu;
                        }
                    }

                    let mut cues = Vec::new();
                    push_output(&mut cues, &out);
                    self.send(&cues);
                    Step::None
                }
                None => Step::None,
            }
        };
        if let Some(inner) = self.inner.as_mut() {
            if let Some(c) = inner.cursors.as_mut() {
                c.update(inner.input.mouse_state, self.dt);
            }
            inner.input.end_frame();
        }
        match step {
            Step::Request(r) => Some(r),
            Step::None => None,
        }
    }

    fn draw(&mut self, ctx: &mut DrawContext<'_>) -> bool {
        if !self.shown {
            return false;
        }
        let dt = self.dt;
        let ui_layout = self.layout();
        let Some(inner) = self.inner.as_mut() else {
            return false;
        };
        let mut dl = DrawList::new(UI_SIZE.0 as i32, UI_SIZE.1 as i32);
        {
            let mut rc = RenderCtx {
                dl: &mut dl,
                assets: &inner.assets,
                dt,
            };
            match &mut inner.mode {
                Mode::Menu => {
                    inner.menu.render(&mut rc);
                    if let Some(d) = inner.campaign_closing.as_mut() {
                        d.render(&mut rc);
                    }
                }
                Mode::SaveLoad(screen) => {
                    inner.menu.render_plate(&mut rc);
                    screen.dialog.render(&mut rc);
                }
                Mode::CampaignPick(d) => {
                    inner.menu.render(&mut rc);
                    d.render(&mut rc);
                }
                Mode::Options(o) => o.render(&mut rc),
                Mode::Sequence => {
                    if let Some(seq) = inner.seq.as_mut() {
                        seq.render(&mut rc);
                    }
                }
                Mode::Results(r) => r.render(&mut rc),
                Mode::Mechlopedia(m) => {
                    m.render(&mut rc);
                }
                Mode::Solo(d) => {
                    inner.menu.render(&mut rc);
                    d.render(&mut rc);
                }
            }
        }
        // `preview_texture` queues a request during the Mechlopedia's render above. Resolve it
        // on the real application path so the following frame draws the selected chassis rather
        // than leaving the panel permanently blank.
        if let (Mode::Mechlopedia(_), Some(previews)) =
            (&inner.mode, inner.mechlopedia_previews.as_mut())
        {
            previews.resolve(&mut inner.assets);
        }
        // The logistics pages and the salvage screen's selected-mech panel ask for their mech
        // pictures while they draw; render them for the next frame (the first frame of a page
        // shows the panel empty, as the C++ camera's first frame does before its model is set
        // up). The options screen's gameplay tab spins a 'Mech too.
        if matches!(
            inner.mode,
            Mode::Sequence | Mode::Results(_) | Mode::Options(_)
        ) {
            let root = inner.data_root.clone();
            let made = inner.logistics_previews.get_or_insert_with(|| {
                let gpu = mc2_render::gpu::Gpu::headless(&RenderConfig::from_env())
                    .map_err(|e| e.to_string())?;
                let data = mc2_render::game::GameData::open(&root).map_err(|e| e.to_string())?;
                ModelPreviews::new(Arc::new(gpu), data).map_err(|e| e.to_string())
            });
            match made {
                Ok(previews) => previews.resolve(&mut inner.assets),
                Err(e) => {
                    if inner.assets.previews.fail_all() != 0 {
                        log::warn!("logistics mech pictures are not available: {e}");
                    }
                }
            }
        }
        let results_composed =
            matches!(ctx.state, GameState::Results(_)) && matches!(inner.mode, Mode::Results(_));
        if let Some((msg, _)) = &self.banner {
            if let Some(f) = inner.assets.font(inner.banner_font) {
                let r = GuiRect::new(40, 520, 760, 580);
                dl.fill_rect(r, 0xe000_0000);
                dl.outline_rect(r, 0xffc6_6600, 0xffc6_6600);
                let a = TextAttrs {
                    color: 0xffff_ffff,
                    size: inner.banner_font.size.max(1) as f32,
                    align: 3,
                    ..TextAttrs::default()
                };
                let inner_r = GuiRect::new(r.left + 8, r.top + 4, r.right - 8, r.bottom - 4);
                text::draw(&mut dl, f, msg, &a, inner_r, (inner_r.left, inner_r.top));
            }
        }
        if inner.input.draw_mouse && inner.input.gos().cursor_inside_window() {
            if let Some(c) = &inner.cursors {
                // the fresh pointer is in game pixels, the cursor drawn in 800x600 UI units
                let at = ctx
                    .pointer
                    .map_or((inner.input.mouse_x(), inner.input.mouse_y()), |p| {
                        let (ux, uy) = ui_layout.to_ui(p.0, p.1);
                        (ux.round() as i32, uy.round() as i32)
                    });
                c.draw(&mut dl, inner.input.mouse_state, at);
            }
        }
        let layout = UiLayout::new(UI_SIZE, ctx.size, ScaleMode::Fit);
        let renderer = self
            .renderer
            .get_or_insert_with(|| UiRenderer::new(ctx.gpu));
        renderer.draw(
            ctx.gpu,
            ctx.encoder,
            ctx.target,
            &layout,
            &dl,
            &inner.assets,
        );
        if self.movie_player.is_some() {
            if let Some(frame) = self.movie_frame.as_ref() {
                self.movie_renderer
                    .get_or_insert_with(|| MovieRenderer::new(ctx.gpu))
                    .draw(ctx, frame, self.movie_placement);
            }
        }
        if let Some(frame) = self.movie_control.as_ref().and_then(MovieControl::frame) {
            self.movie_renderer
                .get_or_insert_with(|| MovieRenderer::new(ctx.gpu))
                .draw(ctx, frame, self.movie_placement);
        }
        // Only the results screens composed in a Results frame count as the Results screen
        results_composed
    }

    fn has_results_screen(&self) -> bool {
        true
    }

    fn mission_ended(&mut self, result: u32, outcome: Option<Outcome>) {
        self.results_pending = Some((result, outcome));
    }

    fn draws_cursor(&self) -> bool {
        self.shown && self.inner.as_ref().is_some_and(|i| i.cursors.is_some())
    }

    fn report(&mut self, message: &str) {
        self.say(message.to_string());
    }

    fn take_applied_settings(&mut self) -> Option<AppliedSettings> {
        self.applied.take()
    }

    fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

/// Notices for options an accept cannot apply on the spot: the Hardware Rasterizer choice is taken
/// when the GPU is created, so it is applied by restarting. Local Shadows and Async Mouse are live
/// and get no notice.
pub fn deferred_option_notices(kept: &Prefs, new: &Prefs) -> Vec<String> {
    let mut notices = Vec::new();
    if new.renderer != kept.renderer {
        notices
            .push("The Hardware Rasterizer choice is applied by restarting the game".to_string());
    }
    notices
}

#[cfg(test)]
mod movie_host_tests {
    #[test]
    fn save_time_formats_known_instants() {
        use chrono::{TimeZone, Utc};
        let at = |s| Utc.timestamp_opt(s, 0).unwrap();
        assert_eq!(
            format_time_in(at(0)),
            ("01/01/1970".into(), "00:00:00".into())
        );
        // 2024-02-29 13:45:09 UTC
        assert_eq!(
            format_time_in(at(1_709_214_309)),
            ("02/29/2024".into(), "13:45:09".into())
        );
    }

    #[test]
    fn save_time_is_local_across_a_date_boundary() {
        use chrono::{FixedOffset, TimeZone, Utc};
        // 2026-10-30 00:30:00 UTC is still the previous evening in Los Angeles (PDT, UTC-7).
        let t = Utc.timestamp_opt(1_793_320_200, 0).unwrap();
        let la = t.with_timezone(&FixedOffset::west_opt(7 * 3600).unwrap());
        let tokyo = t.with_timezone(&FixedOffset::east_opt(9 * 3600).unwrap());
        assert_eq!(format_time_in(la), ("10/29/2026".into(), "17:30:00".into()));
        assert_eq!(
            format_time_in(tokyo),
            ("10/30/2026".into(), "09:30:00".into())
        );
        // The production path goes through `Local` and the same formatter.
        let (d, h) =
            format_save_time(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_793_320_200));
        assert_eq!((d.len(), h.len()), (10, 8));
    }

    #[test]
    fn only_the_adapter_choice_gets_a_restart_notice() {
        let base = Prefs::default();
        assert!(deferred_option_notices(&base, &base).is_empty());
        let mut p = base.clone();
        p.use_local_shadows = !p.use_local_shadows;
        p.async_mouse = !p.async_mouse;
        assert!(deferred_option_notices(&base, &p).is_empty());
        p.renderer = !p.renderer;
        let n = deferred_option_notices(&base, &p);
        assert_eq!(n.len(), 1);
        assert!(n[0].contains("Hardware Rasterizer"));
    }

    use super::*;

    /// A host whose config directory is private to the test, never the player's.
    fn test_host() -> UiHost {
        let mut host = UiHost::new();
        host.set_private_user_directory(crate::headless::private_config_dir());
        host
    }
    use mc2_platform::input::InputState;
    use mc2_platform::viewport::{Fit, Viewport};

    fn update(host: &mut UiHost, vfs: &mut Vfs, state: &GameState, input: &InputState) {
        let mut frame = UiFrame {
            state,
            input,
            dt: 1.0 / 30.0,
            game_size: (800.0, 600.0),
            camera: None,
            paused: false,
            vfs,
        };
        host.update(&mut frame);
    }

    fn broken_cinema_fixture() -> Option<std::path::PathBuf> {
        fn link_tree(from: &std::path::Path, to: &std::path::Path) {
            if from.is_dir() {
                std::fs::create_dir_all(to).expect("create fixture directory");
                for entry in std::fs::read_dir(from).expect("read game data directory") {
                    let entry = entry.expect("read game data entry");
                    link_tree(&entry.path(), &to.join(entry.file_name()));
                }
            } else {
                match std::fs::hard_link(from, to) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::PermissionDenied
                                | std::io::ErrorKind::CrossesDevices
                        ) =>
                    {
                        std::fs::copy(from, to).expect("copy game data fixture");
                    }
                    Err(error) => panic!("hard-link {}: {error}", from.display()),
                }
            }
        }

        let source = std::env::var_os("MC2_DATA").map(std::path::PathBuf::from)?;
        let target = std::env::current_exe()
            .expect("test executable path")
            .parent()
            .expect("test executable directory")
            .join(format!("movie-cinema-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&target);
        link_tree(&source, &target);

        // A loose file wins VFS lookup over the shipped archive, so only this private fixture is
        // changed; the original game data remains untouched.
        let movie = target.join("data/movies/cinema1.mpg");
        std::fs::create_dir_all(movie.parent().expect("movie parent"))
            .expect("create fixture movie directory");
        let _ = std::fs::remove_file(&movie);
        std::fs::write(movie, b"not an MPEG movie").expect("write undecodable fixture movie");
        Some(target)
    }

    #[test]
    fn connected_movie_control_open_failure_is_visible() {
        let Some(root) = std::env::var_os("MC2_DATA") else {
            eprintln!("skipping movie control failure evidence: MC2_DATA is unset");
            return;
        };
        let mut vfs = Vfs::from_system_cfg(root).expect("open game VFS");
        let mut host = test_host();

        host.consume_movie_requests(
            &mut vfs,
            &[MovieRequest::Open("movie_that_does_not_exist".to_string())],
        );

        assert!(host.banner().is_some_and(|message| {
            message.contains("movie unavailable") || message.contains("movie playback failed")
        }));
        assert!(host.notices.iter().any(|message| {
            message.contains("movie unavailable") || message.contains("movie playback failed")
        }));
    }

    #[test]
    fn missing_movie_is_visible_and_stays_stopped() {
        let Some(root) = std::env::var_os("MC2_DATA") else {
            eprintln!("skipping missing movie evidence: MC2_DATA is unset");
            return;
        };
        let mut vfs = Vfs::from_system_cfg(root).expect("open game VFS");
        let mut host = test_host();

        host.start_movie(
            &mut vfs,
            "movie_that_does_not_exist",
            MoviePlacement::FullScreen,
            false,
        );

        assert!(host.movie_player.is_none(), "a missing movie cannot play");
        assert!(
            host.banner()
                .is_some_and(|message| message.contains("movie unavailable")),
            "missing movie must be reported on screen: {:?}",
            host.banner()
        );
    }

    #[test]
    fn later_picture_decode_failure_is_visible_and_stops_playback() {
        let Some(root) = std::env::var_os("MC2_DATA") else {
            eprintln!("skipping damaged movie evidence: MC2_DATA is unset");
            return;
        };
        let root = std::path::PathBuf::from(root);
        let path = root.join("data/movies/msft.mpg");
        let bytes = std::fs::read(path).expect("read intro movie");
        let streams = mc2_movie::demux::demux(&bytes).expect("demux intro movie");
        let video = streams.video.expect("intro video stream");
        let pictures: Vec<_> = video
            .data
            .windows(4)
            .enumerate()
            .filter_map(|(at, bytes)| (bytes == [0, 0, 1, 0]).then_some(at))
            .collect();
        let mut vfs = Vfs::from_system_cfg(root).expect("open game VFS");
        let mut failed_host = None;
        for &picture in pictures.iter().skip(1) {
            let mut damaged_video = video.clone();
            damaged_video.data[picture + 5] &= !0x38; // reserved picture_coding_type = 000
            let Ok(movie) = Movie::from_streams(damaged_video, streams.audio.clone()) else {
                continue; // This picture occurs before the first decoded frame.
            };
            let mut candidate = test_host();
            candidate.movie_player = Some(MoviePlayer::new(movie, None));
            for _ in 0..12 {
                candidate.advance_movie(&mut vfs, &GameState::MainMenu, 1.0 / 30.0);
                if candidate
                    .notices
                    .iter()
                    .any(|message| message.contains("movie playback failed"))
                {
                    failed_host = Some(candidate);
                    break;
                }
                if candidate.movie_player.is_none() {
                    break;
                }
            }
            if failed_host.is_some() {
                break;
            }
        }
        let host = failed_host.expect("a later picture decode failure reaches the host");

        assert!(host.movie_player.is_none(), "decode failure stops playback");
        assert!(
            host.notices
                .iter()
                .any(|message| message.contains("movie playback failed")),
            "the damaged later picture reaches the host error path"
        );
        assert!(
            host.banner()
                .is_some_and(|message| message.contains("movie playback failed")),
            "later picture failure must be reported on screen: {:?}",
            host.banner()
        );
    }

    #[test]
    fn first_group_movie_failure_stays_visible_through_selection_notice() {
        let Some(root) = broken_cinema_fixture() else {
            eprintln!("skipping integrated movie failure evidence: MC2_DATA is unset");
            return;
        };
        let mut vfs = Vfs::from_system_cfg(root).expect("open fixture game VFS");
        let mut host = test_host();
        host.set_config_directory(
            std::env::temp_dir().join(format!("mc2-movie-host-config-{}", std::process::id())),
        );
        host.core_audio = true;
        host.intro_movie_attempted = true;
        let input = InputState::new(Viewport::new(800, 600, 800.0, 600.0, Fit::Letterbox));
        update(&mut host, &mut vfs, &GameState::MainMenu, &input);

        assert_eq!(
            host.new_campaign(&mut vfs, "campaign"),
            Some(StateRequest::Logistics),
            "campaign start error: {:?}; notices: {:?}",
            host.error,
            host.notices
        );
        assert!(host
            .pending_movie
            .as_deref()
            .is_some_and(|name| name.to_ascii_lowercase().contains("cinema1")));

        update(&mut host, &mut vfs, &GameState::Logistics, &input);

        assert!(
            host.banner().is_some_and(|message| {
                message.contains("movie unavailable") || message.contains("movie playback failed")
            }),
            "the failed first-group movie is the visible banner: {:?}",
            host.banner()
        );
        assert!(host.notices.iter().any(|message| {
            message.contains("movie unavailable") || message.contains("movie playback failed")
        }));
        assert!(
            !host
                .notices
                .iter()
                .any(|message| message.contains("mission movie playback")),
            "the connected movie host handles selection playback: {:?}",
            host.notices
        );
    }
}
