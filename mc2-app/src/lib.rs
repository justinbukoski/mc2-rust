// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! mc2-app: the MechCommander 2 game application (the `mc2` binary).
//!
//! * [`game`]: the core both front ends share (input routing, state machine, mission runtime,
//!   frame composition);
//! * [`headless`]: the offscreen front end (`mc2 --headless`), scripted input, PNG screenshots;
//! * [`windowed`]: the `mc2_platform::window::App` front end on a wgpu surface;
//! * [`camera`], [`userinput`], [`hotkeys`]: camera control and the mission input half of
//!   `code/missiongui.cpp`, `code/gamecam.cpp`, `mclib/camera.cpp`, `mclib/userinput.cpp`;
//! * [`visibility`]: the camera visibility the simulation needs (`mc2-sim` DESIGN.md section 5);
//! * [`sim`]: the seam to `mc2_sim::Sim` and the fixed-step clock; [`live_sim`]: the driver around
//!   the live `mc2_sim::Sim`;
//! * [`hud`], [`hud_data`]: the in-mission HUD on live simulation state (`mc2-ui`'s composed `MissionHud`): the
//!   plain inputs read off the sim each frame, selection, camera and pause routing, the order hook;
//! * [`layers`]: the GUI, cursor and order hooks other lanes implement;
//! * [`orders`]: the bridge from merged MissionHud intents to `mc2-sim::PlayerCommand`;
//! * [`ui_host`]: the production GUI: the `mc2-ui` screens in the game's sequence over the campaign;
//! * [`pages`]: the campaign pages of the logistics sequence (purchase, mech lab, options) the
//!   host installs over its placeholders;
//! * [`state`]: Boot / MainMenu / Logistics / MissionLoading / InMission / Results;
//! * [`audio`]: the music and UI sounds of each screen and transition (`mc2-audio`).

pub mod audio;
pub mod bolt_geom;
pub mod bomber;
pub mod camera;
pub mod campaign_player;
pub mod campaign_save;
pub mod cli;
pub mod fx_sink;
pub mod game;
pub mod headless;
pub mod hires;
pub mod hotkeys;
pub mod hud;
pub mod hud_data;
pub mod layers;
pub mod live_sim;
pub mod locate;
pub mod mission_view;
pub mod movie_control;
mod movie_render;
pub mod orders;
pub mod outcome;
pub mod overlay;
pub mod p3_acceptance;
pub mod p3_driver;
pub mod pages;
pub mod pilot_video;
pub mod presentation;
pub mod production;
pub mod script;
pub mod sim;
pub mod sim_audio;
pub mod state;
pub mod ui_host;
pub mod userinput;
pub mod visibility;
pub mod waypoints;
pub mod windowed;
pub mod world_view;
