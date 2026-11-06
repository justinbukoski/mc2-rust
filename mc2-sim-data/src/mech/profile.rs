// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mech profile FIT (`data/missions/profiles/pm*.fit`, `[Header] FileType = "MechProfile"`).
//!
//! Port of `BattleMech::init(FitIniFile*)` (`code/mech.cpp:2692` onwards). That
//! overload is reached through the virtual `init(FitIniFile*)`. The mission code
//! sends `BATTLEMECH` objects to `init(DWORD variantNum)` instead (`code/mission.cpp:1000-1012`),
//! so in this build the FIT path is not used for mission mechs. The data is ported
//! anyway, so the profiles load with the same meaning.
//!
//! Blocks read: `Header`, `General`, `Engine`, `MovementSystem` (optional), `MaxArmorPoints`,
//! `CurArmorPoints`, `InventoryInfo`, `Item:n` (n up to the three counts), and the
//! eight location blocks. `HeatSinks` is read only under `USEHEAT` (`code/mech.cpp:2764-2796`),
//! so it is not loaded.

use super::layout::{
    ArmorLocation, BodyLocation, MAX_INVENTORY_ITEMS, NUM_ARMOR_LOCATIONS, NUM_BODY_LOCATIONS,
};
use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;
use mc2_formats::FormatError;

/// `FileType` value the loader accepts (`code/mech.cpp:2715`).
pub const FILE_TYPE: &str = "MechProfile";

/// Crash-avoid and block tuning from `[MovementSystem]` (`code/mech.cpp:2798-2820`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrashParams {
    /// `CrashAvoidSelf` (long). Default is the mech's compiled default when absent.
    pub avoid_self: Option<i64>,
    /// `CrashAvoidPath` (long).
    pub avoid_path: Option<i64>,
    /// `CrashBlockSelf` (long).
    pub block_self: Option<i64>,
    /// `CrashBlockPath` (long).
    pub block_path: Option<i64>,
    /// `CrashYieldTime` (float). Present only when the key exists.
    pub yield_time: Option<f32>,
    /// The value the C++ assigns to `crashYieldTime` when `CrashYieldTime` reads
    /// (`code/mech.cpp:2820`: `crashYieldTime = crashSize`). That is the last long
    /// read, not the float. Kept so the faithful value is available.
    pub yield_time_as_assigned: Option<f32>,
}

/// One critical slot entry in a location block (`uc[2] Component:n`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileComponent {
    /// Index into the mech's inventory (255 when empty).
    pub inventory_id: u8,
    /// Second value of the pair. Nonzero means hit (`code/mech.cpp:3125-3135`).
    pub hit: bool,
}

/// One body location block (`[Head]` ... `[RightLeg]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileLocation {
    /// `CASE` (`uc`): nonzero means the location has CASE.
    pub case: bool,
    /// `CurInternalStructure` (`uc`).
    pub cur_internal: u8,
    /// `HotSpotNumber` (`uc`).
    pub hot_spot_number: u8,
    /// Critical spaces, `BodyLocation::critical_spaces()` entries.
    pub components: Vec<ProfileComponent>,
}

/// One inventory entry (`[Item:n]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileItem {
    /// `MasterID` (`uc`): the component id, the same value as the chassis `CompBas#` column.
    pub master_id: u8,
    /// `FacesForward` (`uc`), weapons only. Absent for other kinds.
    pub faces_forward: Option<u8>,
    /// `Amount` for ammo, as read (`long`, or `uc` when the long read fails). -1 means
    /// the component table's ammo per ton (`code/mech.cpp:3048-3058`). Ammo only.
    pub amount: Option<i64>,
}

