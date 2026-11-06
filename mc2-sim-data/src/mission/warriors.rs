// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Warriors (pilot slots) and their brain memory: `[Warriors]`, `[WarriorN]`,
//! `[WarriorNCellM]` and `[WarriorNStaticM]`.
//!
//! Port of the `Warriors` read in `Mission::init` (code/mission.cpp:2312-2400)
//! and of `MechWarrior::loadBrainParameters` (code/warrior.cpp:7271-7380).
//! The pilot profile file (`warriorPath` + `Profile` + `.fit`) is not opened
//! here; the pilot lane owns that file. The brain `.abl` module named by
//! `Brain` is not preprocessed either.

use super::read::{opt, req, select};
use crate::bounds::{required_indexed, reserve_entries};
use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;

/// Longest `Profile` value the loader accepts (mission.cpp:2333, buffer 100).
const PROFILE_BUFFER: usize = 99;
/// Longest `Brain` value (mission.cpp:2360, buffer 128).
const BRAIN_BUFFER: usize = 127;
/// Longest static variable name (warrior.cpp:7361, buffer 256).
const NAME_BUFFER: usize = 255;
/// Element capacity of a static integer array (warrior.cpp:7370, `int values[1024]`).
const STATIC_ARRAY_CAPACITY: usize = 1024;

/// One `[WarriorN]` block with its brain memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Warrior {
    /// The N in `WarriorN`, starting at 1.
    pub index: u32,
    /// `st Profile`: the pilot profile name (mission.cpp:2333).
    pub profile: String,
    /// `st Brain`: the brain module name (mission.cpp:2360).
    pub brain: String,
    /// `l NumCells` and `l NumStaticVars`; `0` when the brain block is short.
    pub num_cells: i32,
    pub num_static_vars: i32,
    /// Cells loaded in order. Loading stops at the first cell that fails, so
    /// this can be shorter than `num_cells` (see `brain_warning`).
    pub cells: Vec<WarriorCell>,
    /// Static variables loaded in order, with the same stop rule.
    pub static_vars: Vec<StaticVar>,
}

/// One `[WarriorNCellM]` block (warrior.cpp:7291-7312).
#[derive(Debug, Clone, PartialEq)]
pub struct WarriorCell {
    /// `l Cell`: the memory slot.
    pub cell: i32,
    /// `l MemType` with its `Value`: `0` is integer, `1` is real.
    pub memory: CellMemory,
}

/// Value of a brain memory cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellMemory {
    /// `l Value` when `MemType` is 0.
    Integer(i32),
    /// `f Value` when `MemType` is 1.
    Real(f32),
}

/// One `[WarriorNStaticM]` block (warrior.cpp:7335-7385).
#[derive(Debug, Clone, PartialEq)]
pub struct StaticVar {
    /// `st Name`.
    pub name: String,
    pub kind: StaticVarKind,
}

/// The value of a static variable, selected by `l type` (warrior.cpp:7341-7385).
#[derive(Debug, Clone, PartialEq)]
pub enum StaticVarKind {
    /// `type` 0: `l Value`.
    Integer(i32),
    /// `type` 1: `f Value`.
    Real(f32),
    /// `type` 2: `l NumValues`, then `Values` as an integer array.
    IntegerArray(Vec<i32>),
}

