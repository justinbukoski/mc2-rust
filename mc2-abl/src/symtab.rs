// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Types, symbols and scopes (`mclib/ablsymt.h/.cpp`).
//!
//! The C++ uses pointer-linked binary trees; here symbols live in arenas addressed by small ids. Lookup semantics are
//! preserved: within one scope the *first inserted* symbol with a given (case-folded) name wins, and the "search for
//! function / state / string" variants skip entries that do not qualify.

use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::Body;

/// Index of a type in the type arena. Type identity (`==`) is significant for assignment compatibility.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct TypeId(pub u32);

/// `IntegerTypePtr`.
pub const INTEGER_TYPE: TypeId = TypeId(0);
/// `CharTypePtr`.
pub const CHAR_TYPE: TypeId = TypeId(1);
/// `RealTypePtr`.
pub const REAL_TYPE: TypeId = TypeId(2);
/// `BooleanTypePtr`.
pub const BOOLEAN_TYPE: TypeId = TypeId(3);
/// `DummyType`, used for erroneous types.
pub const DUMMY_TYPE: TypeId = TypeId(4);

/// Index of a symbol in the symbol arena.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct SymId(pub u32);

/// Index of a scope table.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct ScopeId(pub u32);

/// Identifies a module instance (`ABLModule`). Library symbols record the library's instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct InstId(pub u32);

/// `FormType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Form {
    /// Erroneous / unset.
    None,
    /// Integer, char, real.
    Scalar,
    /// Enumeration (and boolean).
    Enum,
    /// Array (also string literals and `char` arrays).
    Array,
}

/// A type descriptor (`Type`).
#[derive(Clone, Debug)]
pub struct TypeInfo {
    /// Form of the type.
    pub form: Form,
    /// Size in bytes (arrays: total bytes of data).
    pub size: i32,
    /// Declaring type identifier, if any.
    pub type_sym: Option<SymId>,
    /// Array: number of elements in this dimension.
    pub elem_count: i32,
    /// Array: index type (always integer here).
    pub index_ty: TypeId,
    /// Array: element type (possibly another array for extra dimensions).
    pub elem_ty: TypeId,
    /// Enum: highest value.
    pub enum_max: i32,
}

impl TypeInfo {
    /// A fresh `createType()` result.
    pub fn new() -> TypeInfo {
        TypeInfo {
            form: Form::None,
            size: 0,
            type_sym: None,
            elem_count: 0,
            index_ty: DUMMY_TYPE,
            elem_ty: DUMMY_TYPE,
            enum_max: 0,
        }
    }
}

impl Default for TypeInfo {
    fn default() -> Self {
        TypeInfo::new()
    }
}

/// `VariableType` (the registered-variable kind of the C++ is not supported).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VarKind {
    /// Lives in the stack frame.
    Normal,
    /// Per module instance.
    Static,
    /// Global across modules.
    Eternal,
}

/// A constant's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstValue {
    /// Integer, boolean or enum value.
    Int(i32),
    /// Real value.
    Real(f32),
    /// Character value.
    Char(u8),
    /// String constant (literal array id).
    Str(u32),
}

/// Routine flags.
pub const ROUTINE_FLAG_ORDER: u8 = 1;
/// The routine is an FSM module.
pub const ROUTINE_FLAG_FSM: u8 = 2;
/// The routine is a state.
pub const ROUTINE_FLAG_STATE: u8 = 4;

/// What kind of routine a function symbol is (`RoutineKey` plus the table index for built-ins).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoutineKey {
    /// A script-defined routine with a body.
    Declared,
    /// Forward declaration without a body yet.
    Forward,
    /// Built-in (standard function or game native); index into the standard table.
    Builtin(u16),
}

/// Information shared by modules and functions (`Routine`).
#[derive(Clone, Debug)]
pub struct Routine {
    /// Routine kind.
    pub key: RoutineKey,
    /// `ROUTINE_FLAG_*`.
    pub flags: u8,
    /// First order-call index belonging to this routine.
    pub order_call_index: u32,
    /// Number of order-call sites in this routine.
    pub num_order_calls: u32,
    /// Formal parameters in order.
    pub params: Vec<SymId>,
    /// Normal (stack) local variables in declaration order.
    pub locals: Vec<SymId>,
    /// This routine's own scope (level 1 for modules, level 2 for functions).
    pub scope: Option<ScopeId>,
    /// Compiled body (`code` ... end token).
    pub body: Option<Rc<Body>>,
    /// Approximate code size in bytes of the C++ code segment.
    pub code_size: usize,
}

