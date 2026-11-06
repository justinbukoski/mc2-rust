// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Ground truth for the 3D mech preview colour (rc-vqa-preview-texel): decodes the Razorback's (`werewolf`, crater.cpp:102)
//! skin texels with the port's own TGA decoder and prints the unpainted-region colours.  No GPU.
//! Needs `MC2_DATA` (skips cleanly without it).

#![allow(clippy::chunks_exact_to_as_chunks)]
use mc2_assets::tgl::decode_shape;
use mc2_formats::vfs::Vfs;

/// `texture_path` (mc2-render shape.rs): file name inside the 128 pixel folder, lower case.
fn texture_path(name: &str) -> String {
    let base = name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    format!("data/tgl/128/{base}")
}

fn is_mask(p: &[u8]) -> bool {
    p[3] != 0 && ((p[1] == 0 && p[2] == 0) || (p[0] == 0 && p[2] == 0) || (p[0] == 0 && p[1] == 0))
}

fn mean(px: &[[u8; 3]]) -> [f32; 3] {
    let n = px.len().max(1) as f32;
    let mut s = [0.0f32; 3];
    for p in px {
        for c in 0..3 {
            s[c] += p[c] as f32;
        }
    }
    [s[0] / n, s[1] / n, s[2] / n]
}

#[test]
fn razorback_skin_texels() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("SKIPPED: MC2_DATA is unset");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let bytes = vfs
        .read("data/tgl/werewolf.tgl")
        .unwrap()
        .expect("werewolf.tgl");
    let shape = decode_shape(&bytes).expect("decode raven.tgl");
    let mut hist = std::collections::BTreeMap::<u32, usize>::new();
    for n in &shape.nodes {
        if let Some(s) = &n.shape {
            for v in &s.vertices {
                *hist.entry(v.argb_light).or_default() += 1;
            }
        }
    }
    println!("VERTEX argb_light histogram (value: count): {:08x?}", hist);
    for (i, t) in shape.textures.iter().enumerate() {
        let path = texture_path(&t.name);
        let Some(tga) = vfs.read(&path).unwrap() else {
            println!("TEX {i} {} -> {path}: not found", t.name);
            continue;
        };
        let img = mc2_assets::tga::decode(&tga).expect("tga");
        let px = &img.pixels;
        let mask = px.chunks_exact(4).filter(|p| is_mask(p)).count();
        let other: Vec<[u8; 3]> = px
            .chunks_exact(4)
            .filter(|p| p[3] != 0 && !is_mask(p))
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        let warm = other.iter().filter(|p| p[0] > p[2]).count();
        println!(
            "TEX {i} {} -> {path} {}x{} mask px {mask}, unmasked px {}, mean RGB {:?}, R>B {warm}, B>R {}",
            t.name,
            img.width,
            img.height,
            other.len(),
            mean(&other),
            other.iter().filter(|p| p[2] > p[0]).count(),
        );
        // coarse 8x8 block grid of the unmasked texels (row-major, top-left origin)
        let (w, h) = (img.width as usize, img.height as usize);
        for by in 0..8 {
            let mut line = String::new();
            for bx in 0..8 {
                let mut cell = Vec::new();
                for y in by * h / 8..(by + 1) * h / 8 {
                    for x in bx * w / 8..(bx + 1) * w / 8 {
                        let p = &px[(y * w + x) * 4..][..4];
                        if p[3] != 0 && !is_mask(p) {
                            cell.push([p[0], p[1], p[2]]);
                        }
                    }
                }
                let m = mean(&cell);
                line += &format!(
                    "({:3.0},{:3.0},{:3.0})/{:<4} ",
                    m[0],
                    m[1],
                    m[2],
                    cell.len()
                );
            }
            println!("  {line}");
        }
    }
}

/// The port's decoder returns RGB order from a TGA's BGR bytes, like `Image::loadTGA` (GameOS/gameos/utils/Image.cpp,
/// `*dest++ = src[2]; src[1]; src[0]`), so both sides hand the GPU true RGB texels.
#[test]
fn tga_decoder_returns_rgb_from_bgr_bytes() {
    // 1x1, 24 bit, uncompressed, bottom-left origin; pixel bytes B=1, G=2, R=3
    let mut t = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 24, 0];
    t.extend([1, 2, 3]);
    let img = mc2_assets::tga::decode(&t).unwrap();
    assert_eq!(img.pixels[..3], [3, 2, 1]);
}

/// The Razorback skin's unpainted texels are neutral grey except a few orange accents, so a neutral light cannot
/// give them a hue: R == G == B for the grey ones, orange only where the artist painted it.
#[test]
fn razorback_unpainted_texels_are_grey_or_orange_accent() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("SKIPPED: MC2_DATA is unset");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let tga = vfs
        .read("data/tgl/128/werewolfrgb.tga")
        .unwrap()
        .expect("tga");
    let img = mc2_assets::tga::decode(&tga).unwrap();
    let (mut grey, mut orange, mut other) = (0, 0, 0);
    for p in img
        .pixels
        .chunks_exact(4)
        .filter(|p| p[3] != 0 && !is_mask(p))
    {
        if p[0] == p[1] && p[1] == p[2] {
            grey += 1;
        } else if p[0] > p[1] && p[1] > p[2] && p[0] > 150 {
            orange += 1;
        } else {
            other += 1;
        }
    }
    println!("unmasked texels: grey {grey}, orange/warm {orange}, other {other}");
    assert!(grey > 2 * (orange + other), "{grey} {orange} {other}");
}
