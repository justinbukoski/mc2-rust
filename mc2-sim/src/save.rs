//! Save/load: a NEW Rust format (the original `.ims` format is not read or written).
//!
//! A save is a versioned, deterministic little-endian byte string of the **dynamic** state of a [`World`]. There is no reflection
//! and no pointers: objects refer to each other by handle and watch id, which makes this a straight copy of each field. Static
//! data that is built from the game files at mission start (object types, chassis, pilot profiles, appearance shape tables,
//! animation types, the compiled ABL code, terrain geometry) is not written. [`load_world`] takes a freshly initialised world of
//! the same mission (what the original does too: it loads the mission, then the savegame over it) and overlays the saved state
//! on it.
//!
//! Each state type implements [`Snapshot`]. Structs are declared with [`snapshot_struct!`], whose generated `load` names every
//! field, so a field that is added later without a save entry does not compile. A field that is static (an `Rc` table, a debug
//! hook, a loader cache) is listed as `field => expr`: it is not written, [`Snapshot::load`] sets it to `expr` and
//! [`Snapshot::load_into`] (the overlay used on the base world) leaves it alone.
//!
//! The state inventory is DESIGN.md section 9. The acceptance test is: save at frame k, load, run to N; the digest must be
//! identical to the uninterrupted run (`tests/save_roundtrip.rs`).

use std::fmt;

/// Snapshot format version. Bump it when the layout of any saved type changes.
pub const SAVE_VERSION: u32 = 17;

/// File magic.
const MAGIC: &[u8; 4] = b"MC2S";

/// A save that cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveError(pub String);

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SaveError {}

/// Result of reading a save.
pub type SaveResult<T> = Result<T, SaveError>;

/// Appends little-endian values to a save buffer.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// An empty buffer.
    pub fn new() -> Writer {
        Writer::default()
    }

    /// The bytes written so far.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// Raw bytes (no length prefix).
    pub fn raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// One byte.
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// `u16`, little-endian.
    pub fn u16(&mut self, v: u16) {
        self.raw(&v.to_le_bytes());
    }

    /// `u32`, little-endian.
    pub fn u32(&mut self, v: u32) {
        self.raw(&v.to_le_bytes());
    }

    /// `u64`, little-endian.
    pub fn u64(&mut self, v: u64) {
        self.raw(&v.to_le_bytes());
    }

    /// `i32`, little-endian.
    pub fn i32(&mut self, v: i32) {
        self.raw(&v.to_le_bytes());
    }

    /// `i64`, little-endian.
    pub fn i64(&mut self, v: i64) {
        self.raw(&v.to_le_bytes());
    }

    /// `f32` as its bit pattern (exact, NaN payloads included).
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }

    /// `f64` as its bit pattern.
    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    /// `bool` as one byte.
    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }

    /// A length prefix (`u32`).
    pub fn len(&mut self, n: usize) {
        let n = u32::try_from(n).expect("save: length exceeds u32");
        self.u32(n);
    }
}

/// Reads little-endian values from a save buffer.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Start reading at the beginning of `buf`.
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    fn fail<T>(&self, what: &str) -> SaveResult<T> {
        Err(SaveError(format!("save at byte {}: {what}", self.pos)))
    }

    /// `n` raw bytes.
    pub fn take(&mut self, n: usize) -> SaveResult<&'a [u8]> {
        if self.buf.len() - self.pos < n {
            return self.fail("truncated");
        }
        let bytes = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> SaveResult<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// One byte.
    pub fn u8(&mut self) -> SaveResult<u8> {
        Ok(self.take(1)?[0])
    }

    /// `u16`.
    pub fn u16(&mut self) -> SaveResult<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    /// `u32`.
    pub fn u32(&mut self) -> SaveResult<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// `u64`.
    pub fn u64(&mut self) -> SaveResult<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// `i32`.
    pub fn i32(&mut self) -> SaveResult<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    /// `i64`.
    pub fn i64(&mut self) -> SaveResult<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    /// `f32` from its bit pattern.
    pub fn f32(&mut self) -> SaveResult<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// `f64` from its bit pattern.
    pub fn f64(&mut self) -> SaveResult<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    /// `bool`; any byte other than 0 or 1 is an error.
    pub fn bool(&mut self) -> SaveResult<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => self.fail("bad bool"),
        }
    }

    /// A length prefix.
    pub fn length(&mut self) -> SaveResult<usize> {
        Ok(self.u32()? as usize)
    }

    /// Fails unless every byte has been read.
    pub fn finish(&self) -> SaveResult<()> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(SaveError(format!(
                "save: {} trailing bytes",
                self.buf.len() - self.pos
            )))
        }
    }
}

