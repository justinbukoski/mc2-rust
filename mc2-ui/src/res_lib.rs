//! Reads the UI string table out of the player's own resource library.
//!
//! Two formats are understood, both opened with the `object` crate:
//!
//! * the Linux build's `libmc2res_64.so` (alariq's port, `res/libmain.cpp`): an ELF64 shared
//!   object whose exported `StringRecords[]` array holds `{ int id; const char* str; }` records.
//!   The pointers are filled by the loader, so they are resolved through the `R_*_RELATIVE`
//!   dynamic relocations of the library (or taken as stored when the slot has none);
//! * the Windows retail `mc2res.dll`: a PE image whose `RT_STRING` resources hold the table in
//!   blocks of sixteen length-prefixed UTF-16 strings (block `n` holds ids `(n - 1) * 16 ..`).
//!   `object` walks the image and its resource tree; the sixteen-string block layout is the
//!   only part decoded here (no crate decodes it).
//!
//! Both readers return `(id, text)` in library order; the caller keeps the first of a duplicate.
//! Every offset is bounds-checked: a malformed library is an error, never a panic.

use std::collections::HashMap;

use object::elf;
use object::pe;
use object::read::elf::ElfFile64;
use object::read::pe::{
    ImageNtHeaders, PeFile, ResourceDirectory, ResourceDirectoryEntryData, ResourceDirectoryTable,
};
use object::{FileKind, LittleEndian as LE, Object, ObjectSection, ObjectSymbol, RelocationFlags};

/// A malformed or unsupported library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResLibError(pub String);

impl std::fmt::Display for ResLibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ResLibError {}

impl From<object::Error> for ResLibError {
    fn from(e: object::Error) -> Self {
        ResLibError(e.to_string())
    }
}

fn err<T>(m: impl Into<String>) -> Result<T, ResLibError> {
    Err(ResLibError(m.into()))
}

fn bytes(b: &[u8], off: usize, n: usize) -> Result<&[u8], ResLibError> {
    off.checked_add(n)
        .and_then(|end| b.get(off..end))
        .ok_or_else(|| ResLibError(format!("truncated at {off:#x}")))
}

