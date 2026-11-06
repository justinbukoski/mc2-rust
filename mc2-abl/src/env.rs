// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The ABL environment: symbol tables, module/library registry and instances (`ablenv.cpp`, `ablrtn.cpp`).
//!
//! [`Env`] corresponds to the process-wide state the C++ keeps in globals (`ModuleRegistry`, `SymTableDisplay`,
//! `stack`, eternal variables, ...). Create one per mission load: eternal variable names and module names must be
//! unique within an environment, exactly as in the original.

use std::collections::HashMap;
use std::rc::Rc;

use crate::error::{AblError, AblResult};
use crate::native::{NativeCtx, NativeFn};
use crate::natives::NATIVES;
use crate::parser::Parser;
use crate::source::{normalize_path, SourceProvider};
use crate::symtab::*;
use crate::value::{Cell, Heap};

/// Limits and behaviour switches. Defaults reproduce `initABL()` in `code/ablmc2.cpp` and the C++ defaults.
#[derive(Clone, Debug)]
pub struct Config {
    /// `AblMaxRegisteredModules`: modules (including libraries) that may be registered.
    pub max_registered_modules: usize,
    /// `AblMaxStaticVariables`: static variable declarations per module.
    pub max_static_variables: usize,
    /// `AblMaxCodeBlockSize`: bytes of crunched code per routine (approximated, see `Parser::crunch`).
    pub max_code_buffer_size: usize,
    /// `MaxEternalVariables`.
    pub max_eternal_variables: usize,
    /// `MAXSIZE_STACK`: runtime stack items (frame slots) before a stack overflow runtime error.
    pub runtime_stack_items: usize,
    /// `MAX_STATE_HANDLES_PER_MODULE`: state handles per FSM including slot 0 (so nine states).
    pub max_state_handles: usize,
    /// `initLibraryRegistry(10)`: libraries that may be loaded.
    pub max_libraries: usize,
    /// Initial `MaxLoopIterations` (loops longer than this are an infinite-loop runtime error). The script function
    /// `setMaxLoops(n)` changes it for the rest of the environment's life.
    pub max_loop_iterations: i64,
    /// Whether a debugger would be attached. The game passes `debug = false`, which makes `assert`, `print` and
    /// `#debug_start` blocks compile out by default.
    pub debugger_active: bool,
    /// Most statement nesting levels the compiler accepts (the C++ has no limit and would overflow its own stack).
    pub max_statement_depth: usize,
    /// Most array dimensions in one type.
    pub max_array_dims: usize,
    /// Largest single array in bytes. The game's ABL stack heap (statics, eternals, locals) is 511999 bytes in total.
    pub max_array_bytes: usize,
    /// Cap on all live array storage of one environment (a safety limit; the original heap is far smaller).
    pub max_heap_bytes: usize,
    /// The C++ only reports division by zero in `_DEBUG` builds; release builds yield 0. `false` = release behaviour.
    pub division_by_zero_is_error: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            max_registered_modules: 200,
            max_static_variables: 100,
            max_code_buffer_size: 102400,
            max_eternal_variables: 5000,
            runtime_stack_items: 10240,
            max_state_handles: 10,
            max_libraries: 10,
            max_loop_iterations: 100001,
            debugger_active: false,
            max_statement_depth: 100,
            max_array_dims: 16,
            max_array_bytes: 511_999,
            max_heap_bytes: 64 * 1024 * 1024,
            division_by_zero_is_error: false,
        }
    }
}

/// Services the interpreter needs from the host (`ABLi_setRandomCallbacks`, `ABLi_setDebugPrintCallback`, the user
/// file callbacks).
pub trait AblHost {
    /// `ABLRandomCallback(range)`: a value in `0..range`.
    fn random(&mut self, range: i32) -> i32;
    /// `ABLSeedRandomCallback`.
    fn seed_random(&mut self, seed: u32);
    /// `ABLDebugPrintCallback` (script `print`, when enabled).
    fn debug_print(&mut self, _msg: &str) {}
    /// `ABLGetTimeCallback`, milliseconds.
    fn time(&mut self) -> u32 {
        0
    }
    /// Open user file slot `handle` (0..6) for writing; return false on failure.
    fn user_file_open(&mut self, _handle: usize, _name: &str) -> bool {
        false
    }
    /// Append a line to a user file.
    fn user_file_write(&mut self, _handle: usize, _line: &str) {}
    /// Close a user file.
    fn user_file_close(&mut self, _handle: usize) {}
}

/// The C++ defaults (`DefaultRandom` returns 0, everything else is a no-op).
#[derive(Default, Clone, Copy, Debug)]
pub struct DefaultHost;

impl AblHost for DefaultHost {
    fn random(&mut self, _range: i32) -> i32 {
        0
    }
    fn seed_random(&mut self, _seed: u32) {}
}

