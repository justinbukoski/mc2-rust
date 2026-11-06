// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Scripting side of `Mission::init`: `initABL()` (the environment with the sim RNG as its host), the three libraries, the mission
//! script and one brain module per pilot (mission.cpp:2255-2380).
//!
//! The ABL compiler reads its sources through [`SourceProvider`]; the game's archives sit behind a `&mut Vfs`, so all `.abl`, `.abx`
//! and `.abi` files are read once into a [`MemorySource`] when the environment is created. Brains compile lazily (`pre_process`
//! returns the existing handle for a file compiled already), exactly one module instance per pilot.
//!
//! RNG: the environment's host is a handle to the world's SIM stream (`SharedSimRng`), so script `random(n)` and the engine draw
//! from one generator like the C++. Loading and compiling draw nothing.

use std::path::Path;

use mc2_abl::{Config, Env, MemorySource, RngHost, SourceProvider, SymId};
use mc2_formats::fst::FastFile;
use mc2_formats::vfs::Vfs;
use mc2_sim_data::mission::{StaticVarKind, Warrior};

/// Libraries loaded before the mission script (mission.cpp:2255-2271).
pub const LIBRARIES: [&str; 3] = [
    "data/missions/orders.abx",
    "data/missions/miscfunc.abx",
    "data/missions/corebrain.abx",
];

/// Warrior brain directory (`warriorPath`).
pub const WARRIOR_DIR: &str = "data/missions/warriors/";

/// `FullPathFileName::init(dir, name, ext)` (mclib/cident.cpp:52): the directory is prepended unless `name` already starts with it, the
/// extension is appended unless `name` already ends with it (case-insensitive), and the result is lower-cased.
pub fn full_path_file_name(dir: &str, name: &str, ext: &str) -> String {
    let mut full = String::new();
    if !name.starts_with(dir) {
        full.push_str(dir);
    }
    full.push_str(name);
    let ends_with_ext =
        full.len() >= ext.len() && full[full.len() - ext.len()..].eq_ignore_ascii_case(ext);
    if !ext.is_empty() && !ends_with_ext {
        full.push_str(ext);
    }
    full.to_ascii_lowercase()
}

fn is_abl_name(lower: &str) -> bool {
    lower.ends_with(".abl") || lower.ends_with(".abx") || lower.ends_with(".abi")
}

fn collect_loose(dir: &Path, root: &Path, out: &mut MemorySource) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_loose(&p, root, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            let name = rel.to_string_lossy().replace('\\', "/");
            if is_abl_name(&name.to_ascii_lowercase()) {
                if let Ok(bytes) = std::fs::read(&p) {
                    out.insert(&name, bytes);
                }
            }
        }
    }
}

/// Every ABL source reachable through the VFS: the archives first, then loose files under the data root (loose files win, as in
/// the VFS lookup order).
pub fn collect_sources(vfs: &Vfs) -> Result<MemorySource, String> {
    let mut out = MemorySource::new();
    for path in vfs.archive_paths() {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut ff = FastFile::open(&canonical).map_err(|e| format!("{}: {e}", path.display()))?;
        let wanted: Vec<(usize, String)> = ff
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, e)| is_abl_name(&e.name.to_ascii_lowercase()))
            .map(|(i, e)| (i, e.name.clone()))
            .collect();
        for (i, name) in wanted {
            let bytes = ff.read_index(i).map_err(|e| format!("{name}: {e}"))?;
            out.insert(&name, bytes);
        }
    }
    collect_loose(&vfs.root().join("data"), vfs.root(), &mut out);
    Ok(out)
}

/// `initABL()` + `ABLi_loadLibrary` x3: a fresh environment whose random numbers come from the sim stream.
pub fn init_env(w: &mut crate::World, mut sources: MemorySource) -> Result<(), String> {
    // Repair the shipped infantry brain's three source errors; see DIFFERENCES.md.
    const INFANTRY: &str = "data/missions/warriors/infantry001.abl";
    if let Some(bytes) = sources.read(INFANTRY) {
        if let Ok(source) = String::from_utf8(bytes) {
            sources.insert(INFANTRY, repair_infantry_source(&source));
        }
    }
    let host = RngHost::new(w.rng.clone());
    let mut env = Env::new(Config::default(), sources, Box::new(host));
    for lib in LIBRARIES {
        env.load_library(lib)
            .map_err(|e| format!("ABL library {lib}: {e}"))?;
    }
    w.abl = Some(env);
    Ok(())
}

