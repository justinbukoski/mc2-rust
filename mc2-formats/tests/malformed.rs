//! Regression tests for malformed input. Each test pins one hardening rule:
//! bad input returns an error, never panics, and never allocates from an
//! untrusted size. These tests need no game data.

use mc2_formats::csv::CsvFile;
use mc2_formats::fst::{elf_hash, FastFile, FST_VERSION_LZ, FST_VERSION_ZLIB};
use mc2_formats::ini::FitIni;
use mc2_formats::lzw;
use mc2_formats::pak::{PacketPolicy, PakFile, PakWriter};
use mc2_formats::vfs::Vfs;
use mc2_formats::FormatError;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

const ENTRY_LEN: usize = 266;
const NAME_LEN: usize = 250;

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

/// One archive entry as (name, real size, stored bytes, stored hash).
struct Entry {
    name: &'static str,
    real: u32,
    packed: Vec<u8>,
    hash: u32,
}

impl Entry {
    fn new(name: &'static str, real: usize, packed: Vec<u8>) -> Self {
        Self {
            name,
            real: real as u32,
            packed,
            hash: elf_hash(name.to_ascii_lowercase().as_bytes()),
        }
    }
}

/// Builds an FST image: header, table, payloads.
fn fst_image(version: u32, entries: &[Entry]) -> Vec<u8> {
    let mut at = (8 + entries.len() * ENTRY_LEN) as u32;
    let mut table = Vec::new();
    let mut payloads = Vec::new();
    for e in entries {
        table.extend_from_slice(&at.to_le_bytes());
        table.extend_from_slice(&(e.packed.len() as u32).to_le_bytes());
        table.extend_from_slice(&e.real.to_le_bytes());
        table.extend_from_slice(&e.hash.to_le_bytes());
        let mut field = [0u8; NAME_LEN];
        field[..e.name.len()].copy_from_slice(e.name.as_bytes());
        table.extend_from_slice(&field);
        payloads.extend_from_slice(&e.packed);
        at += e.packed.len() as u32;
    }
    let mut out = version.to_le_bytes().to_vec();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend(table);
    out.extend(payloads);
    out
}