/// A pluggable random number generator for the script functions `random(n)` and `seedRandom(n)`
/// (`ABLi_setRandomCallbacks`). The game plugs in its deterministic `RandomNumber`; see [`RngHost`] to use one as the
/// host's generator.
pub trait AblRng {
    /// A value in `0..range` (`0` when `range <= 0`).
    fn random(&mut self, range: i32) -> i32;
    /// Reseed (`seedRandom(seed)`).
    fn seed(&mut self, seed: u32);
}

/// Always returns 0, like the C++ `DefaultRandom`.
#[derive(Default, Clone, Copy, Debug)]
pub struct ZeroRng;

impl AblRng for ZeroRng {
    fn random(&mut self, _range: i32) -> i32 {
        0
    }
    fn seed(&mut self, _seed: u32) {}
}

/// A small deterministic xorshift generator, handy for tests and headless runs.
#[derive(Clone, Copy, Debug)]
pub struct XorShiftRng {
    state: u64,
}

impl XorShiftRng {
    /// Create with a seed.
    pub fn new(seed: u64) -> XorShiftRng {
        XorShiftRng { state: seed.max(1) }
    }
}

impl AblRng for XorShiftRng {
    fn random(&mut self, range: i32) -> i32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        if range <= 0 {
            0
        } else {
            ((x >> 33) % range as u64) as i32
        }
    }
    fn seed(&mut self, seed: u32) {
        self.state = (seed as u64).max(1);
    }
}

/// An [`AblHost`] built from a random number generator; `print` output is collected in [`RngHost::printed`] and user
/// files are not supported.
#[derive(Clone, Debug, Default)]
pub struct RngHost<R: AblRng> {
    /// The generator used for `random` / `seedRandom`.
    pub rng: R,
    /// Messages captured from `print`.
    pub printed: Vec<String>,
}

impl<R: AblRng> RngHost<R> {
    /// Wrap a generator.
    pub fn new(rng: R) -> RngHost<R> {
        RngHost {
            rng,
            printed: Vec::new(),
        }
    }
}

impl<R: AblRng> AblHost for RngHost<R> {
    fn random(&mut self, range: i32) -> i32 {
        self.rng.random(range)
    }
    fn seed_random(&mut self, seed: u32) {
        self.rng.seed(seed);
    }
    fn debug_print(&mut self, msg: &str) {
        self.printed.push(msg.to_string());
    }
}

/// A deterministic host (xorshift generator, captured `print` output).
pub type SeededHost = RngHost<XorShiftRng>;

impl SeededHost {
    /// Create with a seed.
    pub fn seeded(seed: u64) -> SeededHost {
        RngHost::new(XorShiftRng::new(seed))
    }
}

/// Implementation of a built-in function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdImpl {
    /// `return`
    Return,
    /// `print`
    Print,
    /// `concat`
    Concat,
    /// `getstatehandle`
    GetStateHandle,
    /// `assert`
    Assert,
    /// `fatal`
    Fatal,
    /// `getcurrentstatehandle`
    GetCurrentStateHandle,
    /// `abs`
    Abs,
    /// `sqrt`
    Sqrt,
    /// `round`
    Round,
    /// `trunc`
    Trunc,
    /// `random`
    Random,
    /// `seedrandom`
    SeedRandom,
    /// `setmaxloops`
    SetMaxLoops,
    /// `fileopen`
    FileOpen,
    /// `filewrite`
    FileWrite,
    /// `fileclose`
    FileClose,
    /// `getmodule`
    GetModule,
    /// `resetorders`
    ResetOrders,
    /// `setstate`
    SetState,
    /// `getfunctionhandle`
    GetFunctionHandle,
    /// `callfunction`
    CallFunction,
    /// `setflag`
    SetFlag,
    /// `getflag`
    GetFlag,
    /// Game function from the generated table (index into [`NATIVES`]).
    Native(u16),
    /// Host-registered function (index into the custom function list).
    Custom(u16),
}

/// An entry of `FunctionInfoTable`.
#[derive(Clone, Debug)]
pub struct StdFn {
    /// Lower-case script name.
    pub name: String,
    /// Implementation.
    pub imp: StdImpl,
    /// Parameter kinds.
    pub params: Vec<ParamKind>,
    /// Return kind.
    pub ret: ReturnKind,
    /// Registered with `isOrder`.
    pub is_order: bool,
    /// `*` parameters whose C++ callback uses `ABLi_popReal` (reads an integer actual's bits as a float).
    pub star_reinterprets: bool,
}

impl StdFn {
    /// True when parameter `i` is a `*` parameter that reinterprets an integer actual as float bits.
    pub fn star_int_reinterprets(&self, i: usize) -> bool {
        self.star_reinterprets && self.params.get(i) == Some(&ParamKind::IntegerReal)
    }
}

/// Handle of a registered (preprocessed) module: index into the module registry.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct ModuleHandle(pub usize);

