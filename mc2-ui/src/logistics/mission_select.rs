// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MissionSelectionScreen` (`code/missionselectionscreen.cpp`): the campaign operation map with a
//! button per mission of the current group, the mission's description list, and the navigation
//! buttons. Choosing a mission button sets the campaign's current mission (`setMission`).
//!
//! The operation layout is the `.fit` the campaign names (`getCurrentOperationFileName`), placed
//! on the map rectangle of the selection layout. Its mission buttons are a radio group: the
//! pressed one is the choice, read in `update` as the C++ does (they send no message).
//! The movie the C++ plays on the screen is requested, not played here: [`MissionSelect::movie`]
//! names it, `begin` opens it (or leaves it stopped once the campaign has shown it), and the
//! movie controls send Play, Pause and Stop ([`MovieRequest`], taken with
//! [`Page::take_movie_requests`]). The app's movie host plays them once it is connected
//! ([`Page::connect_movie_host`]). Until then the movie is reported unavailable, and nothing is
//! requested or marked shown.

use super::movie::{movie_vfs_path, MovieRequest};
use super::{display_text, ids, strings, text_item, Page};
use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::font::FontRef;
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::widgets::ListBox;
use mc2_campaign::Logistics;
use mc2_formats::vfs::Vfs;

/// `MAX_MISSIONS_IN_GROUP`.
pub const MAX_MISSIONS_IN_GROUP: usize = 6;
/// `rects[]` of `mcl_cm_layout.fit`: the operation map and the description list.
const MAP_RECT: usize = 3;
const LB_RECT: usize = 4;
/// The movie viewport in the VidCom frame (`mcl_cm_layout.fit`).
const MOVIE_RECT: usize = 7;
/// `RP_TEXT`: the C-Bills text.
const RP_TEXT: usize = 4;
/// `buttons[5]`: the launch button, shown when the campaign skips logistics.
const LAUNCH_BUTTON: usize = 5;
/// `helpTextArrayID`.
const HELP_TEXT: i32 = 3;
/// What the sequence reports while the movie has no host to play it.
const MOVIE_PLAYBACK: &str = "mission movie playback";
/// Colour of the description list entries (`0xffC66600`).
const ORANGE: u32 = 0xffc6_6600;
pub struct MissionSelect {
    /// The selection layout (`mcl_cm_layout.fit`).
    pub base: LogisticsScreen,
    /// The operation map of the current group.
    pub operation: LogisticsScreen,
    /// The description list (`missionDescriptionListBox`).
    pub description: ListBox,
    font: FontRef,
    /// The movie the mission names (`getCurrentVideoFileName`), when its file exists.
    pub movie: Option<String>,
    /// Whether the player has held the movie (`MN_MSG_PAUSE`). Play and a new opening resume it.
    movie_paused: bool,
    /// Whether the app's movie host drains the requests ([`Page::connect_movie_host`]).
    movie_host_connected: bool,
    /// What the sequence reports as unavailable (the movie, while no host is connected).
    unavailable: Option<&'static str>,
    /// The movie requests not yet taken by the sequence ([`Page::take_movie_requests`]).
    movie_requests: Vec<MovieRequest>,
    /// `missionNames`: the group's mission names, in button order.
    names: Vec<String>,
    /// `pressedButton`: the chosen mission's index.
    pressed: Option<usize>,
    /// What stopped the screen from loading its operation (the sequence reports it as a failure).
    fault: Option<String>,
}

