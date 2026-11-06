// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The mech lab as a page of the logistics sequence: the grid cell `(2,2)` below the mech bay
//! (`singlePlayerScreens`, `code/missionbegin.cpp`). The bay's change-loadout button starts the
//! edit (`MechBayScreen` remembers the mech and reports `DOWN`; the bay's `end` runs
//! `setMechToModify`), and the lab's Accept and Cancel both report `UP` back to the bay
//! (`code/mechlabscreen.cpp`: `begin` :232, `end` :357, `handleMessage` :1022 with
//! `acceptMechModificationsUseOldVariant` :1053 and `cancelMechModfications` :1072).
//!
//! The screen is `mc2_ui::mech_lab::MechLab`; the lab rules (tonnage, slots, heat, cost, what
//! may replace what) are `mc2-campaign`'s (`Logistics::lab_*`, `LogisticsVariant`). This
//! adapter only gives the screen the `Page` interface the sequence drives.
//!
//! The screen's render reads the campaign (the component diagram), but the sequence's
//! `Page::render` hands over no campaign, so the page keeps a copy of the one its `update` just
//! saw (`Logistics` is `Clone`, its game data an `Arc`). The copy is taken every frame the page
//! is on screen, so what the diagram draws is the campaign the last update left; the sequence
//! never renders a page it has not begun.

use mc2_campaign::Logistics;
use mc2_ui::ctx::{Loader, RenderCtx, UiOutput, UpdateCtx};
use mc2_ui::logistics::Page;
use mc2_ui::mech_lab::MechLab;
use mc2_ui::screen::{LogisticsScreen, Status};
use std::any::Any;

/// The mech lab page of the sequence's slot `MECH_LAB`.
pub struct MechLabPage {
    /// The screen (`MechLabScreen`).
    pub lab: MechLab,
    /// The campaign as the last update saw it, for the screen's render.
    snapshot: Option<Logistics>,
}

impl MechLabPage {
    /// `MechLabScreen::init`: the `mcl_mc` layout, the variant drop-down and the dialogs.
    pub fn load(ld: &mut Loader<'_>) -> Result<MechLabPage, String> {
        Ok(MechLabPage {
            lab: MechLab::load(ld)?,
            snapshot: None,
        })
    }
}

impl Page for MechLabPage {
    fn begin(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        // the paper doll of the mech and the component icon art
        self.lab.load_art(ld, lg);
        // the C++ `begin` reads no input; the sequence's begin has none to give
        let mut input = super::scratch_input();
        let mut out = UiOutput::default();
        let mut ctx = super::scratch_ctx(&*ld.assets, &mut input, &mut out);
        self.lab.begin(&mut ctx, lg);
        self.snapshot = Some(lg.clone());
    }

    fn end(&mut self, _lg: &mut Logistics) {
        // `MechLabScreen::end`; the accept or cancel already told the campaign
        self.lab.end();
        self.snapshot = None;
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        self.lab.update(ctx, lg);
        self.snapshot = Some(lg.clone());
    }

    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if let Some(lg) = &self.snapshot {
            self.lab.render(ctx, lg);
        }
    }

    fn begin_fade_in(&mut self, t: f32) {
        self.lab.screen.begin_fade_in(t);
    }

    fn status(&self) -> Status {
        self.lab.status
    }

    fn set_status(&mut self, status: Status) {
        self.lab.status = status;
    }

    fn base(&self) -> &LogisticsScreen {
        &self.lab.screen
    }

    fn base_mut(&mut self) -> &mut LogisticsScreen {
        &mut self.lab.screen
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
