//! Offline mix of a scripted sequence on the shipped data: music underneath, gunfire, a distant
//! explosion, a Betty line. Rendered to a buffer (no audio device), then checked for timing and level.
//!
//! The script is run once for the full mix and once per source ("stem"). The full mix must equal the
//! clamped sum of the stems sample for sample, each stem must start at its scripted time, and each stem's
//! level must match the C++ volume formulas applied to the decoded sample.
//!
//! Skips (passes) when `MC2_DATA` is not set.

use mc2_audio::attenuation::{Listener, Vec3};
use mc2_audio::data::VfsData;
use mc2_audio::engine::AudioEvent;
use mc2_audio::ids::{betty, music, sfx};
use mc2_audio::rng::SeededRng;
use mc2_audio::{GameAudio, Volumes};
use mc2_formats::vfs::Vfs;
use mc2_platform::audio::{Mixer, SharedMixer, Sound};
use std::sync::Arc;

const RATE: u32 = 44100;
const DT: f32 = 0.05;
const SECONDS: f32 = 9.0;

/// `options.cfg` as shipped.
const VOLUMES: Volumes = Volumes {
    digital_master: 192,
    music: 128,
    radio: 128,
    sfx: 160,
    betty: 255,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stem {
    Music,
    Guns,
    Explosion,
    Betty,
    TooFar,
}

const ALL: [Stem; 5] = [
    Stem::Music,
    Stem::Guns,
    Stem::Explosion,
    Stem::Betty,
    Stem::TooFar,
];

// Script (seconds): music from 0; three AC2 shots at 1.0 / 1.4 / 1.8 at 200 units; an explosion at 3.0
// 1000 units away; Betty at 4.0; a shot at 1600 units (out of range) at 4.5.
const MUSIC_AT: f32 = 0.0;
const GUNS_AT: [f32; 3] = [1.0, 1.4, 1.8];
const GUN_POS: Vec3 = Vec3::new(200.0, 0.0, 0.0);
const EXPLOSION_AT: f32 = 3.0;
const EXPLOSION_POS: Vec3 = Vec3::new(0.0, 1000.0, 0.0);
const BETTY_AT: f32 = 4.0;
const FAR_AT: f32 = 4.5;
const FAR_POS: Vec3 = Vec3::new(-1600.0, 0.0, 0.0);

fn load() -> Option<GameAudio> {
    let root = std::env::var_os("MC2_DATA")?;
    let vfs = Vfs::from_system_cfg(root).ok()?;
    // Whole-file decode so the render does not depend on a worker thread keeping up.
    let data = VfsData::new(vfs, false);
    GameAudio::load(
        Box::new(data),
        SharedMixer::new(Mixer::for_game(RATE)),
        VOLUMES,
    )
    .ok()
}

fn render(stems: &[Stem]) -> Vec<f32> {
    let mut a = load().unwrap();
    a.set_listener(Some(Listener {
        position: Vec3::new(0.0, 0.0, 0.0),
        rotation_y: 0.0,
    }));
    let mut rng = SeededRng::new(1);
    let mut out = Vec::new();
    let steps = (SECONDS / DT).round() as usize;
    // (stem, time, event), fired in the first update at or after its time.
    let mut script: Vec<(Stem, f32, AudioEvent)> = vec![(
        Stem::Music,
        MUSIC_AT,
        AudioEvent::Music(music::COMBAT_TUNE_0 as i64),
    )];
    for at in GUNS_AT {
        script.push((
            Stem::Guns,
            at,
            AudioEvent::Sound {
                id: sfx::AC2_FIRE,
                at: Some(GUN_POS),
                allow_dupes: true,
            },
        ));
    }
    script.push((
        Stem::Explosion,
        EXPLOSION_AT,
        AudioEvent::Sound {
            id: sfx::BIGDEATH,
            at: Some(EXPLOSION_POS),
            allow_dupes: true,
        },
    ));
    script.push((
        Stem::Betty,
        BETTY_AT,
        AudioEvent::Betty(betty::BETTY_MISSION_WON),
    ));
    script.push((
        Stem::TooFar,
        FAR_AT,
        AudioEvent::Sound {
            id: sfx::AC2_FIRE,
            at: Some(FAR_POS),
            allow_dupes: true,
        },
    ));
    script.retain(|(s, _, _)| stems.contains(s));
    let mut next = 0;
    for k in 0..steps {
        let t = k as f32 * DT;
        while next < script.len() && t + 1e-4 >= script[next].1 {
            a.handle(&script[next].2, &mut rng);
            next += 1;
        }
        a.render_offline(DT, &mut out);
    }
    out
}

/// Decode a sound to the first channel's f32 samples (the mixer's own source path).
fn samples(s: &Arc<Sound>) -> Vec<f32> {
    let mut src = s.source();
    let ch = s.channels() as usize;
    let mut out = Vec::new();
    let mut buf = vec![0.0f32; 4096 * ch];
    loop {
        let n = src.read(&mut buf);
        if n == 0 {
            break;
        }
        for f in 0..n {
            out.push(buf[f * ch]);
        }
    }
    out
}

fn first_nonzero(v: &[f32]) -> Option<usize> {
    v.iter().position(|x| x.abs() > 1e-7)
}

fn last_nonzero(v: &[f32]) -> Option<usize> {
    v.iter().rposition(|x| x.abs() > 1e-7)
}

fn left(buf: &[f32]) -> Vec<f32> {
    buf.chunks(2).map(|f| f[0]).collect()
}
fn right(buf: &[f32]) -> Vec<f32> {
    buf.chunks(2).map(|f| f[1]).collect()
}
fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0, |m, x| x.abs().max(m))
}
fn frame(t: f32) -> usize {
    (t * RATE as f32).round() as usize
}