impl MissionSelect {
    /// `MissionSelectionScreen::init`.
    pub fn load(ld: &mut Loader<'_>) -> Result<MissionSelect, String> {
        let mut base = LogisticsScreen::load(
            ld,
            "mcl_cm_layout",
            Prefixes {
                statics: Some("CMStatic"),
                texts: Some("CMTextEntry"),
                rects: Some("CMRect"),
                buttons: Some("CMButton"),
                ..Prefixes::default()
            },
        )?;
        base.help_text_array_id = HELP_TEXT;
        for b in &mut base.buttons {
            b.set_message_on_release();
        }
        let rect = base
            .rect(LB_RECT)
            .ok_or("mcl_cm_layout.fit has no list rectangle")?;
        let (x, y, w, h) = (
            rect.obj.left(),
            rect.obj.top(),
            rect.obj.width(),
            rect.obj.height(),
        );
        let font = ld.font(strings::MN_LB_FONT);
        let mut description = ListBox::new(ld, x, y, w, h);
        description.set_orange();
        description.click_sfx = -1;
        description.highlight_sfx = -1;
        description.disabled_sfx = -1;
        for (id, fx) in [
            (ids::MN_MSG_PLAY, crate::ctx::sfx::LOG_VIDEOBUTTONS),
            (ids::MN_MSG_STOP, crate::ctx::sfx::LOG_VIDEOBUTTONS),
            (ids::MN_MSG_PAUSE, crate::ctx::sfx::LOG_VIDEOBUTTONS),
        ] {
            if let Some(b) = base.button_mut(id) {
                b.set_press_fx(fx);
            }
        }
        for id in [ids::MN_MSG_PLAY, ids::MN_MSG_STOP, ids::MN_MSG_PAUSE] {
            if let Some(b) = base.button_mut(id) {
                b.set_highlight_fx(crate::ctx::sfx::LOG_DIGITALHIGHLIGHT);
            }
        }
        Ok(MissionSelect {
            base,
            operation: LogisticsScreen::default(),
            description,
            font,
            movie: None,
            movie_paused: false,
            movie_host_connected: false,
            unavailable: None,
            movie_requests: Vec::new(),
            names: Vec::new(),
            pressed: None,
            fault: None,
        })
    }

    /// The chosen mission's name, if any.
    pub fn pressed_mission(&self) -> Option<&str> {
        self.pressed
            .and_then(|i| self.names.get(i))
            .map(String::as_str)
    }

