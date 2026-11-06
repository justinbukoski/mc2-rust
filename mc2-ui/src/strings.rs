//! The game's UI string table (`cLoadString`, `gos_OpenResourceDLL("mc2res.dll")`).
//!
//! The strings are not in the `.fst` archives: the original links them into its resource library
//! (`mc2res.dll` on Windows; the Linux port's `libmc2res_64.so`, whose `res/libmain.cpp` builds
//! an id -> string map from `StringRecords[]`). Nothing of that text is part of this program: at
//! start-up [`StringTable::init_from_data_root`] reads the table out of the library in the
//! player's own data folder (see [`crate::res_lib`]), and [`StringTable::builtin`] serves it from
//! then on.
//!
//! The library's source is `test_scripts/res_conv/strings.res.cpp` with the ids of
//! `strings.res.h`; both were converted from Microsoft's resource script `Script1.rc` +
//! `resource.h` by `res_conv.pl`. [`generate`] converts that pair into the test fixture
//! `tests/fixtures/mc2res-strings.txt` (`cargo run -p mc2-ui --example gen_strings`), which is
//! read by tests only, never compiled in, and left out of the public export. [`parse_rc`] reads
//! the original resource script for cross-checks.
//!
//! The converted table adds one entry of its own, `IDS_undefined_string_sebi` (id 0); it is left
//! out, so id 0 has no string as in the original DLL. On duplicate ids the first record wins, as
//! `std::map::insert` keeps the first.

use crate::cp1252;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringError(pub String);

impl std::fmt::Display for StringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "string table: {}", self.0)
    }
}

impl std::error::Error for StringError {}

/// The resource libraries looked for in the data folder, in order (names compared without case).
/// The first-run message names only the retail `mc2res.dll`; the Linux data set's
/// `libmc2res_64.so` is accepted silently.
pub const RESOURCE_LIBRARIES: [&str; 2] = ["libmc2res_64.so", "mc2res.dll"];

/// The table served by [`StringTable::builtin`].
static TABLE: OnceLock<StringTable> = OnceLock::new();

/// Resource string id -> text.
#[derive(Debug, Clone, Default)]
pub struct StringTable {
    by_id: HashMap<u32, String>,
}

