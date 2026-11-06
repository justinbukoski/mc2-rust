//! Decodes the shipped sound files. Skipped when `MC2_DATA` is unset.

use mc2_platform::audio::wav::{self, WavStream};
use mc2_platform::audio::{Mixer, PlayMode, Sound};
use std::path::PathBuf;

fn sound_dir() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("MC2_DATA")?).join("data/sound");
    p.is_dir().then_some(p)
}

fn fnv1a(samples: &[i16]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in samples.iter().flat_map(|s| s.to_le_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The expected hashes are of CoreAudio's (`afconvert -d LEI16`) decode of the same files, so
/// they pin our MS-ADPCM output sample-for-sample to an independent decoder.
#[test]
fn adpcm_decode_is_bit_exact_with_reference_decoder() {
    let Some(dir) = sound_dir() else { return };
    for (name, hash, n) in [
        ("mc2_04a.wav", 0x1e1c_31eb_0efd_f07eu64, 109_296usize), // mono 22.05 kHz, 512-byte blocks
        ("music00.wav", 0xcb9b_97dd_2b84_ffe2, 2_825_968), // stereo 44.1 kHz, 2048-byte blocks
    ] {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        let w = wav::parse(&bytes).unwrap();
        assert_eq!(w.format.tag, wav::FORMAT_ADPCM, "{name}");
        let s = w.decode_i16();
        assert_eq!(s.len(), n, "{name} sample count");
        assert_eq!(fnv1a(&s), hash, "{name} decode differs from reference");
    }
}

#[test]
fn every_shipped_wav_parses_and_decodes() {
    let Some(dir) = sound_dir() else { return };
    let mut seen = std::collections::BTreeSet::new();
    let mut count = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "wav") {
            continue;
        }
        let bytes = std::fs::read(&p).unwrap();
        let sound = Sound::from_wav(&bytes).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert!(sound.frames() > 0, "{}", p.display());
        assert!(
            matches!(sound.sample_rate(), 11025 | 22050 | 44100),
            "{}: {} Hz",
            p.display(),
            sound.sample_rate()
        );
        let f = wav::parse(&bytes).unwrap().format;
        seen.insert((f.tag, f.channels, f.sample_rate, f.bits_per_sample));
        count += 1;
    }
    eprintln!("{count} wavs; formats (tag, ch, rate, bits): {seen:?}");
    assert!(count > 100);
    assert!(seen.contains(&(1, 1, 22050, 16)));
}

#[test]
fn streamed_music_matches_in_memory_decode() {
    let Some(dir) = sound_dir() else { return };
    let path = dir.join("music01.wav");
    let bytes = std::fs::read(&path).unwrap();
    let expect = wav::parse(&bytes).unwrap().decode_i16();
    let mut s = WavStream::new(std::fs::File::open(&path).unwrap()).unwrap();
    let mut got = Vec::with_capacity(expect.len());
    let mut buf = vec![0i16; 3001 * 2];
    loop {
        let n = s.read_samples(&mut buf);
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
    }
    assert_eq!(got.len(), expect.len());
    assert!(
        got == expect,
        "streamed decode differs from in-memory decode"
    );
}

#[test]
fn shipped_sound_plays_through_the_mixer_headless() {
    let Some(dir) = sound_dir() else { return };
    let sound = Sound::from_wav(&std::fs::read(dir.join("mc2_01.wav")).unwrap()).unwrap();
    let mut m = Mixer::for_game(44100);
    m.assign_sound(3, &sound);
    m.set_pan(3, -0.5);
    m.set_mode(3, PlayMode::PlayOnce);
    let frames = (sound.duration_secs() * 44100.0) as usize + 1000;
    let mut out = vec![0.0f32; frames * 2];
    m.render(&mut out);
    let (mut l, mut r) = (0f32, 0f32);
    for f in out.chunks(2) {
        l = l.max(f[0].abs());
        r = r.max(f[1].abs());
    }
    assert!(l > 0.01, "sound was silent");
    assert!(
        (r / l - 0.5).abs() < 0.01,
        "pan -0.5 should halve the right side: {l} vs {r}"
    );
    assert_eq!(m.mode(3), PlayMode::Stop);
}
