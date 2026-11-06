// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The executor (`ablexec.cpp`, `ablxstmt.cpp`, `ablxexpr.cpp`, `ablxstd.cpp`, `ABLModule::execute`).
//!
//! The C++ interprets a crunched token stream on a `StackItem` stack. This interpreter walks the typed tree produced
//! by the parser but keeps the observable semantics: stack frames with static links, `ExitWithReturn` /
//! `NewStateSet` flag-based control flow, the order-completion protocol, FSM transition loops, library calls with
//! their own static data, 32-bit integer and single-precision real arithmetic, and the same runtime errors.

use crate::ast::*;
use crate::env::{Env, StdImpl};
use crate::error::{AblError, AblResult, RuntimeCode};
use crate::native::{AnyValue, ArrayRef, NativeArg, NativeCtx, NativeRet};
use crate::natives::{dispatch, GameBindings};
use crate::scanner::f32_to_i32_trunc;
use crate::symtab::*;
use crate::value::{ArrRef, Cell, Loc, MemKind};

type R<T> = AblResult<T>;

/// `MAX_USER_FILES`.
const MAX_USER_FILES: usize = 6;
/// Calls nested deeper than this raise a stack overflow (protects the host stack; the C++ limit is the 10240-item
/// runtime stack, which this comfortably exceeds for any real script).
const MAX_CALL_DEPTH: usize = 1000;

/// A stack frame (`StackFrameHeader` plus bookkeeping).
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    base: usize,
    level: u8,
    static_link: Option<usize>,
    /// `functionValue`: slot where `return(x)` stores the result.
    ret: Cell,
    items: usize,
}

/// Interpreter state that lives across calls (the `ablexec.cpp` globals).
pub(crate) struct Runtime {
    stack: Vec<Cell>,
    frames: Vec<Frame>,
    pub(crate) cur_inst: InstId,
    pub(crate) cur_fsm: Option<InstId>,
    cur_routine: Option<SymId>,
    new_state_set: bool,
    exit_with_return: bool,
    auto_return: bool,
    return_value: Cell,
    call_module_init: bool,
    num_state_transitions: u32,
    statement_count: u32,
    pub(crate) exec_line: u32,
    exec_file: i32,
    pub(crate) skip_order: bool,
    max_loop_iterations: i64,
    stack_items: usize,
    user_files: [bool; MAX_USER_FILES],
}

impl Runtime {
    pub(crate) fn new(max_loops: i64) -> Runtime {
        Runtime {
            stack: Vec::new(),
            frames: Vec::new(),
            cur_inst: InstId(0),
            cur_fsm: None,
            cur_routine: None,
            new_state_set: false,
            exit_with_return: false,
            auto_return: false,
            return_value: Cell::ZERO,
            call_module_init: false,
            num_state_transitions: 0,
            statement_count: 0,
            exec_line: 0,
            exec_file: -1,
            skip_order: false,
            max_loop_iterations: max_loops,
            stack_items: 0,
            user_files: [false; MAX_USER_FILES],
        }
    }
}

/// A module parameter value (`ABLParam`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamValue {
    /// `ABL_PARAM_INTEGER`
    Integer(i32),
    /// `ABL_PARAM_REAL`
    Real(f32),
}

#[derive(Clone, Copy)]
enum Place {
    Scalar(Loc),
    Array(ArrRef),
}

fn mem_kind(ty: TypeId) -> MemKind {
    if ty == REAL_TYPE {
        MemKind::F32
    } else if ty == CHAR_TYPE {
        MemKind::U8
    } else {
        MemKind::I32
    }
}

/// `(int)(double)` with x86 semantics for out-of-range values.
fn f64_to_i32_trunc(v: f64) -> i32 {
    if v.is_nan() || v >= 2147483648.0 || v <= -2147483649.0 {
        i32::MIN
    } else {
        v as i32
    }
}

struct Vm<'a> {
    env: &'a mut Env,
    b: &'a mut dyn GameBindings,
}

impl Env {
    /// `ABLModule::execute()`: run the module (its `init` function the first time, then its code or FSM state loop).
    /// Returns the number of statements executed.
    pub fn execute(&mut self, inst: InstId, bindings: &mut dyn GameBindings) -> R<u32> {
        self.execute_with(inst, &[], None, bindings)
    }

    /// `ABLModule::execute(NULL, functionIdPtr)`: run one function of the module (after the one-time `init`).
    pub fn execute_function(
        &mut self,
        inst: InstId,
        function: SymId,
        bindings: &mut dyn GameBindings,
    ) -> R<u32> {
        self.execute_with(inst, &[], Some(function), bindings)
    }

    /// Full form of `ABLModule::execute` with a module parameter list.
    pub fn execute_with(
        &mut self,
        inst: InstId,
        params: &[ParamValue],
        function: Option<SymId>,
        bindings: &mut dyn GameBindings,
    ) -> R<u32> {
        // `ABLModule::execute` saves and restores the interpreter globals, so a script run from inside a native (a brain or alarm
        // handler the native raised) leaves the outer script's stack, frames and position as they were.
        let outer = if self.exec_depth > 0 {
            let mut fresh = Runtime::new(self.rt.max_loop_iterations);
            fresh.user_files = self.rt.user_files;
            Some(std::mem::replace(&mut self.rt, fresh))
        } else {
            None
        };
        self.exec_depth += 1;
        let mut vm = Vm {
            env: self,
            b: bindings,
        };
        let r = vm.run_instance(inst, params, function);
        vm.cleanup_after_run();
        self.exec_depth -= 1;
        if let Some(outer) = outer {
            self.rt = outer;
        }
        r
    }

    /// How many scripts are running on this environment right now (0 when idle).
    pub fn exec_depth(&self) -> u32 {
        self.exec_depth
    }

    /// `transState` for `ABLi_transState`.
    pub(crate) fn trans_state_handle(&mut self, handle: i32) {
        let Some(fsm) = self.rt.cur_fsm else { return };
        let module = self.instances[fsm.0 as usize].module;
        let n = self.modules[module.0].state_handles.len() as i32;
        if handle > 0 && handle < n {
            let st = self.modules[module.0].state_handles[handle as usize].1;
            self.set_state(fsm, st);
        }
    }

    fn set_state(&mut self, fsm: InstId, st: SymId) {
        let i = &mut self.instances[fsm.0 as usize];
        i.prev_state = i.state;
        i.state = Some(st);
        self.rt.new_state_set = true;
    }
}

impl<'a> Vm<'a> {
    // ------------------------------------------------------------------------------------------
    // errors
    // ------------------------------------------------------------------------------------------

    fn rt_error(&self, code: RuntimeCode) -> AblError {
        let rt = &self.env.rt;
        let module = self.env.instances[rt.cur_inst.0 as usize].module;
        let file = if rt.exec_file >= 0 && module.0 < self.env.modules.len() {
            self.env.modules[module.0]
                .source_files
                .get(rt.exec_file as usize)
                .cloned()
                .unwrap_or_default()
        } else {
            String::new()
        };
        AblError::runtime(code, &file, rt.exec_line)
    }