impl StringTable {
    /// The game's table: the one [`StringTable::init_from_data_root`] loaded. Before that (tools
    /// and tests) it is loaded once from `$MC2_DATA`; without game data it is empty, so every
    /// lookup misses.
    pub fn builtin() -> &'static StringTable {
        TABLE.get_or_init(|| {
            if let Some(root) = std::env::var_os("MC2_DATA").filter(|s| !s.is_empty()) {
                if let Ok(t) = StringTable::from_data_root(Path::new(&root)) {
                    return t;
                }
            }
            #[cfg(test)]
            if let Some(t) = StringTable::fixture() {
                return t;
            }
            StringTable::default()
        })
    }

    /// [`StringTable::builtin`] for a test that checks the game's own text, or `None` (after the
    /// usual skip notice) when there is no game data and no strings fixture.
    pub fn builtin_or_skip() -> Option<&'static StringTable> {
        let t = Self::builtin();
        if t.is_empty() {
            eprintln!("no game data (MC2_DATA) or strings fixture; skipping");
            return None;
        }
        Some(t)
    }

    /// Loads the table from the player's data folder and makes it the one
    /// [`StringTable::builtin`] serves. Errors carry the first-run message.
    pub fn init_from_data_root(root: &Path) -> Result<&'static StringTable, String> {
        let table = StringTable::from_data_root(root)?;
        Ok(TABLE.get_or_init(|| table))
    }

    /// Reads the table out of the first resource library of [`RESOURCE_LIBRARIES`] in `root`.
    /// Errors carry the first-run message.
    pub fn from_data_root(root: &Path) -> Result<StringTable, String> {
        let Some(lib) = find_resource_library(root) else {
            return Err(missing_library_message(root));
        };
        let bytes = std::fs::read(&lib).map_err(|e| {
            format!(
                "{} game data incomplete: cannot read {}: {e}",
                mc2_platform::PRODUCT_NAME,
                lib.display()
            )
        })?;
        let table = StringTable::from_library(&bytes).map_err(|e| {
            format!(
                "{} game data incomplete: {} is not a usable string resource library: {e}",
                mc2_platform::PRODUCT_NAME,
                lib.display()
            )
        })?;
        if table.is_empty() {
            return Err(format!(
                "{} game data incomplete: {} holds no strings",
                mc2_platform::PRODUCT_NAME,
                lib.display()
            ));
        }
        Ok(table)
    }

    /// The table of a resource library's bytes: an ELF `libmc2res` or a PE `mc2res.dll`.
    pub fn from_library(bytes: &[u8]) -> Result<StringTable, crate::res_lib::ResLibError> {
        let pairs: Vec<(u32, String)> = if bytes.starts_with(b"MZ") {
            crate::res_lib::pe_string_table(bytes)?
        } else {
            crate::res_lib::elf_string_records(bytes)?
                .into_iter()
                .map(|(id, s)| (id, cp1252::decode(&s)))
                .collect()
        };
        Ok(StringTable::from_pairs(
            pairs.into_iter().filter(|&(id, _)| id != 0),
        ))
    }

    /// The test fixture converted from the C++ string table (`tests/fixtures`), when the
    /// checkout has it.
    #[cfg(test)]
    pub(crate) fn fixture() -> Option<StringTable> {
        let text = std::fs::read_to_string(fixture_path()).ok()?;
        Some(StringTable::from_pairs(parse_fixture(&text).ok()?))
    }

    /// Apply the running product identity while retaining the original resource table.
    pub(crate) fn for_product() -> StringTable {
        let mut table = Self::builtin().clone();
        // Keep historical references, original media, and legal notices intact.
        for id in [
            26006, 26007, 26008, 26011, 26014, 26017, 26019, 26083, 26378, 26620, 27615, 27702,
            27703, 27704, 27706, 27737, 30853, 43019, 48000, 60024, 60025, 60026, 60031, 60035,
            60037, 60040, 60041, 60043, 60044, 60047,
        ] {
            if let Some(value) = table.by_id.get_mut(&id) {
                *value = value
                    .replace("MechCommander 2", mc2_platform::PRODUCT_NAME)
                    .replace("MechComnder 2", mc2_platform::PRODUCT_NAME);
            }
        }
        table
    }

    /// Builds a table; the first entry of a duplicate id wins.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (u32, String)>) -> StringTable {
        let mut by_id = HashMap::new();
        for (id, s) in pairs {
            by_id.entry(id).or_insert(s);
        }
        StringTable { by_id }
    }

    /// The string, or `None` when the id has no entry.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    /// `cLoadString`: the string, or an empty string when the id has no entry.
    pub fn load(&self, id: u32) -> &str {
        self.get(id).unwrap_or("")
    }

    /// Replaces or adds a string (localisation, tests).
    pub fn set(&mut self, id: u32, s: &str) {
        self.by_id.insert(id, s.to_string());
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// All `(id, text)` pairs in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> + '_ {
        self.by_id.iter().map(|(&k, v)| (k, v.as_str()))
    }
}

// ---------------------------------------------------------------------------------------------
// conversion of strings.res.h / strings.res.cpp

/// `NAME = value` entries of the `eStringIds` enum in `strings.res.h`.
pub fn parse_enum(header: &str) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    for line in header.lines() {
        let t = line.trim().trim_end_matches(',');
        let Some((name, value)) = t.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !name.starts_with("IDS_") {
            continue;
        }
        if let Ok(v) = value.trim().parse::<u32>() {
            out.insert(name.to_string(), v);
        }
    }
    out
}

/// Decodes a C string literal body (between the quotes); `None` on a bad escape.
fn c_unescape(body: &str) -> Option<String> {
    let mut out = String::with_capacity(body.len());
    let mut it = body.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next()? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '0' => out.push('\0'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            _ => return None,
        }
    }
    Some(out)
}

/// `{ NAME, "text" },` records of `StringRecords[]` in `strings.res.cpp`, in file order.
pub fn parse_records(cpp: &str) -> Result<Vec<(String, String)>, StringError> {
    let mut out = Vec::new();
    for (n, line) in cpp.lines().enumerate() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix('{') else {
            continue;
        };
        let Some((name, rest)) = rest.split_once(',') else {
            continue;
        };
        let rest = rest.trim();
        let body = rest
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix("},"))
            .map(str::trim_end)
            .and_then(|r| r.strip_suffix('"'))
            .ok_or_else(|| StringError(format!("line {}: malformed record", n + 1)))?;
        let text =
            c_unescape(body).ok_or_else(|| StringError(format!("line {}: bad escape", n + 1)))?;
        out.push((name.trim().to_string(), text));
    }
    Ok(out)
}

