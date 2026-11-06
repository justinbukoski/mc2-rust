// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The app-side campaign pages of the logistics sequence: [`PurchasePage`] wraps
//! `mc2-ui`'s `MechPurchase`, [`MechLabPage`] its `MechLab`, and [`CampaignOptions`] the options
//! screen `Sequence::open_options` shows over the current screen. The screens' rules live in
//! those crates; these adapters are the `LogisticsScreen`s the sequence's grid drives
//! (`code/missionbegin.cpp`), the way the C++ owns the same screens through `MissionBegin`.
//!
//! The production host (`ui_host`) installs them with [`install`]. `install` takes the page list
//! `build_sequence` made and replaces the three placeholders before the sequence begins.

pub mod campaign_options;
pub mod mech_lab;
pub mod purchase;

pub use campaign_options::CampaignOptions;
pub use mech_lab::MechLabPage;
pub use purchase::PurchasePage;

use mc2_formats::vfs::Vfs;
use mc2_ui::input::UiInput;
use mc2_ui::logistics::sequence::slot;
use mc2_ui::logistics::Page;
use mc2_ui::screens::{OptionsEnv, Prefs};
use mc2_ui::{Loader, UiAssets, UiOutput, UpdateCtx};
use std::path::PathBuf;

/// What the options page is installed with: the inputs the C++ front end reads at start-up.
///
/// * `prefs`: the options the game runs with (`options.cfg` at start-up).
/// * `original`: the defaults the reset buttons restore (`orgprefs.cfg`).
/// * `env`: what the machine offers the options screen (display modes, devices).
/// * `volumes_live`: whether sound is audible now; the options page keeps the old volumes and
///   tells the player when it is not (see [`CampaignOptions`]).
/// * `persist_to`: the file an accepted options edit is written to. The options page refuses to
///   load without one (`CampaignOptions::load`): the C++ always writes `options.cfg`
///   (`code/optionsarea.cpp:270`), so an accept with nowhere to persist would report a success
///   that never happened. The host passes its user config file (`UserPaths::config`); the port
///   never writes the game data, so the host owns the writable location.
#[derive(Clone)]
pub struct OptionsSetup {
    pub prefs: Prefs,
    pub original: Prefs,
    pub env: OptionsEnv,
    pub volumes_live: bool,
    pub persist_to: Option<PathBuf>,
}

/// Replaces the purchase, mech lab and options placeholders of the page list the host's
/// `build_sequence` makes (see `mc2_app::ui_host`) with the production campaign pages, loading
/// their layouts from `vfs` into `assets`; `options` is the [`OptionsSetup`] the options page
/// installs with.
///
/// The swap is all-or-nothing: every page is loaded before any placeholder is replaced, so a
/// load failure (a page's layout or art, or the options page's missing config file) leaves all
/// three placeholders standing — the deferred pages stay explicitly unavailable — and the host
/// is told why. Errs, with the reason, when the page list is not the sequence's own page count.
pub fn install(
    vfs: &mut Vfs,
    assets: &mut UiAssets,
    pages: &mut [Box<dyn Page>],
    options: OptionsSetup,
) -> Result<(), String> {
    if pages.len() != slot::COUNT {
        return Err(format!(
            "the sequence needs {} pages, got {}",
            slot::COUNT,
            pages.len()
        ));
    }
    let mut ld = Loader::new(vfs, assets);
    // all three pages load before any slot is replaced: a later failure must not leave an
    // earlier slot half-installed
    let purchase = PurchasePage::load(&mut ld)?;
    let mech_lab = MechLabPage::load(&mut ld)?;
    let options = CampaignOptions::load(&mut ld, options)?;
    pages[slot::PURCHASE] = Box::new(purchase);
    pages[slot::MECH_LAB] = Box::new(mech_lab);
    pages[slot::OPTIONS] = Box::new(options);
    Ok(())
}

/// A throwaway update context for a screen's `begin`, which the C++ runs without input too
/// (`MechPurchaseScreen::begin`, `MechLabScreen::begin`): the sequence's `Page::begin` hands over
/// only the loader, so the frame is a no-input, no-output one.
pub(crate) fn scratch_ctx<'a>(
    assets: &'a mc2_ui::UiAssets,
    input: &'a mut UiInput,
    out: &'a mut UiOutput,
) -> UpdateCtx<'a> {
    UpdateCtx {
        input,
        assets,
        dt: 0.0,
        out,
    }
}

/// An 800x600 throwaway input, the size the screens' coordinate space has (`Environment`).
pub(crate) fn scratch_input() -> UiInput {
    UiInput::new(800, 600)
}