/// A type that can be written to and read from a save.
pub trait Snapshot: Sized {
    /// Append the deterministic encoding of `self`.
    fn save(&self, w: &mut Writer);
    /// Decode a fresh value from the front of the reader.
    fn load(r: &mut Reader<'_>) -> SaveResult<Self>;
    /// Overlay the saved value onto `self`. Fields that are static (not saved) keep their current value. The default replaces
    /// `self` with [`Snapshot::load`].
    fn load_into(&mut self, r: &mut Reader<'_>) -> SaveResult<()> {
        *self = Self::load(r)?;
        Ok(())
    }
}

/// Implements [`Snapshot`] for a struct by listing its fields in save order.
///
/// ```ignore
/// snapshot_struct!(Pilot {
///     slot,
///     wounds,
///     brain_cache => None,       // static or derived: not saved
/// });
/// ```
///
/// The generated `load` is a struct literal naming every field, so an unlisted field is a compile error. A `field => expr` entry
/// is written by nothing, is set to `expr` by `load`, and is left untouched by `load_into`.
#[macro_export]
macro_rules! snapshot_struct {
    ($ty:ty { $($body:tt)* }) => {
        impl $crate::save::Snapshot for $ty {
            fn save(&self, w: &mut $crate::save::Writer) {
                $crate::snapshot_struct!(@save self, w, $($body)*);
            }
            fn load(r: &mut $crate::save::Reader<'_>) -> $crate::save::SaveResult<Self> {
                $crate::snapshot_struct!(@build r; []; $($body)*)
            }
            fn load_into(&mut self, r: &mut $crate::save::Reader<'_>) -> $crate::save::SaveResult<()> {
                $crate::snapshot_struct!(@into self, r, $($body)*);
                Ok(())
            }
        }
    };
    (@save $s:ident, $w:ident, ) => {};
    (@save $s:ident, $w:ident, $f:ident => $e:expr) => {};
    (@save $s:ident, $w:ident, $f:ident) => {
        $crate::save::Snapshot::save(&$s.$f, $w);
    };
    (@save $s:ident, $w:ident, $f:ident => $e:expr, $($rest:tt)*) => {
        $crate::snapshot_struct!(@save $s, $w, $($rest)*);
    };
    (@save $s:ident, $w:ident, $f:ident, $($rest:tt)*) => {
        $crate::save::Snapshot::save(&$s.$f, $w);
        $crate::snapshot_struct!(@save $s, $w, $($rest)*);
    };
    (@build $r:ident; [$($acc:tt)*]; ) => {
        Ok(Self { $($acc)* })
    };
    (@build $r:ident; [$($acc:tt)*]; $f:ident => $e:expr) => {
        $crate::snapshot_struct!(@build $r; [$($acc)* $f: $e,]; )
    };
    (@build $r:ident; [$($acc:tt)*]; $f:ident) => {
        $crate::snapshot_struct!(@build $r; [$($acc)* $f: $crate::save::Snapshot::load($r)?,]; )
    };
    (@build $r:ident; [$($acc:tt)*]; $f:ident => $e:expr, $($rest:tt)*) => {
        $crate::snapshot_struct!(@build $r; [$($acc)* $f: $e,]; $($rest)*)
    };
    (@build $r:ident; [$($acc:tt)*]; $f:ident, $($rest:tt)*) => {
        $crate::snapshot_struct!(@build $r; [$($acc)* $f: $crate::save::Snapshot::load($r)?,]; $($rest)*)
    };
    (@into $s:ident, $r:ident, ) => {};
    (@into $s:ident, $r:ident, $f:ident => $e:expr) => {};
    (@into $s:ident, $r:ident, $f:ident) => {
        $crate::save::Snapshot::load_into(&mut $s.$f, $r)?;
    };
    (@into $s:ident, $r:ident, $f:ident => $e:expr, $($rest:tt)*) => {
        $crate::snapshot_struct!(@into $s, $r, $($rest)*);
    };
    (@into $s:ident, $r:ident, $f:ident, $($rest:tt)*) => {
        $crate::save::Snapshot::load_into(&mut $s.$f, $r)?;
        $crate::snapshot_struct!(@into $s, $r, $($rest)*);
    };
}

macro_rules! primitive {
    ($($ty:ty => $put:ident, $get:ident;)*) => {$(
        impl Snapshot for $ty {
            fn save(&self, w: &mut Writer) {
                w.$put(*self);
            }
            fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
                r.$get()
            }
        }
    )*};
}

