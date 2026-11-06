// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The logistics brain (`MissionBegin::logisticsBrain`, `code/missionbegin.cpp:130-157`): the tutorial
//! missions carry an ABL script that runs over the logistics screens (`tutorial0.abl` to
//! `tutorial2.abl`). It has its own environment with no libraries loaded (`MissionBegin::init` only
//! calls `initABL`), and its game functions are bound to the logistics screens. It runs once per
//! frame, before the current screen updates (`MissionBegin::update`, "if (logisticsBrain &&
//! !MPlayer) logisticsBrain->execute()").
//!
//! The natives that drive the tutorial's mouse callouts (`logisticsAnimationCallout`,
//! `logisticsInCallout`, `execLogisticsAnimationCallout` and `execLogisticsInCallout` in
//! `code/ablmc2.cpp`) start and read the sequence's [`Callout`].

use super::callout::Callout;
use super::sequence::ScreenVisits;
use mc2_abl::{
    Config, DefaultHost, Env, GameBindings, InstId, MemorySource, NativeCtx, SourceProvider,
};
use mc2_campaign::Logistics;
use mc2_formats::fst::FastFile;
use mc2_formats::vfs::Vfs;

/// `missionPath` for the brain script (`data/missions/<name>.abl`).
const MISSION_DIR: &str = "data/missions/";

/// What the logistics brain's natives reach: the grid cell the sequence is on (`curScreenX`,
/// `curScreenY`), the visit state and the campaign for `getCurrentScreenId`, and the callout.
pub struct BrainBindings<'a> {
    pub cell: (usize, usize),
    pub visits: &'a mut ScreenVisits,
    pub lg: &'a mut Logistics,
    pub callout: &'a mut Callout,
}

impl GameBindings for BrainBindings<'_> {
    fn logistics_screen_id(&mut self, _ctx: &mut NativeCtx<'_>) -> i32 {
        self.visits.screen_id(self.cell, self.lg)
    }
    fn logistics_animation_callout(
        &mut self,
        _ctx: &mut NativeCtx<'_>,
        button_id: i32,
        is_button: bool,
        time_to_scroll: f32,
        num_flashes: i32,
    ) -> bool {
        self.callout
            .start(button_id, is_button, time_to_scroll, num_flashes)
    }
    fn logistics_in_callout(&mut self, _ctx: &mut NativeCtx<'_>) -> bool {
        self.callout.in_callout()
    }
}

/// A loaded logistics brain: the environment and its one module instance.
pub struct LogisticsBrain {
    env: Env,
    inst: InstId,
}

impl LogisticsBrain {
    /// `ABLi_preProcess` + `init` + `setName("Logistics")` for the mission's brain script. `None`
    /// when the mission has no script (`NONE`) or the file is not in the game data, as the C++
    /// checks `fileExists`.
    pub fn load(vfs: &mut Vfs, script: Option<&str>) -> Result<Option<LogisticsBrain>, String> {
        let Some(script) = script.filter(|s| !s.eq_ignore_ascii_case("NONE")) else {
            return Ok(None);
        };
        let path = format!("{MISSION_DIR}{}.abl", script.to_ascii_lowercase());
        let sources = collect_sources(vfs)?;
        if sources.read(&path).is_none() {
            return Ok(None);
        }
        let mut env = Env::new(Config::default(), sources, Box::new(DefaultHost));
        let handle = env
            .pre_process(&path)
            .map_err(|e| format!("logistics script {path}: {e}"))?;
        let inst = env
            .new_instance(handle)
            .map_err(|e| format!("logistics script {path}: {e}"))?;
        env.set_name(inst, "Logistics");
        Ok(Some(LogisticsBrain { env, inst }))
    }

    /// `logisticsBrain->execute()` on the sequence's state.
    pub fn execute(&mut self, mut bindings: BrainBindings<'_>) -> Result<(), String> {
        self.env
            .execute(self.inst, &mut bindings)
            .map(|_| ())
            .map_err(|e| format!("logistics script: {e}"))
    }
}

/// Every ABL source reachable through the VFS: the archives first, then loose files under the data
/// root (loose files win, as in the VFS lookup order). A mirror of
/// `mc2_sim::mission::abl_load::collect_sources`, which the mission brain uses.
fn collect_sources(vfs: &Vfs) -> Result<MemorySource, String> {
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

fn is_abl_name(lower: &str) -> bool {
    lower.ends_with(".abl") || lower.ends_with(".abx") || lower.ends_with(".abi")
}

fn collect_loose(dir: &std::path::Path, root: &std::path::Path, out: &mut MemorySource) {
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