    /// The mission video viewport inside the VidCom outline, in 800x600 UI space.
    pub fn movie_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let rect = &self.base.rect(MOVIE_RECT)?.obj;
        // The FIT rectangle frames the video. C++ mission selection insets playback by one UI
        // pixel on each edge so the movie cannot paint over that outline.
        Some((
            rect.left() + 1.0,
            rect.top() + 1.0,
            rect.width() - 2.0,
            rect.height() - 2.0,
        ))
    }

    /// `begin`: loads the operation, sets up the mission buttons and presses the current mission
    /// (or the first open one when none is chosen yet).
    pub fn begin_screen(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) -> Result<(), String> {
        self.base.status = Status::Running;
        self.load_operation(ld, lg)?;
        self.setup_movie(&*ld.vfs, lg);

        let choices = lg.mission_choices();
        self.names = choices.iter().map(|c| c.name.clone()).collect();
        let selected_name = lg.current_mission().to_string();
        let assets: &UiAssets = &*ld.assets;
        self.pressed = None;
        let mut pressed_here = false;
        for (i, choice) in choices.iter().enumerate() {
            let Some(button) = self.operation.buttons.get_mut(i) else {
                break;
            };
            button.set_id(ids::MSG_FIRST_MISSION + i as i32);
            button.set_press_fx(crate::ctx::sfx::LOG_SELECT);
            if !choice.available {
                button.disable(true);
                continue;
            }
            button.disable(false);
            if choice.selected {
                button.press(true);
                self.set_mission(assets, lg, i);
                pressed_here = true;
                self.pressed = Some(i);
            }
            if !pressed_here && selected_name.is_empty() {
                pressed_here = true;
                self.pressed = Some(i);
                if let Some(button) = self.operation.buttons.get_mut(i) {
                    button.press(true);
                }
                self.set_mission(assets, lg, i);
            }
        }
        for b in self.operation.buttons.iter_mut().skip(choices.len()) {
            b.obj.show_window(false);
        }
        // The layout's placeholder ("88888") must never show: write the real funds even when no
        // mission got pressed above.
        self.refresh_funds(lg);
        Ok(())
    }

    /// `initialize the operation` in `begin`: the operation's `.fit` on the map rectangle.
    fn load_operation(&mut self, ld: &mut Loader<'_>, lg: &Logistics) -> Result<(), String> {
        let Some(name) = lg.mission_info().current_operation_file() else {
            return Err("the campaign has no operation file".into());
        };
        let mut file = ld.art_fit(name)?;
        let mut op = LogisticsScreen::from_fit(
            ld,
            &mut file,
            Prefixes {
                statics: Some("Static"),
                texts: Some("Text"),
                rects: Some("Rect"),
                buttons: Some("Button"),
                ..Prefixes::default()
            },
        );
        op.move_to(0.0, 0.0);
        if let Some(map) = self.base.rect(MAP_RECT) {
            let (x, y) = (map.obj.left(), map.obj.top());
            op.move_to(x, y);
        }
        self.operation = op;
        Ok(())
    }

    /// The movie block of `begin` (`code/missionselectionscreen.cpp:228-267`). When the campaign's
    /// movie file exists, the C++ creates the movie, stops it if the campaign has shown it already,
    /// and sets the campaign's `videoShown` flag. Here that is an [`MovieRequest::Open`] (which
    /// starts it), then a [`MovieRequest::Stop`] when it was shown. A missing file leaves the page
    /// without a movie and the flag unset, as the C++ does (its `fileExists` test). With no movie
    /// host connected, nothing plays, so the flag stays as it is and the movie is reported
    /// unavailable.
    fn setup_movie(&mut self, vfs: &Vfs, lg: &mut Logistics) {
        self.movie = None;
        self.movie_paused = false;
        let Some(name) = lg
            .mission_info()
            .current_video()
            .filter(|v| !v.is_empty())
            .map(str::to_string)
        else {
            return;
        };
        if vfs.locate(&movie_vfs_path(&name)).is_none() {
            return;
        }
        self.movie = Some(name.clone());
        if !self.movie_host_connected {
            self.unavailable = Some(MOVIE_PLAYBACK);
            return;
        }
        self.movie_requests.push(MovieRequest::Open(name));
        if lg.video_shown() {
            self.movie_requests.push(MovieRequest::Stop);
        }
        lg.set_video_shown();
    }

    /// `setMission`: makes the mission the campaign's choice and refreshes the C-Bills text and
    /// the description list.
    fn set_mission(&mut self, assets: &UiAssets, lg: &mut Logistics, which: usize) {
        let Some(name) = self.names.get(which).cloned() else {
            return;
        };
        if let Err(e) = lg.choose_mission(&name) {
            log::warn!("choosing mission {name}: {e}");
        }
        self.refresh_funds(lg);
        self.update_list_box(assets, lg);
    }

    fn refresh_funds(&mut self, lg: &Logistics) {
        if let Some(t) = self.base.texts.get_mut(RP_TEXT) {
            t.set_text(&format!("{} ", lg.c_bills()));
        }
    }

    /// `updateListBox`: the divider, the mission's name, a divider, its description.
    fn update_list_box(&mut self, assets: &UiAssets, lg: &Logistics) {
        self.description.remove_all_items();
        let info = lg.mission_info();
        let width = self.description.obj.width() - self.description.scroll_bar_width() - 2.0;
        let mut add = |text: &str, sized: bool| {
            let mut item = text_item(assets, self.font, width, text, ORANGE);
            if sized {
                item.size_to_text(assets);
            }
            self.description.add_item(Box::new(item));
        };
        let divider = assets.string(strings::MN_DIVIDER).to_string();
        add(&divider, false);
        add(
            &display_text(assets, info.current_mission_friendly_name()),
            false,
        );
        add(&divider, false);
        add(
            &display_text(assets, info.current_mission_description()),
            true,
        );
    }

    /// `handleMessage` of the selection screen.
    fn handle_message(&mut self, assets: &UiAssets, lg: &mut Logistics, who: i32) {
        let first = ids::MSG_FIRST_MISSION;
        if who >= first && who < first + MAX_MISSIONS_IN_GROUP as i32 {
            self.set_mission(assets, lg, (who - first) as usize);
        }
        // with no movie host, the controls of a mission that has a movie are refused and the
        // buttons keep their state (the playback is not connected)
        if self.movie.is_some()
            && !self.movie_host_connected
            && matches!(who, ids::MN_MSG_PLAY | ids::MN_MSG_STOP | ids::MN_MSG_PAUSE)
        {
            self.unavailable = Some(MOVIE_PLAYBACK);
            return;
        }
        match who {
            ids::MB_MSG_NEXT => self.base.status = Status::Next,
            ids::MB_MSG_PREV => self.base.status = Status::Previous,
            // `bMovie->restart()`, then both buttons up (`code/missionselectionscreen.cpp:353-358`).
            // The C++ leaves its pause flag set, so a Play after a Pause shows the Pause button up
            // over a picture that stays held. Play resumes here (a fix; see DIFFERENCES.md).
            ids::MN_MSG_PLAY => {
                self.movie_paused = false;
                self.request_movie(MovieRequest::Play);
                if let Some(b) = self.base.button_mut(ids::MN_MSG_STOP) {
                    b.press(false);
                }
                if let Some(b) = self.base.button_mut(ids::MN_MSG_PAUSE) {
                    b.press(false);
                }
            }
            // `bMovie->stop()` (`:359-364`); the buttons keep their state
            ids::MN_MSG_STOP => self.request_movie(MovieRequest::Stop),
            // The C++ reads `isPressed()` after the release has already cleared the button (its
            // `messageOnRelease` path, `gui/abutton.cpp`), so a click always resumed and never
            // paused. The pause is toggled here and the button shows it (a fix; see DIFFERENCES.md).
            ids::MN_MSG_PAUSE => {
                self.movie_paused = !self.movie_paused;
                self.request_movie(MovieRequest::Pause(self.movie_paused));
                let paused = self.movie_paused;
                if let Some(b) = self.base.button_mut(ids::MN_MSG_PAUSE) {
                    b.press(paused);
                }
            }
            ids::MB_MSG_MAINMENU => self.base.status = Status::MainMenu,
            _ => {}
        }
    }

    /// `update`.
    pub fn update_screen(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        for m in self.base.update(ctx) {
            self.handle_message(ctx.assets, lg, m.who);
        }
        self.operation.update(ctx);

        // a mission button pressed by the player becomes the choice (the radio group)
        let newly = self
            .operation
            .buttons
            .iter()
            .enumerate()
            .find(|(i, b)| b.obj.show && b.is_pressed() && Some(*i) != self.pressed)
            .map(|(i, _)| i);
        if let Some(i) = newly {
            self.set_mission(ctx.assets, lg, i);
            let old = self.pressed.replace(i);
            if let Some(o) = old {
                if let Some(b) = self.operation.buttons.get_mut(o) {
                    b.press(false);
                }
            }
        }

        self.description.update(ctx);

        let name = self.pressed.and_then(|i| self.names.get(i)).cloned();
        let title = match name {
            Some(n) => display_text(ctx.assets, lg.mission_info().mission_friendly_name(&n)),
            None => String::new(),
        };
        if let Some(t) = self.operation.texts.first_mut() {
            t.set_text(&title);
        }

        let skip = lg.skip_logistics();
        if let Some(b) = self.base.buttons.get_mut(LAUNCH_BUTTON) {
            b.obj.show_window(skip);
        }
    }

    /// Sends a movie request when the mission has a movie (the C++ touches `bMovie` only when it
    /// exists), and only once the movie host is connected to take it.
    fn request_movie(&mut self, request: MovieRequest) {
        if self.movie.is_some() && self.movie_host_connected {
            self.movie_requests.push(request);
        }
    }

    /// `end`: releases the movie (`bMovie->stop(); delete bMovie`) and fades out (`beginFadeOut(0)`).
    pub fn end_screen(&mut self) {
        self.request_movie(MovieRequest::Close);
        self.base.begin_fade_out(0.0);
    }

    /// `render`: the description list under the layout, then the operation map.
    pub fn render_screen(&mut self, ctx: &mut RenderCtx<'_>) {
        self.description.render(ctx);
        self.base.render(ctx);
        self.operation.render(ctx);
    }
}

impl Page for MissionSelect {
    fn begin(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.fault = None;
        if let Err(e) = self.begin_screen(ld, lg) {
            log::error!("mission selection: {e}");
            self.fault = Some(format!("mission selection: {e}"));
        }
    }
    fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }
    fn connect_movie_host(&mut self) {
        self.movie_host_connected = true;
    }
    fn take_movie_requests(&mut self) -> Vec<MovieRequest> {
        std::mem::take(&mut self.movie_requests)
    }
    fn take_unavailable(&mut self) -> Option<&'static str> {
        self.unavailable.take()
    }
    fn end(&mut self, _lg: &mut Logistics) {
        self.end_screen();
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        self.update_screen(ctx, lg);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.render_screen(ctx);
    }
    fn begin_fade_in(&mut self, t: f32) {
        self.base.begin_fade_in(t);
    }
    fn status(&self) -> Status {
        self.base.get_status()
    }
    fn set_status(&mut self, status: Status) {
        self.base.status = status;
    }
    fn base(&self) -> &LogisticsScreen {
        &self.base
    }
    fn base_mut(&mut self) -> &mut LogisticsScreen {
        &mut self.base
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
