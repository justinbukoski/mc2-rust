// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The presentation natives as the application performs them (`code/ablmc2.cpp`, S3).
//!
//! The simulation owns what a mission script can read back (movie mode, the letterbox, the fade, the mouse callout, the GUI
//! freeze and the mouse profile: [`mc2_sim::mission::natives_presentation`]); this module is the application's half, driven from
//! the simulation's state each frame:
//!
//! - the letterbox bars and the fade overlay the camera draws in movie mode (`GameCamera::render`, `code/gamecam.cpp:219-302`);
//! - the mission video window: an ABL `PlayVideo` becomes a [`MovieRequest::Open`] on the mission's [`MovieControl`], which plays
//!   the movie the way the C++ `ControlGui::playMovie` does (`code/controlgui.cpp:2851-2890`: one movie at a time, the picture at
//!   `videoRect`, and an end closes the window again);
//! - `TutorialText`: a line in the mission chat under `IDS_TUTORIAL` (`MissionInterfaceManager::setTutorialText`,
//!   `code/missiongui.cpp:418-422`, which is `cLoadString(textId)` shown by `ControlGui::setChatText`);
//! - the mouse callout: a copy of the simulation's callout, and the tutorial pointer the C++ warps for it (`userInput->setMousePos`
//!   in `MissionInterfaceManager::update`, `code/missiongui.cpp:455-580`): [`AppPresentation::pointer`] glides to the target
//!   button and the HUD draws the `mState_TUTORIALS` cursor there;
//! - the GUI gate: in movie mode (`code/missiongui.cpp:435-450`) it neither draws nor takes a player command; a callout or
//!   frozen GUI returns from *update* (`:455`, `:586-592`) but the already-built HUD still renders.
//!
//! The platform's mouse belongs to the input layer and cannot be warped, so the tutorial pointer is a drawn cursor of its own: the
//! callout's timing never depends on it (the simulation advances it from the frame length alone) and no game logic reads it.

use crate::movie_control::{MovieControl, Playback};
use crate::overlay::OverlayBatch;
use mc2_formats::vfs::Vfs;
use mc2_sim::events::SimEvent;
use mc2_sim::mission::natives_presentation::{Callout, Presentation};
use mc2_ui::logistics::movie::MovieRequest;
use mc2_ui::UiAssets;

/// `IDS_TUTORIAL` (`resource.h:4360`): the speaker a tutorial line is shown under.
pub const IDS_TUTORIAL: i32 = 60000;

/// `drawGUIOn` and `toggledGUI` (`code/missiongui.cpp:184-185`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuiToggles {
    /// `drawGUIOn`: the GUI draws and takes input.
    pub draw_gui_on: bool,
    /// `toggledGUI`: what the GUI returns to when the movie ends.
    pub toggled_gui: bool,
}

impl Default for GuiToggles {
    fn default() -> Self {
        GuiToggles {
            draw_gui_on: true,
            toggled_gui: true,
        }
    }
}

impl GuiToggles {
    /// `drawGUIOn ^= true; toggledGUI = drawGUIOn;` (`missiongui.cpp:2719-2720`).
    pub fn toggle(&mut self) {
        self.draw_gui_on = !self.draw_gui_on;
        self.toggled_gui = self.draw_gui_on;
    }
}

/// The presentation requests of one frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresentationEvents {
    /// The `PlayVideo` names, reduced as `_splitpath` does (the [`MovieRequest::Open`]s handed to the movie host).
    pub videos: Vec<String>,
    /// The `TutorialText` ids.
    pub tutorial_texts: Vec<i32>,
}

