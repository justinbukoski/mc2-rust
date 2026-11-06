// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
use flate2::read::ZlibDecoder;
use mc2_assets::{effect, map, tga, tgl};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const FST_MAGIC: u32 = 0xCADDECAF;
const FST_MAGIC_LZ: u32 = 0xFADDECAF;
const PAK_MAGIC: u32 = 0xFEEDFACE;

#[derive(Clone)]
struct Entry {
    name: String,
    bytes: Vec<u8>,
}
fn u32le(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn inflate(b: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(expected);
    ZlibDecoder::new(b)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() != expected {
        return Err(format!(
            "decompressed {} bytes, expected {expected}",
            out.len()
        ));
    }
    Ok(out)
}
fn lzw_decompress(bytes: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    struct Bits<'a> {
        bytes: &'a [u8],
        bit: usize,
    }
    impl Bits<'_> {
        fn code(&mut self, width: usize) -> Option<usize> {
            if self.bit + width > self.bytes.len() * 8 {
                return None;
            }
            let mut value = 0usize;
            for shift in 0..width {
                value |= (((self.bytes[(self.bit + shift) / 8] >> ((self.bit + shift) % 8)) & 1)
                    as usize)
                    << shift;
            }
            self.bit += width;
            Some(value)
        }
    }
    let mut bits = Bits { bytes, bit: 0 };
    let mut dict: Vec<Vec<u8>> = (0..256).map(|v| vec![v as u8]).collect();
    dict.resize_with(258, Vec::new);
    let mut width = 9usize;
    let mut next = 258usize;
    let mut out = Vec::with_capacity(expected);
    let mut previous: Option<Vec<u8>> = None;
    while let Some(code) = bits.code(width) {
        if code == 257 {
            break;
        }
        if code == 256 {
            dict.truncate(258);
            width = 9;
            next = 258;
            previous = None;
            continue;
        }
        let entry = if code < dict.len() && !dict[code].is_empty() {
            dict[code].clone()
        } else if code == next {
            let mut v = previous.clone().ok_or("invalid first LZW code")?;
            v.push(v[0]);
            v
        } else {
            return Err(format!("invalid LZW code {code} (next {next})"));
        };
        out.extend_from_slice(&entry);
        if out.len() > expected {
            return Err("LZW stream exceeds declared output size".into());
        }
        if let Some(prev) = previous.take() {
            let mut added = prev;
            added.push(entry[0]);
            dict.push(added);
            next += 1;
            if next == (1usize << width) && width < 12 {
                width += 1;
            }
        }
        previous = Some(entry);
    }
    if out.len() != expected {
        return Err(format!(
            "LZW produced {} bytes, expected {expected}",
            out.len()
        ));
    }
    Ok(out)
}
fn read_fst(path: &Path) -> Result<Vec<Entry>, String> {
    let b = fs::read(path).map_err(|e| e.to_string())?;
    if b.len() < 8 || !matches!(u32le(&b, 0), FST_MAGIC | FST_MAGIC_LZ) {
        return Err("bad FST header".into());
    }
    let n = u32le(&b, 4) as usize;
    let end = 8usize
        .checked_add(n.checked_mul(266).ok_or("entry count overflow")?)
        .ok_or("table overflow")?;
    if end > b.len() {
        return Err("truncated FST table".into());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p = 8 + i * 266;
        let offset = u32le(&b, p) as usize;
        let size = u32le(&b, p + 4) as usize;
        let real = u32le(&b, p + 8) as usize;
        let nameb = &b[p + 16..p + 266];
        let len = nameb.iter().position(|v| *v == 0).unwrap_or(250);
        let name = String::from_utf8_lossy(&nameb[..len]).into_owned();
        let raw = b
            .get(offset..offset.checked_add(size).ok_or("entry overflow")?)
            .ok_or("FST content outside archive")?;
        let bytes = if size == real {
            raw.to_vec()
        } else {
            inflate(raw, real)?
        };
        out.push(Entry { name, bytes });
    }
    Ok(out)
}
fn read_pak(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if bytes.len() < 12 || u32le(bytes, 0) != PAK_MAGIC {
        return Err("bad PAK header".into());
    }
    let first = u32le(bytes, 4) as usize;
    if first < 8 || !first.is_multiple_of(4) || first > bytes.len() {
        return Err("invalid packet offset table".into());
    }
    let n = first / 4 - 2;
    if n > 100_000 {
        return Err("invalid packet offset table".into());
    }
    let offsets = (0..n).map(|i| u32le(bytes, 8 + i * 4)).collect::<Vec<_>>();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let tagged = offsets[i];
        let kind = tagged >> 29;
        let start = (tagged & 0x1fff_ffff) as usize;
        let stop = if i + 1 < n {
            (offsets[i + 1] & 0x1fff_ffff) as usize
        } else {
            bytes.len()
        };
        let packet = bytes.get(start..stop).ok_or("packet outside file")?;
        out.push(match kind {
            0 | 1 => packet.to_vec(),
            2 | 4 => {
                if packet.len() < 4 {
                    return Err("short zlib packet".into());
                }
                let len = u32le(packet, 0) as usize;
                if kind == 2 {
                    lzw_decompress(&packet[4..], len)?
                } else {
                    inflate(&packet[4..], len)?
                }
            }
            7 => Vec::new(),
            _ => return Err(format!("unsupported packet storage type {kind}")),
        });
    }
    Ok(out)
}
fn heightmap_gray(elevations: &[f32]) -> Vec<u8> {
    let mut sorted = elevations.to_vec();
    sorted.sort_by(f32::total_cmp);
    let low = sorted[(sorted.len() - 1) / 100];
    let high = sorted[((sorted.len() - 1) * 99) / 100];
    let span = high - low;
    elevations
        .iter()
        .map(|&v| {
            if span > 0.0 {
                (((v - low) / span).clamp(0.0, 1.0) * 255.0).round() as u8
            } else {
                0
            }
        })
        .collect()
}
fn collect_fsts(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p)
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fst")) {
                found.push(p)
            }
        }
    }
    found
}
fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?
    }
    let f = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(f, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(pixels).map_err(|e| e.to_string())
}