/// Reads `[Warriors]` and each `[WarriorN]`, including the brain memory.
///
/// A missing `WarriorN` block or `Profile`/`Brain` key is an error, as the C++
/// asserts. Brain memory problems are not errors: like
/// `MechWarrior::loadBrainParameters` (whose result mission.cpp ignores), they
/// stop the load of that warrior's cells and are returned in `warnings`.
pub(crate) fn load_warriors(
    fit: &mut FitIni,
    file: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<Warrior>> {
    if !select(fit, "Warriors")? {
        return Err(SimDataError::schema(file, "missing block [Warriors]"));
    }
    let count = req(fit.read_u32("NumWarriors"), file, "NumWarriors")?;
    let count = required_indexed(fit, file, "NumWarriors", u64::from(count), "Warrior", "", 1)?;
    let mut out = reserve_entries(file, count)?;
    for index in 1..=count as u32 {
        let block = format!("Warrior{index}");
        if !select(fit, &block)? {
            return Err(SimDataError::schema(
                file,
                format!("missing block [{block}]"),
            ));
        }
        let profile = req(fit.read_string("Profile", PROFILE_BUFFER), file, "Profile")?;
        let brain = req(fit.read_string("Brain", BRAIN_BUFFER), file, "Brain")?;
        out.push(Warrior {
            index,
            profile,
            brain,
            num_cells: 0,
            num_static_vars: 0,
            cells: Vec::new(),
            static_vars: Vec::new(),
        });
    }
    for warrior in &mut out {
        load_brain(fit, file, warrior, warnings)?;
    }
    Ok(out)
}

/// `MechWarrior::loadBrainParameters` (warrior.cpp:7271-7385) for one warrior.
fn load_brain(
    fit: &mut FitIni,
    file: &str,
    warrior: &mut Warrior,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let base = format!("Warrior{}", warrior.index);
    if !select(fit, &base)? {
        warnings.push(format!("{file}: [{base}] missing, no brain memory"));
        return Ok(());
    }
    let (Some(num_cells), Some(num_static)) = (
        opt(fit.read_i32("NumCells"))?,
        opt(fit.read_i32("NumStaticVars"))?,
    ) else {
        warnings.push(format!(
            "{file}: [{base}] lacks NumCells or NumStaticVars, no brain memory (warrior.cpp:7271-7385)"
        ));
        return Ok(());
    };
    warrior.num_cells = num_cells;
    warrior.num_static_vars = num_static;

    // Every key miss below stops the load, as the C++ returns on any failed
    // read (warrior.cpp:7271-7385). The C++ reads `Value` with the
    // type-specific `readIdLong`/`readIdFloat`, and those match the `l` and `f`
    // tags only. The shipped data writes `f Value` for MemType 0 cells, so the
    // integer cells stop the load there.
    for c in 0..num_cells.max(0) {
        let block = format!("{base}Cell{c}");
        if !select(fit, &block)? {
            warnings.push(format!("{file}: [{block}] missing, cell loading stopped"));
            return Ok(());
        }
        let Some(cell) = opt(fit.read_i32("Cell"))? else {
            warnings.push(format!(
                "{file}: [{block}] lacks Cell, cell loading stopped"
            ));
            return Ok(());
        };
        let Some(mem_type) = opt(fit.read_i32("MemType"))? else {
            warnings.push(format!(
                "{file}: [{block}] lacks MemType, cell loading stopped"
            ));
            return Ok(());
        };
        let memory = match mem_type {
            0 => match opt(fit.read_i32("Value"))? {
                Some(v) => CellMemory::Integer(v),
                None => {
                    warnings.push(format!(
                        "{file}: [{block}] MemType 0 has no \"l Value\" (the data writes \"f Value\"), cell loading stopped (warrior.cpp:7271-7385)"
                    ));
                    return Ok(());
                }
            },
            1 => match opt(fit.read_f32("Value"))? {
                Some(v) => CellMemory::Real(v),
                None => {
                    warnings.push(format!(
                        "{file}: [{block}] MemType 1 lacks \"f Value\", cell loading stopped"
                    ));
                    return Ok(());
                }
            },
            other => {
                warnings.push(format!(
                    "{file}: [{block}] MemType {other} unknown, cell loading stopped (warrior.cpp:7271-7385)"
                ));
                return Ok(());
            }
        };
        warrior.cells.push(WarriorCell { cell, memory });
    }

    for s in 0..num_static.max(0) {
        let block = format!("{base}Static{s}");
        if !select(fit, &block)? {
            warnings.push(format!("{file}: [{block}] missing, static loading stopped"));
            return Ok(());
        }
        let Some(kind_code) = opt(fit.read_i32("type"))? else {
            warnings.push(format!(
                "{file}: [{block}] lacks type, static loading stopped"
            ));
            return Ok(());
        };
        let Some(name) = opt(fit.read_string("Name", NAME_BUFFER))? else {
            warnings.push(format!(
                "{file}: [{block}] lacks Name, static loading stopped"
            ));
            return Ok(());
        };
        let kind = match kind_code {
            0 => match opt(fit.read_i32("Value"))? {
                Some(v) => StaticVarKind::Integer(v),
                None => {
                    warnings.push(format!(
                        "{file}: [{block}] type 0 lacks \"l Value\", static loading stopped"
                    ));
                    return Ok(());
                }
            },
            1 => match opt(fit.read_f32("Value"))? {
                Some(v) => StaticVarKind::Real(v),
                None => {
                    warnings.push(format!(
                        "{file}: [{block}] type 1 lacks \"f Value\", static loading stopped"
                    ));
                    return Ok(());
                }
            },
            2 => {
                let Some(n) = opt(fit.read_i32("NumValues"))? else {
                    warnings.push(format!(
                        "{file}: [{block}] type 2 lacks NumValues, static loading stopped"
                    ));
                    return Ok(());
                };
                let n = match usize::try_from(n) {
                    Ok(n) if n <= STATIC_ARRAY_CAPACITY => n,
                    _ => {
                        warnings.push(format!(
                            "{file}: [{block}] NumValues {n} out of range, static loading stopped (warrior.cpp:7271-7385)"
                        ));
                        return Ok(());
                    }
                };
                match opt(fit.read_array::<i32>("Values", n))? {
                    Some(values) => StaticVarKind::IntegerArray(values),
                    None => {
                        warnings.push(format!(
                            "{file}: [{block}] type 2 lacks Values, static loading stopped"
                        ));
                        return Ok(());
                    }
                }
            }
            other => {
                warnings.push(format!(
                    "{file}: [{block}] static type {other} unknown, static loading stopped (warrior.cpp:7271-7385)"
                ));
                return Ok(());
            }
        };
        warrior.static_vars.push(StaticVar { name, kind });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "FITini\n[Warriors]\nul NumWarriors = 1\n\
        [Warrior1]\nst Profile = \"Pilot_A\"\nst Brain = \"brainA\"\nl NumCells = 2\nl NumStaticVars = 1\n\
        [Warrior1Cell0]\nl Cell = 4\nl MemType = 0\nl Value = 9\n\
        [Warrior1Cell1]\nl Cell = 5\nl MemType = 1\nf Value = 0.5\n\
        [Warrior1Static0]\nl type = 2\nst Name = \"list\"\nl NumValues = 3\nl[3] Values = 1, 2, 3\n";

    #[test]
    fn warrior_block_and_brain_memory_load() {
        let mut f = FitIni::from_bytes(format!("{SAMPLE}\n").into_bytes()).unwrap();
        let mut warnings = Vec::new();
        let ws = load_warriors(&mut f, "t.fit", &mut warnings).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(ws.len(), 1);
        let w = &ws[0];
        assert_eq!(w.profile, "Pilot_A");
        assert_eq!(w.brain, "brainA");
        assert_eq!(w.num_cells, 2);
        assert_eq!(
            w.cells[0],
            WarriorCell {
                cell: 4,
                memory: CellMemory::Integer(9)
            }
        );
        assert_eq!(
            w.cells[1],
            WarriorCell {
                cell: 5,
                memory: CellMemory::Real(0.5)
            }
        );
        assert_eq!(
            w.static_vars[0].kind,
            StaticVarKind::IntegerArray(vec![1, 2, 3])
        );
    }

    #[test]
    fn unknown_memtype_stops_cell_loading_with_warning() {
        let text = "FITini\n[Warriors]\nul NumWarriors = 1\n[Warrior1]\nst Profile = \"p\"\n\
            st Brain = \"b\"\nl NumCells = 2\nl NumStaticVars = 0\n\
            [Warrior1Cell0]\nl Cell = 1\nl MemType = 7\nl Value = 1\n\
            [Warrior1Cell1]\nl Cell = 2\nl MemType = 0\nl Value = 1\n";
        let mut f = FitIni::from_bytes(format!("{text}\n").into_bytes()).unwrap();
        let mut warnings = Vec::new();
        let ws = load_warriors(&mut f, "t.fit", &mut warnings).unwrap();
        assert!(ws[0].cells.is_empty());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn missing_profile_is_an_error() {
        let text = "FITini\n[Warriors]\nul NumWarriors = 1\n[Warrior1]\nst Brain = \"b\"\n";
        let mut f = FitIni::from_bytes(format!("{text}\n").into_bytes()).unwrap();
        assert!(load_warriors(&mut f, "t.fit", &mut Vec::new()).is_err());
    }
}