/// The application's presentation state of one mission.
pub struct AppPresentation {
    /// The mission's video window player (`ControlGui`'s `bMovie`).
    pub movie: MovieControl,
    /// The callout as the application sees it: the simulation's copy, copied when a frame runs it.
    pub callout: Callout,
    /// `drawGUIOn` / `toggledGUI`.
    pub gui: GuiToggles,
    /// The chat lines `TutorialText` produced, as `ControlGui::setChatText` receives them: `(speaker, message)`.
    pub tutorial_lines: Vec<(String, String)>,
    /// The string table `cLoadString` reads (`UiAssets::string`): the game's own, not the HUD's fonts and textures.
    assets: UiAssets,
    /// Where the tutorial pointer is (game pixels) while a callout runs or the GUI is frozen: the C++ `setMousePos` target.
    pub pointer: Option<(f32, f32)>,
    /// `drawGUIOn` as the last frame left it, with the movie's value (`MissionInterfaceManager::update`: `drawGUIOn = false`
    /// in movie mode, `drawGUIOn = toggledGUI` otherwise).
    pub draw_gui_on: bool,
    /// This frame's requests.
    pub events: PresentationEvents,
    /// The `objective_serial` of the last objective callout handed on by [`take_objective_flash`](Self::take_objective_flash).
    objective_serial: u32,
}

impl AppPresentation {
    /// `MissionInterfaceManager::init` (`missiongui.cpp:354-360`): no callout runs, the GUI draws.
    pub fn new() -> AppPresentation {
        AppPresentation {
            movie: MovieControl::new(None),
            callout: Callout::default(),
            gui: GuiToggles::default(),
            tutorial_lines: Vec::new(),
            assets: UiAssets::default(),
            pointer: None,
            draw_gui_on: true,
            events: PresentationEvents::default(),
            objective_serial: 0,
        }
    }

    /// [`AppPresentation::new`] with the mission's video player (its sound goes to the app mixer, like the logistics movies').
    pub fn with_movie(movie: MovieControl) -> AppPresentation {
        let mut movie = movie;
        movie.hide_first_picture();
        AppPresentation {
            movie,
            ..AppPresentation::new()
        }
    }

    /// `MissionInterfaceManager::update`'s GUI gates (`code/missiongui.cpp:435-455`, `:586-592`): in movie mode the GUI is
    /// off (and returns to `toggledGUI` afterwards) and nothing is drawn; a running callout or a frozen GUI returns before the
    /// GUI is drawn (`MissionInterfaceManager::render` draws nothing then); otherwise the player's `drawGUIOn`.
    pub fn gui_frame(&mut self, presentation: &Presentation) {
        if presentation.movie_mode {
            self.draw_gui_on = false;
            return;
        }
        self.draw_gui_on = self.gui.toggled_gui;
    }

    /// Whether the GUI takes the player's commands this frame: not in movie mode, not during a callout (`animationRunning`,
    /// which the script's `AnimationCallout` sets in the simulation) and not while frozen.
    pub fn gui_active(&self, presentation: &Presentation) -> bool {
        !presentation.movie_mode && !presentation.callout.running && !presentation.gui_frozen
    }

    /// Whether the GUI is drawn this frame. The callout and freeze branches return from update, not render
    /// (`missiongui.cpp:455-592`), so only movie mode hides an enabled HUD.
    pub fn gui_drawn(&self, presentation: &Presentation) -> bool {
        self.draw_gui_on && !presentation.movie_mode
    }

    /// The pointer position the GUI sees: the centre of the screen while the GUI is frozen (`missiongui.cpp:589-590`), the real
    /// pointer otherwise.
    pub fn gui_pointer(
        &self,
        real: (f32, f32),
        screen: (f32, f32),
        presentation: &Presentation,
    ) -> (f32, f32) {
        if !presentation.movie_mode && presentation.gui_frozen {
            (screen.0 * 0.5, screen.1 * 0.5)
        } else {
            real
        }
    }

    /// Takes this frame's presentation events and starts the videos (`ControlGui::playMovie` through the movie host).
    /// `assets` resolves `TutorialText`'s string (`cLoadString`, `execTutorialText`, `ablmc2.cpp:5839`).
    pub fn events(&mut self, vfs: &mut Vfs, events: &[SimEvent]) -> Vec<MovieRequest> {
        let mut requests = Vec::new();
        self.events = PresentationEvents::default();
        for e in events {
            match e {
                SimEvent::PlayVideo { name } => {
                    // `if (moviePlaying) return;`: one movie at a time (controlgui.cpp:2853)
                    if matches!(self.movie.playback(), Playback::Playing | Playback::Paused) {
                        continue;
                    }
                    let stem = movie_stem(name);
                    self.events.videos.push(stem.clone());
                    requests.push(MovieRequest::Open(stem));
                }
                SimEvent::TutorialText { text_id } => {
                    self.events.tutorial_texts.push(*text_id);
                    let speaker = self.assets.string(IDS_TUTORIAL as u32).to_string();
                    let text = self.assets.string(*text_id as u32).to_string();
                    self.tutorial_lines.push((speaker, text));
                }
                _ => {}
            }
        }
        for r in &requests {
            if let Err(e) = self.movie.apply(vfs, r) {
                log::warn!("mission video: {e}");
            }
        }
        requests
    }