/// A mech profile.
#[derive(Debug, Clone, PartialEq)]
pub struct MechProfile {
    /// `[General] Name`, read with a 127-byte buffer (`code/mech.cpp:2722`).
    pub name: String,
    /// `ChassisBR` (`int`), default 100 (`code/mech.cpp:2730`).
    pub chassis_br: i32,
    /// `CurTonnage` (`float`), required.
    pub tonnage: f32,
    /// `DescIndex` (`int`), default -1.
    pub desc_id: i32,
    /// `Pilot` (`int`), default -1.
    pub pilot: i32,
    /// `NotMineYet` (`bool`), default true.
    pub not_mine_yet: bool,
    /// `[Engine] MaxRunSpeed` (`uc`), required.
    pub max_run_speed: u8,
    /// `[MovementSystem]` tuning, if the block exists.
    pub crash: Option<CrashParams>,
    /// `[MaxArmorPoints]`, in `ArmorLocation` order.
    pub max_armor: [u8; NUM_ARMOR_LOCATIONS],
    /// `[CurArmorPoints]`, in `ArmorLocation` order.
    pub cur_armor: [u8; NUM_ARMOR_LOCATIONS],
    /// `NumOther` (`uc`): items 0..n are other components (`code/mech.cpp:2969-2990`).
    pub other: Vec<ProfileItem>,
    /// `NumWeapons`: the next items are weapons.
    pub weapons: Vec<ProfileItem>,
    /// `NumAmmo`: the last items are ammo.
    pub ammo: Vec<ProfileItem>,
    /// Location blocks in `BodyLocation` order.
    pub locations: [ProfileLocation; NUM_BODY_LOCATIONS],
}

impl MechProfile {
    /// Total inventory items (`numOther + numWeapons + numAmmos`).
    pub fn item_count(&self) -> usize {
        self.other.len() + self.weapons.len() + self.ammo.len()
    }
}

fn schema(file: &str, key: &str, e: FormatError) -> SimDataError {
    SimDataError::schema(file, format!("{key}: {e}"))
}

/// A key that may be absent. Only a missing key gives `None`.
fn optional<T>(file: &str, key: &str, r: mc2_formats::Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(FormatError::NotFound { .. }) => Ok(None),
        Err(e) => Err(schema(file, key, e)),
    }
}

fn required<T>(file: &str, key: &str, r: mc2_formats::Result<T>) -> Result<T> {
    r.map_err(|e| schema(file, key, e))
}

fn seek(file: &str, fit: &mut FitIni, block: &str) -> Result<()> {
    required(file, &format!("[{block}]"), fit.seek_block(block))
}

/// Reads `Item:n` and its `MasterID`, returning the entry and its kind-specific fields.
fn read_item(file: &str, fit: &mut FitIni, n: usize, kind: ItemKind) -> Result<ProfileItem> {
    let block = format!("Item:{n}");
    seek(file, fit, &block)?;
    let master_id = required(file, &format!("{block} MasterID"), fit.read_u8("MasterID"))?;
    match kind {
        ItemKind::Other => Ok(ProfileItem {
            master_id,
            faces_forward: None,
            amount: None,
        }),
        ItemKind::Weapon => {
            let faces = required(
                file,
                &format!("{block} FacesForward"),
                fit.read_u8("FacesForward"),
            )?;
            Ok(ProfileItem {
                master_id,
                faces_forward: Some(faces),
                amount: None,
            })
        }
        ItemKind::Ammo => {
            // `readIdLong("Amount")` first, then `readIdUChar` (`code/mech.cpp:3048-3052`).
            let amount = match fit.read_i64("Amount") {
                Ok(v) => v,
                Err(_) => required(
                    file,
                    &format!("{block} Amount"),
                    fit.read_u8("Amount").map(i64::from),
                )?,
            };
            Ok(ProfileItem {
                master_id,
                faces_forward: None,
                amount: Some(amount),
            })
        }
    }
}

#[derive(Clone, Copy)]
enum ItemKind {
    Other,
    Weapon,
    Ammo,
}