fn music_sound() -> Arc<Sound> {
    let root = std::env::var_os("MC2_DATA").unwrap();
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    let bytes = vfs.read("data/sound/music14.wav").unwrap().unwrap();
    Sound::from_wav(&bytes).unwrap()
}

#[test]
fn scripted_mix_has_the_right_timing_and_levels() {
    let Some(a) = load() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let master = 192.0 / 256.0;
    let sfx_gain = 160.0 / 256.0;
    let bank = a.sound.sys.bank().clone();
    let table = a.sound.sys.table().clone();
    drop(a);

    let full = render(&ALL);
    let stems: Vec<(Stem, Vec<f32>)> = ALL.iter().map(|s| (*s, render(&[*s]))).collect();
    let n = (SECONDS * RATE as f32) as usize;
    assert_eq!(full.len(), n * 2, "{SECONDS} s of stereo");
    for (s, b) in &stems {
        assert_eq!(b.len(), full.len(), "{s:?}");
    }

    // 1. Linearity: the mix is the (clamped) sum of the stems.
    let mut worst = 0.0f32;
    for i in 0..full.len() {
        let sum: f32 = stems
            .iter()
            .map(|(_, b)| b[i])
            .sum::<f32>()
            .clamp(-1.0, 1.0);
        worst = worst.max((full[i] - sum).abs());
    }
    assert!(worst < 1e-5, "mix differs from the stem sum by {worst}");

    let stem = |s: Stem| &stems.iter().find(|(x, _)| *x == s).unwrap().1;

    // 2. Gunfire: three AC2 shots 0.4 s apart at 200 units. Distance volume (1500-200)/1500; the camera
    //    minus the source has x = -200 so pan = -200/750 (left side full, right side reduced).
    let ac2 = bank.effect(sfx::AC2_FIRE as usize).unwrap();
    let ac2s = samples(ac2);
    assert_eq!(
        ac2.sample_rate(),
        RATE,
        "44.1 kHz sample: no resampling in this check"
    );
    let dv = (1500.0 - 200.0) / 1500.0;
    let pan = -200.0f32 / 750.0;
    let v = master * table.bites[sfx::AC2_FIRE as usize].volume * dv * sfx_gain;
    let (gl, gr) = (v, v * (1.0 + pan));
    let guns = stem(Stem::Guns);
    let (gl_buf, gr_buf) = (left(guns), right(guns));
    let off = first_nonzero(&ac2s).unwrap();
    // Onset of the first shot is exactly at its scripted time (plus the sample's own lead-in).
    assert_eq!(first_nonzero(&gl_buf).unwrap(), frame(GUNS_AT[0]) + off);
    // Expected level: peak of the sample over the first 0.4 s (before the next shot starts) times the gain.
    let w = frame(0.4);
    let want_l = peak(&ac2s[..w]) * gl;
    let want_r = peak(&ac2s[..w]) * gr;
    let got_l = peak(&gl_buf[frame(GUNS_AT[0])..frame(GUNS_AT[0]) + w]);
    let got_r = peak(&gr_buf[frame(GUNS_AT[0])..frame(GUNS_AT[0]) + w]);
    assert!((got_l - want_l).abs() < 1e-4, "left {got_l} vs {want_l}");
    assert!((got_r - want_r).abs() < 1e-4, "right {got_r} vs {want_r}");
    assert!(got_r < got_l, "pan -0.27 attenuates the right side");
    // The last shot ends at its start plus the sample length, and then the stem is silent.
    let last_end = frame(GUNS_AT[2]) + ac2s.len();
    assert!(last_nonzero(&gl_buf).unwrap() < last_end);
    assert!(last_nonzero(&gl_buf).unwrap() > last_end - 4096);
    // Shots overlap (sample is 2.5 s, shots 0.4 s apart): at the third shot all three sound together.
    let mid = frame(GUNS_AT[2]) + 100;
    let expect_mid: f32 = GUNS_AT.iter().map(|t| ac2s[mid - frame(*t)] * gl).sum();
    assert!(
        (gl_buf[mid] - expect_mid).abs() < 1e-4,
        "{} vs {expect_mid}",
        gl_buf[mid]
    );

    // 3. The explosion 1000 units straight ahead: distance volume 1/3, centred.
    let big = bank.effect(sfx::BIGDEATH as usize).unwrap();
    let bigs = samples(big);
    let v = master * table.bites[sfx::BIGDEATH as usize].volume * (500.0 / 1500.0) * sfx_gain;
    let ex = stem(Stem::Explosion);
    let (el, er) = (left(ex), right(ex));
    assert_eq!(
        first_nonzero(&el).unwrap(),
        frame(EXPLOSION_AT) + first_nonzero(&bigs).unwrap()
    );
    assert!(
        (peak(&el) - peak(&bigs) * v).abs() < 1e-4,
        "{} vs {}",
        peak(&el),
        peak(&bigs) * v
    );
    assert!(
        el.iter().zip(&er).all(|(l, r)| (l - r).abs() < 1e-7),
        "centred"
    );
    assert!(last_nonzero(&el).unwrap() < frame(EXPLOSION_AT) + bigs.len());

    // 4. Betty: 22.05 kHz sample at the Betty volume (255/256), centred; starts at 4.0 s.
    let line = bank.betty.sound(betty::BETTY_MISSION_WON as usize).unwrap();
    let ls = samples(line);
    assert_eq!(line.sample_rate(), 22050);
    let bv = master * (255.0 / 256.0);
    let (bl, br) = (left(stem(Stem::Betty)), right(stem(Stem::Betty)));
    let onset = first_nonzero(&bl).unwrap();
    let want_onset = frame(BETTY_AT) + first_nonzero(&ls).unwrap() * 2;
    assert!(
        onset.abs_diff(want_onset) <= 2,
        "betty onset {onset} vs {want_onset}"
    );
    // Resampling by 2 with linear interpolation never exceeds the source peak.
    let want_peak = peak(&ls) * bv;
    assert!(
        (peak(&bl) - want_peak).abs() < 0.01 * want_peak,
        "{} vs {want_peak}",
        peak(&bl)
    );
    assert_eq!(bl, br);
    let betty_end = frame(BETTY_AT) + ls.len() * 2;
    assert!(last_nonzero(&bl).unwrap() <= betty_end);
    assert!(last_nonzero(&bl).unwrap() + 4096 > betty_end);

    // 5. A shot 1600 units away is never played.
    assert_eq!(peak(stem(Stem::TooFar)), 0.0);

    // 6. Music: fades in over the 1 s stream fade, then sits at master * music * tune volume.
    let ml = left(stem(Stem::Music));
    assert!(
        first_nonzero(&ml).unwrap() < frame(0.2),
        "music starts almost at once"
    );
    let tune = samples(&music_sound());
    let steady = master * (128.0 / 256.0) * table.music[music::COMBAT_TUNE_0 as usize].volume;
    // After the fade (from 1.1 s) the stem is the tune scaled by the steady gain.
    let at = frame(2.0);
    for i in at..at + 4000 {
        assert!((ml[i] - tune[i] * steady).abs() < 1e-4, "music frame {i}");
    }
    // During the fade the level rises with time: RMS of consecutive 0.2 s windows increases.
    let rms =
        |a: usize, b: usize| (ml[a..b].iter().map(|x| x * x).sum::<f32>() / (b - a) as f32).sqrt();
    let w0 = rms(frame(0.0), frame(0.2));
    let w1 = rms(frame(0.4), frame(0.6));
    let w2 = rms(frame(0.8), frame(1.0));
    assert!(w0 < w1 && w1 < w2, "fade-in not monotonic: {w0} {w1} {w2}");
    // The tune is still going at the end (it loops).
    assert!(peak(&ml[frame(8.5)..]) > 0.0);
}