    fn cleanup_after_run(&mut self) {
        // Release anything left behind by an aborted run.
        while let Some(f) = self.env.rt.frames.pop() {
            let _ = f;
        }
        self.env.rt.stack.clear();
        self.env.rt.stack_items = 0;
    }

    // ------------------------------------------------------------------------------------------
    // module execution
    // ------------------------------------------------------------------------------------------

    fn run_instance(
        &mut self,
        inst: InstId,
        params: &[ParamValue],
        function: Option<SymId>,
    ) -> R<u32> {
        let module = self.env.instances[inst.0 as usize].module;
        let module_sym = self.env.modules[module.0].module_sym;
        let is_fsm = self.env.sym(module_sym).flags() & ROUTINE_FLAG_FSM != 0;
        let eternal_slots = self.env.eternals.len();
        {
            let rt = &mut self.env.rt;
            rt.cur_inst = inst;
            rt.cur_fsm = if is_fsm { Some(inst) } else { None };
            rt.num_state_transitions = 0;
            rt.cur_routine = None;
            rt.exec_file = -1;
            rt.statement_count = 0;
            rt.new_state_set = false;
            rt.exit_with_return = false;
            rt.auto_return = false;
            rt.stack.clear();
            rt.frames.clear();
            rt.stack_items = eternal_slots;
        }

        // Module frame (level 1) with its parameters.
        let formals: Vec<SymId> = self
            .env
            .sym(module_sym)
            .routine()
            .map(|r| r.params.clone())
            .unwrap_or_default();
        let mut cells = Vec::new();
        for (i, f) in formals.iter().enumerate() {
            let fty = self.env.sym(*f).ty.unwrap_or(DUMMY_TYPE);
            match (self.env.sym(*f).defn.clone(), params.get(i)) {
                (Defn::ValParam { .. }, Some(ParamValue::Real(v))) if fty == REAL_TYPE => {
                    cells.push(Cell::real(*v))
                }
                (Defn::ValParam { .. }, Some(ParamValue::Integer(v))) if fty == REAL_TYPE => {
                    cells.push(Cell::real(*v as f32))
                }
                (Defn::ValParam { .. }, Some(ParamValue::Integer(v))) if fty == INTEGER_TYPE => {
                    cells.push(Cell::int(*v))
                }
                _ => return Ok(0),
            }
        }
        self.push_frame(1, None, cells)?;

        {
            let i = &mut self.env.instances[inst.0 as usize];
            self.env.rt.call_module_init = !i.init_called;
            i.init_called = true;
        }

        match function {
            None => self.execute_routine(module_sym)?,
            Some(child) => self.execute_child(module_sym, child)?,
        }

        let rv = self.env.rt.return_value;
        self.env.instances[inst.0 as usize].return_val = rv;
        Ok(self.env.rt.statement_count)
    }

    fn push_frame(&mut self, level: u8, static_link: Option<usize>, params: Vec<Cell>) -> R<usize> {
        let base = self.env.rt.stack.len();
        let items = 4 + params.len();
        if self.env.rt.frames.len() >= MAX_CALL_DEPTH
            || self.env.rt.stack_items + items > self.env.cfg.runtime_stack_items
        {
            return Err(self.rt_error(RuntimeCode::StackOverflow));
        }
        self.env.rt.stack_items += items;
        self.env.rt.stack.extend(params);
        self.env.rt.frames.push(Frame {
            base,
            level,
            static_link,
            ret: Cell::ZERO,
            items,
        });
        Ok(self.env.rt.frames.len() - 1)
    }

    /// Pop the top frame, releasing array parameters passed by value and normal array locals.
    fn pop_frame(&mut self, routine: SymId) {
        let (params, locals) = {
            let r = self.env.sym(routine).routine();
            (
                r.map(|r| r.params.clone()).unwrap_or_default(),
                r.map(|r| r.locals.clone()).unwrap_or_default(),
            )
        };
        let frame = self.env.rt.frames.pop().expect("frame underflow");
        for (i, p) in params.iter().enumerate() {
            let is_ref = matches!(self.env.sym(*p).defn, Defn::RefParam { .. });
            if !is_ref {
                if let Some(Cell::Arr(a)) = self.env.rt.stack.get(frame.base + i).copied() {
                    if self.env.types[self.env.sym(*p).ty.unwrap_or(DUMMY_TYPE).0 as usize].form
                        == Form::Array
                    {
                        self.env.heap.release(a.id);
                    }
                }
            }
        }
        for (i, l) in locals.iter().enumerate() {
            if self.env.types[self.env.sym(*l).ty.unwrap_or(DUMMY_TYPE).0 as usize].form
                == Form::Array
            {
                if let Some(Cell::Arr(a)) = self
                    .env
                    .rt
                    .stack
                    .get(frame.base + params.len() + i)
                    .copied()
                {
                    self.env.heap.release(a.id);
                }
            }
        }
        self.env.rt.stack.truncate(frame.base);
        self.env.rt.stack_items -= frame.items;
    }

    /// `routineEntry`: reset the return value and allocate local variables in the current frame.
    fn routine_entry(&mut self, routine: SymId) -> R<()> {
        self.env.rt.return_value = Cell::ZERO;
        let locals: Vec<SymId> = self
            .env
            .sym(routine)
            .routine()
            .map(|r| r.locals.clone())
            .unwrap_or_default();
        let mut add = 0;
        for l in locals {
            let ty = self.env.sym(l).ty.unwrap_or(DUMMY_TYPE);
            let ti = &self.env.types[ty.0 as usize];
            let cell = if ti.form == Form::Array {
                let size = ti.size.max(0) as usize;
                let id = self.env.alloc_array(size)?;
                Cell::Arr(ArrRef { id, off: 0 })
            } else {
                Cell::ZERO
            };
            self.env.rt.stack.push(cell);
            add += 1;
        }
        self.env.rt.stack_items += add;
        if let Some(f) = self.env.rt.frames.last_mut() {
            f.items += add;
        }
        if self.env.rt.stack_items > self.env.cfg.runtime_stack_items {
            return Err(self.rt_error(RuntimeCode::StackOverflow));
        }
        Ok(())
    }

    fn find_init(&self) -> Option<SymId> {
        let inst = self.env.rt.cur_inst;
        let module = self.env.instances[inst.0 as usize].module;
        let sym = self.env.modules[module.0].module_sym;
        let scope = self.env.sym(sym).routine().and_then(|r| r.scope)?;
        self.env.scopes[scope.0 as usize]
            .candidates("init")
            .first()
            .copied()
    }

    /// `execute(routineIdPtr)`.
    fn execute_routine(&mut self, routine: SymId) -> R<()> {
        let this_routine = self.env.rt.cur_routine;
        self.env.rt.cur_routine = Some(routine);

        self.routine_entry(routine)?;

        if self.env.rt.call_module_init {
            self.env.rt.call_module_init = false;
            if let Some(init) = self.find_init() {
                self.call_declared(init, &[], false)?;
            }
        }

        let flags = self.env.sym(routine).flags();
        if flags & ROUTINE_FLAG_FSM != 0 {
            self.run_fsm()?;
        } else {
            let body = self.env.sym(routine).routine().and_then(|r| r.body.clone());
            if let Some(body) = body {
                self.exec_code(routine, &body)?;
            }
            self.env.rt.exit_with_return = false;
        }

        self.pop_locals_only(routine);
        self.env.rt.cur_routine = this_routine;
        Ok(())
    }

