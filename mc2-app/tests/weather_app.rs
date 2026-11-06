//! The weather through the app's own frame on the shipped rainy mission `mc2_10`. The options come through the
//! UI layer's accepted-settings handoff ([`UiLayer::take_applied_settings`]), the same hand the options screen
//! makes, so a test can change them between frames. Real data (`MC2_DATA`) and a headless GPU; every test
//! returns early without them (`MC2_SKIP_GPU_TESTS=1` skips a missing adapter). No window, no device.

use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::layers::{AppliedSettings, UiFrame, UiLayer};
use mc2_app::sim::{FrameInput, FrameOutput, NullSim, SimDriver};
use mc2_app::state::{GameState, StateRequest};
use mc2_audio::system::FIRE_CHANNEL;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// One game frame: 30 per second.
const DT: Duration = Duration::from_nanos(1_000_000_000 / 30);

fn root() -> Option<PathBuf> {
    std::env::var_os("MC2_DATA").map(PathBuf::from)
}

/// The accepted options the test hands the core, read once per frame as the options screen's are.
type Accepted = Rc<RefCell<Option<AppliedSettings>>>;

/// A UI layer that only hands over the accepted options: it asks for no state change and draws nothing.
struct OptionsHandoff(Accepted);

impl UiLayer for OptionsHandoff {
    fn update(&mut self, _frame: &mut UiFrame<'_>) -> Option<StateRequest> {
        None
    }

    fn take_applied_settings(&mut self) -> Option<AppliedSettings> {
        self.0.borrow_mut().take()
    }
}

fn accept(non_weapon_effects: bool) -> AppliedSettings {
    AppliedSettings {
        shadows: false,
        left_right_mouse_profile: false,
        use_non_weapon_effects: non_weapon_effects,
        terrain_detail: true,
        high_object_detail: true,
        local_shadows: true,
        pilot_videos: true,
        async_mouse: false,
        renderer: 0,
        hot_keys: Vec::new(),
        resolution: None,
        saved_resolution: None,
    }
}

/// A headless session on `mc2_10` with `factory`'s driver, or `None` without a GPU adapter.
fn session<D: SimDriver + 'static>(
    root: &std::path::Path,
    factory: mc2_app::game::DriverFactory<D>,
) -> Option<HeadlessRun<D>> {
    let mut opts = HeadlessOptions::new(root, "mc2_10");
    opts.size = (800, 600);
    opts.game.cursor = false;
    match HeadlessRun::with_driver(&opts, factory) {
        Ok(run) => Some(run),
        Err(e)
            if e.contains("adapter")
                && std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1") =>
        {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// The sim of the tests: it runs, and reports `scenarioResult` (a win) from its first step when `ends`.
struct RainySim {
    ends: bool,
}

impl SimDriver for RainySim {
    type Command = ();
    type Event = u8;
    type World = ();

    fn step(&mut self, _input: FrameInput<()>) -> FrameOutput<u8> {
        FrameOutput {
            events: Vec::new(),
            // `mis_PLAYER_WIN_BIG`
            result: if self.ends { 5 } else { 0 },
        }
    }

    fn world(&self) -> &() {
        &()
    }

    fn termination_result(&self) -> i32 {
        i32::from(self.ends)
    }
}

/// The options screen's accepted settings reach the weather: with the non-weapon effects off, `mc2_10` has no
/// rain (no drops, no rain loop, no darkening); accepting them on starts the storm and its loop.
#[test]
fn mc2_10_rain_follows_the_non_weapon_effects_option() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let Some(mut run) = session(&root, Box::new(|_| Ok(NullSim::default()))) else {
        return;
    };
    let accepted: Accepted = Rc::new(RefCell::new(Some(accept(false))));
    run.core.ui = Box::new(OptionsHandoff(accepted.clone()));

    // Boot, the mission load, then 3 seconds of play with the option off.
    for _ in 0..90 {
        run.core.frame(DT);
    }
    let rt = run.core.mission.as_ref().expect("mc2_10 is loaded");
    assert!(rt.weather.is_some(), "mc2_10 has a storm");
    assert_eq!(
        rt.weather.as_ref().unwrap().current_drops(),
        0,
        "off: the weather does not update, so it has no drops"
    );
    assert!(rt.weather_events.is_empty(), "off: no weather sounds");
    let audio = run.core.audio.as_ref().expect("the session has sound");
    assert!(
        !audio.game().sound.sys.is_channel_playing(FIRE_CHANNEL),
        "off: no rain loop"
    );

    // The options screen accepts the option on; the core applies it from the next frame.
    *accepted.borrow_mut() = Some(accept(true));
    for _ in 0..90 {
        run.core.frame(DT);
    }
    let rt = run.core.mission.as_ref().expect("mc2_10 is loaded");
    let drops = rt.weather.as_ref().unwrap().current_drops();
    assert!(drops > 0, "on: the storm falls ({drops} drops)");
    let audio = run.core.audio.as_ref().expect("the session has sound");
    assert!(
        audio.game().sound.sys.is_channel_playing(FIRE_CHANNEL),
        "on: the rain loop plays on the fire channel"
    );
}

/// The mission ends on the tick its first weather check queues the rain loop (`Weather::update` queues one on
/// its first frame). The loop must not restart after the mission's sound is purged: the rain channel stays
/// stopped on the Results screen.
#[test]
fn a_rain_loop_queued_on_the_ending_tick_does_not_play_on_results() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let Some(mut run) = session(&root, Box::new(|_| Ok(RainySim { ends: true }))) else {
        return;
    };
    let accepted: Accepted = Rc::new(RefCell::new(Some(accept(true))));
    run.core.ui = Box::new(OptionsHandoff(accepted));

    let mut on_results = false;
    for _ in 0..300 {
        run.core.frame(DT);
        if matches!(run.core.states.state(), GameState::Results(_)) {
            on_results = true;
            break;
        }
    }
    assert!(on_results, "the mission reaches Results");
    // The Results screen plays for a while: the rain loop would start on its first audio update.
    for _ in 0..30 {
        run.core.frame(DT);
    }
    let audio = run.core.audio.as_ref().expect("the session has sound");
    assert!(
        !audio.game().sound.sys.is_channel_playing(FIRE_CHANNEL),
        "the rain channel is stopped on Results"
    );
}