/// `ModuleEntry`.
#[derive(Debug, Clone)]
pub struct ModuleEntry {
    /// Lower-cased source file name.
    pub file_name: String,
    /// The module's identifier symbol.
    pub module_sym: SymId,
    /// All source files (module + includes), index 0 is the module itself.
    pub source_files: Vec<String>,
    /// Libraries referenced.
    pub libraries_used: Vec<InstId>,
    /// Per static variable: byte size for arrays, 0 for scalars.
    pub static_sizes: Vec<i32>,
    /// Order-call sites in the module (including its functions).
    pub num_order_calls: u32,
    /// State handle table (index 0 unused).
    pub state_handles: Vec<(String, SymId)>,
    /// Instances created from this module.
    pub num_instances: u32,
    /// Library instance if this module is a library.
    pub library: Option<InstId>,
    /// Source lines processed.
    pub lines: usize,
}

/// `ABLModule`: one running instance of a module, brain or library.
#[derive(Debug)]
pub struct Instance {
    /// Registry entry.
    pub module: ModuleHandle,
    /// Static data (arrays are heap ids).
    pub statics: Vec<Cell>,
    /// Order-call completion bits.
    pub order_flags: Vec<u32>,
    /// `initCalled`.
    pub init_called: bool,
    /// FSM previous state.
    pub prev_state: Option<SymId>,
    /// FSM current state.
    pub state: Option<SymId>,
    /// `returnVal` of the last execute.
    pub return_val: Cell,
    /// `setName`.
    pub name: String,
    /// Whether the instance has been initialized (placeholder instances for libraries during compile are not).
    pub live: bool,
}

/// A host-registered function.
pub(crate) struct CustomFn {
    pub f: Box<NativeFn>,
}

#[derive(Clone, Copy, Debug)]
struct BstNode {
    sym: SymId,
    left: u32,
    right: u32,
}

/// The environment.
pub struct Env {
    /// Configuration (limits and switches).
    pub cfg: Config,
    pub(crate) provider: Rc<dyn SourceProvider>,
    pub(crate) host: Box<dyn AblHost>,
    pub(crate) types: Vec<TypeInfo>,
    pub(crate) syms: Vec<Sym>,
    pub(crate) scopes: Vec<Scope>,
    pub(crate) std_fns: Vec<StdFn>,
    pub(crate) custom_fns: Vec<CustomFn>,
    pub(crate) heap: Heap,
    str_lits: HashMap<Vec<u8>, (u32, TypeId)>,
    pub(crate) eternals: Vec<Cell>,
    pub(crate) modules: Vec<ModuleEntry>,
    pub(crate) instances: Vec<Instance>,
    /// Scopes of loaded libraries in the order the C++ `searchLibrarySymTable` visits them: a pre-order walk of the
    /// binary tree that holds the level-0 symbols (so it depends on the tree shape, not on load order).
    pub(crate) library_scopes: Vec<(InstId, ScopeId)>,
    /// Shape of the level-0 symbol tree (`SymTableDisplay[0]`), needed to reproduce that visiting order.
    bst: Vec<BstNode>,
    pub(crate) libraries_loaded: usize,
    pub(crate) rt: crate::interp::Runtime,
    /// Number of `execute*` calls running on this environment (more than one when a native ran a nested brain).
    pub(crate) exec_depth: u32,
    /// Non-fatal observations made while compiling (for example integer actuals for real `*` parameters).
    pub warnings: Vec<String>,
}

impl Env {
    /// Build an environment (`ABLi_init`): predefined types, the standard functions and the game functions.
    pub fn new(
        cfg: Config,
        provider: impl SourceProvider + 'static,
        host: Box<dyn AblHost>,
    ) -> Env {
        let max_loops = cfg.max_loop_iterations;
        let mut env = Env {
            cfg,
            provider: Rc::new(provider),
            host,
            types: Vec::new(),
            syms: Vec::new(),
            scopes: vec![Scope::default()],
            std_fns: Vec::new(),
            custom_fns: Vec::new(),
            heap: Heap::default(),
            str_lits: HashMap::new(),
            eternals: Vec::new(),
            modules: Vec::new(),
            instances: Vec::new(),
            library_scopes: Vec::new(),
            bst: Vec::new(),
            libraries_loaded: 0,
            rt: crate::interp::Runtime::new(max_loops),
            exec_depth: 0,
            warnings: Vec::new(),
        };
        env.init_symtable();
        env
    }

    // ------------------------------------------------------------------------------------------
    // arenas
    // ------------------------------------------------------------------------------------------

    pub(crate) fn sym(&self, id: SymId) -> &Sym {
        &self.syms[id.0 as usize]
    }

    pub(crate) fn sym_mut(&mut self, id: SymId) -> &mut Sym {
        &mut self.syms[id.0 as usize]
    }

    pub(crate) fn new_sym(&mut self, name: &str, level: u8) -> SymId {
        self.syms.push(Sym {
            name: name.to_string(),
            defn: Defn::Undefined,
            ty: None,
            level,
            library: None,
        });
        SymId((self.syms.len() - 1) as u32)
    }

