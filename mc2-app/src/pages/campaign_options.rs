// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The in-campaign options screen as the sequence's options page: the `OptionsScreenWrapper`
//! `Sequence::open_options` shows over the current screen (`code/optionsscreenwrapper.cpp`,
//! reached from the main menu in the C++). The screen itself is `mc2_ui`'s `OptionsScreen`
//! (`code/optionsarea.cpp`); this page owns what the C++ front end owns around it:
//!
//! * Yes accepts: the edited prefs are written to the host's config file (`prefs.save()`,
//!   `code/optionsarea.cpp:270`) and handed to the host through [`CampaignOptions::take_accepted`].
//! * No cancels: the edits are dropped and the saved prefs stand (`prefs.load()`,
//!   `code/optionsarea.cpp:293`); nothing is written.
//! * Every setting applies, as in the main-menu options of the production host; only the volumes are
//!   refused when no audio output is installed (nothing audible would change). `rust/docs/DIFFERENCES.md` (D-90).
//!
//! The campaign itself (`Logistics`) is not changed by the options; the page takes it only
//! because the sequence's `Page` interface hands it to every call.

use mc2_campaign::Logistics;
use mc2_ui::ctx::{Loader, RenderCtx, UpdateCtx};
use mc2_ui::logistics::Page;
use mc2_ui::screen::{LogisticsScreen, Status};
use mc2_ui::screens::{OptionsEnv, OptionsFrame, OptionsResult, OptionsScreen, Prefs};
use std::any::Any;
use std::path::PathBuf;

/// The options page of the sequence's `OPTIONS` slot.
pub struct CampaignOptions {
    /// The screen (`OptionsXScreen` inside the wrapper).
    pub options: OptionsScreen,
    /// The status the sequence moves on (`bDone` closes the wrapper).
    pub status: Status,
    /// The prefs the game runs with; the accept path replaces them.
    prefs: Prefs,
    /// `orgprefs.cfg`: what the reset buttons restore.
    original: Prefs,
    /// What the machine offers the screen; the page rebuilds the screen after an accept.
    env: OptionsEnv,
    /// Whether sound is audible now (`volumes_live` of [`super::install`]).
    volumes_live: bool,
    /// The file an accepted edit is written to (`prefs.save()`). The page refuses to load without
    /// one: the C++ always writes `options.cfg` (`code/optionsarea.cpp:270`), so an accept with
    /// nowhere to persist would report a success that never happened.
    persist_to: PathBuf,
    /// The prefs of the last accept, for the host to apply.
    accepted: Option<Prefs>,
    /// The refusals to tell the player about.
    notices: Vec<String>,
    /// The volumes being edited, for the host's audio cues.
    volumes: Option<[i32; 5]>,
    /// The screen must be rebuilt from the prefs on the next begin.
    dirty: bool,
    /// The fade-in the sequence asked for before the page was rebuilt.
    fade: Option<f32>,
    /// A failure that must stop the sequence (a config file that cannot be written).
    fault: Option<String>,
}

impl CampaignOptions {
    /// `OptionsXScreen::init` over the wrapper's own copies of the prefs.
    pub fn load(
        ld: &mut Loader<'_>,
        setup: super::OptionsSetup,
    ) -> Result<CampaignOptions, String> {
        let super::OptionsSetup {
            prefs,
            original,
            env,
            volumes_live,
            persist_to,
        } = setup;
        // the accept must persist (`prefs.save()`, `code/optionsarea.cpp:270`), so a page with
        // nowhere to write is refused here rather than accepting and forgetting later
        let persist_to = persist_to.ok_or_else(|| {
            String::from(
                "the options page needs a config file to write: pass OptionsSetup::persist_to",
            )
        })?;
        Ok(CampaignOptions {
            options: OptionsScreen::load(ld, prefs.clone(), original.clone(), &env)?,
            status: Status::Running,
            prefs,
            original,
            env,
            volumes_live,
            persist_to,
            accepted: None,
            notices: Vec::new(),
            volumes: None,
            dirty: false,
            fade: None,
            fault: None,
        })
    }

    /// The prefs of the last accept, for the host to apply to the running game.
    pub fn take_accepted(&mut self) -> Option<Prefs> {
        self.accepted.take()
    }

    /// The refusals since the last take, for the host to show the player.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// The volumes being edited (master, music, sfx, radio, betty), for the host's `UiCue::Volumes`.
    pub fn take_volumes(&mut self) -> Option<[i32; 5]> {
        self.volumes.take()
    }