/// A PAK image with one packet whose stored bytes are `body`.
fn pak_one_packet(stored_type: u32, body: &[u8]) -> Vec<u8> {
    let mut img = 0xFEED_FACEu32.to_le_bytes().to_vec();
    img.extend_from_slice(&12u32.to_le_bytes());
    img.extend_from_slice(&((stored_type << 29) | 12).to_le_bytes());
    img.extend_from_slice(body);
    img
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mc2-malformed-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// 1. LZW: invalid codes are errors and the chain walk is bounded.

#[test]
fn lzw_self_referencing_code_is_an_error_not_a_hang() {
    // Build "clear, 511, 511" with the 9-bit LSB-first packing the codec uses.
    let codes = [256u32, 511, 511, 257, 257];
    let mut bits: Vec<bool> = Vec::new();
    for c in codes {
        for i in 0..9 {
            bits.push((c >> i) & 1 == 1);
        }
    }
    let mut bytes = vec![0u8; bits.len().div_ceil(8)];
    for (i, b) in bits.iter().enumerate() {
        if *b {
            bytes[i / 8] |= 1 << (i % 8);
        }
    }
    let err = lzw::decompress(&bytes, 1 << 20).expect_err("must be rejected");
    assert!(matches!(err, FormatError::Decompress { .. }), "{err}");
}

#[test]
fn lzw_output_beyond_declared_size_is_an_error() {
    let packed = lzw::compress(&vec![0xAB; 50_000]);
    assert!(lzw::decompress(&packed, 49_999).is_err());
    assert_eq!(lzw::decompress(&packed, 50_000).unwrap().len(), 50_000);
}

// 2. Sizes from untrusted headers never force allocation or unbounded output.

#[test]
fn zlib_compression_bomb_is_capped_at_real_size() {
    // 8 MiB of zeros deflates to a few kilobytes, but the table claims 100 bytes.
    let bomb = zlib(&vec![0u8; 8 << 20]);
    let img = fst_image(FST_VERSION_ZLIB, &[Entry::new("bomb.bin", 100, bomb)]);
    let mut ff = FastFile::from_reader(Cursor::new(img)).unwrap();
    assert!(matches!(
        ff.read_index(0),
        Err(FormatError::Decompress { .. })
    ));
}

#[test]
fn payload_past_end_of_archive_is_rejected_before_allocation() {
    let mut img = fst_image(FST_VERSION_LZ, &[Entry::new("a.bin", 4, zlib(b"abcd"))]);
    // Patch packed_size (entry field at 8 + 4) to u32::MAX.
    img[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut ff = FastFile::from_reader(Cursor::new(img)).unwrap();
    assert!(matches!(
        ff.read_packed(0),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(ff.read_index(0).is_err());
}

#[test]
fn pak_zlib_packet_with_huge_unpacked_size_is_an_error() {
    let body = zlib(b"tiny");
    let mut stored = u32::MAX.to_le_bytes().to_vec();
    stored.extend_from_slice(&body);
    let pak = PakFile::from_bytes(pak_one_packet(4, &stored)).unwrap();
    assert!(pak.read_packet(0).is_err());
}

#[test]
fn pak_lzd_packet_output_is_capped_at_its_header_size() {
    let mut stored = 10u32.to_le_bytes().to_vec();
    stored.extend_from_slice(&lzw::compress(&vec![0u8; 1000]));
    let pak = PakFile::from_bytes(pak_one_packet(2, &stored)).unwrap();
    assert!(pak.read_packet(0).is_err());
}

// 3. FST LZ version word: codec chosen from the payload, both paths tested.

#[test]
fn fst_lz_archive_reads_zlib_and_lzw_payloads() {
    let zlib_data = b"zlib body ".repeat(40);
    let lzw_data = b"lzw body ".repeat(40);
    let img = fst_image(
        FST_VERSION_LZ,
        &[
            Entry::new("z.bin", zlib_data.len(), zlib(&zlib_data)),
            Entry::new("l.bin", lzw_data.len(), lzw::compress(&lzw_data)),
        ],
    );
    let mut ff = FastFile::from_reader(Cursor::new(img)).unwrap();
    assert_eq!(ff.read("z.bin").unwrap(), Some(zlib_data));
    assert_eq!(ff.read("l.bin").unwrap(), Some(lzw_data));
}

#[test]
fn fst_lz_payload_that_is_neither_codec_is_an_error() {
    let garbage = vec![0xFFu8; 64];
    let img = fst_image(FST_VERSION_LZ, &[Entry::new("g.bin", 64, garbage)]);
    let mut ff = FastFile::from_reader(Cursor::new(img)).unwrap();
    assert!(ff.read_index(0).is_err());
}

// 4. FIT array counts: negative or absurd counts are errors, never panics.

#[test]
fn fit_negative_array_count_is_an_error() {
    let ini =
        FitIni::from_bytes(b"FITini\r\n[a]\r\nf[-1] xs = 1.0\r\nFITend\r\n".to_vec()).unwrap();
    let mut ini = ini;
    ini.seek_block("a").unwrap();
    assert!(ini.array_len::<f32>("xs").is_err());
    assert!(ini.read_array::<f32>("xs", usize::MAX).is_err());
}

#[test]
fn fit_absurd_array_count_is_an_error_without_large_allocation() {
    let mut ini =
        FitIni::from_bytes(b"FITini\r\n[a]\r\nf[999999999] xs = 1.0\r\nFITend\r\n".to_vec())
            .unwrap();
    ini.seek_block("a").unwrap();
    // Caller capacity is unbounded, so only the data itself can stop the read.
    assert!(ini.read_array::<f32>("xs", usize::MAX).is_err());
}

// 5. VFS: names cannot escape the data root.

#[test]
fn vfs_refuses_names_that_escape_the_root() {
    let dir = scratch("escape");
    let root = dir.join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(dir.join("secret.txt"), b"outside").unwrap();
    let mut vfs = Vfs::new(&root);
    for name in [
        "../secret.txt",
        "..\\secret.txt",
        "/etc/passwd",
        "\\etc\\passwd",
        "data/../../secret.txt",
        "C:\\secret.txt",
    ] {
        assert!(vfs.read(name).is_err(), "{name} must be refused");
        assert!(vfs.locate(name).is_none(), "{name} must not resolve");
    }
    assert!(vfs.read("missing.txt").unwrap().is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn vfs_mount_names_cannot_escape_the_root() {
    let dir = scratch("mount");
    let mut vfs = Vfs::new(&dir);
    assert!(vfs.mount_archive("../outside.fst").is_err());
    assert!(vfs.mount_archive("/abs/outside.fst").is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

// 6. NumFastFiles governs mounting. Every configured archive must load.

fn write_cfg(dir: &Path, body: &str) {
    std::fs::write(
        dir.join("system.cfg"),
        format!("FITini \r\n[FastFiles]\r\n{body}\r\nFITend \r\n"),
    )
    .unwrap();
}

fn empty_archive(dir: &Path, name: &str) {
    // Version word and zero entries: a valid archive with an empty table.
    let mut img = FST_VERSION_LZ.to_le_bytes().to_vec();
    img.extend_from_slice(&0u32.to_le_bytes());
    std::fs::write(dir.join(name), img).unwrap();
}

#[test]
fn zero_num_fast_files_mounts_nothing() {
    let dir = scratch("numzero");
    write_cfg(&dir, "l NumFastFiles = 0\r\nst File0=\"missing.fst\"");
    let vfs = Vfs::from_system_cfg(&dir).unwrap();
    assert_eq!(vfs.archive_paths().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extra_file_keys_beyond_num_fast_files_are_an_error() {
    let dir = scratch("numextra");
    empty_archive(&dir, "a.fst");
    empty_archive(&dir, "b.fst");
    write_cfg(
        &dir,
        "l NumFastFiles = 1\r\nst File0=\"a.fst\"\r\nst File1=\"b.fst\"",
    );
    assert!(Vfs::from_system_cfg(&dir).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn every_configured_archive_must_be_present_and_usable() {
    let dir = scratch("nummissing");
    empty_archive(&dir, "a.fst");
    write_cfg(&dir, "l NumFastFiles = 2\r\nst File0=\"a.fst\"");
    assert!(
        Vfs::from_system_cfg(&dir).is_err(),
        "missing File1 must fail"
    );

    write_cfg(&dir, "l NumFastFiles = 1\r\nst File0=\"absent.fst\"");
    assert!(
        Vfs::from_system_cfg(&dir).is_err(),
        "an unavailable configured archive must fail"
    );

    std::fs::write(dir.join("empty.fst"), []).unwrap();
    write_cfg(&dir, "l NumFastFiles = 1\r\nst File0=\"empty.fst\"");
    assert!(
        Vfs::from_system_cfg(&dir).is_err(),
        "a zero-byte configured archive must fail"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn negative_num_fast_files_is_an_error() {
    let dir = scratch("numneg");
    write_cfg(&dir, "l NumFastFiles = -1");
    assert!(Vfs::from_system_cfg(&dir).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

// 7. Archive lookup: stored hash AND case-insensitive name, no slash retry.

#[test]
fn archive_lookup_requires_the_stored_hash() {
    let mut e = Entry::new("data/x.dat", 2, zlib(b"hi"));
    e.hash ^= 1; // name is right, hash is wrong
    let img = fst_image(FST_VERSION_LZ, &[e]);
    let ff = FastFile::from_reader(Cursor::new(img)).unwrap();
    assert_eq!(ff.index_of("data/x.dat"), None);
}

#[test]
fn archive_lookup_does_not_treat_backslash_as_slash() {
    let img = fst_image(FST_VERSION_LZ, &[Entry::new("data/x.dat", 2, zlib(b"hi"))]);
    let ff = FastFile::from_reader(Cursor::new(img)).unwrap();
    assert_eq!(ff.index_of("DATA/X.DAT"), Some(0));
    assert_eq!(ff.index_of("data\\x.dat"), None);
}

// 8. PAK table boundaries.

#[test]
fn pak_first_offset_must_be_a_multiple_of_four() {
    let mut img = pak_one_packet(0, b"hello");
    img[4..8].copy_from_slice(&10u32.to_le_bytes());
    assert!(matches!(
        PakFile::from_bytes(img),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn pak_packet_offset_inside_header_or_table_is_an_error() {
    let mut img = pak_one_packet(0, b"hello");
    // Point the packet at offset 4, inside the version word.
    img[8..12].copy_from_slice(&4u32.to_le_bytes());
    let pak = PakFile::from_bytes(img).unwrap();
    assert!(pak.info(0).is_err());
    assert!(pak.read_packet(0).is_err());
}

#[test]
fn pak_writer_output_still_reads() {
    let mut w = PakWriter::new(2);
    w.write_packet(0, b"payload", PacketPolicy::Raw).unwrap();
    w.write_packet(1, &vec![1u8; 4096], PacketPolicy::Auto)
        .unwrap();
    let pak = PakFile::from_bytes(w.finish()).unwrap();
    assert_eq!(pak.read_packet(0).unwrap(), b"payload".to_vec());
    assert_eq!(pak.read_packet(1).unwrap().len(), 4096);
}

// 9. FIT strings must be terminated.

#[test]
fn fit_unterminated_string_is_an_error() {
    let mut ini =
        FitIni::from_bytes(b"FITini\r\n[a]\r\nst s = \"abc\r\nFITend\r\n".to_vec()).unwrap();
    ini.seek_block("a").unwrap();
    assert!(ini.read_string("s", 64).is_err());
}

// Symlinks: a mount or loose file that reaches outside the root is refused.

#[cfg(unix)]
#[test]
fn archive_mount_through_symlink_outside_root_is_refused() {
    let dir = scratch("symlink-mount");
    let root = dir.join("data");
    let outside = dir.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    empty_archive(&outside, "evil.fst");
    empty_archive(&root, "good.fst");
    std::os::unix::fs::symlink(outside.join("evil.fst"), root.join("link.fst")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("linkdir")).unwrap();

    let mut vfs = Vfs::new(&root);
    assert!(vfs.mount_archive("link.fst").is_err(), "symlinked file");
    assert!(
        vfs.mount_archive("linkdir/evil.fst").is_err(),
        "symlinked directory"
    );
    assert_eq!(vfs.archive_paths().count(), 0);
    vfs.mount_archive("good.fst").unwrap();
    assert_eq!(vfs.archive_paths().count(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn loose_file_through_symlink_outside_root_is_not_found() {
    let dir = scratch("symlink-loose");
    let root = dir.join("data");
    let outside = dir.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"outside").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();

    let mut vfs = Vfs::new(&root);
    assert_eq!(vfs.read("link.txt").unwrap(), None);
    assert!(vfs.locate("link.txt").is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

// Robustness sweep: nothing panics on arbitrary bytes, and every structured
// family is mutated from a valid seed so the parsers are actually reached.

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }
}

/// Valid FST image: one zlib entry and one LZW entry, under the LZ version word.
fn fst_seed() -> Vec<u8> {
    fst_image(
        FST_VERSION_LZ,
        &[
            Entry::new("a.bin", 40, zlib(&[7u8; 40])),
            Entry::new("b.bin", 40, lzw::compress(&[9u8; 40])),
        ],
    )
}

/// Valid PAK image: raw, zlib and LZD packets.
fn pak_seed() -> Vec<u8> {
    let raw = b"raw packet bytes".to_vec();
    let mut zl = 200u32.to_le_bytes().to_vec();
    zl.extend(zlib(&[3u8; 200]));
    let mut lz = 60u32.to_le_bytes().to_vec();
    lz.extend(lzw::compress(&[5u8; 60]));
    let fpo = 8 + 4 * 3u32;
    let off0 = fpo;
    let off1 = off0 + raw.len() as u32;
    let off2 = off1 + zl.len() as u32;
    let mut img = 0xFEED_FACEu32.to_le_bytes().to_vec();
    img.extend_from_slice(&fpo.to_le_bytes());
    img.extend_from_slice(&off0.to_le_bytes());
    img.extend_from_slice(&((4u32 << 29) | off1).to_le_bytes());
    img.extend_from_slice(&((2u32 << 29) | off2).to_le_bytes());
    img.extend(raw);
    img.extend(zl);
    img.extend(lz);
    img
}

const FIT_SEED: &[u8] = b"FITini \r\n[a]\r\nf speed = 2.5\r\nl count = 0x10\r\nst s = \"hi\"\r\nf[3] xs = 1, 2, 3\r\nFITend \r\n";
const CSV_SEED: &[u8] = b"name,a,b\r\nx,1,2\r\ny,3,4\r\nz,5,6\r\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Family {
    Fst,
    Pak,
    Fit,
    Csv,
    Random,
}

const FAMILIES: [Family; 5] = [
    Family::Fst,
    Family::Pak,
    Family::Fit,
    Family::Csv,
    Family::Random,
];

fn seed_for(family: Family) -> Option<Vec<u8>> {
    match family {
        Family::Fst => Some(fst_seed()),
        Family::Pak => Some(pak_seed()),
        Family::Fit => Some(FIT_SEED.to_vec()),
        Family::Csv => Some(CSV_SEED.to_vec()),
        Family::Random => None,
    }
}

/// Runs the family's parser over `data`, touching its contents. Returns true
/// when the structured parser accepted the image (always false for `Random`).
fn exercise(family: Family, data: &[u8]) -> bool {
    match family {
        Family::Fst => match FastFile::from_reader(Cursor::new(data.to_vec())) {
            Ok(mut ff) => {
                for j in 0..ff.entries().len().min(16) {
                    let _ = ff.read_index(j);
                }
                true
            }
            Err(_) => false,
        },
        Family::Pak => match PakFile::from_bytes(data.to_vec()) {
            Ok(pak) => {
                for p in 0..pak.packet_count().min(16) {
                    let _ = pak.read_packet(p);
                }
                true
            }
            Err(_) => false,
        },
        Family::Fit => match FitIni::from_bytes(data.to_vec()) {
            Ok(mut ini) => {
                let ids: Vec<String> = ini.blocks().map(str::to_owned).collect();
                for id in ids {
                    let _ = ini.seek_block(&id);
                    let _ = ini.read_i64("count");
                    let _ = ini.read_string("s", 64);
                    let _ = ini.read_array::<f32>("xs", 16);
                    let _ = ini.array_len::<i64>("xs");
                }
                true
            }
            Err(_) => false,
        },
        Family::Csv => match CsvFile::from_bytes(data.to_vec()) {
            Ok(csv) => {
                for r in 0..=csv.rows().min(4) {
                    for c in 0..=csv.cols().min(4) {
                        let _ = csv.read_f32(r, c);
                    }
                }
                true
            }
            Err(_) => false,
        },
        Family::Random => {
            let _ = lzw::decompress(data, 4096);
            let _ = FitIni::from_bytes(data.to_vec());
            let _ = PakFile::from_bytes(data.to_vec());
            let _ = FastFile::from_reader(Cursor::new(data.to_vec()));
            let _ = CsvFile::from_bytes(data.to_vec());
            false
        }
    }
}

#[test]
fn seeds_are_valid_and_carry_the_right_magic() {
    // The FST and PAK magic words are little-endian on disk.
    assert_eq!(&fst_seed()[..4], &0xFADD_ECAFu32.to_le_bytes());
    assert_eq!(&fst_seed()[..4], &[0xAF, 0xEC, 0xDD, 0xFA]);
    assert_eq!(&pak_seed()[..4], &[0xCE, 0xFA, 0xED, 0xFE]);
    for family in [Family::Fst, Family::Pak, Family::Fit, Family::Csv] {
        let seed = seed_for(family).unwrap();
        assert!(exercise(family, &seed), "{family:?} seed must parse");
    }
}

#[test]
fn arbitrary_and_mutated_bytes_never_panic() {
    const ROUNDS: usize = 4000;
    let mut rng = Lcg(0x5EED);
    let mut attempts: std::collections::HashMap<Family, usize> = Default::default();
    let mut accepted: std::collections::HashMap<Family, usize> = Default::default();

    for round in 0..ROUNDS {
        let family = FAMILIES[round % FAMILIES.len()];
        *attempts.entry(family).or_default() += 1;
        let data = match seed_for(family) {
            Some(mut seed) => {
                // Corrupt a few bytes, and sometimes truncate.
                for _ in 0..rng.below(7) {
                    let at = rng.below(seed.len());
                    seed[at] = rng.next() as u8;
                }
                if rng.below(4) == 0 {
                    let keep = 1 + rng.below(seed.len());
                    seed.truncate(keep);
                }
                seed
            }
            None => {
                let len = rng.below(600);
                rng.bytes(len)
            }
        };
        if exercise(family, &data) {
            *accepted.entry(family).or_default() += 1;
        }
    }

    for family in FAMILIES {
        let n = attempts.get(&family).copied().unwrap_or(0);
        assert_eq!(
            n,
            ROUNDS / FAMILIES.len(),
            "{family:?} must be exercised every round"
        );
    }
    // Mutations must still reach the structured parsers and be accepted
    // sometimes, or the sweep is not testing them.
    for family in [Family::Fst, Family::Pak, Family::Fit, Family::Csv] {
        assert!(
            accepted.get(&family).copied().unwrap_or(0) > 0,
            "{family:?} parser was never reached past its header"
        );
    }
}