    /// Insert into a scope table. Level 0 additionally maintains the C++ binary-tree shape.
    pub(crate) fn scope_insert(&mut self, scope: ScopeId, name: &str, id: SymId) {
        self.scopes[scope.0 as usize].insert(name, id);
        if scope.0 == 0 {
            self.bst_insert(name, id);
        }
    }

    fn bst_insert(&mut self, name: &str, id: SymId) {
        const NONE: u32 = u32::MAX;
        let new = self.bst.len() as u32;
        self.bst.push(BstNode {
            sym: id,
            left: NONE,
            right: NONE,
        });
        if new == 0 {
            return;
        }
        let mut cur = 0u32;
        loop {
            let go_left = name.as_bytes()
                < self.syms[self.bst[cur as usize].sym.0 as usize]
                    .name
                    .as_bytes();
            let next = if go_left {
                self.bst[cur as usize].left
            } else {
                self.bst[cur as usize].right
            };
            if next == NONE {
                if go_left {
                    self.bst[cur as usize].left = new;
                } else {
                    self.bst[cur as usize].right = new;
                }
                return;
            }
            cur = next;
        }
    }

    /// Order `library_scopes` like a pre-order walk of the level-0 tree.
    fn sort_library_scopes(&mut self) {
        const NONE: u32 = u32::MAX;
        let mut order: Vec<SymId> = Vec::new();
        let mut stack: Vec<u32> = if self.bst.is_empty() {
            Vec::new()
        } else {
            vec![0]
        };
        while let Some(n) = stack.pop() {
            let node = self.bst[n as usize];
            order.push(node.sym);
            if node.right != NONE {
                stack.push(node.right);
            }
            if node.left != NONE {
                stack.push(node.left);
            }
        }
        let mut sorted = Vec::new();
        for sym in order {
            let s = &self.syms[sym.0 as usize];
            if let (Defn::Module(r), Some(lib)) = (&s.defn, s.library) {
                if let Some(scope) = r.scope {
                    if self
                        .library_scopes
                        .iter()
                        .any(|&(i, sc)| i == lib && sc == scope)
                    {
                        sorted.push((lib, scope));
                    }
                }
            }
        }
        self.library_scopes = sorted;
    }

    pub(crate) fn new_scope(&mut self) -> ScopeId {
        self.scopes.push(Scope::default());
        ScopeId((self.scopes.len() - 1) as u32)
    }

    pub(crate) fn new_type(&mut self, t: TypeInfo) -> TypeId {
        self.types.push(t);
        TypeId((self.types.len() - 1) as u32)
    }

    /// Intern a string literal (heap array holding the bytes plus NUL) and its array type.
    pub(crate) fn intern_string(&mut self, bytes: &[u8]) -> (u32, TypeId) {
        if let Some(&r) = self.str_lits.get(bytes) {
            return r;
        }
        let arr = self.heap.alloc_cstr(bytes);
        let mut t = TypeInfo::new();
        t.form = Form::Array;
        t.size = bytes.len() as i32;
        t.index_ty = INTEGER_TYPE;
        t.elem_ty = CHAR_TYPE;
        t.elem_count = bytes.len() as i32 + 1;
        let ty = self.new_type(t);
        self.str_lits.insert(bytes.to_vec(), (arr, ty));
        (arr, ty)
    }

    /// Allocate script array storage within `max_array_bytes` / `max_heap_bytes`.
    pub(crate) fn alloc_array(&mut self, size: usize) -> AblResult<u32> {
        if size > self.cfg.max_array_bytes
            || self.heap.live_bytes() + size > self.cfg.max_heap_bytes
        {
            return Err(AblError::fatal(format!(
                " ABL: out of array memory (requested {size} bytes, {} in use)",
                self.heap.live_bytes()
            )));
        }
        Ok(self.heap.alloc(size))
    }

    /// Reserve an eternal variable slot, zero-initialized; arrays get their own zeroed heap array.
    pub(crate) fn alloc_eternal(&mut self, is_array: bool, size: i32) -> AblResult<u32> {
        if self.eternals.len() >= self.cfg.max_eternal_variables {
            return Err(AblError::fatal(" ABL: too many eternal variables "));
        }
        let cell = if is_array {
            let id = self.alloc_array(size.max(0) as usize)?;
            Cell::Arr(crate::value::ArrRef { id, off: 0 })
        } else {
            Cell::ZERO
        };
        self.eternals.push(cell);
        Ok((self.eternals.len() - 1) as u32)
    }

    // ------------------------------------------------------------------------------------------
    // symbol table initialization (initSymTable, initStandardRoutines, ABLi_addFunction)
    // ------------------------------------------------------------------------------------------

