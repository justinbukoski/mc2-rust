// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Saving and restoring script state (`ABLi_saveEnvironment` / `ABLi_loadEnvironment`).
//!
//! The C++ writes eternal variables and, per module instance, the name, the FSM state names, the "init called" flag and
//! the static data to the savegame (order-call flags and the last return value are *not* saved: `ABLModule::read`
//! allocates the flags zeroed, so every completed order runs again after a load), then rebuilds everything else by recompiling the scripts. This port does the same with
//! its own little-endian format (not byte-compatible with the C++ files): [`Env::save_state`] captures that data and
//! [`Env::load_state`] restores it into an environment in which the same libraries have been loaded and the same
//! modules compiled and instantiated in the same order.

use crate::env::Env;
use crate::error::{AblError, AblResult};
use crate::symtab::*;
use crate::value::Cell;

const MAGIC: &[u8; 4] = b"ABLS";
const VERSION: u32 = 2;

struct Writer(Vec<u8>);

impl Writer {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
}

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn fail<T>(&self, what: &str) -> AblResult<T> {
        Err(AblError::fatal(format!("ABL state: {what}")))
    }
    fn u32(&mut self) -> AblResult<u32> {
        if self.pos + 4 > self.d.len() {
            return self.fail("truncated data");
        }
        let v = u32::from_le_bytes(self.d[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }
    fn bytes(&mut self) -> AblResult<&'a [u8]> {
        let n = self.u32()? as usize;
        if self.pos + n > self.d.len() {
            return self.fail("truncated data");
        }
        let b = &self.d[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }
    fn str(&mut self) -> AblResult<String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }
}

impl Env {
    fn write_cell(&self, w: &mut Writer, c: Cell) {
        match c {
            Cell::Bits(b) => {
                w.u32(0);
                w.u32(b);
            }
            Cell::Arr(a) => {
                w.u32(1);
                w.bytes(self.heap.bytes(a.id));
            }
            Cell::Ref(_) => {
                w.u32(0);
                w.u32(0);
            }
        }
    }

    fn read_cell(&mut self, r: &mut Reader<'_>, into: Cell) -> AblResult<Cell> {
        let tag = r.u32()?;
        match (tag, into) {
            (0, Cell::Arr(_)) => r.fail("cell kind mismatch (different script version?)"),
            (0, _) => Ok(Cell::Bits(r.u32()?)),
            (1, Cell::Arr(a)) => {
                let data = r.bytes()?;
                let dst = self.heap.bytes_mut(a.id);
                if dst.len() != data.len() {
                    return r.fail("array size mismatch (different script version?)");
                }
                dst.copy_from_slice(data);
                Ok(into)
            }
            _ => r.fail("cell kind mismatch (different script version?)"),
        }
    }

    /// Capture eternal variables and the state of every live module instance.
    pub fn save_state(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(MAGIC);
        w.u32(VERSION);
        w.u32(self.modules.len() as u32);
        for m in &self.modules {
            w.str(&m.file_name);
        }
        w.u32(self.eternals.len() as u32);
        for &c in &self.eternals {
            self.write_cell(&mut w, c);
        }
        w.u32(self.instances.len() as u32);
        for inst in &self.instances {
            w.u32(inst.live as u32);
            if !inst.live {
                continue;
            }
            w.u32(inst.module.0 as u32);
            w.str(&inst.name);
            w.u32(inst.init_called as u32);
            w.str(inst.state.map(|s| self.sym(s).name.as_str()).unwrap_or(""));
            w.str(
                inst.prev_state
                    .map(|s| self.sym(s).name.as_str())
                    .unwrap_or(""),
            );
            w.u32(inst.statics.len() as u32);
            for &c in &inst.statics {
                self.write_cell(&mut w, c);
            }
        }
        w.0
    }