/// The id -> string pairs the C++ resource library serves, sorted by id.
pub fn convert(header: &[u8], cpp: &[u8]) -> Result<Vec<(u32, String)>, StringError> {
    let ids = parse_enum(&cp1252::decode(header));
    let records = parse_records(&cp1252::decode(cpp))?;
    let mut seen = HashMap::new();
    for (name, text) in records {
        if name == "IDS_undefined_string_sebi" {
            continue;
        }
        let id = *ids
            .get(&name)
            .ok_or_else(|| StringError(format!("{name} has no id in strings.res.h")))?;
        seen.entry(id).or_insert(text);
    }
    let mut v: Vec<(u32, String)> = seen.into_iter().collect();
    v.sort_by_key(|p| p.0);
    Ok(v)
}

/// The test fixture text (`tests/fixtures/mc2res-strings.txt`) for the C++ table.
pub fn generate(header: &[u8], cpp: &[u8]) -> Result<String, StringError> {
    Ok(fixture_text(&convert(header, cpp)?))
}

/// The fixture format: a `#` header, then one `id<TAB>text` line per string sorted by id, the text
/// UTF-8 with `\\`, `\n`, `\r`, `\t` and `\0` escaped.
pub fn fixture_text(pairs: &[(u32, String)]) -> String {
    let mut s = String::from(
        "# @generated by `cargo run -p mc2-ui --example gen_strings` from\n\
         # test_scripts/res_conv/strings.res.h and strings.res.cpp (the mc2res string table).\n\
         # Test fixture only: never compiled into a build, left out of the public export.\n",
    );
    for (id, text) in pairs {
        s.push_str(&id.to_string());
        s.push('\t');
        for c in text.chars() {
            match c {
                '\\' => s.push_str("\\\\"),
                '\n' => s.push_str("\\n"),
                '\r' => s.push_str("\\r"),
                '\t' => s.push_str("\\t"),
                '\0' => s.push_str("\\0"),
                c => s.push(c),
            }
        }
        s.push('\n');
    }
    s
}

/// Reads [`fixture_text`] back.
pub fn parse_fixture(text: &str) -> Result<Vec<(u32, String)>, StringError> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let bad = || StringError(format!("fixture line {}: malformed", n + 1));
        let (id, body) = line.split_once('\t').ok_or_else(bad)?;
        let id = id.parse::<u32>().map_err(|_| bad())?;
        out.push((id, c_unescape(body).ok_or_else(bad)?));
    }
    Ok(out)
}

/// Where the fixture lives in the checkout.
#[cfg(test)]
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mc2res-strings.txt")
}

/// The resource library in `root` (the first of [`RESOURCE_LIBRARIES`], names without case).
pub fn find_resource_library(root: &Path) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    RESOURCE_LIBRARIES.iter().find_map(|want| {
        entries
            .iter()
            .find(|p| {
                p.is_file()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case(want))
            })
            .cloned()
    })
}

/// The first-run text when the data folder has no string resource library.
pub fn missing_library_message(root: &Path) -> String {
    format!(
        "{} game data incomplete: the string resource library mc2res.dll was not found in {}.\n\
         The game's text is read from your own copy of the game: copy mc2res.dll from your \
         retail installation into this folder, or point mc2 at a complete data folder with \
         --data, MC2_DATA or mc2.cfg.\n",
        mc2_platform::PRODUCT_NAME,
        root.display()
    )
}

// ---------------------------------------------------------------------------------------------
// the original resource script

/// `#define NAME number` lines. Block comments are not honoured: in the repository copies of
/// `resource.h` the string ids sit inside a `/* */` block (alariq's build takes them from
/// `strings.res.h` instead), and the cross-check needs them.
pub fn parse_defines(src: &str) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    for line in src.lines() {
        let line = line.split("//").next().unwrap_or("");
        let mut it = line.split_whitespace();
        if it.next() != Some("#define") {
            continue;
        }
        if let (Some(name), Some(value)) = (it.next(), it.next()) {
            if let Ok(v) = value.parse::<u32>() {
                out.insert(name.to_string(), v);
            }
        }
    }
    out
}