    fn init_symtable(&mut self) {
        let mk_type = |env: &mut Env, name: &str, form: Form, size: i32| -> TypeId {
            let mut t = TypeInfo::new();
            t.form = form;
            t.size = size;
            let id = env.new_type(t);
            let s = env.new_sym(name, 0);
            env.scope_insert(ScopeId(0), name, s);
            let sym = env.sym_mut(s);
            sym.defn = Defn::Type;
            sym.ty = Some(id);
            env.types[id.0 as usize].type_sym = Some(s);
            id
        };
        let i = mk_type(self, "integer", Form::Scalar, 4);
        let c = mk_type(self, "char", Form::Scalar, 1);
        let r = mk_type(self, "real", Form::Scalar, 4);
        let b = mk_type(self, "boolean", Form::Enum, 4);
        assert_eq!(
            (i, c, r, b),
            (INTEGER_TYPE, CHAR_TYPE, REAL_TYPE, BOOLEAN_TYPE)
        );
        let d = self.new_type(TypeInfo::new());
        assert_eq!(d, DUMMY_TYPE);
        self.types[BOOLEAN_TYPE.0 as usize].enum_max = 1;

        for (name, v) in [("false", 0), ("true", 1)] {
            let s = self.new_sym(name, 0);
            self.scope_insert(ScopeId(0), name, s);
            let sym = self.sym_mut(s);
            sym.defn = Defn::Const(ConstValue::Int(v));
            sym.ty = Some(BOOLEAN_TYPE);
        }

        use StdImpl::*;
        let std: &[(&str, StdImpl, &str, &str)] = &[
            ("return", Return, "", ""),
            ("print", Print, "", ""),
            ("concat", Concat, "", ""),
            ("getstatehandle", GetStateHandle, "C", "i"),
            ("fatal", Fatal, "iC", ""),
            ("assert", Assert, "biC", ""),
            ("getcurrentstatehandle", GetCurrentStateHandle, "", "i"),
            ("abs", Abs, "*", "r"),
            ("sqrt", Sqrt, "*", "r"),
            ("round", Round, "r", "i"),
            ("trunc", Trunc, "r", "i"),
            ("random", Random, "i", "i"),
            ("seedrandom", SeedRandom, "i", ""),
            ("setmaxloops", SetMaxLoops, "i", ""),
            ("fileopen", FileOpen, "C", "i"),
            ("filewrite", FileWrite, "iC", ""),
            ("fileclose", FileClose, "i", ""),
            ("getmodule", GetModule, "CC", "i"),
            ("resetorders", ResetOrders, "i", ""),
            ("setstate", SetState, "i", ""),
            ("getfunctionhandle", GetFunctionHandle, "C", "i"),
            ("callfunction", CallFunction, "i", ""),
            ("setflag", SetFlag, "iib", "i"),
            ("getflag", GetFlag, "ii", "b"),
        ];
        for &(name, imp, params, ret) in std {
            self.add_builtin(name, imp, false, params, ret, false);
        }
        for (i, n) in NATIVES.iter().enumerate() {
            self.add_builtin(
                n.name,
                StdImpl::Native(i as u16),
                n.is_order,
                n.params,
                n.ret,
                n.star_reinterprets,
            );
        }
    }

    fn add_builtin(
        &mut self,
        name: &str,
        imp: StdImpl,
        is_order: bool,
        params: &str,
        ret: &str,
        star: bool,
    ) {
        let params: Vec<ParamKind> = params
            .chars()
            .map(|c| {
                ParamKind::from_char(c).unwrap_or_else(|| panic!("bad param type {c} for {name}"))
            })
            .collect();
        let ret = ret
            .chars()
            .next()
            .map(|c| ReturnKind::from_char(c).expect("bad return type"))
            .unwrap_or(ReturnKind::None);
        self.std_fns.push(StdFn {
            name: name.to_string(),
            imp,
            params,
            ret,
            is_order,
            star_reinterprets: star,
        });
        let idx = (self.std_fns.len() - 1) as u16;
        let s = self.new_sym(name, 0);
        self.scope_insert(ScopeId(0), name, s);
        let flags = if is_order { ROUTINE_FLAG_ORDER } else { 0 };
        let sym = self.sym_mut(s);
        sym.defn = Defn::Function(Box::new(Routine::new(RoutineKey::Builtin(idx), flags)));
        sym.ty = None;
    }

