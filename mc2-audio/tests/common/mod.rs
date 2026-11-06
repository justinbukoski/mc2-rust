#![allow(dead_code)]

use mc2_audio::testutil::{dc, FixtureBuilder};
use mc2_audio::{GameAudio, Volumes};
use mc2_platform::audio::{Mixer, SharedMixer};

pub const RATE: u32 = 8000;
pub const DT: f32 = 0.05;

/// Master 255, everything else 128 (gain 0.5; master 255/256).
pub fn volumes() -> Volumes {
    Volumes {
        digital_master: 255,
        music: 128,
        sfx: 128,
        radio: 128,
        betty: 128,
    }
}

pub const MASTER: f32 = 255.0 / 256.0;

pub fn mixer() -> SharedMixer {
    SharedMixer::new(Mixer::for_game(RATE))
}

/// A fixture with `n` filler bites (empty packets, volume 1) so real ids (e.g. SENSOR2 = 86) fit.
pub fn builder_with_bites(n: usize) -> FixtureBuilder {
    let mut b = FixtureBuilder::new(RATE);
    for _ in 0..n {
        b = b.bite(None, 1.0);
    }
    b
}

pub fn game(b: FixtureBuilder) -> GameAudio {
    GameAudio::load(Box::new(b.build()), mixer(), volumes()).expect("fixture loads")
}

/// Run `secs` seconds in steps of `DT`, returning the rendered stereo buffer.
pub fn run(a: &mut GameAudio, secs: f32) -> Vec<f32> {
    let mut out = Vec::new();
    let steps = (secs / DT).round() as usize;
    for _ in 0..steps {
        a.render_offline(DT, &mut out);
    }
    out
}

pub fn left(buf: &[f32]) -> Vec<f32> {
    buf.chunks(2).map(|f| f[0]).collect()
}

pub fn right(buf: &[f32]) -> Vec<f32> {
    buf.chunks(2).map(|f| f[1]).collect()
}

pub fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0, |m, x| x.abs().max(m))
}

pub fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

pub fn level(v: f32) -> Vec<i16> {
    dc(RATE, 1.0, v)
}
