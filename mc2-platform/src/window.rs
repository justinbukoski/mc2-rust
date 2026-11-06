//! Window and event loop on winit.
//!
//! [`run`] creates a resizable window at the configured logical game resolution, feeds input into
//! an [`InputState`], and calls the [`App`] once per redraw. The window is exposed to the renderer
//! as an `Arc<winit::window::Window>`, which implements `raw_window_handle::HasWindowHandle` and
//! `HasDisplayHandle` (so a wgpu surface can be created from it directly).
//!
//! HiDPI: the window is created with a *logical* size equal to the game resolution; the renderer
//! should size its surface from [`Context::physical_size`] (native pixels). Pointer coordinates
//! are converted physical -> logical with the live scale factor before they meet the game
//! resolution (see [`crate::viewport`]).

use crate::input::{InputEvent, InputState, MouseButton, PIXELS_PER_WHEEL_NOTCH};
use crate::keys;
use crate::viewport::{Fit, Viewport};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

#[derive(Debug)]
pub enum PlatformError {
    EventLoop(String),
    Window(String),
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformError::EventLoop(e) => write!(f, "event loop error: {e}"),
            PlatformError::Window(e) => write!(f, "window error: {e}"),
        }
    }
}

impl std::error::Error for PlatformError {}

/// Window creation settings.
#[derive(Clone, Debug)]
pub struct WindowConfig {
    pub title: String,
    /// Logical game resolution (e.g. 800x600), independent of window and pixel size.
    pub game_width: u32,
    pub game_height: u32,
    pub fullscreen: bool,
    pub resizable: bool,
    pub fit: Fit,
    /// Hide the OS cursor over the window (the game draws its own).
    pub hide_os_cursor: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: crate::PRODUCT_NAME.into(),
            game_width: 800,
            game_height: 600,
            fullscreen: false,
            resizable: true,
            fit: Fit::Letterbox,
            hide_os_cursor: false,
        }
    }
}

