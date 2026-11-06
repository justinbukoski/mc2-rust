//! Headless driver for widgets and screens: feeds scripted events, runs one update per frame and
//! records the draws. Used by this crate's tests and available to the screen lanes (U2, U3) and
//! the app's scripted runs.

use crate::ctx::{RenderCtx, UiAssets, UiOutput, UpdateCtx};
use crate::draw::DrawList;
use crate::input::UiInput;
use mc2_platform::input::InputEvent;

pub use crate::input::events;

pub struct Harness {
    pub input: UiInput,
    pub assets: UiAssets,
    pub out: UiOutput,
    /// Seconds per frame.
    pub dt: f32,
}

impl Harness {
    /// An 800x600 UI with the built-in strings and no fonts or textures loaded.
    pub fn new() -> Harness {
        Harness::with_assets(UiAssets::default())
    }

    pub fn with_assets(assets: UiAssets) -> Harness {
        let (w, h) = (assets.screen.0 as u32, assets.screen.1 as u32);
        Harness {
            input: UiInput::new(w, h),
            assets,
            out: UiOutput::default(),
            dt: 1.0 / 30.0,
        }
    }

    /// Feeds `events`, runs `f` as this frame's update and ends the frame. The output collected
    /// during the frame is left in [`Harness::out`] (cleared at the start of each frame).
    pub fn frame<R>(
        &mut self,
        events: &[InputEvent],
        f: impl FnOnce(&mut UpdateCtx<'_>) -> R,
    ) -> R {
        for e in events {
            self.input.handle(e);
        }
        self.input.begin_frame(self.dt);
        self.out = UiOutput::default();
        let r = {
            let mut ctx = UpdateCtx {
                input: &mut self.input,
                assets: &self.assets,
                dt: self.dt,
                out: &mut self.out,
            };
            f(&mut ctx)
        };
        self.input.end_frame();
        r
    }

    /// Like [`Harness::frame`], but every event is routed through `screen` first, as the app
    /// does for a non-modal screen; returns which events the UI took and the update's value.
    pub fn routed_frame<R>(
        &mut self,
        screen: &mut crate::screen::LogisticsScreen,
        events: &[InputEvent],
        f: impl FnOnce(&mut crate::screen::LogisticsScreen, &mut UpdateCtx<'_>) -> R,
    ) -> (Vec<bool>, R) {
        let taken: Vec<bool> = events
            .iter()
            .map(|e| screen.route(&mut self.input, &self.assets, e))
            .collect();
        let r = self.frame(&[], |ctx| f(screen, ctx));
        (taken, r)
    }

    /// Runs `n` frames without input.
    pub fn idle<R>(&mut self, n: usize, mut f: impl FnMut(&mut UpdateCtx<'_>) -> R) {
        for _ in 0..n {
            self.frame(&[], &mut f);
        }
    }

    /// Records a render into a fresh draw list.
    pub fn render(&self, f: impl FnOnce(&mut RenderCtx<'_>)) -> DrawList {
        let mut dl = DrawList::new(self.assets.screen.0, self.assets.screen.1);
        let mut ctx = RenderCtx {
            dl: &mut dl,
            assets: &self.assets,
            dt: self.dt,
        };
        f(&mut ctx);
        dl
    }
}

impl Default for Harness {
    fn default() -> Self {
        Harness::new()
    }
}