    /// `routineExit` for the routine whose frame stays on the stack (module frames are popped by the caller of
    /// `run_instance`; function frames by `call_declared`). Releases the local arrays.
    fn pop_locals_only(&mut self, routine: SymId) {
        let Some(frame_base) = self.env.rt.frames.last().map(|f| f.base) else {
            return;
        };
        let (nparams, locals) = {
            let r = self.env.sym(routine).routine();
            (
                r.map(|r| r.params.len()).unwrap_or(0),
                r.map(|r| r.locals.clone()).unwrap_or_default(),
            )
        };
        for (i, l) in locals.iter().enumerate() {
            let ty = self.env.sym(*l).ty.unwrap_or(DUMMY_TYPE);
            if self.env.types[ty.0 as usize].form == Form::Array {
                let slot = frame_base + nparams + i;
                if let Some(Cell::Arr(a)) = self.env.rt.stack.get(slot).copied() {
                    self.env.heap.release(a.id);
                    self.env.rt.stack[slot] = Cell::ZERO;
                }
            }
        }
    }

    /// `executeChild`.
    fn execute_child(&mut self, module_sym: SymId, child: SymId) -> R<()> {
        let this_routine = self.env.rt.cur_routine;
        self.env.rt.cur_routine = Some(module_sym);
        self.routine_entry(module_sym)?;
        let mut init = None;
        if self.env.rt.call_module_init {
            self.env.rt.call_module_init = false;
            init = self.find_init();
            if let Some(i) = init {
                self.call_declared(i, &[], false)?;
            }
        }
        if init != Some(child) {
            self.call_declared(child, &[], false)?;
        }
        self.env.rt.exit_with_return = false;
        self.pop_locals_only(module_sym);
        self.env.rt.cur_routine = this_routine;
        Ok(())
    }

    fn run_fsm(&mut self) -> R<()> {
        self.env.rt.new_state_set = true;
        while self.env.rt.new_state_set {
            self.env.rt.num_state_transitions += 1;
            if self.env.rt.num_state_transitions == 50 {
                // "ABL endless state loop": the original logs and then simply stops transitioning.
                let fsm = self.env.rt.cur_fsm.expect("fsm");
                let msg = format!(
                    " ABL endless state loop in {} [{}:{}] ",
                    self.env.modules[self.env.instances[fsm.0 as usize].module.0].file_name,
                    self.env.state_name(fsm).unwrap_or(""),
                    self.env.prev_state_name(fsm).unwrap_or("")
                );
                self.env.warnings.push(msg);
                self.env.rt.new_state_set = false;
            } else {
                self.env.rt.new_state_set = false;
                let fsm = self.env.rt.cur_fsm.expect("fsm");
                let Some(state) = self.env.instances[fsm.0 as usize].state else {
                    return Err(AblError::fatal(" ABL.execute: NULL state in FSM "));
                };
                self.call_declared(state, &[], false)?;
            }
            self.env.rt.exit_with_return = false;
        }
        Ok(())
    }

    /// The `TKN_CODE` case of `execStatement`: run a routine's statements.
    fn exec_code(&mut self, routine: SymId, body: &Body) -> R<()> {
        let flags = self.env.sym(routine).flags();
        let was_auto = self.env.rt.auto_return;
        self.env.rt.auto_return = flags & (ROUTINE_FLAG_ORDER | ROUTINE_FLAG_STATE) != 0;
        for s in &body.stmts {
            if self.env.rt.new_state_set {
                return Ok(());
            }
            self.exec_stmt(s)?;
            if self.env.rt.exit_with_return {
                break;
            }
        }
        if self.env.rt.new_state_set {
            return Ok(());
        }
        self.env.rt.auto_return = was_auto;
        Ok(())
    }

    // ------------------------------------------------------------------------------------------
    // calls
    // ------------------------------------------------------------------------------------------

    fn cur_frame(&self) -> usize {
        self.env.rt.frames.len() - 1
    }

    /// `execDeclaredRoutineCall`. Returns the function value for typed routines.
    fn call_declared(&mut self, sym: SymId, args: &[CallArg], skip_order: bool) -> R<Option<Cell>> {
        let (formals, has_type, level_of_sym, lib) = {
            let s = self.env.sym(sym);
            (
                s.routine().map(|r| r.params.clone()).unwrap_or_default(),
                s.ty.is_some(),
                s.level,
                s.library,
            )
        };
        let key = self.env.sym(sym).routine().map(|r| r.key);
        if key != Some(RoutineKey::Declared) {
            let name = self.env.sym(sym).name.clone();
            return Err(AblError::fatal(format!(
                " ABL: Undefined ABL RoutineKey in {}:{} ({})",
                self.cur_module_name(),
                self.env.rt.exec_line,
                name
            )));
        }

        if skip_order {
            // Parameters are still evaluated; the call itself is skipped and reports success.
            for a in args {
                match a {
                    CallArg::Val(e) => {
                        self.eval(e)?;
                    }
                    CallArg::Ref(v) => {
                        self.resolve_var(v)?;
                    }
                }
            }
            return Ok(Some(Cell::int(1)));
        }

        let cur = self.cur_frame();
        let old_level = self.env.rt.frames[cur].level;
        let new_level = level_of_sym + 1;
        let cur_routine_lib = self
            .env
            .rt
            .cur_routine
            .and_then(|r| self.env.sym(r).library);
        let is_library_call = lib.is_some() && lib != cur_routine_lib;
        let static_link = if is_library_call {
            None
        } else if new_level == old_level + 1 {
            Some(cur)
        } else if new_level == old_level {
            self.env.rt.frames[cur].static_link
        } else {
            return Err(self.rt_error(RuntimeCode::NestedFunctionCall));
        };

        // Evaluate actual parameters in the caller's context.
        let mut cells = Vec::with_capacity(formals.len());
        for (i, a) in args.iter().enumerate() {
            let formal = formals[i];
            let fty = self.env.sym(formal).ty.unwrap_or(DUMMY_TYPE);
            match a {
                CallArg::Val(e) => {
                    let mut c = self.eval(e)?;
                    if fty == REAL_TYPE && e.ty() == INTEGER_TYPE {
                        c = Cell::real(c.as_int() as f32);
                    }
                    if self.env.types[fty.0 as usize].form == Form::Array {
                        if let Cell::Arr(src) = c {
                            let size = self.env.types[fty.0 as usize].size.max(0) as usize;
                            let id = self.env.alloc_array(size)?;
                            let data: Vec<u8> =
                                self.env.heap.tail(src).iter().take(size).copied().collect();
                            self.env.heap.bytes_mut(id)[..data.len()].copy_from_slice(&data);
                            c = Cell::Arr(ArrRef { id, off: 0 });
                        }
                    }
                    cells.push(c);
                }
                CallArg::Ref(v) => {
                    let c = match self.resolve_var(v)? {
                        Place::Scalar(loc) => Cell::Ref(loc),
                        Place::Array(a) => Cell::Arr(a),
                    };
                    cells.push(c);
                }
            }
        }

        // Direct calls from the host (the one-time `init`, `execute_function`) pass no actuals even if the routine
        // declares parameters; the C++ would read uninitialised stack there. Supply zeroed parameters instead.
        for &formal in &formals[cells.len()..] {
            let fty = self.env.sym(formal).ty.unwrap_or(DUMMY_TYPE);
            let ti = &self.env.types[fty.0 as usize];
            let is_ref = matches!(self.env.sym(formal).defn, Defn::RefParam { .. });
            if ti.form == Form::Array && !is_ref {
                let id = self.env.alloc_array(ti.size.max(0) as usize)?;
                cells.push(Cell::Arr(ArrRef { id, off: 0 }));
            } else {
                cells.push(Cell::ZERO);
            }
        }

        self.push_frame(new_level, static_link, cells)?;

        // Library calls run in the library instance's context (its static data, order flags and init state).
        let prev_inst = self.env.rt.cur_inst;
        if is_library_call {
            let lib_inst = lib.expect("library instance");
            self.env.rt.cur_inst = lib_inst;
            let li = &mut self.env.instances[lib_inst.0 as usize];
            self.env.rt.call_module_init = !li.init_called;
            li.init_called = true;
        }

        let res = self.execute_routine(sym);

        if is_library_call {
            self.env.rt.cur_inst = prev_inst;
        }
        let ret = self
            .env
            .rt
            .frames
            .last()
            .map(|f| f.ret)
            .unwrap_or(Cell::ZERO);
        match res {
            Ok(()) => {
                self.pop_frame(sym);
                Ok(if has_type { Some(ret) } else { None })
            }
            Err(e) => Err(e),
        }
    }

