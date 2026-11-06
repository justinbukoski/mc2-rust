// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The weather's sounds on the shipped data: a storm's crack of thunder is one of the four `THUNDER` samples of
//! `sound.pak`, and the rain loop ids are the `RAIN` samples. Set `MC2_DATA` to the data root; without it the test
//! returns early.

use mc2_audio::bank::SoundBank;
use mc2_audio::data::VfsData;
use mc2_audio::ids::sfx;
use mc2_formats::vfs::Vfs;
use mc2_render::math::Vec3;
use mc2_render::weather::{Weather, WeatherConfig, WeatherEvent, RAIN_SOUNDS, THUNDER_SOUNDS};

#[test]
fn the_weather_sound_ids_are_the_shipped_thunder_and_rain_samples() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping the weather sound test");
        return;
    };
    let vfs = Vfs::from_system_cfg(root).expect("system.cfg mounts");
    let mut data = VfsData::new(vfs, false);
    let bank = SoundBank::load(&mut data, "sound").expect("sound.snd and sound.pak load");

    // The render crate keeps its own copy of the ids (it does not depend on the audio crate): they must be the
    // `mclib/sounds.h` numbers the audio crate names.
    assert_eq!(
        THUNDER_SOUNDS,
        [sfx::THUNDER1, sfx::THUNDER2, sfx::THUNDER3, sfx::THUNDER4]
    );
    assert_eq!(
        RAIN_SOUNDS,
        [sfx::RAIN1, sfx::RAIN2, sfx::RAIN3, sfx::RAIN4]
    );
    for id in THUNDER_SOUNDS.iter().chain(RAIN_SOUNDS.iter()) {
        assert!(
            bank.effect(*id as usize).is_some(),
            "sound id {id} has a sample in sound.pak"
        );
    }

    // The storm of `mc2_10` (`[Weather]`: StartingRainLevel 3, ChanceOfRain 100, BaseLighteningChance 0.04). Its
    // flashes queue thunder; every crack it reports is a THUNDER sample of the table.
    let storm = WeatherConfig::new(Some(500), Some(3.0), Some(100), Some(0.04));
    let mut weather = Weather::new(storm, 42);
    let eye = Vec3::new(0.0, 0.0, 100.0);
    let mut thunder = Vec::new();
    let mut rain_loops = Vec::new();
    for _ in 0..6000 {
        weather.update(0.5, eye, &|_, _| 0.0);
        for event in weather.take_events() {
            match event {
                WeatherEvent::Thunder(id) => thunder.push(id),
                WeatherEvent::RainLoop(id) => rain_loops.push(id),
            }
        }
        if thunder.len() >= 3 {
            break;
        }
    }
    assert!(
        thunder.len() >= 3,
        "a storm of 3000 s thunders: {thunder:?}"
    );
    assert!(
        thunder.iter().all(|id| THUNDER_SOUNDS.contains(id)),
        "{thunder:?}"
    );
    assert!(
        rain_loops
            .iter()
            .all(|id| *id == 0 || RAIN_SOUNDS.contains(id)),
        "rain loops are RAIN samples or none: {rain_loops:?}"
    );
    assert!(
        rain_loops.contains(&RAIN_SOUNDS[0]),
        "a storm above level 3 loops RAIN1"
    );
}