fn repair_infantry_source(source: &str) -> String {
    source
        .replace("\r\n", "\n")
        .replace("testNumInteger = round(14.56);", "")
        .replace(
            "patrol(patrolState1, patrolPath1, PILOT_STATE_RETREAT_BUILDING)",
            "corePatrol(patrolState1, patrolPath1, PILOT_STATE_RETREAT_BUILDING)",
        )
        .replace("return(0);\n\nendfunction;", "return(0);\n\nendorder;")
}

fn env_mut(w: &mut crate::World) -> Result<&mut Env, String> {
    w.abl
        .as_mut()
        .ok_or_else(|| "the ABL environment is not initialised".to_string())
}

/// The mission script: `ABLi_preProcess`, `new ABLModule; init(handle); setName("Mission")`, and the `handlemessage` callback.
pub fn load_mission_script(w: &mut crate::World, script: &str) -> Result<(), String> {
    let path = full_path_file_name("data/missions/", script, ".abl");
    let env = env_mut(w)?;
    let handle = env
        .pre_process(&path)
        .map_err(|e| format!("mission script {path}: {e}"))?;
    let inst = env
        .new_instance(handle)
        .map_err(|e| format!("mission script {path}: {e}"))?;
    env.set_name(inst, "Mission");
    let callback: Option<SymId> = env.find_function(inst, "handlemessage", true);
    w.mission.brain = Some(inst);
    w.mission.brain_callback = callback;
    Ok(())
}

/// `pilot->setBrainName(name)` + `ABLi_preProcess(warriorPath + name + ".abl")` + `pilot->setBrain(handle)`.
pub fn make_pilot_brain(w: &mut crate::World, slot: usize, brain: &str) -> Result<(), String> {
    let mut env = w
        .abl
        .take()
        .ok_or_else(|| "the ABL environment is not initialised".to_string())?;
    let result = make_pilot_brain_in_env(w, &mut env, slot, brain);
    w.abl = Some(env);
    result
}

pub(crate) fn make_pilot_brain_in_env(
    w: &mut crate::World,
    env: &mut Env,
    slot: usize,
    brain: &str,
) -> Result<(), String> {
    let path = full_path_file_name(WARRIOR_DIR, brain, ".abl");
    let handle = env
        .pre_process(&path)
        .map_err(|e| format!("pilot brain {path}: {e}"))?;
    let inst = env
        .new_instance(handle)
        .map_err(|e| format!("pilot brain {path}: {e}"))?;
    env.set_name(inst, "Pilot");
    w.pilots.slots[slot].brain = Some(inst);
    Ok(())
}

/// The static-variable half of `MechWarrior::loadBrainParameters` (warrior.cpp:7335-7385). The C++ ignores the return codes of
/// `setStaticInteger` and friends, so an unknown name is silently skipped; the number of such skips is returned.
pub fn apply_static_vars(
    w: &mut crate::World,
    slot: usize,
    warrior: &Warrior,
) -> Result<usize, String> {
    let Some(inst) = w.pilots.slots[slot].brain else {
        return Err(format!(
            "Warrior{}: loadBrainParameters with a NULL brain",
            warrior.index
        ));
    };
    let env = env_mut(w)?;
    let mut skipped = 0;
    for v in &warrior.static_vars {
        let code = match &v.kind {
            StaticVarKind::Integer(x) => env.set_static_integer(inst, &v.name, *x),
            StaticVarKind::Real(x) => env.set_static_real(inst, &v.name, *x),
            StaticVarKind::IntegerArray(a) => env.set_static_integer_array(inst, &v.name, a),
        };
        if code != 0 {
            skipped += 1;
        }
    }
    Ok(skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infantry_repair_accepts_both_line_endings() {
        for source in [
            "return(0);\n\nendfunction;",
            "return(0);\r\n\r\nendfunction;",
        ] {
            assert_eq!(repair_infantry_source(source), "return(0);\n\nendorder;");
        }
    }

    #[test]
    fn full_path_names_follow_cident() {
        assert_eq!(
            full_path_file_name(WARRIOR_DIR, "PBrain", ".abl"),
            "data/missions/warriors/pbrain.abl"
        );
        // a name that already carries the extension is not given a second one
        assert_eq!(
            full_path_file_name(WARRIOR_DIR, "mc2_12_CvA_NCV_AC.abl", ".abl"),
            "data/missions/warriors/mc2_12_cva_ncv_ac.abl"
        );
        // a name that already starts with the directory is not given it twice
        assert_eq!(
            full_path_file_name("data/missions/", "data/missions/mc2_01", ".abl"),
            "data/missions/mc2_01.abl"
        );
    }
}
