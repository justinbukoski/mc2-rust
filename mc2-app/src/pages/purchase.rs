// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The purchase screen as a page of the logistics sequence: the grid cell `(2,0)` above the mech
//! bay (`singlePlayerScreens`, `code/missionbegin.cpp`), reached from the bay's buy button
//! (`MechBayScreen` reports `UP`) and left by Accept or Cancel (`DOWN`, back to the bay).
//!
//! The screen itself is `mc2_ui::mech_purchase::MechPurchase` (`code/mechpurchasescreen.cpp`:
//! `begin` :255, the Add/Remove handlers :454 with `addSelectedMech` :504 and
//! `removeSelectedMech` :541, and `end` :346, which sells the buys back unless the player
//! accepted). This adapter only gives it the `Page` interface the sequence drives: buying and
//! selling, the C-Bill checks and the accept/cancel restore are the screen's and the campaign's
//! (`mc2_campaign::Logistics::purchase_mech` / `sell_mech` / `can_purchase_mech`).

use mc2_campaign::Logistics;
use mc2_ui::ctx::{Loader, RenderCtx, UiOutput, UpdateCtx};
use mc2_ui::logistics::Page;
use mc2_ui::mech_purchase::MechPurchase;
use mc2_ui::screen::{LogisticsScreen, Status};
use std::any::Any;

/// The purchase page of the sequence's slot `PURCHASE`.
pub struct PurchasePage {
    /// The screen (`MechPurchaseScreen`).
    pub purchase: MechPurchase,
}

impl PurchasePage {
    /// `MechPurchaseScreen::init`: the `mcl_m$` layout and its two mech lists.
    pub fn load(ld: &mut Loader<'_>) -> Result<PurchasePage, String> {
        Ok(PurchasePage {
            purchase: MechPurchase::load(ld)?,
        })
    }
}

impl Page for PurchasePage {
    fn begin(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        // the C++ `begin` reads no input; the sequence's begin has none to give
        let mut input = super::scratch_input();
        let mut out = UiOutput::default();
        let mut ctx = super::scratch_ctx(&*ld.assets, &mut input, &mut out);
        self.purchase.begin(&mut ctx, lg);
    }

    fn end(&mut self, lg: &mut Logistics) {
        // `MechPurchaseScreen::end`: without Accept the buys are sold and the starting
        // inventory bought back
        self.purchase.end(lg);
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        self.purchase.update(ctx, lg);
    }

    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.purchase.render(ctx);
    }

    fn begin_fade_in(&mut self, t: f32) {
        self.purchase.screen.begin_fade_in(t);
    }

    fn status(&self) -> Status {
        self.purchase.status
    }

    fn set_status(&mut self, status: Status) {
        self.purchase.status = status;
    }

    fn base(&self) -> &LogisticsScreen {
        &self.purchase.screen
    }

    fn base_mut(&mut self) -> &mut LogisticsScreen {
        &mut self.purchase.screen
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
