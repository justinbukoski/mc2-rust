// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Game audio for the MechCommander 2 Rust port.
//!
//! Built on the mixer in `mc2-platform` (`audio::Mixer`, `Sound`, `StreamingSource`): this crate is the
//! game layer on top of it.
//!
//! - [`snd`], [`bank`], [`ids`]: the sound table (`sound.snd`), the sample banks (`sound.pak`, `betty.pak`,
//!   `support.pak`, `noise.pak`) and the C++ sound ids (`mclib/sounds.h`).
//! - [`attenuation`]: distance falloff and stereo pan from the camera.
//! - [`system`]: effects, Betty, support officer, voice-overs, rain, music with cross-fades, volume groups
//!   (`mclib/soundsys.cpp`).
//! - [`radio`], [`game`]: pilot radio chatter with per-pilot voice sets, static, message priorities
//!   (`code/radio.cpp`, `code/gamesound.cpp`).
//! - [`voicepack`]: the optional directory of replacement pilot clips (off by default).
//! - [`engine`]: [`GameAudio`] and [`AudioEvent`], the API the app and sim drive.
//!
//! Nothing here opens an audio device, so everything runs headless: tests render the mixer into buffers.

pub mod attenuation;
pub mod bank;
pub mod data;
pub mod engine;
pub mod error;
pub mod game;
pub mod ids;
pub mod radio;
pub mod rng;
pub mod snd;
pub mod system;
#[doc(hidden)]
pub mod testutil;
pub mod voicepack;

pub use attenuation::{Listener, Spatial, Vec3};
pub use engine::{AudioEvent, GameAudio};
pub use error::{AudioError, Result};
pub use radio::{PilotId, RadioClock, RadioMessageType};
pub use rng::AudioRng;
pub use system::{Refused, Volumes};
pub use voicepack::VoicePack;
