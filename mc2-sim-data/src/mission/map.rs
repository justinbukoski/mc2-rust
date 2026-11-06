// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Map features: `[NavMarkers]`/`[NavMarkerN]`, `[ForestN]` and
//! `[CommanderNGroup:G]`.
//!
//! Nav markers are read by `ReadNavMarkers` (code/objective.cpp:2311-2356),
//! which `Team::loadObjectives` calls once per team (code/team.cpp:160-176).
//! The commander groups are read in `Mission::init` (code/mission.cpp:2855-2895).
//! The forest blocks have no reader in the shipped C++ code.

use super::read::{opt, req, select};
use crate::error::Result;
use mc2_formats::ini::FitIni;

/// Commander ID range used by the group loop (multplyr.h:101, MAX_MC_PLAYERS).
pub const MAX_COMMANDERS: u32 = 8;
/// Element count of `Mates` (unitdesg.h:83, MAX_MOVERGROUP_COUNT_START).
pub const MAX_GROUP_MATES: usize = 12;
/// Trees per forest, the number of `TreeTypeN` keys in the data.
pub const TREE_TYPES: usize = 15;

/// One `[NavMarkerN]` block: a circle that becomes a hidden objective.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavMarker {
    /// The N in `NavMarkerN`. Kept because the derived objective title uses it.
    pub index: u32,
    /// `f xPos`, `f yPos`, `f radius`.
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}

/// Reads `[NavMarkers]`. A missing block or `NumNavMarkers` gives no markers
/// (objective.cpp:2314-2319). A marker with a missing key is dropped with a
/// warning (objective.cpp:2336-2340).
pub(crate) fn load_nav_markers(
    fit: &mut FitIni,
    warnings: &mut Vec<String>,
) -> Result<Vec<NavMarker>> {
    if !select(fit, "NavMarkers")? {
        return Ok(Vec::new());
    }
    let Some(count) = opt(fit.read_u32("NumNavMarkers"))? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for index in 0..count {
        let block = format!("NavMarker{index}");
        if !select(fit, &block)? {
            warnings.push(format!("[{block}] missing, nav marker skipped"));
            continue;
        }
        let x = opt(fit.read_f32("xPos"))?;
        let y = opt(fit.read_f32("yPos"))?;
        let radius = opt(fit.read_f32("radius"))?;
        match (x, y, radius) {
            (Some(x), Some(y), Some(radius)) => out.push(NavMarker {
                index,
                x,
                y,
                radius,
            }),
            _ => warnings.push(format!(
                "[{block}] lacks xPos, yPos or radius, nav marker skipped"
            )),
        }
    }
    Ok(out)
}

/// One `[ForestN]` block. Every key is optional because no shipped reader
/// consumes the block and the typed model only records what is present.
#[derive(Debug, Clone, PartialEq)]
pub struct Forest {
    /// The N in `ForestN`, starting at 0.
    pub index: u32,
    /// `st Name`, `st FileName`.
    pub name: Option<String>,
    pub file_name: Option<String>,
    /// `f CenterX`, `f CenterY`, `f Radius`.
    pub center_x: Option<f32>,
    pub center_y: Option<f32>,
    pub radius: Option<f32>,
    /// `f HeightMin`, `f HeightMax`, `f DensityMin`, `f DensityMax`.
    pub height_min: Option<f32>,
    pub height_max: Option<f32>,
    pub density_min: Option<f32>,
    pub density_max: Option<f32>,
    /// `f TreeType0` .. `f TreeType14`, by index.
    pub tree_types: [Option<f32>; TREE_TYPES],
    /// `b Random`.
    pub random: Option<bool>,
}