primitive! {
    u8 => u8, u8;
    u16 => u16, u16;
    u32 => u32, u32;
    u64 => u64, u64;
    i32 => i32, i32;
    i64 => i64, i64;
    f32 => f32, f32;
    f64 => f64, f64;
    bool => bool, bool;
}

impl Snapshot for i8 {
    fn save(&self, w: &mut Writer) {
        w.u8(*self as u8);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(r.u8()? as i8)
    }
}

impl Snapshot for i16 {
    fn save(&self, w: &mut Writer) {
        w.u16(*self as u16);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(r.u16()? as i16)
    }
}

impl Snapshot for usize {
    fn save(&self, w: &mut Writer) {
        w.u64(*self as u64);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        usize::try_from(r.u64()?).map_err(|_| SaveError("save: usize out of range".into()))
    }
}

impl Snapshot for String {
    fn save(&self, w: &mut Writer) {
        w.len(self.len());
        w.raw(self.as_bytes());
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let n = r.length()?;
        let bytes = r.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| SaveError("save: string is not UTF-8".into()))
    }
}

impl<T: Snapshot> Snapshot for Option<T> {
    fn save(&self, w: &mut Writer) {
        match self {
            None => w.u8(0),
            Some(v) => {
                w.u8(1);
                v.save(w);
            }
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        match r.u8()? {
            0 => Ok(None),
            1 => Ok(Some(T::load(r)?)),
            _ => Err(SaveError("save: bad option tag".into())),
        }
    }
}

impl<T: Snapshot> Snapshot for Vec<T> {
    fn save(&self, w: &mut Writer) {
        w.len(self.len());
        for v in self {
            v.save(w);
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let n = r.length()?;
        let mut out = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            out.push(T::load(r)?);
        }
        Ok(out)
    }
}

impl<T: Snapshot> Snapshot for Box<T> {
    fn save(&self, w: &mut Writer) {
        (**self).save(w);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(Box::new(T::load(r)?))
    }
}

impl<T: Snapshot, const N: usize> Snapshot for [T; N] {
    fn save(&self, w: &mut Writer) {
        for v in self {
            v.save(w);
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let mut items = Vec::with_capacity(N);
        for _ in 0..N {
            items.push(T::load(r)?);
        }
        items
            .try_into()
            .map_err(|_| SaveError("save: array length".into()))
    }
    fn load_into(&mut self, r: &mut Reader<'_>) -> SaveResult<()> {
        for v in self.iter_mut() {
            v.load_into(r)?;
        }
        Ok(())
    }
}

impl<A: Snapshot, B: Snapshot> Snapshot for (A, B) {
    fn save(&self, w: &mut Writer) {
        self.0.save(w);
        self.1.save(w);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok((A::load(r)?, B::load(r)?))
    }
}

macro_rules! id_snapshot {
    ($($ty:ident($inner:ident);)*) => {$(
        impl Snapshot for crate::ids::$ty {
            fn save(&self, w: &mut Writer) {
                self.0.save(w);
            }
            fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
                Ok(crate::ids::$ty($inner::load(r)?))
            }
        }
    )*};
}

id_snapshot! {
    ObjectHandle(u32);
    WatchId(u32);
    PartId(i32);
    TeamId(i8);
    CommanderId(i8);
}

/// Writes a whole world's dynamic state (DESIGN.md section 9). Call it between frames: `Sim::step` drains the presentation
/// events, and the visibility is an input that the caller supplies every frame.
pub fn save_world(w: &crate::World) -> Vec<u8> {
    use crate::save::Snapshot as _;
    let mut out = Writer::new();
    write_header(&mut out, &w.mission.name);
    // the reinforcement log first: the base world replays it before any state is overlaid (its ABL instances must exist)
    w.mission.spawn_log.save(&mut out);
    w.clock.save(&mut out);
    w.next_weapon_shot_id.save(&mut out);
    w.rng.save(&mut out);
    w.terrain.save(&mut out);
    w.objects.save(&mut out);
    w.appearance.save(&mut out);
    w.pilots.save(&mut out);
    w.teams.save(&mut out);
    w.paths.save(&mut out);
    w.move_scratch.save(&mut out);
    w.sensors.save(&mut out);
    w.mission.save(&mut out);
    save_abl(&mut out, w.abl.as_ref());
    out.into_bytes()
}

/// Reads a save over `base`, a freshly initialised world of the same mission (same spec, same seed or any seed: the base's
/// random draws are overwritten). Static data (game tables, terrain geometry, appearance shapes, the compiled ABL code) is
/// taken from `base`; the dynamic state comes from the save. The result continues exactly where the saved run stopped.
///
/// `vfs` is the game data, needed to replay reinforcements whose chassis, appearance type or pilot the base does not hold yet
/// (`spawn::replay_spawn`). `None` is for a base that holds them already, such as a test world; a save that needs more is refused.
pub fn load_world(
    mut base: crate::World,
    vfs: Option<&mut mc2_formats::vfs::Vfs>,
    bytes: &[u8],
) -> SaveResult<crate::World> {
    use crate::save::Snapshot as _;
    let mut r = Reader::new(bytes);
    let mission = read_header(&mut r)?;
    if mission != base.mission.name {
        return Err(SaveError(format!(
            "save: mission {mission}, the base world is {}",
            base.mission.name
        )));
    }
    let saved_log: Vec<crate::mission::spawn::SpawnRecord> = Snapshot::load(&mut r)?;
    // the base world was initialised the same way, so its log is a prefix of the saved one; the rest is replayed
    let known = base.mission.spawn_log.len();
    if saved_log.len() < known || saved_log[..known] != base.mission.spawn_log[..] {
        return Err(SaveError(
            "save: the base world's reinforcements differ from the save's (not the same initialisation)".into(),
        ));
    }
    let mut vfs = vfs;
    for rec in &saved_log[known..] {
        crate::mission::spawn::replay_spawn(&mut base, vfs.as_deref_mut(), rec)
            .map_err(SaveError)?;
    }
    base.clock.load_into(&mut r)?;
    base.next_weapon_shot_id.load_into(&mut r)?;
    base.rng.load_into(&mut r)?;
    base.terrain.load_into(&mut r)?;
    base.objects.load_into(&mut r)?;
    base.appearance.load_into(&mut r)?;
    base.pilots.load_into(&mut r)?;
    base.teams.load_into(&mut r)?;
    base.paths.load_into(&mut r)?;
    base.move_scratch.load_into(&mut r)?;
    base.sensors.load_into(&mut r)?;
    base.mission.load_into(&mut r)?;
    load_abl(&mut r, &mut base.abl)?;
    r.finish()?;
    base.events.clear();
    crate::appearance::fx::restore_after_load(&mut base);
    Ok(base)
}

/// The ABL environment: the C++-style state block (`save_state`) and the runtime block (`save_runtime_state`). The compiled
/// modules and the instances come from the base world.
fn save_abl(w: &mut Writer, env: Option<&mc2_abl::Env>) {
    match env {
        None => w.u8(0),
        Some(env) => {
            w.u8(1);
            for block in [env.save_state(), env.save_runtime_state()] {
                w.len(block.len());
                w.raw(&block);
            }
        }
    }
}

fn load_abl(r: &mut Reader<'_>, env: &mut Option<mc2_abl::Env>) -> SaveResult<()> {
    let saved = r.u8()?;
    match (saved, env.as_mut()) {
        (0, None) => Ok(()),
        (1, Some(env)) => {
            let n = r.length()?;
            let state = r.take(n)?;
            env.load_state(state)
                .map_err(|e| SaveError(format!("save: ABL state: {e:?}")))?;
            let n = r.length()?;
            let runtime = r.take(n)?;
            env.load_runtime_state(runtime)
                .map_err(|e| SaveError(format!("save: ABL runtime state: {e:?}")))
        }
        _ => Err(SaveError(
            "save: the ABL environment is present in one of the save and the base world, not both"
                .into(),
        )),
    }
}

/// Writes the file header: magic, format version, mission name.
pub fn write_header(w: &mut Writer, mission: &str) {
    w.raw(MAGIC);
    w.u32(SAVE_VERSION);
    mission.to_string().save(w);
}

/// Checks the header and returns the mission name it records.
pub fn read_header(r: &mut Reader<'_>) -> SaveResult<String> {
    if r.take(4)? != MAGIC {
        return Err(SaveError("save: not an mc2-sim save".into()));
    }
    let version = r.u32()?;
    if version != SAVE_VERSION {
        return Err(SaveError(format!(
            "save: version {version}, this build reads {SAVE_VERSION}"
        )));
    }
    String::load(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq)]
    struct Pair {
        a: u32,
        b: Vec<f32>,
        cache: u8,
    }

