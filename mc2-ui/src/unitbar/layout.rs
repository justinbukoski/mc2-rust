// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The unit bar's slot rectangles: what `ForceGroupIcon::init` reads from the `MechIconN` blocks
//! (`code/mechicon.cpp:197-296`), called for N = 0 to 15 by `ForceGroupBar::init`
//! (`code/forcegroupbar.cpp:379-399`).
//!
//! The offsets are `ControlGui::hiResOffsetX/Y`, read from the `HiresOffsets` block of the same
//! layout file (`code/controlgui.cpp:2671-2689`); a file without the block has none.

use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::unitbar::MAX_ICONS;
use mc2_formats::ini::FitIni;

/// The rectangles of one slot (`ForceGroupIcon`'s static arrays indexed by `locationIndex`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    /// `textArea`: the force group colour behind the pilot's name.
    pub text_area: GuiRect,
    /// `pilotTextTop`: the y of the pilot's name.
    pub pilot_text_top: i32,
    /// `bmpLocation`: where the unit's icon is drawn.
    pub icon_area: GuiRect,
    /// `pilotLocation`: where the pilot's portrait is drawn.
    pub pilot_area: GuiRect,
    /// `pilotRect`: the portrait's frame.
    pub pilot_rect: GuiRect,
    /// `selectionRect`: the clickable area and the selection outline.
    pub selection: GuiRect,
    /// `healthBar`: the full-length health bar.
    pub health_bar: GuiRect,
}

/// The sixteen slots, left to right in the order the bar fills them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub slots: [Slot; MAX_ICONS],
    /// The mission's video window (`VideoWindow` in the layout file), where `ControlGui::playMovie` plays an ABL
    /// `PlayVideo` (`code/controlgui.cpp:2851-2890`).
    pub video_window: VideoWindow,
}

/// `ControlGui`'s `videoRect` (`code/controlgui.cpp:2489-2499`): where a mission video plays, moved by the x hi-res
/// offset only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoWindow {
    pub rect: GuiRect,
    /// `VideoStaticCount`: the static pictures drawn under the movie (`videoInfos`). Not drawn yet.
    pub static_count: i32,
}

impl VideoWindow {
    /// The `VideoWindow` block: an error when the block or a key is missing (`init` asserts on the block,
    /// `controlgui.cpp:2487`, and reads the keys without defaults).
    fn from_fit(file: &mut FitIni, ox: i32) -> Result<VideoWindow, String> {
        file.seek_block("VideoWindow")
            .map_err(|e| format!("unit bar block VideoWindow: {e}"))?;
        let static_count = key(file, "VideoStaticCount")?;
        let left = key(file, "left")? + ox;
        let right = key(file, "right")? + ox;
        let top = key(file, "top")?;
        let bottom = key(file, "bottom")?;
        Ok(VideoWindow {
            rect: GuiRect::new(left, top, right, bottom),
            static_count,
        })
    }
}

impl Layout {
    /// A layout with no slots: what a test built by hand starts from.
    #[cfg(test)]
    pub(crate) fn default_for_tests() -> Layout {
        Layout {
            slots: [Slot::default(); MAX_ICONS],
            video_window: VideoWindow::default(),
        }
    }

    /// Reads the sixteen `MechIconN` blocks and the hi-res offsets from a layout `.fit` file. A missing
    /// block or key is an error, not zeros: `ForceGroupIcon::init` asserts on a missing icon block
    /// (`code/mechicon.cpp:200-201`), and a slot of zero rectangles would take no clicks and draw nowhere.
    pub fn from_fit(file: &mut FitIni) -> Result<Layout, String> {
        Self::from_fit_wide(file, false)
    }

    /// [`Layout::from_fit`] for a wide screen (`wide`), which puts the HUD block at its left edge: the layout's x
    /// offset, which centres the block, is dropped (a port extension, see `ControlPanel::load_wide`).
    pub fn from_fit_wide(file: &mut FitIni, wide: bool) -> Result<Layout, String> {
        let (ox, oy) = hires_offsets(file);
        let ox = if wide { 0 } else { ox };
        let mut slots = [Slot::default(); MAX_ICONS];
        for (i, slot) in slots.iter_mut().enumerate() {
            *slot = read_slot(file, i, ox, oy)?;
        }
        let video_window = VideoWindow::from_fit(file, ox)?;
        Ok(Layout {
            slots,
            video_window,
        })
    }
}

/// `ControlGui::init`'s `HiresOffsets` (`code/controlgui.cpp:2671-2689`): both read as 0 when the block
/// or a key is missing.
fn hires_offsets(file: &mut FitIni) -> (i32, i32) {
    if file.seek_block("HiresOffsets").is_err() {
        return (0, 0);
    }
    (
        file.long_or("xOffset", 0) as i32,
        file.long_or("yOffset", 0) as i32,
    )
}