impl MechProfile {
    /// Parses a mech profile from its FIT image. `file` names the source for errors.
    pub fn from_fit(file: &str, fit: &mut FitIni) -> Result<MechProfile> {
        seek(file, fit, "Header")?;
        let file_type = required(file, "FileType", fit.read_string("FileType", 127))?;
        if file_type != FILE_TYPE {
            return Err(SimDataError::schema(
                file,
                format!("FileType is '{file_type}', expected '{FILE_TYPE}'"),
            ));
        }

        seek(file, fit, "General")?;
        let name = required(file, "Name", fit.read_string("Name", 127))?;
        let chassis_br = optional(file, "ChassisBR", fit.read_i32("ChassisBR"))?.unwrap_or(100);
        let tonnage = required(file, "CurTonnage", fit.read_f32("CurTonnage"))?;
        let desc_id = optional(file, "DescIndex", fit.read_i32("DescIndex"))?.unwrap_or(-1);
        let pilot = optional(file, "Pilot", fit.read_i32("Pilot"))?.unwrap_or(-1);
        let not_mine_yet =
            optional(file, "NotMineYet", fit.read_bool("NotMineYet"))?.unwrap_or(true);

        seek(file, fit, "Engine")?;
        let max_run_speed = required(file, "MaxRunSpeed", fit.read_u8("MaxRunSpeed"))?;

        let crash = if fit.seek_block("MovementSystem").is_ok() {
            Some(read_crash(file, fit)?)
        } else {
            None
        };

        seek(file, fit, "MaxArmorPoints")?;
        let max_armor = read_armor(file, fit, "MaxArmorPoints")?;
        seek(file, fit, "CurArmorPoints")?;
        let cur_armor = read_armor(file, fit, "CurArmorPoints")?;

        seek(file, fit, "InventoryInfo")?;
        let num_other = required(file, "NumOther", fit.read_u8("NumOther"))? as usize;
        let num_weapons = required(file, "NumWeapons", fit.read_u8("NumWeapons"))? as usize;
        let num_ammo = required(file, "NumAmmo", fit.read_u8("NumAmmo"))? as usize;
        let total = num_other + num_weapons + num_ammo;
        if total > MAX_INVENTORY_ITEMS {
            return Err(SimDataError::out_of_range(
                file,
                format!("{total} inventory items, the game allows {MAX_INVENTORY_ITEMS}"),
            ));
        }

        let mut n = 0usize;
        let mut other = Vec::with_capacity(num_other);
        while n < num_other {
            other.push(read_item(file, fit, n, ItemKind::Other)?);
            n += 1;
        }
        let mut weapons = Vec::with_capacity(num_weapons);
        while n < num_other + num_weapons {
            weapons.push(read_item(file, fit, n, ItemKind::Weapon)?);
            n += 1;
        }
        let mut ammo = Vec::with_capacity(num_ammo);
        while n < total {
            ammo.push(read_item(file, fit, n, ItemKind::Ammo)?);
            n += 1;
        }

        let mut locations: Vec<ProfileLocation> = Vec::with_capacity(NUM_BODY_LOCATIONS);
        for loc in BodyLocation::ALL {
            seek(file, fit, loc.profile_block())?;
            let block = loc.profile_block();
            let case = required(file, &format!("[{block}] CASE"), fit.read_u8("CASE"))? != 0;
            let cur_internal = required(
                file,
                &format!("[{block}] CurInternalStructure"),
                fit.read_u8("CurInternalStructure"),
            )?;
            let hot_spot_number = required(
                file,
                &format!("[{block}] HotSpotNumber"),
                fit.read_u8("HotSpotNumber"),
            )?;
            let mut components = Vec::with_capacity(loc.critical_spaces());
            for s in 0..loc.critical_spaces() {
                let key = format!("Component:{s}");
                let pair = required(
                    file,
                    &format!("[{block}] {key}"),
                    fit.read_array::<u8>(&key, 2),
                )?;
                if pair.len() != 2 {
                    return Err(SimDataError::schema(
                        file,
                        format!("[{block}] {key} has {} values", pair.len()),
                    ));
                }
                components.push(ProfileComponent {
                    inventory_id: pair[0],
                    hit: pair[1] != 0,
                });
            }
            locations.push(ProfileLocation {
                case,
                cur_internal,
                hot_spot_number,
                components,
            });
        }

        Ok(MechProfile {
            name,
            chassis_br,
            tonnage,
            desc_id,
            pilot,
            not_mine_yet,
            max_run_speed,
            crash,
            max_armor,
            cur_armor,
            other,
            weapons,
            ammo,
            locations: locations
                .try_into()
                .map_err(|_| SimDataError::schema(file, "location count mismatch"))?,
        })
    }
}