/// The `STRINGTABLE` entries of a resource script (`rc.exe` syntax: `""` is a quote, C escapes,
/// a string may sit on the line after its name), resolved through `resource.h`.
pub fn parse_rc(script: &[u8], header: &[u8]) -> Result<Vec<(u32, String)>, StringError> {
    let ids = parse_defines(&cp1252::decode(header));
    let src = cp1252::decode(script);
    let mut out = Vec::new();
    let mut lines = src.lines();
    while let Some(line) = lines.next() {
        if line.split_whitespace().next() != Some("STRINGTABLE") {
            continue;
        }
        for l in lines.by_ref() {
            if matches!(l.trim(), "BEGIN" | "{") {
                break;
            }
        }
        let mut pending: Option<String> = None;
        for l in lines.by_ref() {
            let t = l.trim();
            if t == "END" || t == "}" {
                break;
            }
            if t.is_empty() {
                continue;
            }
            let (name, rest) = match pending.take() {
                Some(n) => (n, t),
                None => {
                    let end = t.find(char::is_whitespace).unwrap_or(t.len());
                    (t[..end].to_string(), t[end..].trim_start())
                }
            };
            if rest.is_empty() {
                pending = Some(name);
                continue;
            }
            let text =
                rc_literal(rest).ok_or_else(|| StringError(format!("bad string for {name}")))?;
            // `ID_*` entries are the editor's menu prompts; the game only uses `IDS_*`
            if !name.starts_with("IDS_") {
                continue;
            }
            if let Some(&id) = ids.get(&name) {
                out.push((id, text));
            }
        }
    }
    Ok(out)
}