    fn cur_module_name(&self) -> String {
        let inst = &self.env.instances[self.env.rt.cur_inst.0 as usize];
        inst.name.clone()
    }

    /// Execute any call; `statement_site` carries the order protocol data for statement-level calls.
    fn call(&mut self, call: &Call, skip_order: bool) -> R<Option<Cell>> {
        match call.target {
            CallTarget::Declared(sym) => self.call_declared(sym, &call.args, skip_order),
            CallTarget::Builtin(idx) => self.call_builtin(idx, &call.args, skip_order),
        }
    }

    fn arg_expr<'e>(&self, a: &'e CallArg) -> &'e Expr {
        match a {
            CallArg::Val(e) => e,
            CallArg::Ref(_) => unreachable!("reference argument to a built-in"),
        }
    }

    fn eval_str(&mut self, e: &Expr) -> R<String> {
        match self.eval(e)? {
            Cell::Arr(a) => Ok(String::from_utf8_lossy(self.env.heap.cstr(a)).into_owned()),
            _ => Ok(String::new()),
        }
    }

    fn format_item(&self, ty: TypeId, c: Cell, real_decimals: usize) -> String {
        if ty == INTEGER_TYPE {
            format!("{}", c.as_int())
        } else if ty == BOOLEAN_TYPE {
            (if c.as_int() != 0 { "true" } else { "false" }).to_string()
        } else if ty == CHAR_TYPE {
            (c.as_byte() as char).to_string()
        } else if ty == REAL_TYPE {
            format!("{:.*}", real_decimals, c.as_real())
        } else if let Cell::Arr(a) = c {
            String::from_utf8_lossy(self.env.heap.cstr(a)).into_owned()
        } else {
            String::new()
        }
    }

    fn call_builtin(&mut self, idx: u16, args: &[CallArg], skip_order: bool) -> R<Option<Cell>> {
        let info = self.env.std_fns[idx as usize].clone();
        match info.imp {
            StdImpl::Return => {
                self.env.rt.return_value = Cell::ZERO;
                let cur = self.cur_frame();
                if let Some(a) = args.first() {
                    let e = self.arg_expr(a);
                    let c = self.eval(e)?;
                    let routine = self.env.rt.cur_routine.expect("routine");
                    let target_ty = self.env.sym(routine).ty;
                    let c = match (target_ty, e.ty()) {
                        (Some(REAL_TYPE), INTEGER_TYPE) => Cell::real(c.as_int() as f32),
                        _ => c,
                    };
                    self.env.rt.frames[cur].ret = c;
                    self.env.rt.return_value = c;
                }
                self.env.rt.exit_with_return = true;
                Ok(None)
            }
            StdImpl::Print => {
                let e = self.arg_expr(&args[0]);
                let c = self.eval(e)?;
                let s = self.format_item(e.ty(), c, 4);
                self.env.host.debug_print(&s);
                Ok(None)
            }
            StdImpl::Concat => {
                let dest = match self.eval(self.arg_expr(&args[0]))? {
                    Cell::Arr(a) => a,
                    _ => return Ok(Some(Cell::ZERO)),
                };
                let item_expr = self.arg_expr(&args[1]);
                let c = self.eval(item_expr)?;
                let ty = item_expr.ty();
                let s = if ty == INTEGER_TYPE
                    || ty == CHAR_TYPE
                    || ty == REAL_TYPE
                    || ty == BOOLEAN_TYPE
                    || matches!(c, Cell::Arr(_))
                {
                    self.format_item(ty, c, 2)
                } else {
                    String::new()
                };
                // strcat into the destination buffer, truncated to its size.
                let cur_len = self.env.heap.cstr(dest).len();
                let tail = self.env.heap.tail_mut(dest);
                let room = tail.len().saturating_sub(cur_len + 1);
                let n = s.len().min(room);
                tail[cur_len..cur_len + n].copy_from_slice(&s.as_bytes()[..n]);
                if cur_len + n < tail.len() {
                    tail[cur_len + n] = 0;
                }
                Ok(Some(Cell::ZERO))
            }
            StdImpl::GetStateHandle => {
                let name = self.eval_str(self.arg_expr(&args[0]))?;
                let Some(fsm) = self.env.rt.cur_fsm else {
                    return Err(AblError::fatal(
                        " ABL: getStateHandle called outside of an FSM ",
                    ));
                };
                Ok(Some(Cell::int(self.env.find_state_handle(fsm, &name))))
            }
            StdImpl::GetCurrentStateHandle => {
                let Some(fsm) = self.env.rt.cur_fsm else {
                    return Err(AblError::fatal(
                        " ABL: getCurrentStateHandle called outside of an FSM ",
                    ));
                };
                Ok(Some(Cell::int(self.env.state_handle(fsm))))
            }
            StdImpl::Assert => {
                let expression = self.eval(self.arg_expr(&args[0]))?.as_int();
                let code = self.eval(self.arg_expr(&args[1]))?.as_int();
                let s = self.eval_str(self.arg_expr(&args[2]))?;
                if expression == 0 {
                    return Err(AblError::fatal(format!("ABL ASSERT: [{}] {}", code, s)));
                }
                Ok(None)
            }
            StdImpl::Fatal => {
                let _code = self.eval(self.arg_expr(&args[0]))?.as_int();
                let s = self.eval_str(self.arg_expr(&args[1]))?;
                Err(AblError::fatal(s))
            }
            StdImpl::Abs | StdImpl::Sqrt => {
                let e = self.arg_expr(&args[0]);
                let c = self.eval(e)?;
                let v = if e.ty() == INTEGER_TYPE {
                    c.as_int() as f32
                } else {
                    c.as_real()
                };
                if info.imp == StdImpl::Abs {
                    Ok(Some(Cell::real(if v < 0.0 { -v } else { v })))
                } else if v < 0.0 {
                    Err(self.rt_error(RuntimeCode::InvalidFunctionArgument))
                } else {
                    Ok(Some(Cell::real((v as f64).sqrt() as f32)))
                }
            }
            StdImpl::Round => {
                let v = self.eval(self.arg_expr(&args[0]))?.as_real();
                let r = if v > 0.0 {
                    f64_to_i32_trunc(v as f64 + 0.5)
                } else {
                    f64_to_i32_trunc(v as f64 - 0.5)
                };
                Ok(Some(Cell::int(r)))
            }
            StdImpl::Trunc => {
                let v = self.eval(self.arg_expr(&args[0]))?.as_real();
                Ok(Some(Cell::int(f32_to_i32_trunc(v))))
            }
            StdImpl::Random => {
                let n = self.eval(self.arg_expr(&args[0]))?.as_int();
                Ok(Some(Cell::int(self.env.host.random(n))))
            }
            StdImpl::SeedRandom => {
                let seed = self.eval(self.arg_expr(&args[0]))?.as_int();
                let s = if seed == -1 {
                    self.env.host.time()
                } else {
                    seed as u32
                };
                self.env.host.seed_random(s);
                Ok(None)
            }
            StdImpl::SetMaxLoops => {
                let n = self.eval(self.arg_expr(&args[0]))?.as_int();
                self.env.rt.max_loop_iterations = n as i64 + 1;
                Ok(None)
            }
            StdImpl::FileOpen => {
                let name = self.eval_str(self.arg_expr(&args[0]))?;
                let mut handle = -1;
                if let Some(i) = self.env.rt.user_files.iter().position(|u| !u) {
                    if self.env.host.user_file_open(i, &name) {
                        self.env.rt.user_files[i] = true;
                        handle = i as i32;
                    }
                }
                Ok(Some(Cell::int(handle)))
            }
            StdImpl::FileWrite => {
                let h = self.eval(self.arg_expr(&args[0]))?.as_int();
                let s = self.eval_str(self.arg_expr(&args[1]))?;
                if h >= 0 && (h as usize) < MAX_USER_FILES && self.env.rt.user_files[h as usize] {
                    self.env.host.user_file_write(h as usize, &s);
                }
                Ok(None)
            }
            StdImpl::FileClose => {
                let h = self.eval(self.arg_expr(&args[0]))?.as_int();
                if h >= 0 && (h as usize) < MAX_USER_FILES && self.env.rt.user_files[h as usize] {
                    self.env.host.user_file_close(h as usize);
                    self.env.rt.user_files[h as usize] = false;
                }
                Ok(None)
            }
            StdImpl::GetModule => {
                let cur_buf = self.eval(self.arg_expr(&args[0]))?;
                let fsm_buf = self.eval(self.arg_expr(&args[1]))?;
                let inst = self.env.rt.cur_inst;
                let module = self.env.instances[inst.0 as usize].module;
                let cur_name = self.env.modules[module.0].file_name.clone();
                let fsm_name = match self.env.rt.cur_fsm {
                    Some(f) => self.env.modules[self.env.instances[f.0 as usize].module.0]
                        .file_name
                        .clone(),
                    None => "none".to_string(),
                };
                for (buf, s) in [(cur_buf, cur_name), (fsm_buf, fsm_name)] {
                    if let Cell::Arr(a) = buf {
                        let dst = self.env.heap.tail_mut(a);
                        if !dst.is_empty() {
                            let n = s.len().min(dst.len() - 1);
                            dst[..n].copy_from_slice(&s.as_bytes()[..n]);
                            dst[n] = 0;
                        }
                    }
                }
                Ok(Some(Cell::int(module.0 as i32)))
            }
            StdImpl::ResetOrders => {
                let scope = self.eval(self.arg_expr(&args[0]))?.as_int();
                let inst = self.env.rt.cur_inst;
                if scope == 0 {
                    self.env.reset_orders(inst);
                } else if scope == 1 {
                    if let Some(r) = self
                        .env
                        .rt
                        .cur_routine
                        .and_then(|r| self.env.sym(r).routine().cloned())
                    {
                        for i in r.order_call_index..r.order_call_index + r.num_order_calls {
                            let flags = &mut self.env.instances[inst.0 as usize].order_flags;
                            if let Some(w) = flags.get_mut((i / 32) as usize) {
                                *w &= !(1u32 << (i % 32));
                            }
                        }
                    }
                }
                Ok(None)
            }
            StdImpl::SetState => {
                let h = self.eval(self.arg_expr(&args[0]))?.as_int();
                if h > 0 {
                    let Some(fsm) = self.env.rt.cur_fsm else {
                        return Err(AblError::fatal(" ABL: setState called outside of an FSM "));
                    };
                    let module = self.env.instances[fsm.0 as usize].module;
                    let Some((_, st)) = self.env.modules[module.0]
                        .state_handles
                        .get(h as usize)
                        .cloned()
                    else {
                        return Err(self.rt_error(RuntimeCode::InvalidFunctionArgument));
                    };
                    self.env.set_state(fsm, st);
                }
                Ok(None)
            }
            StdImpl::GetFunctionHandle | StdImpl::CallFunction => {
                // `assert(0)` in the C++ source: function handles cannot be represented in an integer.
                Err(self.rt_error(RuntimeCode::UnimplementedFeature))
            }
            StdImpl::SetFlag => {
                let mut bits = self.eval(self.arg_expr(&args[0]))?.as_int() as u32;
                let flag = self.eval(self.arg_expr(&args[1]))?.as_int() as u32;
                let set = self.eval(self.arg_expr(&args[2]))?.as_int() == 1;
                bits &= !flag;
                if set {
                    bits |= flag;
                }
                Ok(Some(Cell::int(bits as i32)))
            }
            StdImpl::GetFlag => {
                let bits = self.eval(self.arg_expr(&args[0]))?.as_int() as u32;
                let flag = self.eval(self.arg_expr(&args[1]))?.as_int() as u32;
                Ok(Some(Cell::int(((bits & flag) != 0) as i32)))
            }
            StdImpl::Native(_) | StdImpl::Custom(_) => {
                self.call_native(idx, &info, args, skip_order)
            }
        }
    }

    fn call_native(
        &mut self,
        idx: u16,
        info: &crate::env::StdFn,
        args: &[CallArg],
        skip_order: bool,
    ) -> R<Option<Cell>> {
        // Evaluate arguments in order.
        let mut native_args = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let e = self.arg_expr(a);
            let c = self.eval(e)?;
            let t = e.ty();
            let kind = info.params[i];
            let arg = match kind {
                ParamKind::Integer => NativeArg::Int(c.as_int()),
                ParamKind::Real => NativeArg::Real(c.as_real()),
                ParamKind::Boolean => NativeArg::Bool(c.as_int() == 1),
                ParamKind::Char => NativeArg::Char(c.as_byte()),
                ParamKind::IntegerReal => {
                    if t == INTEGER_TYPE {
                        if info.star_reinterprets {
                            NativeArg::Real(f32::from_bits(c.as_int() as u32))
                        } else {
                            NativeArg::Real(c.as_int() as f32)
                        }
                    } else {
                        NativeArg::Real(c.as_real())
                    }
                }
                ParamKind::CharArray
                | ParamKind::IntegerArray
                | ParamKind::RealArray
                | ParamKind::BooleanArray => match c {
                    Cell::Arr(a) => NativeArg::Array(ArrayRef(a)),
                    _ => NativeArg::Int(0),
                },
                ParamKind::Anything => NativeArg::Any(self.any_value(t, c)),
            };
            native_args.push(arg);
        }

        if skip_order && info.is_order {
            return Ok(Some(Cell::int(1)));
        }

        self.env.rt.skip_order = skip_order;
        let ret = match info.imp {
            StdImpl::Native(id) => {
                let mut ctx = NativeCtx::new(self.env);
                dispatch(id, self.b, &mut ctx, &native_args)
            }
            StdImpl::Custom(ci) => {
                let mut f = std::mem::replace(
                    &mut self.env.custom_fns[ci as usize].f,
                    Box::new(|_: &mut NativeCtx<'_>, _: &[NativeArg]| NativeRet::None),
                );
                let r = {
                    let mut ctx = NativeCtx::new(self.env);
                    f(&mut ctx, &native_args)
                };
                self.env.custom_fns[ci as usize].f = f;
                r
            }
            _ => unreachable!(),
        };
        let _ = idx;
        // The C++ runs a brain / alarm handler a native raised inside the native call: run it now, nested, before the result is used.
        self.b.after_native(self.env);
        Ok(match (info.ret, ret) {
            (ReturnKind::None, _) => None,
            (ReturnKind::Integer, NativeRet::Int(v)) => Some(Cell::int(v)),
            (ReturnKind::Integer, NativeRet::Bool(v)) => Some(Cell::int(v as i32)),
            (ReturnKind::Real, NativeRet::Real(v)) => Some(Cell::real(v)),
            (ReturnKind::Real, NativeRet::Int(v)) => Some(Cell::real(v as f32)),
            (ReturnKind::Boolean, NativeRet::Bool(v)) => Some(Cell::int(v as i32)),
            (ReturnKind::Boolean, NativeRet::Int(v)) => Some(Cell::int((v != 0) as i32)),
            _ => Some(Cell::ZERO),
        })
    }

    fn any_value(&self, t: TypeId, c: Cell) -> AnyValue {
        if t == INTEGER_TYPE {
            AnyValue::Int(c.as_int())
        } else if t == BOOLEAN_TYPE {
            AnyValue::Bool(c.as_int() != 0)
        } else if t == CHAR_TYPE {
            AnyValue::Char(c.as_byte())
        } else if t == REAL_TYPE {
            AnyValue::Real(c.as_real())
        } else if let Cell::Arr(a) = c {
            match self.env.types[self.env.types[t.0 as usize].elem_ty.0 as usize].form {
                _ if self.env.types[t.0 as usize].elem_ty == CHAR_TYPE => {
                    AnyValue::CharArray(ArrayRef(a))
                }
                _ if self.env.types[t.0 as usize].elem_ty == REAL_TYPE => {
                    AnyValue::RealArray(ArrayRef(a))
                }
                _ if self.env.types[t.0 as usize].elem_ty == BOOLEAN_TYPE => {
                    AnyValue::BoolArray(ArrayRef(a))
                }
                _ => AnyValue::IntArray(ArrayRef(a)),
            }
        } else {
            AnyValue::Int(0)
        }
    }

    // ------------------------------------------------------------------------------------------
    // statements
    // ------------------------------------------------------------------------------------------

    fn exec_block(&mut self, stmts: &[Stmt]) -> R<()> {
        for s in stmts {
            self.exec_stmt(s)?;
            if self.env.rt.exit_with_return {
                return Ok(());
            }
        }
        Ok(())
    }

    /// `execOrderReturn(0)`: the calling order/state ends because the order it called did not complete.
    fn exec_order_return(&mut self, ret: i32) {
        let routine = self.env.rt.cur_routine.expect("routine");
        let is_state = self.env.sym(routine).flags() & ROUTINE_FLAG_STATE != 0;
        if !is_state {
            let cur = self.cur_frame();
            let c = Cell::int(ret);
            self.env.rt.frames[cur].ret = c;
            self.env.rt.return_value = c;
        }
        self.env.rt.exit_with_return = true;
    }

    fn order_flag_set(&self, site: u32) -> bool {
        let inst = &self.env.instances[self.env.rt.cur_inst.0 as usize];
        inst.order_flags
            .get((site / 32) as usize)
            .map(|w| w & (1u32 << (site % 32)) != 0)
            .unwrap_or(false)
    }

    fn set_order_flag(&mut self, site: u32) {
        let inst = &mut self.env.instances[self.env.rt.cur_inst.0 as usize];
        if let Some(w) = inst.order_flags.get_mut((site / 32) as usize) {
            *w |= 1u32 << (site % 32);
        }
    }

    /// One stack slot that the C++ would leave allocated until the current frame exits.
    fn leak_stack_item(&mut self) -> R<()> {
        self.env.rt.stack_items += 1;
        if let Some(f) = self.env.rt.frames.last_mut() {
            f.items += 1;
        }
        if self.env.rt.stack_items > self.env.cfg.runtime_stack_items {
            return Err(self.rt_error(RuntimeCode::StackOverflow));
        }
        Ok(())
    }

    fn exec_stmt(&mut self, s: &Stmt) -> R<()> {
        self.env.rt.exec_line = s.line;
        self.env.rt.exec_file = s.file as i32;
        self.env.rt.statement_count = self.env.rt.statement_count.wrapping_add(1);
        match &s.kind {
            StmtKind::Nop => {}
            StmtKind::Assign { target, value } => {
                let place = self.resolve_var(target)?;
                let v = self.eval(value)?;
                self.assign(place, target.ty, value.ty(), v)?;
            }
            StmtKind::Call { call, order_site } => {
                let (is_order, has_flags) = {
                    let inst = &self.env.instances[self.env.rt.cur_inst.0 as usize];
                    let flags = match call.target {
                        CallTarget::Declared(sym) => self.env.sym(sym).flags(),
                        CallTarget::Builtin(idx) => {
                            if self.env.std_fns[idx as usize].is_order {
                                ROUTINE_FLAG_ORDER
                            } else {
                                0
                            }
                        }
                    };
                    (
                        flags & ROUTINE_FLAG_ORDER != 0,
                        !inst.order_flags.is_empty(),
                    )
                };
                let mut skip = false;
                if is_order && has_flags {
                    if let Some(site) = order_site {
                        skip = !self.env.is_library(self.env.rt.cur_inst)
                            && self.order_flag_set(*site);
                    }
                }
                let result = self.call(call, skip)?;
                if is_order && !self.env.rt.auto_return {
                    // The C++ leaves an order's result on the stack unless an order/state is executing it
                    // (it is only popped in the auto-return branch); the leak lasts until the frame exits.
                    self.leak_stack_item()?;
                }
                if is_order && self.env.rt.auto_return {
                    let rv = result.map(Cell::as_int).unwrap_or(0);
                    if rv == 0 {
                        self.exec_order_return(0);
                    } else if has_flags {
                        if let Some(site) = order_site {
                            self.set_order_flag(*site);
                        }
                    }
                }
            }
            StmtKind::If {
                cond,
                then_body,
                else_body,
            } => {
                let t = self.eval(cond)?.as_int() == 1;
                if t {
                    self.exec_block(then_body)?;
                } else if let Some(e) = else_body {
                    self.exec_block(e)?;
                }
            }
            StmtKind::While { cond, body } => {
                let mut iterations: i64 = 0;
                loop {
                    if self.eval(cond)?.as_int() == 0 {
                        break;
                    }
                    self.exec_block(body)?;
                    if self.env.rt.exit_with_return {
                        return Ok(());
                    }
                    iterations += 1;
                    if iterations == self.env.rt.max_loop_iterations {
                        return Err(self.rt_error(RuntimeCode::InfiniteLoop));
                    }
                }
            }
            StmtKind::Repeat { body, cond } => {
                let mut iterations: i64 = 0;
                loop {
                    self.exec_block(body)?;
                    if self.env.rt.exit_with_return {
                        return Ok(());
                    }
                    iterations += 1;
                    if iterations == self.env.rt.max_loop_iterations {
                        return Err(self.rt_error(RuntimeCode::InfiniteLoop));
                    }
                    if self.eval(cond)?.as_int() != 0 {
                        break;
                    }
                }
            }
            StmtKind::For {
                var,
                from,
                to,
                body,
            } => {
                let place = self.resolve_var(var)?;
                let initial = self.eval(from)?.as_int();
                let fin = self.eval(to)?.as_int();
                let mut cv = initial;
                let mut iterations: i64 = 0;
                while cv <= fin {
                    if let Place::Scalar(loc) = place {
                        self.write_loc(loc, Cell::int(cv));
                    }
                    self.exec_block(body)?;
                    if self.env.rt.exit_with_return {
                        return Ok(());
                    }
                    iterations += 1;
                    if iterations == self.env.rt.max_loop_iterations {
                        return Err(self.rt_error(RuntimeCode::InfiniteLoop));
                    }
                    cv = cv.wrapping_add(1);
                    if cv < initial && fin == i32::MAX {
                        break;
                    }
                }
            }
            StmtKind::Switch { expr, cases } => {
                let c = self.eval(expr)?;
                let ty = expr.ty();
                let v = if ty == INTEGER_TYPE || self.env.types[ty.0 as usize].form == Form::Enum {
                    c.as_int()
                } else {
                    c.as_byte() as i32
                };
                for case in cases {
                    if case.labels.contains(&v) {
                        self.exec_block(&case.body)?;
                        break;
                    }
                }
            }
            StmtKind::Trans(state) => {
                if let Some(fsm) = self.env.rt.cur_fsm {
                    self.env.set_state(fsm, *state);
                }
            }
            StmtKind::TransBack => {
                let inst = self.env.rt.cur_inst;
                let prev = self.env.instances[inst.0 as usize].prev_state;
                match prev {
                    None => return Err(self.rt_error(RuntimeCode::NullPrevState)),
                    Some(p) => {
                        if let Some(fsm) = self.env.rt.cur_fsm {
                            self.env.set_state(fsm, p);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------
    // variables
    // ------------------------------------------------------------------------------------------

    fn frame_for_level(&self, level: u8) -> R<usize> {
        let mut fi = self.cur_frame();
        while self.env.rt.frames[fi].level > level {
            match self.env.rt.frames[fi].static_link {
                Some(n) => fi = n,
                None => {
                    return Err(AblError::fatal(format!(
                        " ABL: variable of an outer scope is not reachable from here (library routines cannot see their module-level non-static variables) [{}:{}]",
                        self.cur_module_name(),
                        self.env.rt.exec_line
                    )))
                }
            }
        }
        Ok(fi)
    }

    fn read_loc(&self, loc: Loc) -> Cell {
        match loc {
            Loc::Stack(i) => self.env.rt.stack[i as usize],
            Loc::Static(inst, i) => self.env.instances[inst as usize].statics[i as usize],
            Loc::Eternal(i) => self.env.eternals[i as usize],
            Loc::Mem(r, MemKind::U8) => Cell::Bits(self.env.heap.read_u8(r) as u32),
            Loc::Mem(r, _) => Cell::Bits(self.env.heap.read_u32(r)),
        }
    }

    fn write_loc(&mut self, loc: Loc, c: Cell) {
        match loc {
            Loc::Stack(i) => self.env.rt.stack[i as usize] = c,
            Loc::Static(inst, i) => self.env.instances[inst as usize].statics[i as usize] = c,
            Loc::Eternal(i) => self.env.eternals[i as usize] = c,
            Loc::Mem(r, MemKind::U8) => self.env.heap.write_u8(r, c.as_byte()),
            Loc::Mem(r, _) => {
                if let Cell::Bits(b) = c {
                    self.env.heap.write_u32(r, b)
                }
            }
        }
    }

    /// Compute where a variable (with its subscripts) lives (`execVariable` with `USE_TARGET`).
    fn resolve_var(&mut self, v: &VarExpr) -> R<Place> {
        let (cell, loc) = match v.place {
            VarPlace::Frame { level, slot } => {
                let fi = self.frame_for_level(level)?;
                let abs = self.env.rt.frames[fi].base + slot as usize;
                (self.env.rt.stack[abs], Loc::Stack(abs as u32))
            }
            VarPlace::Static { idx, lib } => {
                let inst = lib.unwrap_or(self.env.rt.cur_inst);
                (
                    self.env.instances[inst.0 as usize].statics[idx as usize],
                    Loc::Static(inst.0, idx),
                )
            }
            VarPlace::Eternal(idx) => (self.env.eternals[idx as usize], Loc::Eternal(idx)),
        };
        let is_array = self.env.types[v.decl_ty.0 as usize].form == Form::Array;
        let mut place = if v.is_ref && !is_array {
            match cell {
                Cell::Ref(l) => Place::Scalar(l),
                _ => return Err(AblError::fatal(" ABL: reference parameter has no target ")),
            }
        } else if is_array {
            match cell {
                Cell::Arr(a) => Place::Array(a),
                _ => return Err(AblError::fatal(" ABL: array variable has no storage ")),
            }
        } else {
            Place::Scalar(loc)
        };
        if !v.subs.is_empty() {
            let Place::Array(mut arr) = place else {
                return Err(AblError::fatal(" ABL: subscript applied to a scalar "));
            };
            for sub in &v.subs {
                let idx = self.eval(&sub.expr)?.as_int();
                if !(0..sub.count).contains(&idx) {
                    return Err(self.rt_error(RuntimeCode::ValueOutOfRange));
                }
                arr.off = arr
                    .off
                    .wrapping_add((idx as i64 * sub.stride as i64) as u32);
            }
            place = if self.env.types[v.ty.0 as usize].form == Form::Array {
                Place::Array(arr)
            } else {
                Place::Scalar(Loc::Mem(arr, mem_kind(v.ty)))
            };
        }
        Ok(place)
    }

    fn assign(&mut self, place: Place, target_ty: TypeId, expr_ty: TypeId, v: Cell) -> R<()> {
        let tti = self.env.types[target_ty.0 as usize].clone();
        match place {
            Place::Array(dest) => {
                if let Cell::Arr(src) = v {
                    let size = tti.size.max(0) as usize;
                    let src_bytes: Vec<u8> = self.env.heap.tail(src).to_vec();
                    let n = if tti.elem_ty == CHAR_TYPE {
                        // strcpy-like: copy the string and its terminator, never more than the destination holds.
                        let len = src_bytes
                            .iter()
                            .position(|&b| b == 0)
                            .map(|p| p + 1)
                            .unwrap_or(src_bytes.len());
                        len.min(size)
                    } else {
                        size.min(src_bytes.len())
                    };
                    let dst = self.env.heap.tail_mut(dest);
                    let n = n.min(dst.len());
                    dst[..n].copy_from_slice(&src_bytes[..n]);
                }
            }
            Place::Scalar(loc) => {
                let c = if target_ty == REAL_TYPE && expr_ty == INTEGER_TYPE {
                    Cell::real(v.as_int() as f32)
                } else if target_ty == CHAR_TYPE {
                    Cell::Bits(v.as_byte() as u32)
                } else {
                    v
                };
                self.write_loc(loc, c);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------
    // expressions
    // ------------------------------------------------------------------------------------------

    fn eval(&mut self, e: &Expr) -> R<Cell> {
        Ok(match e {
            Expr::Int(v, _) => Cell::int(*v),
            Expr::Real(v) => Cell::real(*v),
            Expr::Char(c) => Cell::int(*c as i32),
            Expr::Str(id, _) => Cell::Arr(ArrRef { id: *id, off: 0 }),
            Expr::Var(v) => match self.resolve_var(v)? {
                Place::Array(a) => Cell::Arr(a),
                Place::Scalar(loc) => self.read_loc(loc),
            },
            Expr::Call(call, _) => match self.call(call, false)? {
                Some(c) => c,
                None => Cell::ZERO,
            },
            Expr::Neg(inner, ty) => {
                let c = self.eval(inner)?;
                if *ty == INTEGER_TYPE {
                    Cell::int(c.as_int().wrapping_neg())
                } else {
                    Cell::real(-c.as_real())
                }
            }
            Expr::Not(inner, _) => {
                let c = self.eval(inner)?;
                Cell::int(1i32.wrapping_sub(c.as_int()))
            }
            Expr::Bin(b) => {
                let l = self.eval(&b.l)?;
                let r = self.eval(&b.r)?;
                self.binop(b.op, l, b.l.ty(), r, b.r.ty())?
            }
        })
    }

    fn promote(c: Cell, ty: TypeId) -> f32 {
        if ty == INTEGER_TYPE {
            c.as_int() as f32
        } else {
            c.as_real()
        }
    }

    fn binop(&mut self, op: BinOp, l: Cell, lt: TypeId, r: Cell, rt: TypeId) -> R<Cell> {
        let strict_div = self.env.cfg.division_by_zero_is_error;
        Ok(match op {
            BinOp::And => Cell::int((l.as_int() != 0 && r.as_int() != 0) as i32),
            BinOp::Or => Cell::int((l.as_int() != 0 || r.as_int() != 0) as i32),
            BinOp::Mul => {
                if lt == INTEGER_TYPE && rt == INTEGER_TYPE {
                    Cell::int(l.as_int().wrapping_mul(r.as_int()))
                } else {
                    Cell::real(Self::promote(l, lt) * Self::promote(r, rt))
                }
            }
            BinOp::Add => {
                if lt == INTEGER_TYPE && rt == INTEGER_TYPE {
                    Cell::int(l.as_int().wrapping_add(r.as_int()))
                } else {
                    Cell::real(Self::promote(l, lt) + Self::promote(r, rt))
                }
            }
            BinOp::Sub => {
                if lt == INTEGER_TYPE && rt == INTEGER_TYPE {
                    Cell::int(l.as_int().wrapping_sub(r.as_int()))
                } else {
                    Cell::real(Self::promote(l, lt) - Self::promote(r, rt))
                }
            }
            BinOp::Div => {
                if lt == INTEGER_TYPE && rt == INTEGER_TYPE {
                    let d = r.as_int();
                    if d == 0 {
                        if strict_div {
                            return Err(self.rt_error(RuntimeCode::DivisionByZero));
                        }
                        Cell::int(0)
                    } else {
                        Cell::int(l.as_int().wrapping_div(d))
                    }
                } else {
                    let (a, d) = (Self::promote(l, lt), Self::promote(r, rt));
                    if d == 0.0 {
                        if strict_div {
                            return Err(self.rt_error(RuntimeCode::DivisionByZero));
                        }
                        Cell::real(0.0)
                    } else {
                        Cell::real(a / d)
                    }
                }
            }
            BinOp::IntDiv | BinOp::Mod => {
                let d = r.as_int();
                if d == 0 {
                    if strict_div {
                        return Err(self.rt_error(RuntimeCode::DivisionByZero));
                    }
                    Cell::int(0)
                } else if op == BinOp::IntDiv {
                    Cell::int(l.as_int().wrapping_div(d))
                } else {
                    Cell::int(l.as_int().wrapping_rem(d))
                }
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let cmp = |o: std::cmp::Ordering, eq_nan: bool| -> bool {
                    let _ = eq_nan;
                    match op {
                        BinOp::Eq => o == std::cmp::Ordering::Equal,
                        BinOp::Ne => o != std::cmp::Ordering::Equal,
                        BinOp::Lt => o == std::cmp::Ordering::Less,
                        BinOp::Le => o != std::cmp::Ordering::Greater,
                        BinOp::Gt => o == std::cmp::Ordering::Greater,
                        _ => o != std::cmp::Ordering::Less,
                    }
                };
                let lform = self.env.types[lt.0 as usize].form;
                let lelem = self.env.types[lt.0 as usize].elem_ty;
                let result = if (lt == INTEGER_TYPE && rt == INTEGER_TYPE) || lform == Form::Enum {
                    cmp(l.as_int().cmp(&r.as_int()), false)
                } else if lt == CHAR_TYPE {
                    cmp(l.as_byte().cmp(&r.as_byte()), false)
                } else if lform == Form::Array && lelem == CHAR_TYPE {
                    // "Strings. For now, always return true..."
                    true
                } else if lt == REAL_TYPE || rt == REAL_TYPE {
                    let (a, b) = (Self::promote(l, lt), Self::promote(r, rt));
                    match op {
                        BinOp::Eq => a == b,
                        BinOp::Ne => a != b,
                        BinOp::Lt => a < b,
                        BinOp::Le => a <= b,
                        BinOp::Gt => a > b,
                        _ => a >= b,
                    }
                } else {
                    false
                };
                Cell::int(result as i32)
            }
        })
    }
}