    /// Rebuilds the screen from the prefs the game runs with now: `OptionsXScreen::begin` resets
    /// every tab from the live `CPrefs` (`prefs.load()` then `tabAreas[i]->begin()`,
    /// `code/optionsarea.cpp:198-201`), which an accept has just changed. A screen the player
    /// closed (Yes or No) is rebuilt too: `OptionsXScreen::begin` restarts the closed
    /// `OptionsXScreen` itself, so a reopen after a cancel offers the tabs again instead of
    /// closing the wrapper on the stale `bDone`.
    fn refresh(&mut self, ld: &mut Loader<'_>) -> Result<(), String> {
        if !self.dirty && !self.options.is_done() {
            return Ok(());
        }
        self.options =
            OptionsScreen::load(ld, self.prefs.clone(), self.original.clone(), &self.env)?;
        self.dirty = false;
        if let Some(t) = self.fade.take() {
            self.options.screen.begin_fade_in(t);
        }
        Ok(())
    }

    /// Yes: `prefs.save()`; the host applies the settings (`AppliedSettings`).
    fn accepted(&mut self, prefs: Prefs) {
        let kept = self.prefs.clone();
        let mut p = prefs;
        let mut refused: Vec<&str> = Vec::new();
        // no audio output is installed, so a volume edit would change nothing audible
        if !self.volumes_live {
            let volumes = |q: &Prefs| {
                [
                    q.digital_master_volume,
                    q.music_volume,
                    q.sfx_volume,
                    q.radio_volume,
                    q.betty_volume,
                ]
            };
            if volumes(&p) != volumes(&kept) {
                p.digital_master_volume = kept.digital_master_volume;
                p.music_volume = kept.music_volume;
                p.sfx_volume = kept.sfx_volume;
                p.radio_volume = kept.radio_volume;
                p.betty_volume = kept.betty_volume;
                refused.push("Volumes");
            }
        }
        if !refused.is_empty() {
            self.notices.push(format!(
                "Changing {} is not available yet: the old setting stays",
                refused.join(", ")
            ));
        }
        self.notices
            .extend(crate::ui_host::deferred_option_notices(&kept, &p));
        // `prefs.save()`: the config file the host owns
        if let Err(e) = std::fs::write(&self.persist_to, p.to_fit()) {
            self.fault = Some(format!(
                "options: {:?} cannot be written: {e}",
                self.persist_to
            ));
            return;
        }
        self.prefs = p.clone();
        self.accepted = Some(p);
        self.dirty = true;
    }
}

impl Page for CampaignOptions {
    fn begin(&mut self, ld: &mut Loader<'_>, _lg: &mut Logistics) {
        self.status = Status::Running;
        if let Err(e) = self.refresh(ld) {
            self.fault = Some(e);
        }
    }

    /// The wrapper's `end`: the wrapper only goes away (`code/optionsscreenwrapper.cpp:59`); the
    /// accept or the cancel already told the page what to keep.
    fn end(&mut self, _lg: &mut Logistics) {}

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _lg: &mut Logistics) {
        let frame: OptionsFrame = self.options.update(ctx);
        // the frames that do not touch a slider report no volumes; the cue keeps the last
        // edited ones until the host takes it (the screen's sliders have no release event)
        if frame.volumes.is_some() {
            self.volumes = frame.volumes;
        }
        match frame.result {
            Some(OptionsResult::Accepted {
                prefs,
                resolution_changed,
            }) => {
                if resolution_changed {
                    // the host applies it now, so the screen's "next mission" dialog is dropped
                    self.options.dismiss_warning();
                }
                self.accepted(*prefs);
                if self.fault.is_some() {
                    return;
                }
                if self.options.is_done() {
                    self.status = Status::Done;
                }
            }
            // No: the saved prefs stand (`prefs.load()`); the host's volumes go back too
            Some(OptionsResult::Cancelled) => {
                let p = &self.prefs;
                self.volumes = Some([
                    p.digital_master_volume,
                    p.music_volume,
                    p.sfx_volume,
                    p.radio_volume,
                    p.betty_volume,
                ]);
                self.status = Status::Previous;
            }
            None => {}
        }
        if self.options.is_done() && self.status == Status::Running {
            self.status = Status::Done;
        }
    }

    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.options.render(ctx);
    }

    fn begin_fade_in(&mut self, t: f32) {
        if self.dirty || self.options.is_done() {
            // the screen is rebuilt on the next begin; the fade waits for it
            self.fade = Some(t);
        } else {
            self.options.screen.begin_fade_in(t);
        }
    }

    fn status(&self) -> Status {
        self.status
    }

    fn set_status(&mut self, status: Status) {
        self.status = status;
    }

    fn base(&self) -> &LogisticsScreen {
        &self.options.screen
    }

    fn base_mut(&mut self) -> &mut LogisticsScreen {
        &mut self.options.screen
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }
}