fn rc_literal(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' if chars.peek() == Some(&'"') => {
                chars.next();
                out.push('"');
            }
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '0' => out.push('\0'),
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_file(rel: &str) -> Option<Vec<u8>> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../").to_string() + rel).ok()
    }

    /// The fixture, or `None` (with the skip note) in a checkout without it.
    fn fixture() -> Option<StringTable> {
        let t = StringTable::fixture();
        if t.is_none() {
            eprintln!("{} not found; skipping", fixture_path().display());
        }
        t
    }

    #[test]
    fn converts_records() {
        let h = b"enum eStringIds {\n    IDS_undefined_string_sebi = 0,\n\tIDS_A\t = 10,\n\tIDS_B\t = 20\n};\n";
        let cpp = b"StringRecords[] = {\n    { IDS_undefined_string_sebi, \"undefined_string_sebi\" },\n\t{ IDS_B, \"say \\\"hi\\\"\\nVictor\x92s\" },\n\t{ IDS_A, \"one\" },\n\t{ IDS_A, \"dup\" },\n};\n";
        let pairs = convert(h, cpp).unwrap();
        assert_eq!(
            pairs,
            vec![
                (10, "one".into()),
                (20, "say \"hi\"\nVictor\u{2019}s".into())
            ]
        );
        let src = generate(h, cpp).unwrap();
        assert!(src.contains("\n20\tsay \"hi\"\\nVictor\u{2019}s\n"));
        assert_eq!(parse_fixture(&src).unwrap(), pairs);
        let odd = vec![(1, "a\\b\tc\r\0".to_string())];
        assert_eq!(parse_fixture(&fixture_text(&odd)).unwrap(), odd);
        assert!(parse_fixture("x\ty\n").is_err());
        assert!(convert(b"", cpp).is_err());
    }

    #[test]
    fn rc_syntax() {
        let h = b"#define IDS_A 10\r\n#define IDS_B 20 // two\r\n";
        let rc = b"STRINGTABLE DISCARDABLE\r\nBEGIN\r\n    IDS_A  \"a \"\"q\"\" \\\"r\\\"\"\r\n    IDS_B\r\n       \"two\\nlines\"\r\n    IDS_C \"x\"\r\nEND\r\n";
        let v = parse_rc(rc, h).unwrap();
        assert_eq!(
            v,
            vec![(10, "a \"q\" \"r\"".into()), (20, "two\nlines".into())]
        );
    }

    #[test]
    fn fixture_matches_the_cpp_source() {
        let (Some(h), Some(cpp)) = (
            repo_file("test_scripts/res_conv/strings.res.h"),
            repo_file("test_scripts/res_conv/strings.res.cpp"),
        ) else {
            eprintln!("strings.res.* not found; skipping");
            return;
        };
        let Ok(checked_in) = std::fs::read_to_string(fixture_path()) else {
            eprintln!("{} not found; skipping", fixture_path().display());
            return;
        };
        assert!(
            generate(&h, &cpp).unwrap() == checked_in,
            "the strings fixture is stale: run `cargo run -p mc2-ui --example gen_strings`"
        );
    }

    #[test]
    fn fixture_agrees_with_the_original_resource_script() {
        let (Some(rc), Some(h)) = (
            repo_file("test_scripts/res_conv/Script1.rc"),
            repo_file("test_scripts/res_conv/resource.h"),
        ) else {
            return;
        };
        let Some(t) = fixture() else { return };
        let rc = parse_rc(&rc, &h).unwrap();
        let mut differ = Vec::new();
        for (id, s) in &rc {
            if t.get(*id) != Some(s.as_str()) {
                differ.push(*id);
            }
        }
        assert!(
            differ.is_empty(),
            "{} differ, e.g. {:?}",
            differ.len(),
            &differ[..differ.len().min(5)]
        );
        assert!(rc.len() >= t.len() - 6, "rc {} table {}", rc.len(), t.len());
    }

    #[test]
    fn builtin_table_has_the_menu_strings() {
        let t = StringTable::builtin();
        if t.is_empty() {
            eprintln!("no game data or strings fixture; skipping");
            return;
        }
        assert!(t.len() > 3900, "only {} strings", t.len());
        assert_eq!(t.load(27616).to_ascii_lowercase(), "agencyfb17.d3f");
        assert_eq!(t.load(27630).to_ascii_lowercase(), "agencyfb11.d3f");
        assert_eq!(t.get(0), None);
        assert!(!t.load(27600).is_empty());
    }

    #[test]
    fn the_players_library_serves_the_fixture_table() {
        let Some(root) = std::env::var_os("MC2_DATA") else {
            eprintln!("MC2_DATA not set; skipping shipped-data test");
            return;
        };
        let Some(fixture) = fixture() else { return };
        let loaded = StringTable::from_data_root(Path::new(&root)).unwrap();
        assert_eq!(loaded.len(), fixture.len(), "id count");
        let mut differ: Vec<u32> = fixture
            .iter()
            .filter(|&(id, s)| loaded.get(id) != Some(s))
            .map(|(id, _)| id)
            .collect();
        differ.sort_unstable();
        assert!(
            differ.is_empty(),
            "{} differ, e.g. {:?}",
            differ.len(),
            &differ[..differ.len().min(5)]
        );
        assert_eq!(loaded.get(0), None);
    }

    #[test]
    fn a_data_folder_without_the_library_is_a_first_run_error() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-data-folder");
        let e = StringTable::from_data_root(&root).unwrap_err();
        assert!(
            e.contains(
                "game data incomplete: the string resource library mc2res.dll was not found"
            ),
            "{e}"
        );
        assert!(
            e.contains("copy mc2res.dll from your retail installation"),
            "{e}"
        );
        assert!(!e.contains("libmc2res"), "{e}");
        assert!(e.contains(&root.display().to_string()), "{e}");
        // the folder of this crate has no library either
        assert_eq!(
            find_resource_library(Path::new(env!("CARGO_MANIFEST_DIR"))),
            None
        );
        assert!(StringTable::from_library(b"not a library").is_err());
    }

    #[test]
    fn a_retail_dll_or_the_linux_library_in_the_data_folder_is_read() {
        use crate::res_lib::tests::{elf_with_record, pe_with_strings};
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target")
            .join(format!("strings-lib-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // the retail DLL (a 32-bit image), its name in any case
        let dll = dir.join("MC2RES.DLL");
        std::fs::write(&dll, pe_with_strings(&[(2, &["", "first"])], true)).unwrap();
        let t = StringTable::from_data_root(&dir).unwrap();
        assert_eq!((t.len(), t.get(17)), (1, Some("first")));
        // the Linux data set's library, accepted without a word
        std::fs::remove_file(&dll).unwrap();
        std::fs::write(dir.join("libmc2res_64.so"), elf_with_record(0x40)).unwrap();
        let t = StringTable::from_data_root(&dir).unwrap();
        assert_eq!((t.len(), t.get(7)), (1, Some("hello")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
