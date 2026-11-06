# mc2-app

The `mc2` binary and its headless twin: window / offscreen render loop, game states, the fixed-step live simulation
(`LiveSim` over `mc2-sim`), the camera, order input and the camera visibility producer. Headless runs
(`mc2 --headless`, `HeadlessRun`) never open a window. `--hires-dir DIR` (or `$MC2_HIRES_DIR`) loads an
upscaled texture pack; default off. Missing or corrupt pack entries keep the original texture and log
a line naming the file. The production app does not read a `hires_dir = ...` settings file.

## Object visibility

Before each live `Sim::step`, `LiveSim` runs R1's production `select_shapes` on its bound mover, building, turret, gate and terrain
object appearances for that frame's camera. The resulting `inView` and retained LOD values are the sim's `Visibility` input; the
headless trace records `object-visibility=production`. `tests/sim_replay.rs` compares 60 mc2_01 turns with the C++ oracle: at most
32 selection boxes per class may differ on a turn during camera-edge or intro-frame selection, and the non-`iv` B/V digest fields must remain
identical. The app-to-direct-sim replay is bit-identical. Terrain-vertex visibility remains exact on turns 2..60; turn 1 has the
separate 16-vertex clip-edge bound in `docs/DIFFERENCES.md`.

## The mission HUD

With the live simulation the mission draws `mc2-ui`'s composed HUD over the scene (`src/hud.rs`, `src/hud_data.rs`): unit bar, info
window, tactical map, control panel, objectives, pause window and the mission message log, all built every frame from the simulation's
state. Selection, tactical-map and unit-bar camera moves and pause (which stops the simulation's objects, as `Mission::update` does while
paused) are applied by the app. Player orders go through one hook, `hud::apply_order_intents`, which reports them unavailable until
the orders bridge (`feat/rust-a3-orders`) is merged. What the HUD cannot show yet is listed in `hud_data::UNAVAILABLE` and in the
headless report's `sim_unavailable`. Evidence: `tests/hud_live.rs` and `docs/mc2_01_live_hud_1920x1080.png`.

## Scripted timeline

An input script's frames are mission simulation frames. Events, resizes and screenshots are delivered at their sim frame at any
render rate (`GameCore::scheduled_events`, `scheduled_resizes`, `capture_frames`).