    /// `ABLi_addFunction`: register a host function implemented by a closure. `params` and `ret` use the same
    /// signature letters as the C++ (`i r b c * ? C I R B`).
    pub fn add_function(
        &mut self,
        name: &str,
        is_order: bool,
        params: &str,
        ret: &str,
        f: impl FnMut(&mut NativeCtx<'_>, &[crate::native::NativeArg]) -> crate::native::NativeRet
            + 'static,
    ) {
        self.custom_fns.push(CustomFn { f: Box::new(f) });
        let idx = (self.custom_fns.len() - 1) as u16;
        self.add_builtin(
            &name.to_ascii_lowercase(),
            StdImpl::Custom(idx),
            is_order,
            params,
            ret,
            false,
        );
    }

    // ------------------------------------------------------------------------------------------
    // module registry
    // ------------------------------------------------------------------------------------------

    /// `ABLi_preProcess`: compile a module (or return the existing handle if this file was already compiled).
    pub fn pre_process(&mut self, path: &str) -> AblResult<ModuleHandle> {
        self.pre_process_inner(path, None)
    }

    fn pre_process_inner(
        &mut self,
        path: &str,
        library: Option<InstId>,
    ) -> AblResult<ModuleHandle> {
        let norm = normalize_path(path);
        if let Some(i) = self.modules.iter().position(|m| m.file_name == norm) {
            return Ok(ModuleHandle(i));
        }
        if self.modules.len() >= self.cfg.max_registered_modules {
            return Err(AblError::fatal(" ABL: too many registered modules "));
        }
        let parsed = Parser::new(self, library).parse_module(&norm)?;
        self.modules.push(ModuleEntry {
            file_name: norm,
            module_sym: parsed.module_sym,
            source_files: parsed.source_files,
            libraries_used: parsed.libraries_used,
            static_sizes: parsed.static_sizes,
            num_order_calls: parsed.num_order_calls,
            state_handles: parsed.state_handles,
            num_instances: 0,
            library,
            lines: parsed.lines,
        });
        Ok(ModuleHandle(self.modules.len() - 1))
    }

    /// `ABLi_loadLibrary`: compile a library and create its single instance. Returns `None` when the library was
    /// already loaded.
    pub fn load_library(&mut self, path: &str) -> AblResult<Option<InstId>> {
        let norm = normalize_path(path);
        if self.modules.iter().any(|m| m.file_name == norm) {
            return Ok(None);
        }
        if self.libraries_loaded >= self.cfg.max_libraries {
            return Err(AblError::fatal(" ABL: too many libraries loaded "));
        }
        let inst = InstId(self.instances.len() as u32);
        self.instances.push(Instance {
            module: ModuleHandle(usize::MAX),
            statics: Vec::new(),
            order_flags: Vec::new(),
            init_called: false,
            prev_state: None,
            state: None,
            return_val: Cell::ZERO,
            name: String::new(),
            live: false,
        });
        let handle = self.pre_process_inner(&norm, Some(inst))?;
        let module_sym = self.modules[handle.0].module_sym;
        let scope = self
            .sym(module_sym)
            .routine()
            .and_then(|r| r.scope)
            .expect("library scope");
        self.library_scopes.push((inst, scope));
        self.sort_library_scopes();
        self.libraries_loaded += 1;
        self.init_instance(inst, handle)?;
        self.instances[inst.0 as usize].name = short_name(path);
        Ok(Some(inst))
    }

    /// `new ABLModule; module->init(handle)`.
    pub fn new_instance(&mut self, handle: ModuleHandle) -> AblResult<InstId> {
        let inst = InstId(self.instances.len() as u32);
        self.instances.push(Instance {
            module: handle,
            statics: Vec::new(),
            order_flags: Vec::new(),
            init_called: false,
            prev_state: None,
            state: None,
            return_val: Cell::ZERO,
            name: String::new(),
            live: false,
        });
        self.init_instance(inst, handle)?;
        Ok(inst)
    }

    fn init_instance(&mut self, inst: InstId, handle: ModuleHandle) -> AblResult<()> {
        let (sizes, num_orders, module_sym) = {
            let m = &self.modules[handle.0];
            (m.static_sizes.clone(), m.num_order_calls, m.module_sym)
        };
        let mut statics = Vec::with_capacity(sizes.len());
        for &s in &sizes {
            if s > 0 {
                let id = self.alloc_array(s as usize)?;
                statics.push(Cell::Arr(crate::value::ArrRef { id, off: 0 }));
            } else {
                statics.push(Cell::ZERO);
            }
        }
        let order_flags = if num_orders > 0 {
            vec![0u32; 1 + (num_orders / 32) as usize]
        } else {
            Vec::new()
        };
        let mut state = None;
        if self.sym(module_sym).flags() & ROUTINE_FLAG_FSM != 0 {
            let scope = self
                .sym(module_sym)
                .routine()
                .and_then(|r| r.scope)
                .expect("module scope");
            let start = self.scopes[scope.0 as usize]
                .candidates("start")
                .first()
                .copied();
            match start {
                Some(s) => state = Some(s),
                None => {
                    return Err(AblError::fatal(format!(
                        "ABL: FSM has no Start state [{}]",
                        self.modules[handle.0].file_name
                    )))
                }
            }
        }
        let i = &mut self.instances[inst.0 as usize];
        i.module = handle;
        i.statics = statics;
        i.order_flags = order_flags;
        i.state = state;
        i.prev_state = None;
        i.init_called = false;
        i.live = true;
        self.modules[handle.0].num_instances += 1;
        Ok(())
    }

    /// Destroy an instance and release its static arrays (`ABLModule::destroy`).
    pub fn destroy_instance(&mut self, inst: InstId) {
        let statics = std::mem::take(&mut self.instances[inst.0 as usize].statics);
        for c in statics {
            if let Cell::Arr(a) = c {
                self.heap.release(a.id);
            }
        }
        self.instances[inst.0 as usize].live = false;
    }

    // ------------------------------------------------------------------------------------------
    // queries (ABLModule accessors)
    // ------------------------------------------------------------------------------------------

    /// Registry entry.
    pub fn module(&self, handle: ModuleHandle) -> &ModuleEntry {
        &self.modules[handle.0]
    }

    /// Number of registered modules (including libraries).
    pub fn num_modules(&self) -> usize {
        self.modules.len()
    }

    /// The registry handle of an instance.
    pub fn instance_module(&self, inst: InstId) -> ModuleHandle {
        self.instances[inst.0 as usize].module
    }

    /// `ABLModule::setName`.
    pub fn set_name(&mut self, inst: InstId, name: &str) {
        self.instances[inst.0 as usize].name = name.to_string();
    }

    /// `ABLModule::getName`.
    pub fn name(&self, inst: InstId) -> &str {
        &self.instances[inst.0 as usize].name
    }

    /// `ABLModule::getInteger` (the module's return value after `execute`).
    pub fn get_integer(&self, inst: InstId) -> i32 {
        self.instances[inst.0 as usize].return_val.as_int()
    }

    /// `ABLModule::getReal`.
    pub fn get_real(&self, inst: InstId) -> f32 {
        self.instances[inst.0 as usize].return_val.as_real()
    }

    /// `ABLModule::isLibrary`.
    pub fn is_library(&self, inst: InstId) -> bool {
        self.modules[self.instances[inst.0 as usize].module.0]
            .library
            .is_some()
    }

    /// Current FSM state name, if the instance is an FSM.
    pub fn state_name(&self, inst: InstId) -> Option<&str> {
        self.instances[inst.0 as usize]
            .state
            .map(|s| self.sym(s).name.as_str())
    }

    /// Previous FSM state name.
    pub fn prev_state_name(&self, inst: InstId) -> Option<&str> {
        self.instances[inst.0 as usize]
            .prev_state
            .map(|s| self.sym(s).name.as_str())
    }

    /// `ABLModule::getStateHandle`: index into the module's state handle table (0 if none).
    pub fn state_handle(&self, inst: InstId) -> i32 {
        let i = &self.instances[inst.0 as usize];
        let Some(state) = i.state else { return 0 };
        let name = &self.sym(state).name;
        self.modules[i.module.0]
            .state_handles
            .iter()
            .position(|(n, _)| n == name)
            .unwrap_or(0) as i32
    }

    /// `ABLModule::findStateHandle`.
    pub fn find_state_handle(&self, inst: InstId, name: &str) -> i32 {
        let m = &self.modules[self.instances[inst.0 as usize].module.0];
        let lower = name.to_ascii_lowercase();
        m.state_handles
            .iter()
            .skip(1)
            .position(|(n, _)| *n == lower)
            .map(|p| p as i32 + 1)
            .unwrap_or(0)
    }

    /// `ABLModule::findFunction`.
    pub fn find_function(&self, inst: InstId, name: &str, search_libraries: bool) -> Option<SymId> {
        let m = &self.modules[self.instances[inst.0 as usize].module.0];
        let lower = name.to_ascii_lowercase();
        let scope = self.sym(m.module_sym).routine().and_then(|r| r.scope)?;
        let found = self.scopes[scope.0 as usize]
            .candidates(&lower)
            .iter()
            .copied()
            .find(|&s| self.sym(s).ty.is_none() && matches!(self.sym(s).defn, Defn::Function(_)));
        if found.is_some() || !search_libraries {
            return found;
        }
        for lib in &m.libraries_used {
            let lm = &self.modules[self.instances[lib.0 as usize].module.0];
            if let Some(sc) = self.sym(lm.module_sym).routine().and_then(|r| r.scope) {
                if let Some(&s) = self.scopes[sc.0 as usize].candidates(&lower).first() {
                    return Some(s);
                }
            }
        }
        None
    }

    /// Name of a symbol.
    pub fn sym_name(&self, id: SymId) -> &str {
        &self.sym(id).name
    }

    /// `ABLModule::findSymbol` restricted to the module's own scope.
    pub fn find_symbol(&self, inst: InstId, name: &str) -> Option<SymId> {
        let m = &self.modules[self.instances[inst.0 as usize].module.0];
        let scope = self.sym(m.module_sym).routine().and_then(|r| r.scope)?;
        self.scopes[scope.0 as usize]
            .candidates(&name.to_ascii_lowercase())
            .first()
            .copied()
    }

    /// `ABLModule::setStaticInteger`. Returns 0 on success, 1 unknown, 2 wrong type, 3 not static.
    pub fn set_static_integer(&mut self, inst: InstId, name: &str, value: i32) -> i32 {
        self.set_static_scalar(inst, name, INTEGER_TYPE, Cell::int(value))
    }

    /// `ABLModule::setStaticReal`.
    pub fn set_static_real(&mut self, inst: InstId, name: &str, value: f32) -> i32 {
        self.set_static_scalar(inst, name, REAL_TYPE, Cell::real(value))
    }

    fn set_static_scalar(&mut self, inst: InstId, name: &str, ty: TypeId, v: Cell) -> i32 {
        let Some(sym) = self.find_symbol(inst, name) else {
            return 1;
        };
        let s = self.sym(sym);
        if s.ty != Some(ty) {
            return 2;
        }
        match s.defn {
            Defn::Var {
                kind: VarKind::Static,
                offset,
            } => {
                self.instances[inst.0 as usize].statics[offset as usize] = v;
                0
            }
            _ => 3,
        }
    }

    /// `ABLModule::getStaticInteger` (`None` when unknown / wrong type / not static).
    pub fn get_static_integer(&self, inst: InstId, name: &str) -> Option<i32> {
        self.get_static_cell(inst, name, INTEGER_TYPE)
            .map(Cell::as_int)
    }

    /// `ABLModule::getStaticReal`.
    pub fn get_static_real(&self, inst: InstId, name: &str) -> Option<f32> {
        self.get_static_cell(inst, name, REAL_TYPE)
            .map(Cell::as_real)
    }

    fn get_static_cell(&self, inst: InstId, name: &str, ty: TypeId) -> Option<Cell> {
        let sym = self.find_symbol(inst, name)?;
        let s = self.sym(sym);
        if s.ty != Some(ty) {
            return None;
        }
        match s.defn {
            Defn::Var {
                kind: VarKind::Static,
                offset,
            } => Some(self.instances[inst.0 as usize].statics[offset as usize]),
            _ => None,
        }
    }

    /// `ABLModule::setStaticIntegerArray`: copy `values` into a static integer array. Returns 0 on success.
    pub fn set_static_integer_array(&mut self, inst: InstId, name: &str, values: &[i32]) -> i32 {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.set_static_array_bytes(inst, name, &bytes)
    }

    /// `ABLModule::setStaticRealArray`.
    pub fn set_static_real_array(&mut self, inst: InstId, name: &str, values: &[f32]) -> i32 {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.set_static_array_bytes(inst, name, &bytes)
    }

    fn set_static_array_bytes(&mut self, inst: InstId, name: &str, bytes: &[u8]) -> i32 {
        let Some(sym) = self.find_symbol(inst, name) else {
            return 1;
        };
        match self.sym(sym).defn {
            Defn::Var {
                kind: VarKind::Static,
                offset,
            } => {
                if let Cell::Arr(a) = self.instances[inst.0 as usize].statics[offset as usize] {
                    let dst = self.heap.bytes_mut(a.id);
                    let n = bytes.len().min(dst.len());
                    dst[..n].copy_from_slice(&bytes[..n]);
                    0
                } else {
                    2
                }
            }
            _ => 3,
        }
    }

    /// `ABLModule::getStaticIntegerArray`.
    pub fn get_static_integer_array(
        &self,
        inst: InstId,
        name: &str,
        count: usize,
    ) -> Option<Vec<i32>> {
        let bytes = self.static_array_bytes(inst, name, count * 4)?;
        Some(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| i32::from_le_bytes(*c))
                .collect(),
        )
    }