#[test]
fn scan_game_data_archives() {
    let Ok(root) = std::env::var("MC2_DATA") else {
        eprintln!("skipping game-data scan: MC2_DATA is unset");
        return;
    };
    let root = PathBuf::from(root);
    let mut entries = Vec::new();
    for p in collect_fsts(&root) {
        // `effect.fst` in the installed data is an optional, zero-byte
        // placeholder.  `Vfs::from_system_cfg` deliberately leaves such files
        // unmounted (the C++ startup path likewise keeps going), so the
        // archive-wide scanner must inspect the same set of usable archives.
        if fs::metadata(&p).is_ok_and(|metadata| metadata.len() == 0) {
            continue;
        }
        match read_fst(&p) {
            Ok(mut e) => entries.append(&mut e),
            Err(e) => panic!("{}: {e}", p.display()),
        }
    }
    let mut counts = [0usize; 5];
    let mut failures = Vec::new();
    let mut texture_dumps = Vec::new();
    let mut terrain_dump = None;
    for e in &entries {
        let low = e.name.to_ascii_lowercase();
        let ext = Path::new(&low)
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("");
        match ext {
            "tga" => {
                counts[0] += 1;
                match tga::decode(&e.bytes) {
                    Ok(img) => {
                        if texture_dumps.len() < 3 {
                            texture_dumps.push((img.width, img.height, img.pixels.clone()));
                        }
                    }
                    Err(x) => failures.push(format!("{} TGA: {x}", e.name)),
                }
            }
            "tgl" => {
                counts[1] += 1;
                if let Err(x) = tgl::decode_shape(&e.bytes) {
                    failures.push(format!("{} TGL: {x}", e.name))
                }
            }
            "agl" => {
                counts[2] += 1;
                if let Err(x) = tgl::decode_animation(&e.bytes) {
                    failures.push(format!("{} AGL: {x}", e.name))
                }
            }
            "fx" if low.ends_with("mc2.fx") => {
                counts[4] += 1;
                if let Err(x) = effect::decode_header(&e.bytes) {
                    failures.push(format!("{} FX header: {x}", e.name))
                }
            }
            "pak" if low.contains("mission") || low.contains("map") => {
                counts[3] += 1;
                match read_pak(&e.bytes) {
                    Ok(pkts) => {
                        for i in 0..4 {
                            if let Some(packet) = pkts.get(i) {
                                match map::decode_packet(i as u32, packet) {
                                    Ok(map::MissionPacket::Terrain { side, vertices })
                                        if i == 0 =>
                                    {
                                        if terrain_dump.is_none() {
                                            let mut px = Vec::with_capacity(side * side * 4);
                                            let gray = heightmap_gray(
                                                &vertices
                                                    .iter()
                                                    .map(|v| v.elevation)
                                                    .collect::<Vec<_>>(),
                                            );
                                            for h in gray {
                                                px.extend([h, h, h, 255]);
                                            }
                                            terrain_dump = Some((side as u32, px));
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(x) => failures.push(format!("{} packet {i}: {x}", e.name)),
                                }
                            }
                        }
                    }
                    Err(x) => failures.push(format!("{} PAK: {x}", e.name)),
                }
            }
            _ => {}
        }
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("data-scan");
    for (i, (w, h, p)) in texture_dumps.iter().enumerate() {
        write_png(
            &out.join(format!("texture-{}.png", i + 1)),
            u32::from(*w),
            u32::from(*h),
            p,
        )
        .unwrap();
    }
    if let Some((side, p)) = terrain_dump {
        write_png(&out.join("terrain-heightmap.png"), side, side, &p).unwrap();
    }
    eprintln!(
        "MC2 scan: TGA={} TGL={} AGL={} mission-pak={} mc2.fx headers={}",
        counts[0], counts[1], counts[2], counts[3], counts[4]
    );
    if !failures.is_empty() {
        panic!(
            "{} asset decode failures (first 40):\n{}",
            failures.len(),
            failures
                .iter()
                .take(40)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn malformed_pak_offsets_return_errors() {
    for first in [0u32, 4] {
        let mut bytes = PAK_MAGIC.to_le_bytes().to_vec();
        bytes.extend(first.to_le_bytes());
        bytes.extend([0; 4]);
        assert!(read_pak(&bytes).is_err());
    }
}

#[test]
fn heightmap_uses_one_to_ninety_nine_percentile_range() {
    let mut elevations = (0..10_000).map(|n| n as f32).collect::<Vec<_>>();
    elevations.extend([-81.0, -80.0, -79.0, -78.0, 1_000_000.0]);
    let gray = heightmap_gray(&elevations);
    assert_eq!(gray[10_000], 0);
    assert_eq!(*gray.last().unwrap(), 255);
    assert!(gray[100] < 10);
    assert!(gray[9_900] > 245);
}
