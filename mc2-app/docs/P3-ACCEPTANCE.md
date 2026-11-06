# P3 acceptance: mc2_01 player win

This is the P3 acceptance replay, not a simulation fixture. A player driver
([`../src/p3_driver.rs`](../src/p3_driver.rs)) watches only what a player can see on the HUD
(objectives panel, unit bar, tactical-map blips, selection boxes) and keeps producing normal keyboard
and mouse input through `mc2-app`'s headless production front end until the objectives panel shows
the job done. It never invokes a simulation order API and never reads hidden simulation state. The
checked-in [`../tests/scripts/p3_mc2_01_win.input`](../tests/scripts/p3_mc2_01_win.input) is the
older fixed one-pass schedule, kept as the `parse_player_script` contract fixture and as the plan
described below; it is not what the acceptance run plays. With `feat/rust-sim-movement` merged the
driver wins `mc2_01` (frame 13,191) and the replay test is no longer ignored.

## Source facts

`mc2_01.fit` lives at `data/missions/mc2_01.fit` in the shipped `mission.fst`;
the line references below are from its decompressed FIT text.

| Fact | Evidence |
| --- | --- |
| The player starts with three PlayerPart mechs: Werewolf at `(2496,-1941)`, and Bushwackers at `(2368,-1941)` and `(2539,-2069)`. | `data/missions/mc2_01.fit:94-119`, `:127-152`, `:160-185` |
| Primary 1 is investigate the airfield by moving any unit to `(3136,-789)`, radius 256. | `data/missions/mc2_01.fit:1654-1691` |
| Primary 2 is destroy the hangar at `(3221,-277)`; it activates after primary 1's flag. | `data/missions/mc2_01.fit:1699-1744` |
| Primary 3 is eight specific enemy movers around `(2837..3563,1984..2197)`. | `data/missions/mc2_01.fit:1746-1864` |
| Primary 4 is move any unit to north island `(832,2752)`, radius 256. | `data/missions/mc2_01.fit:1866-1909` |
| Primary 5 is four specific enemy movers around `(-1216..-1003,3051..3093)`. | `data/missions/mc2_01.fit:1911-1991` |
| Primary 6 requires all surviving mechs at extraction `(-4117,513)`, radius 256, and requires all previous primaries. | `data/missions/mc2_01.fit:1993-2030` |
| The shipped ABL's main return value is `ScenarioResult`; win/loss music reacts to the engine's `getMissionWon` / `getMissionLost`, rather than a scripted cheat result. | `data/missions/mc2_01.abl:283-303`, `:751` |
| The C++ mission turns a successful primary-objective status into `mis_PLAYER_WIN_BIG`; it turns a failed status or an all-disabled/destroyed player force into loss. | `code/mission.cpp:543-563`, `:627-661` |
| The ABL begins with its tutorial/camera sequence before it waits for objective 0, so the replay must be treated as a mission-input replay rather than a direct objective fixture. | `data/missions/mc2_01.abl:489-593` |

## Player plan

The HUD defaults to the Left/Right (AOE) mouse profile (`useLeftRightMouseProfile = true`). In that profile `MissionInterfaceManager` makes `commandClicked` from `rightMouseReleased`, while left release is selection (`code/missiongui.cpp:760-761`, `:847-853`); `updateAOEStyle` sends a command on right release (`:1482-1486`, `:1514-1585`). Thus a tactical-map right click sends `doMove`, an enemy under a right-click release reaches `doAttack`, and a left tactical-map click recentres the camera (`code/gametacmap.cpp:83-110`). The map rectangle at 800x600 is `(4,435)..(165,596)` (`rust/mc2-ui/examples/render_tacmap.rs:4-9`, `:34-45`).

1. Wait 90 seconds for the ABL intro sequence. Left-click `(116,540)` to centre the tactical-map camera on the three tightly grouped player mechs, then press `E` to Select Visible; this selects the visible home-commander lance (`code/missiongui.cpp:2077-2100`).
2. Right-click `(124,525)` in the tactical map, the map-cell equivalent of airfield `(3136,-789)`.
3. Left-click `(124,519)` to centre the hangar, press `D`, then right-click-release world-screen centre. `D` selects optimal Default Attack range (`code/missiongui.cpp:1941-1946`); the right release over an enemy reaches `doAttack` (`:1514-1585`), which sends an attack-to-destroy order to every selected home-commander mover (`:1801-1901`).
4. Repeat camera-centre / `D` / world-centre right-click-release on the eight patrol points `(125,490)`, `(127,490)`, `(120,488)` twice, `(121,488)`, `(126,490)`, `(129,490)`, and `(121,487)`. The duplicated cell is deliberate: the FIT has two distinct targets in that tactical-map cell.
5. Right-click north island `(95,481)`, then attack the four northern bandits at `(71,476)` twice, `(69,476)`, and `(68,476)` in the same sequence. The second duplicate likewise advances to the next target in that exact map cell after the first is destroyed.
6. Right-click extraction `(32,508)` only after the attack phases. This orders the still-selected full lance into the exact all-surviving-mechs objective area.