/// The game side of the event loop.
pub trait App {
    /// Called once, right after the window exists. Create the renderer surface here.
    fn init(&mut self, _ctx: &mut Context<'_>) {}
    /// Called when the window's physical size or scale factor changed.
    fn resized(
        &mut self,
        _ctx: &mut Context<'_>,
        _physical: PhysicalSize<u32>,
        _scale_factor: f64,
    ) {
    }
    /// Called for every input event (pointer positions in window logical points), just before it
    /// is applied to [`Context::input`]. Apps that route events (a GUI consuming them before the
    /// game sees them) handle them here; apps that poll can ignore it.
    fn input_event(&mut self, _ctx: &mut Context<'_>, _event: &InputEvent) {}
    /// Called once per redraw. Poll [`Context::input`], update and render.
    fn frame(&mut self, ctx: &mut Context<'_>);
    /// `true` when the app wants each frame split in two redraws, [`App::update`] then [`App::render`], so
    /// the pointer events that arrive in between are delivered before the render (Async Mouse: the cursor
    /// follows the OS pointer between the update and the render). Otherwise [`App::frame`] does both.
    fn split_frames(&self) -> bool {
        false
    }
    /// The update half of a split frame.
    fn update(&mut self, _ctx: &mut Context<'_>) {}
    /// The render half of a split frame (after the events that arrived since the update).
    fn render(&mut self, _ctx: &mut Context<'_>) {}
    /// Return `false` to veto closing the window (e.g. to show a confirmation).
    fn close_requested(&mut self, _ctx: &mut Context<'_>) -> bool {
        true
    }
}

/// Access to the window and input from inside [`App`] callbacks.
pub struct Context<'a> {
    window: &'a Arc<Window>,
    pub input: &'a mut InputState,
    exit: &'a mut bool,
}

impl Context<'_> {
    /// The window; clone the `Arc` to give it to the renderer.
    pub fn window(&self) -> &Arc<Window> {
        self.window
    }

    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    /// Window inner size in native pixels (the surface size).
    pub fn physical_size(&self) -> PhysicalSize<u32> {
        self.window.inner_size()
    }

    /// Logical game resolution.
    pub fn game_size(&self) -> (u32, u32) {
        self.input.viewport().game_size()
    }

    /// Where the game screen sits inside the window, in physical pixels (for the renderer's
    /// viewport / scissor).
    pub fn content_rect_physical(&self) -> crate::viewport::Rect {
        self.input
            .viewport()
            .content_rect_physical(self.scale_factor())
    }

    pub fn request_exit(&mut self) {
        *self.exit = true;
    }

    pub fn is_fullscreen(&self) -> bool {
        self.window.fullscreen().is_some()
    }

    /// Borderless fullscreen on the current monitor (works on every platform).
    pub fn set_fullscreen(&mut self, on: bool) {
        self.window
            .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
    }

    pub fn toggle_fullscreen(&mut self) {
        let on = !self.is_fullscreen();
        self.set_fullscreen(on);
    }

    /// Fullscreen at the display mode of `w` x `h` (the C++ switched the display mode to the screen size).  Picks the
    /// monitor's mode with that size (deepest colour, then fastest refresh) and enters exclusive fullscreen; with no
    /// such mode the borderless fullscreen stays.  Returns whether the display mode now has the requested size.
    pub fn set_fullscreen_resolution(&mut self, w: u32, h: u32) -> bool {
        let Some(monitor) = self.window.current_monitor() else {
            return false;
        };
        let modes: Vec<_> = monitor.video_modes().collect();
        let keys: Vec<_> = modes
            .iter()
            .map(|m| {
                (
                    m.size().width,
                    m.size().height,
                    m.bit_depth(),
                    m.refresh_rate_millihertz(),
                )
            })
            .collect();
        match pick_video_mode(&keys, w, h) {
            Some(i) => {
                self.window
                    .set_fullscreen(Some(Fullscreen::Exclusive(modes[i].clone())));
                true
            }
            None => {
                self.window
                    .set_fullscreen(Some(Fullscreen::Borderless(None)));
                false
            }
        }
    }

    /// Change the logical game resolution; the window keeps its size.
    pub fn set_game_resolution(&mut self, w: u32, h: u32) {
        self.input.set_game_size(w, h);
    }

    /// Resize the window so the game screen is shown 1:1 in logical points at the current game
    /// resolution (no-op in fullscreen).
    pub fn fit_window_to_game(&mut self) {
        let (w, h) = self.game_size();
        let _ = self
            .window
            .request_inner_size(LogicalSize::new(w as f64, h as f64));
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.window.set_cursor_visible(visible);
    }

    /// Confine the cursor to the window (falls back to locking where confinement is unsupported).
    pub fn set_cursor_confined(&mut self, confined: bool) {
        let result = if confined {
            self.window
                .set_cursor_grab(CursorGrabMode::Confined)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Locked))
        } else {
            self.window.set_cursor_grab(CursorGrabMode::None)
        };
        if let Err(e) = result {
            log::debug!("cursor grab not supported: {e}");
        }
    }

    /// Move the OS cursor to the centre of a game pixel. Returns `false` if the platform refuses
    /// (e.g. Wayland); the input state is updated either way.
    pub fn warp_cursor(&mut self, game_x: i32, game_y: i32) -> bool {
        let (wx, wy) = self.input.warp_mouse(game_x, game_y);
        self.window
            .set_cursor_position(LogicalPosition::new(wx, wy))
            .is_ok()
    }

    pub fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }
}

// ---- winit event translation (pure functions, unit-tested) ---------------------------------

/// Pointer position from winit (physical pixels) to window logical points.
pub fn translate_cursor(position: PhysicalPosition<f64>, scale_factor: f64) -> InputEvent {
    let logical: LogicalPosition<f64> = position.to_logical(scale_factor);
    InputEvent::CursorMoved {
        x: logical.x,
        y: logical.y,
    }
}

pub fn translate_button(
    button: winit::event::MouseButton,
    state: ElementState,
) -> Option<InputEvent> {
    let button = match button {
        winit::event::MouseButton::Left => MouseButton::Left,
        winit::event::MouseButton::Middle => MouseButton::Middle,
        winit::event::MouseButton::Right => MouseButton::Right,
        winit::event::MouseButton::Back => MouseButton::X1,
        winit::event::MouseButton::Forward => MouseButton::X2,
        winit::event::MouseButton::Other(_) => return None,
    };
    Some(InputEvent::Button {
        button,
        pressed: state == ElementState::Pressed,
    })
}

