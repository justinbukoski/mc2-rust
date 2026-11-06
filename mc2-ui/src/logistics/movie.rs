// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The movie requests of the logistics pages (`MovieRequest`).
//!
//! The C++ screens drive their movie directly: `MissionSelectionScreen::handleMessage` restarts,
//! stops and pauses its `MC2Movie` (`code/missionselectionscreen.cpp:326-376`), and `Logistics`
//! plays the final video full screen (`code/logistics.cpp:144-157`, `Logistics::playFullScreenVideo`).
//! The port's pages do not own a movie player. They state the same actions as requests; the app's
//! movie host (`mc2-app/src/movie_control.rs`) plays them, and the pages carry on without waiting.

use mc2_campaign::Logistics;

/// A request to the app's movie host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MovieRequest {
    /// Load the screen's movie and start it from its first picture (`MC2Movie::init`, which starts
    /// playing). The name is the campaign's movie name (`getCurrentVideoFileName`).
    Open(String),
    /// Start again from the first picture, not paused (`MC2Movie::restart`, the Play button).
    Play,
    /// Hold the current picture (`true`) or resume it (`false`) (`MC2Movie::pause`, the Pause
    /// button). While held, no picture or sound is advanced.
    Pause(bool),
    /// Stop: no picture is shown and nothing plays until the next [`MovieRequest::Play`]
    /// (`MC2Movie::stop`, the Stop button).
    Stop,
    /// The player's skip (space, escape or the left mouse button, `Logistics::update`): the movie
    /// ends now, as if it had finished.
    Skip,
    /// Release the screen's movie (`MissionSelectionScreen::end`, `delete bMovie`).
    Close,
    /// The campaign's final video (`Logistics::playFullScreenVideo(getFinalVideo())`). When it ends,
    /// or is skipped, the game goes to the main menu (`missionBegin->beginSplash()`,
    /// `code/logistics.cpp:144-157`). Made by [`final_video_request`].
    FinalVideo(String),
}

/// The request for the campaign's final video once its last mission is won
/// (`code/logistics.cpp:144-152`). Call it after the mission is recorded as complete
/// (`setMissionCompleted`). `None` when the campaign goes on, when the mission is a single-mission
/// run (which has no final video), or when the group names no video: the C++ then goes straight to
/// the splash, since `playFullScreenVideo` returns at once for an empty name.
pub fn final_video_request(lg: &Logistics) -> Option<MovieRequest> {
    let info = lg.mission_info();
    if !info.campaign_over() || info.is_single_mission() {
        return None;
    }
    let name = info.final_video();
    (!name.is_empty()).then(|| MovieRequest::FinalVideo(name.to_string()))
}

/// The VFS path of a campaign movie (`moviePath` plus the name and `.mpg`,
/// `code/missionselectionscreen.cpp:237`). A name with a directory or extension keeps only its stem.
pub fn movie_vfs_path(name: &str) -> String {
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    format!("data/movies/{stem}.mpg")
}
