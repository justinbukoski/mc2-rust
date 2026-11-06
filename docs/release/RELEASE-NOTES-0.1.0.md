# MC2 Rust 0.1.0: release notes

Where a statement rests on a recorded run, the run is named.

## What this is

An all-Rust port of the 2001 game MechCommander 2, based on Microsoft's 2006 shared-source release of it (alariq's Linux port is the behaviour reference). Not affiliated with or endorsed by Microsoft, FASA, Catalyst Game Labs, Topps, or alariq. It needs your own copy of the
original game data (not included). See [README.md](README.md) and [INSTALL.md](INSTALL.md).

## What is in 0.1.0

The scope:

- The game as one Rust program (`mc2`): window and render loop through wgpu (Metal on macOS, Vulkan on Linux, DirectX 12 or Vulkan on Windows),
  the simulation, audio, the on-screen interface and the menus.
- Missions: selection and orders, hot keys, camera, the in-mission interface (control panel, unit bar, tactical map, objectives, messages),
  weapon and other effects, sound, radio and music, and the mission end with the results screen.
- Campaign: main menu, mission selection, briefings, logistics (mech bay, mech lab, purchase, pilot ready room), the results flow (salvage,
  pilot promotion and skill gain, carrying the force into the next mission), movies, saving and loading. The plan covers the 24 campaign
  missions and the 5 tutorials.
- The interface text is read from your own data: the string resource library (`mc2res.dll` in a retail install) in your data folder.
  Without it the game stops with "game data incomplete" before any screen. The retail `mc2res.dll` path is fixture-tested only (a
  synthetic test library, not a retail file).
- Upscaled art, built from your own data on first start and optional (`--hires-dir off`).
- Optional separate packs, off by default: a replacement pilot-voice pack (`--voice-dir`) and a prototype pack of replacement mech models
  (`--art-dir`). No such pack ships with or is published for this release.
- Packages for Linux x86_64 (`.tar.gz`), Windows x86_64 (`.zip`) and macOS arm64 (`.zip` with an ad-hoc signed app), built reproducibly from a
  committed revision of the development repository.
- A built-in check, `mc2 --selftest-play`, that plays `mc2_01` headless for 9,000 frames and then to its win by its objectives. Differences
  from the original that were decided on purpose are in [KNOWN-DIFFERENCES.md](KNOWN-DIFFERENCES.md).

## What was verified

On 2026-10-28 each package's `--selftest-play` passed with the player's own data: 9,000 frames, result 0, objective check passed for `mc2_01`
(6 objectives, 183 frames). Where each ran:

| Package | Where it ran |
|---|---|
| macOS arm64 | the build Mac |
| Linux x86_64 | a Fedora machine |
| Windows x86_64 | Linux with Proton GE-Proton11-9, **not** Windows |

The Linux log is transcribed from run notes, because no raw runtime log was kept.

## Known limitations

- The Windows package has not been run on Windows.
- The macOS app is ad-hoc signed and not notarised; Gatekeeper asks you to confirm the first start (see [INSTALL.md](INSTALL.md)).
- Multiplayer is not ported (D-22). The support VTOL model and the airstrike
  bomber's sound and shadow are not drawn (see [KNOWN-DIFFERENCES.md](KNOWN-DIFFERENCES.md)).
- No full 24-mission playthrough was recorded for this release, so this page makes no claim that every campaign mission can be played
  through to its end.
- The player guide (`packaging/README-PLAYERS.md`) lists further known gaps (some enemy pilot AI script functions, vehicle behaviour, air
  and sensor strikes).
- Release package verification was recorded for `mc2_01` only. Report anything else that breaks.
- Licence: Ms-SSLPL for source and binaries; see [README.md](README.md) and [LICENSE-MS-SSLPL.txt](../../LICENSE-MS-SSLPL.txt).

## Differences from the original

See [KNOWN-DIFFERENCES.md](KNOWN-DIFFERENCES.md).