/// Wheel delta -> notches. winit reports pixel deltas in physical pixels.
pub fn translate_wheel(delta: MouseScrollDelta, scale_factor: f64) -> InputEvent {
    let (x, y) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (x, y),
        MouseScrollDelta::PixelDelta(p) => {
            let l: LogicalPosition<f64> = p.to_logical(scale_factor);
            (
                (l.x / PIXELS_PER_WHEEL_NOTCH) as f32,
                (l.y / PIXELS_PER_WHEEL_NOTCH) as f32,
            )
        }
    };
    InputEvent::Wheel { x, y }
}

/// A keyboard transition -> one event per game key it drives.
pub fn translate_key(
    physical: PhysicalKey,
    state: ElementState,
    repeat: bool,
    text: Option<&str>,
) -> Vec<InputEvent> {
    let PhysicalKey::Code(code) = physical else {
        return Vec::new();
    };
    let pressed = state == ElementState::Pressed;
    keys::map_physical_key(code)
        .into_iter()
        .enumerate()
        .map(|(i, m)| InputEvent::Key {
            key: m.key,
            phys_id: m.phys_id,
            pressed,
            repeat,
            // Only the primary mapping carries the text so it is delivered once.
            text: if i == 0 {
                text.map(str::to_owned)
            } else {
                None
            },
        })
        .collect()
}

fn logical_size(window: &Window) -> (f64, f64) {
    let s = window.inner_size();
    let f = window.scale_factor();
    (
        crate::viewport::physical_to_logical(s.width as f64, f),
        crate::viewport::physical_to_logical(s.height as f64, f),
    )
}

/// What one redraw does ([`FrameSchedule`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramePhase {
    /// [`App::frame`]: update and render together.
    Whole,
    /// [`App::update`]; the render follows on the next redraw.
    Update,
    /// [`App::render`].
    Render,
}

/// Orders the redraws: a split frame's render comes one redraw after its update, so the event loop delivers
/// the pointer events that arrived meanwhile in between.
#[derive(Default, Debug)]
pub struct FrameSchedule {
    render_next: bool,
}

impl FrameSchedule {
    /// The phase of the next redraw; `split` is [`App::split_frames`].
    pub fn next(&mut self, split: bool) -> FramePhase {
        if std::mem::take(&mut self.render_next) {
            FramePhase::Render
        } else if split {
            self.render_next = true;
            FramePhase::Update
        } else {
            FramePhase::Whole
        }
    }
}

struct Runner<A: App> {
    config: WindowConfig,
    app: A,
    window: Option<Arc<Window>>,
    input: Option<InputState>,
    exit: bool,
    error: Option<PlatformError>,
    initialised: bool,
    schedule: FrameSchedule,
}