    /// Restore state captured by [`Env::save_state`]. The environment must contain the same modules (compiled in the
    /// same order) and the same instances (created in the same order).
    pub fn load_state(&mut self, data: &[u8]) -> AblResult<()> {
        let mut r = Reader { d: data, pos: 0 };
        if data.len() < 8 || &data[..4] != MAGIC {
            return r.fail("not an ABL state block");
        }
        r.pos = 4;
        if r.u32()? != VERSION {
            return r.fail("unsupported version");
        }
        let nmod = r.u32()? as usize;
        if nmod != self.modules.len() {
            return r.fail("different number of registered modules");
        }
        for i in 0..nmod {
            if r.str()? != self.modules[i].file_name {
                return r.fail("module list differs");
            }
        }
        let nets = r.u32()? as usize;
        if nets != self.eternals.len() {
            return r.fail("different number of eternal variables");
        }
        for i in 0..nets {
            let c = self.eternals[i];
            self.eternals[i] = self.read_cell(&mut r, c)?;
        }
        let ninst = r.u32()? as usize;
        if ninst != self.instances.len() {
            return r.fail("different number of module instances");
        }
        for i in 0..ninst {
            let live = r.u32()? != 0;
            if live != self.instances[i].live {
                return r.fail("instance liveness differs");
            }
            if !live {
                continue;
            }
            let module = r.u32()? as usize;
            if module != self.instances[i].module.0 {
                return r.fail("instance module differs");
            }
            let name = r.str()?;
            let init_called = r.u32()? != 0;
            let state_name = r.str()?;
            let prev_name = r.str()?;
            let nstatics = r.u32()? as usize;
            if nstatics != self.instances[i].statics.len() {
                return r.fail("static variable count differs");
            }
            for k in 0..nstatics {
                let c = self.instances[i].statics[k];
                self.instances[i].statics[k] = self.read_cell(&mut r, c)?;
            }
            let scope = {
                let msym = self.modules[module].module_sym;
                self.sym(msym).routine().and_then(|rt| rt.scope)
            };
            let find_state = |env: &Env, name: &str| -> Option<SymId> {
                if name.is_empty() {
                    return None;
                }
                let sc = scope?;
                env.scopes[sc.0 as usize]
                    .candidates(name)
                    .iter()
                    .copied()
                    .find(|&s| {
                        env.sym(s).ty.is_none()
                            && matches!(env.sym(s).defn, Defn::Function(_))
                            && env.sym(s).flags() & ROUTINE_FLAG_STATE != 0
                    })
            };
            let state = find_state(self, &state_name);
            let prev = find_state(self, &prev_name);
            if (!state_name.is_empty() && state.is_none())
                || (!prev_name.is_empty() && prev.is_none())
            {
                return r.fail("unknown FSM state name");
            }
            let inst = &mut self.instances[i];
            inst.init_called = init_called;
            inst.state = state;
            inst.prev_state = prev;
            inst.name = name;
            // `ABLModule::read` hands out zeroed order-call flags: completed orders are issued again after a load.
            inst.order_flags.fill(0);
        }
        Ok(())
    }
}

const RUNTIME_MAGIC: &[u8; 4] = b"ABLR";

impl Env {
    /// The runtime state that [`Env::save_state`] leaves out: each live instance's order-call flags and last return value.
    /// The C++ savegame drops them (orders run again after a load), which a continuous run cannot afford: the mc2-sim save
    /// format writes this block next to `save_state`, so a loaded run continues exactly as the original.
    pub fn save_runtime_state(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.0.extend_from_slice(RUNTIME_MAGIC);
        w.u32(VERSION);
        w.u32(self.instances.len() as u32);
        for inst in &self.instances {
            w.u32(inst.live as u32);
            if !inst.live {
                continue;
            }
            w.u32(inst.order_flags.len() as u32);
            for &flag in &inst.order_flags {
                w.u32(flag);
            }
            self.write_cell(&mut w, inst.return_val);
        }
        w.0
    }

    /// Restores [`Env::save_runtime_state`] into an environment with the same instances (created in the same order).
    pub fn load_runtime_state(&mut self, data: &[u8]) -> AblResult<()> {
        let mut r = Reader { d: data, pos: 0 };
        if data.len() < 8 || &data[..4] != RUNTIME_MAGIC {
            return r.fail("not an ABL runtime state block");
        }
        r.pos = 4;
        if r.u32()? != VERSION {
            return r.fail("unsupported runtime version");
        }
        let n = r.u32()? as usize;
        if n != self.instances.len() {
            return r.fail("different number of instances");
        }
        for i in 0..n {
            let live = r.u32()? != 0;
            if live != self.instances[i].live {
                return r.fail("instance liveness differs");
            }
            if !live {
                continue;
            }
            let flags = r.u32()? as usize;
            if flags != self.instances[i].order_flags.len() {
                return r.fail("different number of order flags");
            }
            for k in 0..flags {
                self.instances[i].order_flags[k] = r.u32()?;
            }
            let into = self.instances[i].return_val;
            self.instances[i].return_val = self.read_cell(&mut r, into)?;
        }
        Ok(())
    }
}