    /// `ABLModule::getStaticRealArray`.
    pub fn get_static_real_array(
        &self,
        inst: InstId,
        name: &str,
        count: usize,
    ) -> Option<Vec<f32>> {
        let bytes = self.static_array_bytes(inst, name, count * 4)?;
        Some(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect(),
        )
    }

    fn static_array_bytes(&self, inst: InstId, name: &str, len: usize) -> Option<Vec<u8>> {
        let sym = self.find_symbol(inst, name)?;
        match self.sym(sym).defn {
            Defn::Var {
                kind: VarKind::Static,
                offset,
            } => {
                if let Cell::Arr(a) = self.instances[inst.0 as usize].statics[offset as usize] {
                    let src = self.heap.bytes(a.id);
                    Some(src[..len.min(src.len())].to_vec())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Read an eternal variable by name (diagnostics and tests). Arrays return `None`.
    pub fn eternal_integer(&self, name: &str) -> Option<i32> {
        let s = self.scopes[0]
            .candidates(&name.to_ascii_lowercase())
            .first()
            .copied()?;
        match self.sym(s).defn {
            Defn::Var {
                kind: VarKind::Eternal,
                offset,
            } => match self.eternals[offset as usize] {
                Cell::Bits(b) => Some(b as i32),
                _ => None,
            },
            _ => None,
        }
    }

    /// Number of live heap arrays (strings, locals, statics, eternals); useful to check that frames release their
    /// arrays.
    pub fn live_arrays(&self) -> usize {
        self.heap.live()
    }

    /// Number of eternal variable slots in use.
    pub fn num_eternals(&self) -> usize {
        self.eternals.len()
    }

    /// Call `ABLi_resetOrders` for an instance (clears all order-completion bits).
    pub fn reset_orders(&mut self, inst: InstId) {
        self.instances[inst.0 as usize].order_flags.fill(0);
    }
}

fn short_name(path: &str) -> String {
    // `ABLModule::setName` keeps the first MAX_ABLMODULE_NAME-1 characters.
    path.chars().take(4).collect()
}