impl Routine {
    /// A blank routine of the given kind.
    pub fn new(key: RoutineKey, flags: u8) -> Routine {
        Routine {
            key,
            flags,
            order_call_index: 0,
            num_order_calls: 0,
            params: Vec::new(),
            locals: Vec::new(),
            scope: None,
            body: None,
            code_size: 0,
        }
    }
}

/// `Definition`.
#[derive(Clone, Debug)]
pub enum Defn {
    /// `DFN_UNDEFINED`.
    Undefined,
    /// `DFN_CONST`.
    Const(ConstValue),
    /// `DFN_TYPE`.
    Type,
    /// `DFN_VAR`: kind and slot (frame slot index, static index or eternal index).
    Var {
        /// Storage class.
        kind: VarKind,
        /// Slot index within that storage.
        offset: u32,
    },
    /// `DFN_VALPARAM`: frame slot.
    ValParam {
        /// Slot index within the frame.
        offset: u32,
    },
    /// `DFN_REFPARAM`: frame slot.
    RefParam {
        /// Slot index within the frame.
        offset: u32,
    },
    /// `DFN_MODULE`.
    Module(Box<Routine>),
    /// `DFN_FUNCTION`.
    Function(Box<Routine>),
}

/// A symbol table node (`SymTableNode`).
#[derive(Clone, Debug)]
pub struct Sym {
    /// Case-folded name.
    pub name: String,
    /// Definition.
    pub defn: Defn,
    /// Type. For functions this is the return type (`None` = procedure); for modules the return type.
    pub ty: Option<TypeId>,
    /// Scope level the symbol was entered at.
    pub level: u8,
    /// Owning library instance (`SymTableNode::library`), `None` for modules and built-ins.
    pub library: Option<InstId>,
}

impl Sym {
    /// The routine info if this is a function or module.
    pub fn routine(&self) -> Option<&Routine> {
        match &self.defn {
            Defn::Function(r) | Defn::Module(r) => Some(r),
            _ => None,
        }
    }

    /// Mutable routine info.
    pub fn routine_mut(&mut self) -> Option<&mut Routine> {
        match &mut self.defn {
            Defn::Function(r) | Defn::Module(r) => Some(r),
            _ => None,
        }
    }

    /// Flags of the routine (0 for non-routines).
    pub fn flags(&self) -> u8 {
        self.routine().map(|r| r.flags).unwrap_or(0)
    }
}

/// One scope table.
#[derive(Default, Debug)]
pub struct Scope {
    map: HashMap<String, Vec<SymId>>,
}

impl Scope {
    /// All symbols with this exact name in insertion order.
    pub fn candidates(&self, name: &str) -> &[SymId] {
        self.map.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Insert a symbol (duplicates allowed, as `enterSymTable`).
    pub fn insert(&mut self, name: &str, id: SymId) {
        self.map.entry(name.to_string()).or_default().push(id);
    }
}

/// Parameter kinds of built-in functions (`FunctionParamType`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamKind {
    /// `?`
    Anything,
    /// `c`
    Char,
    /// `i`
    Integer,
    /// `r`
    Real,
    /// `b`
    Boolean,
    /// `*`
    IntegerReal,
    /// `C`
    CharArray,
    /// `I`
    IntegerArray,
    /// `R`
    RealArray,
    /// `B`
    BooleanArray,
}

impl ParamKind {
    /// Parse one signature character.
    pub fn from_char(c: char) -> Option<ParamKind> {
        Some(match c {
            '?' => ParamKind::Anything,
            'c' => ParamKind::Char,
            'i' => ParamKind::Integer,
            'r' => ParamKind::Real,
            'b' => ParamKind::Boolean,
            '*' => ParamKind::IntegerReal,
            'C' => ParamKind::CharArray,
            'I' => ParamKind::IntegerArray,
            'R' => ParamKind::RealArray,
            'B' => ParamKind::BooleanArray,
            _ => return None,
        })
    }
}

/// Return kinds of built-in functions (`FunctionReturnType`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReturnKind {
    /// No value.
    None,
    /// Integer.
    Integer,
    /// Real.
    Real,
    /// Boolean.
    Boolean,
}

impl ReturnKind {
    /// Parse the first character of a return type string.
    pub fn from_char(c: char) -> Option<ReturnKind> {
        Some(match c {
            'i' => ReturnKind::Integer,
            'r' => ReturnKind::Real,
            'b' => ReturnKind::Boolean,
            _ => return None,
        })
    }

    /// The ABL type of the return value.
    pub fn type_id(self) -> Option<TypeId> {
        match self {
            ReturnKind::None => None,
            ReturnKind::Integer => Some(INTEGER_TYPE),
            ReturnKind::Real => Some(REAL_TYPE),
            ReturnKind::Boolean => Some(BOOLEAN_TYPE),
        }
    }
}