/// Reads `[ForestN]` blocks in order until the first missing N.
pub(crate) fn load_forests(fit: &mut FitIni) -> Result<Vec<Forest>> {
    let mut out = Vec::new();
    let mut index = 0u32;
    while select(fit, &format!("Forest{index}"))? {
        let mut tree_types = [None; TREE_TYPES];
        for (t, slot) in tree_types.iter_mut().enumerate() {
            *slot = opt(fit.read_f32(&format!("TreeType{t}")))?;
        }
        out.push(Forest {
            index,
            name: opt(fit.read_string("Name", 256))?,
            file_name: opt(fit.read_string("FileName", 256))?,
            center_x: opt(fit.read_f32("CenterX"))?,
            center_y: opt(fit.read_f32("CenterY"))?,
            radius: opt(fit.read_f32("Radius"))?,
            height_min: opt(fit.read_f32("HeightMin"))?,
            height_max: opt(fit.read_f32("HeightMax"))?,
            density_min: opt(fit.read_f32("DensityMin"))?,
            density_max: opt(fit.read_f32("DensityMax"))?,
            tree_types,
            random: opt(fit.read_bool("Random"))?,
        });
        index += 1;
    }
    Ok(out)
}

/// One `[CommanderNGroup:G]` block: a group of part numbers for a commander.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommanderGroup {
    /// The commander the group belongs to (the N in the block name).
    pub commander_id: u32,
    /// The group number G.
    pub group_index: u32,
    /// `l[12] Mates`: part numbers, `0` or `-1` where the slot is empty.
    pub mates: Vec<i32>,
}

/// Reads every `Commander{c}Group:{g}` block, `g` counting from 0 until a
/// block is missing, for each commander below [`MAX_COMMANDERS`]
/// (mission.cpp:2855-2895). The C++ skips commanders with no slot in the
/// session (`commandersToLoad`); the data-only load reads them all.
pub(crate) fn load_commander_groups(fit: &mut FitIni, file: &str) -> Result<Vec<CommanderGroup>> {
    let mut out = Vec::new();
    for commander_id in 0..MAX_COMMANDERS {
        let mut group_index = 0u32;
        while select(fit, &format!("Commander{commander_id}Group:{group_index}"))? {
            let mates = req(
                fit.read_array::<i32>("Mates", MAX_GROUP_MATES),
                file,
                "Mates",
            )?;
            out.push(CommanderGroup {
                commander_id,
                group_index,
                mates,
            });
            group_index += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_markers_read_and_drop_incomplete() {
        let text = "FITini\n[NavMarkers]\nul NumNavMarkers = 2\n\
            [NavMarker0]\nf xPos = 1\nf yPos = 2\nf radius = 3\n\
            [NavMarker1]\nf xPos = 4\n";
        let mut f = FitIni::from_bytes(format!("{text}\n").into_bytes()).unwrap();
        let mut warnings = Vec::new();
        let markers = load_nav_markers(&mut f, &mut warnings).unwrap();
        assert_eq!(
            markers,
            vec![NavMarker {
                index: 0,
                x: 1.0,
                y: 2.0,
                radius: 3.0
            }]
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn forests_stop_at_first_missing_block() {
        let text = "FITini\n[Forest0]\nst Name = \"oak\"\nf TreeType3 = 0.5\nb Random = TRUE\n\
            [Forest1]\nf Radius = 9\n";
        let mut f = FitIni::from_bytes(format!("{text}\n").into_bytes()).unwrap();
        let forests = load_forests(&mut f).unwrap();
        assert_eq!(forests.len(), 2);
        assert_eq!(forests[0].name.as_deref(), Some("oak"));
        assert_eq!(forests[0].tree_types[3], Some(0.5));
        assert_eq!(forests[0].random, Some(true));
        assert_eq!(forests[1].radius, Some(9.0));
    }

    #[test]
    fn commander_groups_read_mates_in_order() {
        let mut mates = String::from("l[12] Mates = ");
        for i in 0..12 {
            if i > 0 {
                mates.push_str(", ");
            }
            mates.push_str(&(i + 1).to_string());
        }
        let text = format!("FITini\n[Commander0Group:0]\n{mates}\n[Commander0Group:1]\n{mates}\n");
        let mut f = FitIni::from_bytes(format!("{text}\n").into_bytes()).unwrap();
        let groups = load_commander_groups(&mut f, "t.fit").unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].group_index, 1);
        assert_eq!(groups[0].mates.len(), 12);
        assert_eq!(groups[0].mates[11], 12);
    }
}
