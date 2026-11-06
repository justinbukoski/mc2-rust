// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Settled visual-QA questions (triage 3d111c56) that are answered by the shipped layout data.
//! Without `MC2_DATA` every test returns early.

use mc2_formats::vfs::Vfs;
use mc2_ui::logistics::pilot_ready::PilotReady;
use mc2_ui::screens::SaveLoadDialog;
use mc2_ui::widgets::basic::font_width;
use mc2_ui::{Loader, UiAssets};

fn data() -> Option<Vfs> {
    Some(Vfs::from_system_cfg(std::env::var_os("MC2_DATA")?).expect("vfs"))
}

/// The four specialty lines of the Pilot Ready right panel are right-justified in a 200-wide
/// region that ends at x 675 (`mcl_pr_layout.fit` `PilotReadyTextEntry9..12`: x 475, width 200,
/// `Alignment = 1`; `gos_TextDraw` puts the line at `region_width - str_width`,
/// gameos_graphics.cpp:2086-2089), so a long name starts left of the panel's left edge at x 528
/// (statics 61/72) in the original too: nothing wraps below 200 px and the font is 8 px a glyph.
#[test]
fn pilot_ready_specialty_lines_are_right_justified_and_overflow_the_panel_like_the_original() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let pr = PilotReady::load(&mut ld).unwrap();
    for i in 9..13 {
        let t = &pr.base.texts[i];
        assert_eq!(
            (t.obj.left(), t.obj.width(), t.alignment),
            (475.0, 200.0, 1)
        );
    }
    let font = pr.base.texts[9].font;
    let widths: Vec<(String, i32)> = (0..23)
        .map(|k| {
            let s = assets.string(41001 + k).to_string();
            let w = font_width(&assets, font, &s);
            (s, w)
        })
        .collect();
    let right = 475 + 200;
    let panel_left = 528;
    assert!(
        widths.iter().all(|(_, w)| *w < 200),
        "no specialty name wraps in the 200 px region"
    );
    let overflowing: Vec<_> = widths
        .iter()
        .filter(|(_, w)| right - w < panel_left)
        .collect();
    assert!(
        overflowing
            .iter()
            .any(|(s, w)| s == "Light 'Mech Specialist" && *w == 176),
        "Light 'Mech Specialist starts at x {} (left of {panel_left})",
        right - 176
    );
    println!(
        "{} of 23 names start left of the panel edge",
        overflowing.len()
    );
}

/// DELETE of the save dialog (`mcl_sm.fit` `Button0`, id 40) has `AnimateBmp = 1`, so its
/// picture and label take the animation colour: Normal `0xffD20000` (red), and for Disabled the
/// keys are written `DisabledColor` without an index, so `aAnimation::init` falls back to
/// `0xffffffff` (aanim.cpp:100-103), i.e. white. `LogisticsSaveDialog::update` enables the button
/// every frame (logisticsdialog.cpp:684-685), so the original's DELETE is red, not white.
#[test]
fn save_dialog_delete_is_enabled_and_red_like_the_original() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let dialog = SaveLoadDialog::load(&mut ld).unwrap();
    let delete = dialog.screen.button(40).expect("DELETE");
    assert_eq!(delete.normal.color(), 0xffd2_0000, "Normal is red");
    assert_eq!(
        delete.disabled.color(),
        0xffff_ffff,
        "Disabled falls back to white"
    );
    assert!(delete.animate_bmp && delete.animate_text);
}