impl<A: App> Runner<A> {
    fn with_ctx<R>(&mut self, f: impl FnOnce(&mut A, &mut Context<'_>) -> R) -> Option<R> {
        let window = self.window.as_ref()?;
        let input = self.input.as_mut()?;
        let mut ctx = Context {
            window,
            input,
            exit: &mut self.exit,
        };
        Some(f(&mut self.app, &mut ctx))
    }

    /// Hands one event to the app ([`App::input_event`]), then applies it to the input state.
    fn dispatch(&mut self, event: InputEvent) {
        self.with_ctx(|app, ctx| {
            app.input_event(ctx, &event);
            ctx.input.handle(event);
        });
    }
}

impl<A: App> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(
                self.config.game_width as f64,
                self.config.game_height as f64,
            ))
            .with_min_inner_size(LogicalSize::new(320.0, 240.0))
            .with_resizable(self.config.resizable)
            .with_fullscreen(
                self.config
                    .fullscreen
                    .then_some(Fullscreen::Borderless(None)),
            );
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(PlatformError::Window(e.to_string()));
                event_loop.exit();
                return;
            }
        };
        if self.config.hide_os_cursor {
            window.set_cursor_visible(false);
        }
        let (lw, lh) = logical_size(&window);
        let viewport = Viewport::new(
            self.config.game_width,
            self.config.game_height,
            lw,
            lh,
            self.config.fit,
        );
        self.input = Some(InputState::new(viewport));
        self.window = Some(window);
        if !self.initialised {
            self.initialised = true;
            self.with_ctx(|app, ctx| app.init(ctx));
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let scale = window.scale_factor();
        match event {
            WindowEvent::CloseRequested => {
                let allow = self
                    .with_ctx(|app, ctx| app.close_requested(ctx))
                    .unwrap_or(true);
                if allow {
                    self.exit = true;
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let (lw, lh) = logical_size(&window);
                self.dispatch(InputEvent::Resized {
                    logical_w: lw,
                    logical_h: lh,
                });
                let size = window.inner_size();
                self.with_ctx(|app, ctx| app.resized(ctx, size, scale));
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.dispatch(translate_cursor(position, scale));
            }
            WindowEvent::CursorEntered { .. } => self.dispatch(InputEvent::CursorEntered),
            WindowEvent::CursorLeft { .. } => self.dispatch(InputEvent::CursorLeft),
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(ev) = translate_button(button, state) {
                    self.dispatch(ev);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.dispatch(translate_wheel(delta, scale));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                for ev in translate_key(
                    event.physical_key,
                    event.state,
                    event.repeat,
                    event.text.as_deref(),
                ) {
                    self.dispatch(ev);
                }
            }
            WindowEvent::Focused(false) => self.dispatch(InputEvent::FocusLost),
            WindowEvent::RedrawRequested => {
                let split = self.app.split_frames();
                let phase = self.schedule.next(split);
                match phase {
                    FramePhase::Whole => {
                        self.with_ctx(|app, ctx| app.frame(ctx));
                    }
                    FramePhase::Update => {
                        self.with_ctx(|app, ctx| app.update(ctx));
                    }
                    FramePhase::Render => {
                        self.with_ctx(|app, ctx| app.render(ctx));
                    }
                }
                // the update consumed the frame's edges; those that arrive before the render are the next update's
                if phase != FramePhase::Render {
                    if let Some(input) = self.input.as_mut() {
                        input.end_frame();
                    }
                }
                window.request_redraw();
            }
            _ => {}
        }
        if self.exit {
            event_loop.exit();
        }
    }
}