    /// `ControlGui::update`'s movie block (`controlgui.cpp:944-954`): a playing video advances by the frame time; a video that
    /// ends closes the window.
    pub fn advance_movie(&mut self, vfs: &mut Vfs, frame_length: f32) {
        if let Err(e) = self.movie.advance(vfs, frame_length) {
            log::warn!("mission video: {e}");
        }
    }

    /// `ControlGui::render`'s video block (`controlgui.cpp:367-372`): the picture, while a movie plays.
    pub fn video_frame(&self) -> Option<&mc2_platform::movie::VideoFrame> {
        self.movie.frame()
    }

    /// A tac-map objective callout started since the last call (`startAnimation`, `missiongui.cpp:381-383`): the arguments of
    /// `controlGui.animateTacMap` (`abs(buttonId)`, the flash count), once per start.
    pub fn take_objective_flash(&mut self, presentation: &Presentation) -> Option<(i32, i32)> {
        let c = &presentation.callout;
        if c.objective_serial == self.objective_serial {
            return None;
        }
        self.objective_serial = c.objective_serial;
        (c.objective_id > 0).then_some((c.objective_id, c.objective_flashes))
    }

    /// One frame of the callout, after the simulation advanced it (`MissionInterfaceManager::update`, `missiongui.cpp:455-580`).
    /// Returns the control button that C++ `pushButton`s as this callout ends. The callout is refused in movie mode, as the GUI
    /// update returns first.
    pub fn callout_frame(&mut self, presentation: &Presentation) -> Option<i32> {
        let sim = &presentation.callout;
        let pushed = (self.callout.running
            && !sim.running
            && self.callout.target_is_button
            && self.callout.pressed_at_end)
            .then_some(self.callout.target);
        if presentation.movie_mode || !sim.running {
            self.callout = sim.clone();
            return pushed;
        }
        self.callout = sim.clone();
        None
    }

    /// The tutorial pointer of this frame (game pixels), after [`callout_frame`](Self::callout_frame): a running callout glides
    /// from `real` (the player's pointer, where the glide starts) to the centre of its target `target` over its remaining glide
    /// time, then sits on it while it flashes (`missiongui.cpp:476-500`, `:540-563`, `userInput->setMousePos`); a frozen GUI
    /// holds the pointer at the screen centre (`:586-592`). `fl` is the frame length; the simulation already took this frame's
    /// step off `time_left`, so the C++'s `timeLeftToScroll` of the frame is `time_left + fl`. `None` when the pointer is the
    /// player's own.
    pub fn pointer_frame(
        &mut self,
        presentation: &Presentation,
        real: (f32, f32),
        target: Option<(f32, f32)>,
        screen: (f32, f32),
        fl: f32,
    ) -> Option<(f32, f32)> {
        self.pointer = if presentation.movie_mode {
            None
        } else if self.callout.running {
            target.map(|t| {
                let p = self.pointer.unwrap_or(real);
                let before = self.callout.time_left + fl;
                if before > 0.0 {
                    let k = (fl / before).min(1.0);
                    (p.0 + (t.0 - p.0) * k, p.1 + (t.1 - p.1) * k)
                } else {
                    t
                }
            })
        } else if presentation.gui_frozen {
            Some((screen.0 * 0.5, screen.1 * 0.5))
        } else {
            None
        };
        self.pointer
    }

    /// The running callout's flashing control button and the colour `setColor` gives it this frame (`missiongui.cpp:503-511`):
    /// `0xff7f7f7f` in the dark half of a flash, white otherwise.
    pub fn callout_flash(&self) -> Option<(i32, u32)> {
        let c = &self.callout;
        (c.running && c.target_is_button).then(|| {
            (
                c.target,
                if c.dark_phase() {
                    0xff7f7f7f
                } else {
                    0xffffffff
                },
            )
        })
    }