/// Reads the `[MovementSystem]` keys (`code/mech.cpp:2798-2820`).
fn read_crash(file: &str, fit: &mut FitIni) -> Result<CrashParams> {
    // `crashSize` is only assigned by a successful long read, and the yield time
    // takes that last value (see `yield_time_as_assigned`).
    let mut crash_size: i64 = 0;
    let mut long = |key: &str, fit: &FitIni| -> Result<Option<i64>> {
        let v = optional(file, key, fit.read_i64(key))?;
        if let Some(v) = v {
            crash_size = v;
        }
        Ok(v)
    };
    let avoid_self = long("CrashAvoidSelf", fit)?;
    let avoid_path = long("CrashAvoidPath", fit)?;
    let block_self = long("CrashBlockSelf", fit)?;
    let block_path = long("CrashBlockPath", fit)?;
    let yield_time = optional(file, "CrashYieldTime", fit.read_f32("CrashYieldTime"))?;
    let yield_time_as_assigned = yield_time.map(|_| crash_size as f32);
    Ok(CrashParams {
        avoid_self,
        avoid_path,
        block_self,
        block_path,
        yield_time,
        yield_time_as_assigned,
    })
}

/// Reads the eleven armor values of one block, keyed by `ArmorLocation::profile_key`.
fn read_armor(file: &str, fit: &FitIni, block: &str) -> Result<[u8; NUM_ARMOR_LOCATIONS]> {
    let mut out = [0u8; NUM_ARMOR_LOCATIONS];
    for loc in ArmorLocation::ALL {
        out[loc.index()] = required(
            file,
            &format!("[{block}] {}", loc.profile_key()),
            fit.read_u8(loc.profile_key()),
        )?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a mech profile image with one item per kind and every location block.
    /// `armor_block_keys` lets a test drop a key to exercise the error path.
    fn profile_text(drop_key: Option<&str>) -> Vec<u8> {
        let mut s = String::from("FITini\n\n[Header]\nst FileType = \"MechProfile\"\n\n");
        s.push_str("[General]\nst Name = \"Test Mad Cat\"\nl ChassisBR = 75\nf CurTonnage = 75\nl DescIndex = 435\nb NotMineYet = FALSE\n\n");
        s.push_str("[Engine]\nf Tonnage = -13.5\nul Rating = 375\nuc MaxRunSpeed = 24\n\n");
        s.push_str("[MovementSystem]\nl CrashAvoidSelf = 5\nl CrashBlockPath = 9\nf CrashYieldTime = 2.5\n\n");
        for (block, keys) in [
            (
                "MaxArmorPoints",
                [16, 34, 30, 30, 25, 25, 27, 27, 13, 10, 10],
            ),
            (
                "CurArmorPoints",
                [16, 34, 30, 30, 25, 25, 27, 27, 13, 10, 10],
            ),
        ] {
            s.push_str(&format!("[{block}]\n"));
            for (loc, v) in ArmorLocation::ALL.iter().zip(keys) {
                if drop_key != Some(loc.profile_key()) {
                    s.push_str(&format!("uc {} = {}\n", loc.profile_key(), v));
                }
            }
            s.push('\n');
        }
        s.push_str("[InventoryInfo]\nuc NumOther = 2\nuc NumWeapons = 1\nuc NumAmmo = 1\n\n");
        s.push_str("[Item:0]\nuc MasterID = 1\n\n[Item:1]\nuc MasterID = 4\n\n");
        s.push_str("[Item:2]\nuc MasterID = 6\nuc FacesForward = 1\n\n");
        s.push_str("[Item:3]\nuc MasterID = 53\nl Amount = -1\n\n");
        for loc in BodyLocation::ALL {
            s.push_str(&format!(
                "[{}]\nuc CASE = 1\nuc CurInternalStructure = 3\nuc HotSpotNumber = 2\n",
                loc.profile_block()
            ));
            for c in 0..loc.critical_spaces() {
                let (id, hit) = if c == 0 { (0u8, 1u8) } else { (255, 0) };
                s.push_str(&format!("uc[2] Component:{c} = {id},{hit}\n"));
            }
            s.push('\n');
        }
        s.push_str("FITEnd\n");
        s.into_bytes()
    }

    #[test]
    fn reads_a_full_mech_profile() {
        let mut fit = FitIni::from_bytes(profile_text(None)).unwrap();
        let p = MechProfile::from_fit("pm-test.fit", &mut fit).unwrap();
        assert_eq!(p.name, "Test Mad Cat");
        assert_eq!(p.chassis_br, 75);
        assert_eq!(p.tonnage, 75.0);
        assert_eq!(p.desc_id, 435);
        assert_eq!(p.pilot, -1, "absent Pilot keeps the default");
        assert!(!p.not_mine_yet);
        assert_eq!(p.max_run_speed, 24);
        let crash = p.crash.expect("MovementSystem present");
        assert_eq!(crash.avoid_self, Some(5));
        assert_eq!(crash.block_path, Some(9));
        assert_eq!(crash.avoid_path, None);
        assert_eq!(crash.yield_time, Some(2.5));
        assert_eq!(
            crash.yield_time_as_assigned,
            Some(9.0),
            "C++ assigns the last long read"
        );
        assert_eq!(p.max_armor[ArmorLocation::CenterTorso.index()], 34);
        assert_eq!(p.cur_armor[ArmorLocation::RearRightTorso.index()], 10);
        assert_eq!(p.other.len(), 2);
        assert_eq!(p.weapons.len(), 1);
        assert_eq!(p.weapons[0].faces_forward, Some(1));
        assert_eq!(p.ammo.len(), 1);
        assert_eq!(p.ammo[0].amount, Some(-1));
        assert_eq!(p.item_count(), 4);
        assert_eq!(p.locations[BodyLocation::Head.index()].components.len(), 6);
        assert_eq!(
            p.locations[BodyLocation::LeftLeg.index()].components.len(),
            6
        );
        assert_eq!(
            p.locations[BodyLocation::LeftArm.index()].components.len(),
            12
        );
        assert_eq!(
            p.locations[BodyLocation::CenterTorso.index()].components[0],
            ProfileComponent {
                inventory_id: 0,
                hit: true
            }
        );
        assert!(p.locations[BodyLocation::Head.index()].case);
        assert_eq!(p.locations[BodyLocation::Head.index()].hot_spot_number, 2);
    }

    #[test]
    fn missing_armor_key_is_an_error() {
        let mut fit = FitIni::from_bytes(profile_text(Some("LeftTorso"))).unwrap();
        let err = MechProfile::from_fit("pm-test.fit", &mut fit).unwrap_err();
        assert!(matches!(err, SimDataError::Schema { .. }), "{err}");
    }

    #[test]
    fn wrong_file_type_is_refused() {
        let text = String::from_utf8(profile_text(None))
            .unwrap()
            .replace("\"MechProfile\"", "\"GroundVehicleProfile\"");
        let mut fit = FitIni::from_bytes(text.into_bytes()).unwrap();
        assert!(MechProfile::from_fit("engine.fit", &mut fit).is_err());
    }
}