/// A key that the slot needs: an error when it is missing.
fn key(file: &FitIni, name: &str) -> Result<i32, String> {
    file.read_i64(name)
        .map(|v| v as i32)
        .map_err(|e| format!("{name}: {e}"))
}

/// One area of a slot: its `left`, `right`, `top` and `bottom` keys, moved by the hi-res offsets
/// (`mechicon.cpp:212-289`).
fn area(file: &FitIni, names: [&str; 4], dx: i32, dy: i32) -> Result<GuiRect, String> {
    Ok(GuiRect::new(
        key(file, names[0])? + dx,
        key(file, names[2])? + dy,
        key(file, names[1])? + dx,
        key(file, names[3])? + dy,
    ))
}

fn read_slot(file: &mut FitIni, i: usize, ox: i32, oy: i32) -> Result<Slot, String> {
    let block = format!("MechIcon{i}");
    file.seek_block(&block)
        .map_err(|e| format!("unit bar block {block}: {e}"))?;
    // mechicon.cpp:207-210 (keys are spelled as the file spells them, including `TextAReaLeft`)
    let text_area = area(
        file,
        [
            "TextAReaLeft",
            "TextAreaRight",
            "TextAReaTop",
            "TextAreaBottom",
        ],
        ox,
        oy,
    )?;
    // mechicon.cpp:217-219
    let pilot_text_top = key(file, "PilotTextTop")? + oy;
    // mechicon.cpp:222-230 (`MechIconArearight` is spelled that way in the file)
    let icon_area = area(
        file,
        [
            "MechIconAreaLeft",
            "MechIconArearight",
            "MechIconAreaTop",
            "MechIconAreaBottom",
        ],
        ox,
        oy,
    )?;
    // mechicon.cpp:245-259
    let pilot_area = area(
        file,
        [
            "PilotAreaLeft",
            "PilotAreaRight",
            "PilotAreaTop",
            "PilotAreaBottom",
        ],
        ox,
        oy,
    )?;
    // mechicon.cpp:261-269
    let pilot_rect = area(
        file,
        [
            "PilotRectLeft",
            "PilotRectRight",
            "PilotRectTop",
            "PilotRectBottom",
        ],
        ox,
        oy,
    )?;
    // mechicon.cpp:271-279
    let selection = area(
        file,
        [
            "SelectionRectLeft",
            "SelectionRectRight",
            "SelectionRectTop",
            "SelectionRectBottom",
        ],
        ox,
        oy,
    )?;
    // mechicon.cpp:281-289
    let health_bar = area(
        file,
        [
            "HealthBarLeft",
            "HealthBarRight",
            "HealthBarTop",
            "HealthBarBottom",
        ],
        ox,
        oy,
    )?;
    Ok(Slot {
        text_area,
        pilot_text_top,
        icon_area,
        pilot_area,
        pilot_rect,
        selection,
        health_bar,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_formats::ini::FitWriter;

    /// The keys of one slot, as the file spells them: the values of slot 0 moved `62 * i` to the right,
    /// which is the step within a row of the shipped layout (slots 8 to 15 start a second row).
    fn slot_keys(i: usize) -> [(&'static str, i64); 25] {
        let k = i as i64 * 62;
        [
            ("TextAReaLeft", 179 + k),
            ("TextAreaRight", 237 + k),
            ("TextAReaTop", 487),
            ("TextAreaBottom", 497),
            ("PilotTextTop", 488),
            ("MechIconAreaLeft", 179 + k),
            ("MechIconArearight", 210 + k),
            ("MechIconAreaTop", 502),
            ("MechIconAreaBottom", 539),
            ("PilotAreaLeft", 212 + k),
            ("PilotAreaRight", 236 + k),
            ("PilotAreaTop", 503),
            ("PilotAreaBottom", 538),
            ("PilotRectLeft", 211 + k),
            ("PilotRectRight", 237 + k),
            ("PilotRectTop", 502),
            ("PilotRectBottom", 539),
            ("SelectionRectLeft", 178 + k),
            ("SelectionRectRight", 238 + k),
            ("SelectionRectTop", 486),
            ("SelectionRectBottom", 540),
            ("HealthBarLeft", 179 + k),
            ("HealthBarRight", 210 + k),
            ("HealthBarTop", 498),
            ("HealthBarBottom", 501),
        ]
    }

    /// A layout file with sixteen slots, `omit_block` left out and `omit_key` (a slot and a key name)
    /// left out of its block, with the given hi-res offsets.
    fn synthetic_with(
        offset: Option<(i64, i64)>,
        omit_block: Option<usize>,
        omit_key: Option<(usize, &str)>,
    ) -> FitIni {
        synthetic_without_video(offset, omit_block, omit_key, false)
    }

    /// [`synthetic_with`], with `omit_video` leaving the `VideoWindow` block out.
    fn synthetic_without_video(
        offset: Option<(i64, i64)>,
        omit_block: Option<usize>,
        omit_key: Option<(usize, &str)>,
        omit_video: bool,
    ) -> FitIni {
        let mut w = FitWriter::new();
        if let Some((x, y)) = offset {
            w.block("HiresOffsets");
            w.write_i64("xOffset", x);
            w.write_i64("yOffset", y);
        }
        for i in 0..MAX_ICONS {
            if omit_block == Some(i) {
                continue;
            }
            w.block(&format!("MechIcon{i}"));
            for (name, v) in slot_keys(i) {
                if omit_key != Some((i, name)) {
                    w.write_i64(name, v);
                }
            }
        }
        // the shipped 800x600 buttonlayout800.fit's VideoWindow block
        if !omit_video {
            w.block("VideoWindow");
            w.write_i64("VideoStaticCount", 0);
            w.write_i64("left", 667);
            w.write_i64("right", 795);
            w.write_i64("top", 5);
            w.write_i64("bottom", 77);
        }
        FitIni::from_bytes(w.finish()).expect("fit")
    }

    fn synthetic(offset: Option<(i64, i64)>) -> FitIni {
        synthetic_with(offset, None, None)
    }

    #[test]
    fn reads_every_key_of_each_slot_without_offsets() {
        // slot 0 values are the shipped buttonlayout800.fit's MechIcon0 block (read with the
        // dump of the game data): selection 178..238 x 486..540, icon 179..210 x 502..539
        let layout = Layout::from_fit(&mut synthetic(None)).expect("layout");
        let s0 = layout.slots[0];
        assert_eq!(s0.selection, GuiRect::new(178, 486, 238, 540));
        assert_eq!(s0.text_area, GuiRect::new(179, 487, 237, 497));
        assert_eq!(s0.pilot_text_top, 488);
        assert_eq!(s0.icon_area, GuiRect::new(179, 502, 210, 539));
        assert_eq!(s0.pilot_area, GuiRect::new(212, 503, 236, 538));
        assert_eq!(s0.pilot_rect, GuiRect::new(211, 502, 237, 539));
        assert_eq!(s0.health_bar, GuiRect::new(179, 498, 210, 501));
        // slot 15 starts 15 steps of 62 to the right
        assert_eq!(
            layout.slots[15].selection,
            GuiRect::new(178 + 930, 486, 238 + 930, 540)
        );
    }

    #[test]
    fn hires_offsets_move_every_area_as_the_original_does() {
        // mechicon.cpp:212-289: x offsets move left and right edges, y offsets move top and bottom,
        // and PilotTextTop moves with y only (mechicon.cpp:219)
        let layout = Layout::from_fit(&mut synthetic(Some((10, -4)))).expect("layout");
        let s0 = layout.slots[0];
        assert_eq!(s0.selection, GuiRect::new(188, 482, 248, 536));
        assert_eq!(s0.text_area, GuiRect::new(189, 483, 247, 493));
        assert_eq!(s0.pilot_text_top, 484);
        assert_eq!(s0.health_bar, GuiRect::new(189, 494, 220, 497));
    }

    #[test]
    fn the_video_window_is_read_from_the_layout() {
        let layout = Layout::from_fit(&mut synthetic(None)).expect("layout");
        assert_eq!(layout.video_window.rect, GuiRect::new(667, 5, 795, 77));
        // only the x hi-res offset moves it (`controlgui.cpp:2495-2496`)
        let layout = Layout::from_fit(&mut synthetic(Some((10, -4)))).expect("layout");
        assert_eq!(layout.video_window.rect, GuiRect::new(677, 5, 805, 77));
    }

    #[test]
    fn a_missing_video_window_block_is_an_error() {
        // ControlGui::init asserts on it (`controlgui.cpp:2487`)
        let err =
            Layout::from_fit(&mut synthetic_without_video(None, None, None, true)).unwrap_err();
        assert!(err.contains("VideoWindow"), "{err}");
    }

    #[test]
    fn a_missing_icon_block_is_an_error() {
        // ForceGroupIcon::init asserts on a missing icon block (mechicon.cpp:200-201)
        let err = Layout::from_fit(&mut synthetic_with(None, Some(15), None)).unwrap_err();
        assert!(err.contains("MechIcon15"), "{err}");
    }

    #[test]
    fn a_missing_key_is_an_error() {
        // a slot without its selection top has no clickable area: the read fails, it does not read 0
        let err = Layout::from_fit(&mut synthetic_with(
            None,
            None,
            Some((3, "SelectionRectTop")),
        ))
        .unwrap_err();
        assert!(err.contains("SelectionRectTop"), "{err}");
    }

    #[test]
    fn missing_hires_offsets_are_zero_not_an_error() {
        // controlgui.cpp:2671-2689 reads the offsets with defaults, so a file without them is valid
        let layout = Layout::from_fit(&mut synthetic(None)).expect("layout");
        assert_eq!(layout.slots[0].pilot_text_top, 488);
    }
}