    /// `GameCamera::render`'s movie overlay (`code/gamecam.cpp:219-302`): the two letterbox bars (each `letterBoxPos` of the
    /// screen high, black at `letterBoxAlpha`), then the fade colour over the whole screen at its alpha. `size` is the frame in
    /// pixels; the batch is empty outside movie mode.
    pub fn overlay(&self, presentation: &Presentation, size: (u32, u32)) -> OverlayBatch {
        let mut batch = OverlayBatch::default();
        let (w, h) = (size.0 as f32, size.1 as f32);
        if presentation.movie_mode {
            let (pos, alpha) = presentation.letterbox();
            if alpha != 0 {
                let bar = h * pos;
                let c = [0.0, 0.0, 0.0, f32::from(alpha) / 255.0];
                batch.rect(0.0, 0.0, w, bar, c);
                batch.rect(0.0, h - bar, w, bar, c);
            }
        }
        if let Some(argb) = presentation.fade_overlay() {
            let a = ((argb >> 24) & 0xff) as f32 / 255.0;
            let r = ((argb >> 16) & 0xff) as f32 / 255.0;
            let g = ((argb >> 8) & 0xff) as f32 / 255.0;
            let b = (argb & 0xff) as f32 / 255.0;
            batch.rect(0.0, 0.0, w, h, [r, g, b, a]);
        }
        batch
    }
}

impl Default for AppPresentation {
    fn default() -> Self {
        Self::new()
    }
}