/// Create the window and run the game loop until the app exits or the window is closed.
///
/// Must be called from the main thread (a winit requirement on macOS).
pub fn run<A: App>(config: WindowConfig, app: A) -> Result<(), PlatformError> {
    let event_loop = EventLoop::new().map_err(|e| PlatformError::EventLoop(e.to_string()))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut runner = Runner {
        config,
        app,
        window: None,
        input: None,
        exit: false,
        error: None,
        initialised: false,
        schedule: FrameSchedule::default(),
    };
    event_loop
        .run_app(&mut runner)
        .map_err(|e| PlatformError::EventLoop(e.to_string()))?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

    /// The renderer creates its wgpu surface from `Arc<Window>`: it must expose both handles.
    #[test]
    fn window_arc_exposes_raw_handles() {
        fn assert_handles<
            T: winit::raw_window_handle::HasWindowHandle + winit::raw_window_handle::HasDisplayHandle,
        >() {
        }
        assert_handles::<Arc<Window>>();
    }

    #[test]
    fn a_split_frame_renders_one_redraw_after_its_update() {
        let mut s = FrameSchedule::default();
        // whole frames until a split one is wanted; the render always follows its update
        assert_eq!(s.next(false), FramePhase::Whole);
        assert_eq!(s.next(true), FramePhase::Update);
        assert_eq!(s.next(true), FramePhase::Render);
        assert_eq!(s.next(true), FramePhase::Update);
        // turned off between the update and the render: the render still happens
        assert_eq!(s.next(false), FramePhase::Render);
        assert_eq!(s.next(false), FramePhase::Whole);
    }

    #[test]
    fn cursor_physical_to_logical() {
        let ev = translate_cursor(PhysicalPosition::new(1600.0, 1200.0), 2.0);
        assert_eq!(ev, InputEvent::CursorMoved { x: 800.0, y: 600.0 });
        let ev = translate_cursor(PhysicalPosition::new(300.0, 150.0), 1.5);
        assert_eq!(ev, InputEvent::CursorMoved { x: 200.0, y: 100.0 });
    }

    #[test]
    fn cursor_end_to_end_2x() {
        let scale = 2.0;
        let vp = Viewport::from_physical(800, 600, 1600, 1200, scale, Fit::Stretch);
        let mut input = InputState::new(vp);
        input.handle(translate_cursor(PhysicalPosition::new(1599.0, 0.0), scale));
        assert_eq!(input.mouse_position(), (799.0, 0.0));
        input.handle(translate_cursor(PhysicalPosition::new(800.0, 600.0), scale));
        assert_eq!(input.mouse_position(), (400.0, 300.0));
    }

    #[test]
    fn buttons() {
        assert_eq!(
            translate_button(winit::event::MouseButton::Right, ElementState::Pressed),
            Some(InputEvent::Button {
                button: MouseButton::Right,
                pressed: true
            })
        );
        assert_eq!(
            translate_button(winit::event::MouseButton::Back, ElementState::Released),
            Some(InputEvent::Button {
                button: MouseButton::X1,
                pressed: false
            })
        );
        assert_eq!(
            translate_button(winit::event::MouseButton::Other(9), ElementState::Pressed),
            None
        );
    }

    #[test]
    fn wheel_lines_and_pixels() {
        assert_eq!(
            translate_wheel(MouseScrollDelta::LineDelta(0.0, 2.0), 1.0),
            InputEvent::Wheel { x: 0.0, y: 2.0 }
        );
        // 80 physical px at 2x = 40 logical px = one notch.
        assert_eq!(
            translate_wheel(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 80.0)),
                2.0
            ),
            InputEvent::Wheel { x: 0.0, y: 1.0 }
        );
    }

    #[test]
    fn key_translation() {
        let evs = translate_key(
            PhysicalKey::Code(KeyCode::KeyA),
            ElementState::Pressed,
            false,
            Some("a"),
        );
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            InputEvent::Key {
                key, pressed, text, ..
            } => {
                assert_eq!(*key, keys::KEY_A);
                assert!(*pressed);
                assert_eq!(text.as_deref(), Some("a"));
            }
            e => panic!("unexpected {e:?}"),
        }
        let evs = translate_key(
            PhysicalKey::Code(KeyCode::AltLeft),
            ElementState::Pressed,
            false,
            None,
        );
        assert_eq!(evs.len(), 2);
        assert!(translate_key(
            PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
            ElementState::Pressed,
            false,
            None
        )
        .is_empty());
    }

    #[test]
    fn default_config_is_800x600_letterboxed() {
        let c = WindowConfig::default();
        assert_eq!((c.game_width, c.game_height), (800, 600));
        assert_eq!(c.fit, Fit::Letterbox);
        assert!(c.resizable);
    }
}

/// The index of the display mode `(width, height, bit depth, refresh mHz)` of exactly `w` x `h` with the deepest
/// colour, then the fastest refresh; `None` when the monitor has no such size.
pub fn pick_video_mode(modes: &[(u32, u32, u16, u32)], w: u32, h: u32) -> Option<usize> {
    modes
        .iter()
        .enumerate()
        .filter(|(_, m)| m.0 == w && m.1 == h)
        .max_by_key(|(_, m)| (m.2, m.3))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod video_mode_tests {
    use super::pick_video_mode;

    #[test]
    fn the_requested_size_with_deepest_colour_and_fastest_refresh_is_picked() {
        let modes = [
            (800, 600, 16, 60_000),
            (1024, 768, 32, 60_000),
            (1024, 768, 32, 144_000),
            (1024, 768, 24, 240_000),
        ];
        assert_eq!(pick_video_mode(&modes, 1024, 768), Some(2));
        assert_eq!(pick_video_mode(&modes, 800, 600), Some(0));
        assert_eq!(pick_video_mode(&modes, 1280, 1024), None);
    }
}

#[cfg(test)]
mod product_name_tests {
    #[test]
    fn default_window_title_is_product_name() {
        assert_eq!(super::WindowConfig::default().title, crate::PRODUCT_NAME);
    }
}