fn u16_at(b: &[u8], off: usize) -> Result<u16, ResLibError> {
    let b = bytes(b, off, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(b: &[u8], off: usize) -> Result<u32, ResLibError> {
    let b = bytes(b, off, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(b: &[u8], off: usize) -> Result<u64, ResLibError> {
    let b = bytes(b, off, 8)?;
    let mut v = [0; 8];
    v.copy_from_slice(b);
    Ok(u64::from_le_bytes(v))
}

// ---------------------------------------------------------------------------------------------
// ELF

/// File-backed bytes at virtual address `va`, up to the end of the section holding it.
fn elf_data_at<'d>(file: &ElfFile64<'d, LE>, va: u64) -> Result<&'d [u8], ResLibError> {
    let s = file
        .sections()
        .find(|s| s.address() != 0 && va >= s.address() && va - s.address() < s.size())
        .ok_or_else(|| ResLibError(format!("address {va:#x} is in no section")))?;
    // `data()` checks the section's file span against the file
    let data: &'d [u8] = s.data()?;
    usize::try_from(va - s.address())
        .ok()
        .and_then(|off| data.get(off..))
        .filter(|d| !d.is_empty())
        .ok_or_else(|| ResLibError(format!("address {va:#x} has no file data")))
}

/// `(id, raw bytes)` of the `StringRecords[]` array of a little-endian ELF64 library; the text is
/// the library's own code page (cp1252).
pub fn elf_string_records(so: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, ResLibError> {
    if FileKind::parse(so).ok() != Some(FileKind::Elf64) {
        return err("not a 64-bit ELF library");
    }
    let file = ElfFile64::<LE>::parse(so)
        .map_err(|e| ResLibError(format!("not a little-endian 64-bit ELF library: {e}")))?;

    // the `StringRecords` symbol, from the dynamic symbols (exported) or the full table
    let name: &[u8] = b"StringRecords";
    let (addr, size) = file
        .dynamic_symbols()
        .chain(file.symbols())
        .find(|s| !s.is_undefined() && s.name_bytes().ok() == Some(name))
        .map(|s| (s.address(), s.size()))
        .ok_or_else(|| ResLibError("no StringRecords symbol".into()))?;
    let len = usize::try_from(size).map_err(|_| ResLibError("StringRecords too large".into()))?;
    let records = bytes(elf_data_at(&file, addr)?, 0, len)?;

    // the loader's fill-ins for the pointer slots, by offset into the array
    let mut relocated = HashMap::new();
    for (at, r) in file.dynamic_relocations().into_iter().flatten() {
        let r_type = match r.flags() {
            RelocationFlags::Elf { r_type } => r_type,
            _ => continue,
        };
        if at >= addr
            && at - addr < size
            && (r_type == elf::R_X86_64_RELATIVE || r_type == elf::R_AARCH64_RELATIVE)
        {
            relocated.insert(at - addr, r.addend() as u64);
        }
    }

    let mut out = Vec::with_capacity(records.len() / 16);
    for i in 0..records.len() / 16 {
        let rec = i * 16;
        let id = u32_at(records, rec)?;
        let ptr = match relocated.get(&(rec as u64 + 8)) {
            Some(&p) => p,
            None => u64_at(records, rec + 8)?,
        };
        if ptr == 0 {
            return err(format!("record {i} (id {id}) has no string"));
        }
        let tail =
            elf_data_at(&file, ptr).map_err(|e| ResLibError(format!("string of id {id}: {e}")))?;
        let len = tail
            .iter()
            .position(|&c| c == 0)
            .ok_or_else(|| ResLibError(format!("string of id {id} is not terminated")))?;
        out.push((id, tail[..len].to_vec()));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// PE

const LANG_EN_US: u16 = 0x409;

/// `(id, text)` of the `RT_STRING` resources of a PE image, 32- or 64-bit (one language: US
/// English when present, else the first listed).
pub fn pe_string_table(dll: &[u8]) -> Result<Vec<(u32, String)>, ResLibError> {
    match FileKind::parse(dll) {
        Ok(FileKind::Pe32) => pe_strings::<pe::ImageNtHeaders32>(dll),
        Ok(FileKind::Pe64) => pe_strings::<pe::ImageNtHeaders64>(dll),
        _ => err("not a PE file"),
    }
}

/// The entry of resource table `t` with numeric id `id`.
fn res_entry<'d>(
    t: &ResourceDirectoryTable<'d>,
    id: u16,
) -> Option<&'d pe::ImageResourceDirectoryEntry> {
    t.entries.iter().find(|e| e.name_or_id().id() == Some(id))
}

/// The subtable a resource entry points at.
fn res_table<'d>(
    e: &pe::ImageResourceDirectoryEntry,
    rsrc: ResourceDirectory<'d>,
) -> Result<ResourceDirectoryTable<'d>, ResLibError> {
    match e.data(rsrc)? {
        ResourceDirectoryEntryData::Table(t) => Ok(t),
        ResourceDirectoryEntryData::Data(_) => err("resource entry is not a directory"),
    }
}

fn pe_strings<Pe: ImageNtHeaders>(dll: &[u8]) -> Result<Vec<(u32, String)>, ResLibError> {
    let file = PeFile::<Pe>::parse(dll)?;
    let sections = file.section_table();
    let rsrc = file
        .data_directories()
        .resource_directory(dll, &sections)?
        .ok_or_else(|| ResLibError("no resources".into()))?;
    let root = rsrc.root()?;
    let Some(strings) = res_entry(&root, pe::RT_STRING) else {
        return err("no string table resources");
    };
    let mut out = Vec::new();
    for block in res_table(strings, rsrc)?.entries {
        let Some(block_id) = block.name_or_id().id().filter(|&b| b != 0) else {
            continue;
        };
        let langs = res_table(block, rsrc)?;
        let Some(lang) = res_entry(&langs, LANG_EN_US).or_else(|| langs.entries.first()) else {
            continue;
        };
        let entry = match lang.data(rsrc)? {
            ResourceDirectoryEntryData::Data(d) => d,
            ResourceDirectoryEntryData::Table(_) => return err("string block is a directory"),
        };
        let rva = entry.offset_to_data.get(LE);
        let blob = sections
            .pe_data_at(dll, rva)
            .ok_or_else(|| ResLibError(format!("RVA {rva:#x} is in no section")))?;
        let blob = bytes(blob, 0, entry.size.get(LE) as usize)?;
        // the block: sixteen strings, each a u16 length and that many UTF-16 units
        let mut p = 0;
        for i in 0..16u32 {
            let len = usize::from(u16_at(blob, p)?);
            p += 2;
            let units: Vec<u16> = bytes(blob, p, len * 2)?
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            p += len * 2;
            if len > 0 {
                let id = (u32::from(block_id) - 1) * 16 + i;
                out.push((id, String::from_utf16_lossy(&units)));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal PE32 (`pe32`) or PE32+ image with one `.rsrc` section holding the given string
    /// blocks.
    pub(crate) fn pe_with_strings(blocks: &[(u32, &[&str])], pe32: bool) -> Vec<u8> {
        const RSRC_RVA: u32 = 0x1000;
        // resource tree: root -> RT_STRING -> blocks -> language 0x409 -> data entries
        let mut rsrc = Vec::new();
        let dir = |out: &mut Vec<u8>, entries: &[(u32, u32)]| {
            out.extend_from_slice(&[0; 12]);
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for (id, off) in entries {
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&off.to_le_bytes());
            }
        };
        let n = blocks.len() as u32;
        let type_dir = 16 + 8;
        let block_dir = type_dir + 16 + 8 * n;
        let lang_dirs = block_dir;
        let data_entries = lang_dirs + n * (16 + 8);
        let blobs = data_entries + n * 16;
        dir(
            &mut rsrc,
            &[(u32::from(pe::RT_STRING), 0x8000_0000 | type_dir)],
        );
        let block_entries: Vec<(u32, u32)> = (0..n)
            .map(|i| (blocks[i as usize].0, 0x8000_0000 | (lang_dirs + i * 24)))
            .collect();
        // the type directory lists the blocks; their language directories follow it
        let mut type_part = Vec::new();
        dir(&mut type_part, &block_entries);
        rsrc.extend_from_slice(&type_part);
        assert_eq!(rsrc.len() as u32, lang_dirs);
        for i in 0..n {
            dir(&mut rsrc, &[(u32::from(LANG_EN_US), data_entries + i * 16)]);
        }
        let mut payloads = Vec::new();
        for (_, strings) in blocks {
            let mut b = Vec::new();
            for i in 0..16 {
                let units: Vec<u16> = strings
                    .get(i)
                    .copied()
                    .unwrap_or("")
                    .encode_utf16()
                    .collect();
                b.extend_from_slice(&(units.len() as u16).to_le_bytes());
                for u in units {
                    b.extend_from_slice(&u.to_le_bytes());
                }
            }
            payloads.push(b);
        }
        let mut at = blobs;
        for p in &payloads {
            rsrc.extend_from_slice(&(RSRC_RVA + at).to_le_bytes());
            rsrc.extend_from_slice(&(p.len() as u32).to_le_bytes());
            rsrc.extend_from_slice(&[0; 8]);
            at += p.len() as u32;
        }
        for p in &payloads {
            rsrc.extend_from_slice(p);
        }

        let mut f = vec![0u8; 0x200];
        f[..2].copy_from_slice(b"MZ");
        f[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        f[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        f[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        // the optional header: PE32 (the retail DLL) or PE32+, then sixteen data directories
        let (magic, fixed) = if pe32 { (0x10bu16, 96) } else { (0x20b, 112) };
        let opt_size = fixed + 16 * 8;
        f[coff + 16..coff + 18].copy_from_slice(&(opt_size as u16).to_le_bytes());
        let opt = coff + 20;
        f[opt..opt + 2].copy_from_slice(&magic.to_le_bytes());
        f[opt + fixed - 4..opt + fixed].copy_from_slice(&16u32.to_le_bytes());
        let res_dir = opt + fixed + 2 * 8;
        f[res_dir..res_dir + 4].copy_from_slice(&RSRC_RVA.to_le_bytes());
        f[res_dir + 4..res_dir + 8].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        let sh = opt + opt_size;
        f[sh..sh + 5].copy_from_slice(b".rsrc");
        f[sh + 8..sh + 12].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        f[sh + 12..sh + 16].copy_from_slice(&RSRC_RVA.to_le_bytes());
        f[sh + 16..sh + 20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        f[sh + 20..sh + 24].copy_from_slice(&0x200u32.to_le_bytes());
        f.extend_from_slice(&rsrc);
        f
    }

    #[test]
    fn reads_pe_string_blocks() {
        // block 2 holds ids 16..32; block 1563 holds 24992..25008
        let blocks: &[(u32, &[&str])] = &[(2, &["", "first", "", "caf\u{e9}"]), (1563, &["x"])];
        let want = vec![
            (17, "first".to_string()),
            (19, "caf\u{e9}".to_string()),
            (24992, "x".to_string()),
        ];
        for pe32 in [true, false] {
            let dll = pe_with_strings(blocks, pe32);
            assert_eq!(pe_string_table(&dll).unwrap(), want, "pe32 {pe32}");
            // a string block cut short is an error, not a panic
            let cut = &dll[..dll.len() - 3];
            assert!(pe_string_table(cut).is_err(), "pe32 {pe32}");
        }
        assert!(pe_string_table(b"MZ").is_err());
        assert!(pe_string_table(b"\x7fELF").is_err());
    }

    /// A minimal ELF64 library: `StringRecords` (in `.data` at 0x3000) holds one record
    /// `{ 7, 0x2000 }`, and `.rodata` (0x2000) holds "hello" at file offset `rodata_offset`
    /// (0x40 for a well-formed file).
    pub(crate) fn elf_with_record(rodata_offset: u64) -> Vec<u8> {
        let mut f = vec![0u8; 0x98 + 6 * 64];
        let put = |f: &mut Vec<u8>, at: usize, v: &[u8]| f[at..at + v.len()].copy_from_slice(v);
        put(&mut f, 0, b"\x7fELF\x02\x01\x01");
        put(&mut f, 0x10, &3u16.to_le_bytes()); // ET_DYN
        put(&mut f, 0x12, &62u16.to_le_bytes()); // EM_X86_64
        put(&mut f, 0x14, &1u32.to_le_bytes());
        put(&mut f, 0x28, &0x98u64.to_le_bytes()); // e_shoff
        put(&mut f, 0x34, &64u16.to_le_bytes());
        put(&mut f, 0x36, &56u16.to_le_bytes());
        put(&mut f, 0x3a, &64u16.to_le_bytes());
        put(&mut f, 0x3c, &6u16.to_le_bytes());
        put(&mut f, 0x3e, &4u16.to_le_bytes()); // .shstrtab
        put(&mut f, 0x40, b"hello\0");
        put(&mut f, 0x48, &7u32.to_le_bytes());
        put(&mut f, 0x50, &0x2000u64.to_le_bytes());
        put(&mut f, 0x58, b"\0StringRecords\0");
        // .symtab: the null symbol, then StringRecords (global object in section 2)
        put(&mut f, 0x68 + 24, &1u32.to_le_bytes());
        put(&mut f, 0x68 + 24 + 4, &[0x11, 0]);
        put(&mut f, 0x68 + 24 + 6, &2u16.to_le_bytes());
        put(&mut f, 0x68 + 24 + 8, &0x3000u64.to_le_bytes());
        put(&mut f, 0x68 + 24 + 16, &16u64.to_le_bytes());
        // (type, flags, addr, offset, size, link, entsize) of sections 1..=5
        let sections: [(u32, u64, u64, u64, u64, u32, u64); 5] = [
            (1, 2, 0x2000, rodata_offset, 6, 0, 0), // .rodata
            (1, 3, 0x3000, 0x48, 16, 0, 0),         // .data
            (3, 0, 0, 0x58, 15, 0, 0),              // .strtab
            (3, 0, 0, 0x67, 1, 0, 0),               // .shstrtab
            (2, 0, 0, 0x68, 48, 3, 24),             // .symtab
        ];
        for (i, (kind, flags, addr, offset, size, link, entsize)) in sections.iter().enumerate() {
            let h = 0x98 + (i + 1) * 64;
            put(&mut f, h + 4, &kind.to_le_bytes());
            put(&mut f, h + 8, &flags.to_le_bytes());
            put(&mut f, h + 16, &addr.to_le_bytes());
            put(&mut f, h + 24, &offset.to_le_bytes());
            put(&mut f, h + 32, &size.to_le_bytes());
            put(&mut f, h + 40, &link.to_le_bytes());
            put(&mut f, h + 44, &u32::from(*kind == 2).to_le_bytes());
            put(&mut f, h + 56, &entsize.to_le_bytes());
        }
        f
    }

    #[test]
    fn reads_elf_string_records() {
        let so = elf_with_record(0x40);
        assert_eq!(
            elf_string_records(&so).unwrap(),
            vec![(7, b"hello".to_vec())]
        );
    }

    #[test]
    fn an_elf_string_section_outside_the_file_is_an_error() {
        // `.rodata` claims file bytes past the end of the file: an error, never a panic
        for offset in [0x10_0000, u64::MAX - 2, 0x98 + 6 * 64 - 2] {
            let so = elf_with_record(offset);
            let e = elf_string_records(&so).unwrap_err();
            assert!(e.0.contains("string of id 7"), "{offset:#x}: {e}");
        }
        // so is a records array cut off by the end of the file
        let mut so = elf_with_record(0x40);
        so.truncate(0x50);
        assert!(elf_string_records(&so).is_err());
    }

    #[test]
    fn rejects_what_is_not_a_library() {
        assert!(elf_string_records(b"").is_err());
        assert!(elf_string_records(b"MZ\0\0").is_err());
        let mut elf32 = vec![0u8; 64];
        elf32[..4].copy_from_slice(b"\x7fELF");
        elf32[4] = 1;
        assert!(elf_string_records(&elf32).is_err());
    }
}