/// `ControlGui::playMovie`'s `_splitpath` name: the file name without its directory and extension (`code/controlgui.cpp:2862-2869`),
/// which the movie host resolves under `data/movies`.
pub fn movie_stem(file_name: &str) -> String {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    match base.rfind('.') {
        Some(i) if i > 0 => base[..i].to_string(),
        _ => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_sim::mission::natives_presentation::{MAX_LETTERBOX_POS, MAX_LETTERBOX_TIME};

    #[test]
    fn the_movie_stem_drops_directory_and_extension_as_splitpath_does() {
        assert_eq!(movie_stem("V1A.bik"), "V1A");
        assert_eq!(movie_stem("data/movies/v1c.bik"), "v1c");
        assert_eq!(movie_stem("Data\\Movies\\V1D.BIK"), "V1D");
        assert_eq!(movie_stem("noext"), "noext");
        assert_eq!(movie_stem(".hidden"), ".hidden");
        assert_eq!(movie_stem("a.b.c"), "a.b");
    }

    #[test]
    fn the_gui_is_off_in_movie_mode_and_back_after_it() {
        let mut app = AppPresentation::new();
        let mut p = Presentation::default();
        app.gui_frame(&p);
        assert!(app.gui_drawn(&p) && app.gui_active(&p));
        p.movie_mode = true;
        app.gui_frame(&p);
        assert!(!app.gui_active(&p), "movie mode takes no command");
        assert!(!app.gui_drawn(&p), "movie mode hides the GUI");
        assert!(!app.draw_gui_on, "drawGUIOn = false in movie mode");
        p.movie_mode = false;
        app.gui_frame(&p);
        assert!(app.gui_drawn(&p), "the movie ended: the GUI returns");
        // the player's toggle (`drawGUIOn ^= true`, `missiongui.cpp:2719-2720`): hidden, and back after a movie
        app.gui.toggle();
        app.gui_frame(&p);
        assert!(!app.gui_drawn(&p), "the player hid it");
        assert!(!app.gui.toggled_gui);
        p.movie_mode = true;
        app.gui_frame(&p);
        p.movie_mode = false;
        app.gui_frame(&p);
        assert!(!app.gui_drawn(&p), "it returns to toggledGUI");
    }

    #[test]
    fn tutorial_text_and_play_video_become_a_chat_line_and_a_video_request() {
        let Some(root) = std::env::var_os("MC2_DATA") else {
            eprintln!("skipping the presentation events test: MC2_DATA is unset");
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("game VFS");
        let mut app = AppPresentation::new();
        let requests = app.events(
            &mut vfs,
            &[
                SimEvent::TutorialText { text_id: 44001 },
                SimEvent::PlayVideo {
                    name: "data\\movies\\V1A.bik".into(),
                },
            ],
        );
        // `_splitpath` keeps the name only; `ControlGui::playMovie` starts it
        assert_eq!(
            requests,
            vec![MovieRequest::Open("V1A".into())],
            "the video request is the reduced name"
        );
        // `setTutorialText`: `IDS_TUTORIAL`'s text as the speaker, the line under it
        assert_eq!(app.tutorial_lines.len(), 1);
        let (speaker, text) = &app.tutorial_lines[0];
        assert_eq!(speaker, "Tutorial", "resource.h:4360 IDS_TUTORIAL");
        assert_eq!(text, "Mission Successful!");
        // a second video is refused while the first plays (`if (moviePlaying) return;`)
        let more = app.events(
            &mut vfs,
            &[SimEvent::PlayVideo {
                name: "V1C.bik".into(),
            }],
        );
        assert!(more.is_empty());
        assert!(app.events(&mut vfs, &[]).is_empty());
    }

    #[test]
    fn a_callout_or_a_frozen_gui_closes_the_gate_and_centres_the_pointer() {
        let app = AppPresentation::new();
        let mut p = Presentation {
            gui_frozen: true,
            ..Presentation::default()
        };
        assert!(!app.gui_active(&p));
        assert!(
            app.gui_drawn(&p),
            "freeze blocks update but not HUD rendering"
        );
        assert_eq!(
            app.gui_pointer((10.0, 20.0), (800.0, 600.0), &p),
            (400.0, 300.0)
        );
        p.gui_frozen = false;
        p.callout.running = true;
        assert!(!app.gui_active(&p));
        assert!(
            app.gui_drawn(&p),
            "callout blocks update but leaves its target visible"
        );
        assert_eq!(
            app.gui_pointer((10.0, 20.0), (800.0, 600.0), &p),
            (10.0, 20.0)
        );
    }

    #[test]
    fn the_letterbox_bars_and_the_fade_are_drawn_from_the_simulation_state() {
        let app = AppPresentation::new();
        let mut p = Presentation::default();
        assert!(app.overlay(&p, (800, 600)).is_empty(), "no movie: nothing");
        p.movie_mode = true;
        assert!(
            app.overlay(&p, (800, 600)).is_empty(),
            "the bars are shut at the first frame (alpha 0)"
        );
        p.letterbox_time = MAX_LETTERBOX_TIME;
        p.letterbox_pos = MAX_LETTERBOX_POS;
        let b = app.overlay(&p, (800, 600));
        // two bars, two triangles each, fully black and opaque
        assert_eq!(b.colors(), vec![[0.0, 0.0, 0.0, 1.0]; 4]);
        p.fade_color = 0;
        p.fade_alpha = 128;
        let b = app.overlay(&p, (800, 600));
        assert_eq!(
            b.colors(),
            vec![
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0, 128.0 / 255.0],
                [0.0, 0.0, 0.0, 128.0 / 255.0],
            ]
        );
        // a half-open letterbox: each bar is MaxLetterBoxPos / 2 of the screen high, at half alpha
        p.fade_alpha = 0;
        p.letterbox_time = MAX_LETTERBOX_TIME / 2.0;
        p.letterbox_pos = MAX_LETTERBOX_POS / 2.0;
        let b = app.overlay(&p, (800, 600));
        let c = b.colors();
        assert_eq!(c.len(), 4);
        assert!((c[0][3] - 127.0 / 255.0).abs() < 1e-6, "{:?}", c[0]);
        // the first triangle is the top bar's upper half: its third corner is the bar's lower right (x, height)
        let tri = b.triangles();
        let (_, _, lower_right, _) = tri[0];
        assert_eq!(lower_right[0], 800.0, "the bar spans the frame");
        let bar = lower_right[1];
        assert!(
            (bar - 600.0 * MAX_LETTERBOX_POS / 2.0).abs() < 1e-3,
            "bar height {bar}"
        );
        // the bottom bar starts where the top one ends, mirrored
        let (_, br, _, _) = b.triangles()[2];
        assert!(
            (br[1] - (600.0 - bar)).abs() < 1e-3,
            "bottom bar top {}",
            br[1]
        );
    }

    #[test]
    fn the_fade_colour_reaches_the_overlay() {
        let app = AppPresentation::new();
        let mut p = Presentation {
            movie_mode: true,
            fade_color: 0x00ff_ffff, // FadeToColor(0, t): RGB white
            fade_alpha: 255,
            letterbox_pos: 0.0,
            ..Presentation::default()
        };
        assert_eq!(
            app.overlay(&p, (800, 600)).colors(),
            vec![[1.0, 1.0, 1.0, 1.0]; 2]
        );
        p.fade_color = 0x8012_3456;
        p.fade_alpha = 255;
        p.letterbox_pos = 0.0;
        let c = app.overlay(&p, (800, 600)).colors()[0];
        assert!((c[0] - 0x12 as f32 / 255.0).abs() < 1e-6);
        assert!((c[1] - 0x34 as f32 / 255.0).abs() < 1e-6);
        assert!((c[2] - 0x56 as f32 / 255.0).abs() < 1e-6);
        assert_eq!(c[3], 1.0);
    }

    #[test]
    fn the_tutorial_pointer_glides_to_the_target_then_sits_on_it() {
        let mut app = AppPresentation::new();
        let target = Some((400.0, 100.0));
        let screen = (800.0, 600.0);
        let mut p = Presentation {
            callout: Callout {
                running: true,
                time_left: 1.0,
                target: 7,
                target_is_button: true,
                flashes: 2,
                ..Callout::default()
            },
            ..Presentation::default()
        };
        // the glide: the simulation has stepped 0.25 s off a 1.25 s glide, so the pointer covers 0.25/1.25 of the way
        assert!(app.callout_frame(&p).is_none());
        let at = app
            .pointer_frame(&p, (0.0, 0.0), target, screen, 0.25)
            .unwrap();
        assert!(
            (at.0 - 80.0).abs() < 1e-3 && (at.1 - 20.0).abs() < 1e-3,
            "{at:?}"
        );
        // the pointer stays its own start while the player's mouse moves on
        p.callout.time_left = 0.75;
        app.callout_frame(&p);
        let next = app
            .pointer_frame(&p, (700.0, 500.0), target, screen, 0.25)
            .unwrap();
        assert!(next.0 > at.0 && next.0 < 400.0);
        // the glide is over: it sits on the target while the flashes run
        p.callout.time_left = -0.1;
        app.callout_frame(&p);
        assert_eq!(
            app.pointer_frame(&p, (700.0, 500.0), target, screen, 0.25),
            target
        );
        // the callout is over: the player's pointer again
        p.callout = Callout::default();
        app.callout_frame(&p);
        assert_eq!(
            app.pointer_frame(&p, (1.0, 2.0), target, screen, 0.25),
            None
        );
        // a frozen GUI holds the screen centre
        p.gui_frozen = true;
        assert_eq!(
            app.pointer_frame(&p, (1.0, 2.0), None, screen, 0.25),
            Some((400.0, 300.0))
        );
    }

    #[test]
    fn an_objective_callout_is_handed_to_the_tac_map_once() {
        let mut app = AppPresentation::new();
        let mut p = Presentation::default();
        assert_eq!(app.take_objective_flash(&p), None);
        assert!(p.callout.start(-2, false, false, 1.0, 3));
        assert_eq!(app.take_objective_flash(&p), Some((2, 3)));
        assert_eq!(app.take_objective_flash(&p), None);
        p.callout = Callout::default();
        assert_eq!(app.take_objective_flash(&p), None);
    }

    #[test]
    fn a_callout_never_runs_while_the_movie_plays() {
        let mut app = AppPresentation::new();
        let p = Presentation {
            movie_mode: true,
            callout: Callout {
                running: true,
                time_left: 1.0,
                ..Callout::default()
            },
            ..Presentation::default()
        };
        assert!(app.callout_frame(&p).is_none());
        assert_eq!(
            app.pointer_frame(&p, (5.0, 5.0), Some((1.0, 1.0)), (800.0, 600.0), 0.25),
            None
        );
    }
}