The screen-centre clicks are intentional real player clicks: each preceding tactical-map left click uses the game UI to place the camera on that target. They are not object IDs or injected commands.

## Harness state and budget

`tests/p3_acceptance.rs` always runs the script parser/mapping test. It checks
that `E` and `D` resolve through the production app's `hotkeys::COMMANDS`, that
every map coordinate converts through the merged `mc2_ui::TacMap` using
mc2_01's real 100-vertex bounds and `buttonlayout800.fit` rectangle, and that
each centre-screen attack is a complete right-click-release following Select
Visible, a map focus, and Default Attack. The
always-on results-state regression test takes the real state machine through
`StartMission` → `MissionReady` → `EndMission(mis_PLAYER_WIN_BIG)` and verifies
that its production report spelling is lowercase `results`
(`rust/mc2-app/src/state.rs:30-41`). This is the same value the eventual
headless win assertion uses.

The production headless session (`HeadlessRun::production`, the one behind `mc2 --headless`) now
constructs the live `mc2_sim::Sim`, registers the effects and battle sound sinks, and a finished
mission drives the C++ results flow (the HUD's in-mission results tell the sim `resultsDone`, then
`UiHost` shows the salvage / pilot review screens after a win). `tests/p3_integration.rs` pins that
path with forced wins and losses.

The end-to-end replay test runs by default (movement, attack decisions, firing and visual LOS have merged). It launches `mc2_01` at 800x600 for at most 18,000 simulation
frames (600 seconds at 30 Hz), then must assert `mis_PLAYER_WIN_BIG`, `GameState::Results`, a drawn
Results screen, and the frame budget.

Run the no-window progress probe after merges:

```sh
cd rust
cargo run -p mc2-app --bin p3_acceptance
```

It prints the parsed action count, every executed player step, and the first
unavailable production seam or replay failure. When the production seams are
wired, it is the orchestrator's step-by-step execution report.


## Outcome-driven player (2026-10-20, `feat/rust-p3-win`)

The fixed one-pass `p3_mc2_01_win.input` cannot say "repeat until destroyed", so the replay is now
played by `mc2_app::p3_driver::PlayerDriver` (test-only seam; `p3_acceptance::replay`). It reads only what a
player sees (the objectives panel, tactical-map blips, the unit bar, the on-screen selection boxes) and
emits only physical input (tac-map click to centre the camera, E, D, right-click on the map or on an
enemy on screen). `parse_player_script` still validates the one-pass script. Set `MC2_P3_TRACE=1` for
per-frame traces: emitted `PlayerCommand`s with the selection and the ground point under the pointer
(`game.rs`), and the driver's own decisions.

Findings with the production headless path (GPU adapter available; ~6 min per 18,000-frame run):

* The one-pass script's "attacks" produced `MoveToPoint`, not `AttackObject`: at the fixed screen centre
  no enemy is on the pick list. The driver instead clicks the target it sees on screen.
* With the driver and the merged movement port the lance reaches the retreating patrol; objectives 1-6 turn
  Successful at frames 3393, 3609, 6355, 6867, 8967 and 12757 and the mission is won at frame 13,191 (budget 18,000).
* Watch 13 (one of the four "Locate and Destroy Bandits" movers) ends *disabled*, not destroyed; the objective
  counts it as done. The report says so on its own line.

Recorded `p3_acceptance` binary output (headless, release, `MC2_P3_TRACE=1`, exit 0):

```text
P3 frame 2700: primary objectives [Undetermined, Undetermined, Undetermined, Undetermined, Undetermined, Undetermined]
P3 frame 3393: primary objectives [Successful, Undetermined, Undetermined, Undetermined, Undetermined, Undetermined]
P3 frame 3604: target watch 30 destroyed
P3 frame 3609: primary objectives [Successful, Successful, Undetermined, Undetermined, Undetermined, Undetermined]
P3 frame 4332: target watch 12 destroyed
P3 frame 4553: target watch 28 destroyed
P3 frame 4778: target watch 27 destroyed
P3 frame 4891: target watch 29 destroyed
P3 frame 5402: target watch 10 destroyed
P3 frame 5711: target watch 11 destroyed
P3 frame 6000: target watch 5 destroyed
P3 frame 6294: target watch 4 destroyed
P3 frame 6355: primary objectives [Successful, Successful, Successful, Undetermined, Undetermined, Undetermined]
P3 frame 6867: primary objectives [Successful, Successful, Successful, Successful, Undetermined, Undetermined]
P3 frame 8496: target watch 6 destroyed
P3 frame 8946: target watch 7 destroyed
P3 frame 8966: target watch 8 destroyed
P3 frame 8967: primary objectives [Successful, Successful, Successful, Successful, Successful, Undetermined]
P3 frame 12757: primary objectives [Successful, Successful, Successful, Successful, Successful, Successful]
P3 target watch 13 never reported destroyed: present at frame 13191, destroyed false, disabled true
P3 final: 12 of 13 targets destroyed (not destroyed: [13]), mission won at sim frame 13191
P3 acceptance complete: mission result = win, Results screen reached, 13191 simulation frames
```