    snapshot_struct!(Pair {
        a,
        b,
        cache => 0,
    });

    #[test]
    fn primitives_round_trip_bit_exact() {
        let v = Pair {
            a: 0xdead_beef,
            b: vec![1.5, -0.0, f32::NAN, f32::INFINITY],
            cache: 9,
        };
        let mut w = Writer::new();
        v.save(&mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let back = Pair::load(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(back.a, v.a);
        assert_eq!(
            back.b.iter().map(|f| f.to_bits()).collect::<Vec<_>>(),
            v.b.iter().map(|f| f.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(back.cache, 0, "a static field is not written");
    }

    #[test]
    fn overlay_keeps_static_fields() {
        let mut target = Pair {
            a: 1,
            b: Vec::new(),
            cache: 7,
        };
        let mut w = Writer::new();
        Pair {
            a: 2,
            b: vec![3.0],
            cache: 0,
        }
        .save(&mut w);
        let bytes = w.into_bytes();
        target.load_into(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(
            (target.a, target.b.as_slice(), target.cache),
            (2, &[3.0][..], 7)
        );
    }

    #[test]
    fn header_checks_magic_and_version() {
        let mut w = Writer::new();
        write_header(&mut w, "mc2_01");
        let mut bytes = w.into_bytes();
        assert_eq!(read_header(&mut Reader::new(&bytes)).unwrap(), "mc2_01");
        bytes[4] = (SAVE_VERSION as u8).wrapping_add(1);
        assert!(read_header(&mut Reader::new(&bytes)).is_err());
        assert!(read_header(&mut Reader::new(b"NOPE\x01\0\0\0")).is_err());
    }

    #[test]
    fn previous_format_header_is_rejected_before_payload() {
        let previous_version = SAVE_VERSION - 1;
        let mut w = Writer::new();
        w.raw(MAGIC);
        w.u32(previous_version);
        "mc2_01".to_string().save(&mut w);

        let err = read_header(&mut Reader::new(&w.into_bytes())).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("save: version {previous_version}, this build reads {SAVE_VERSION}")
        );
    }

    #[test]
    fn version_12_saves_are_rejected_before_payload() {
        // 12 was written by two incompatible layouts (the hulk flag without the smoke level, and the reverse)
        let mut w = Writer::new();
        w.raw(MAGIC);
        w.u32(12);
        "mc2_01".to_string().save(&mut w);
        let err = read_header(&mut Reader::new(&w.into_bytes())).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("save: version 12, this build reads {SAVE_VERSION}")
        );
    }

    #[test]
    fn version_13_saves_are_rejected_before_payload() {
        // 14 adds VehicleState::vehicle_pilot_turn; a 13 payload would decode eight bytes out of alignment (15 merges the
        // path-parity fields with it)
        let mut w = Writer::new();
        w.raw(MAGIC);
        w.u32(13);
        "mc2_01".to_string().save(&mut w);
        let err = read_header(&mut Reader::new(&w.into_bytes())).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("save: version 13, this build reads {SAVE_VERSION}")
        );
    }

    #[test]
    fn version_15_saves_are_rejected_before_payload() {
        // 16 adds BoltState::hit_effect_left (a bolt that hit lingers for its hit effect); a 15 payload would decode out of alignment
        let mut w = Writer::new();
        w.raw(MAGIC);
        w.u32(15);
        "mc2_01".to_string().save(&mut w);
        let err = read_header(&mut Reader::new(&w.into_bytes())).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("save: version 15, this build reads {SAVE_VERSION}")
        );
    }

    #[test]
    fn truncated_input_is_an_error() {
        let mut r = Reader::new(&[1, 0, 0]);
        assert!(r.u32().is_err());
    }
}

crate::snapshot_struct!(mc2_path::Vec3 { x, y, z });
