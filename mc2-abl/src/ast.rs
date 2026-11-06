// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The compiled form of ABL routines.
//!
//! The C++ compiles to a "crunched" token stream and interprets that. This port compiles to a typed tree instead;
//! every node keeps the static type the C++ parser computed because the executor's arithmetic and comparison rules are
//! driven by those types.

use crate::symtab::{SymId, TypeId, BOOLEAN_TYPE, CHAR_TYPE, INTEGER_TYPE, REAL_TYPE};

/// A routine body: the statements between `code` and the matching end token.
#[derive(Debug, Clone, Default)]
pub struct Body {
    /// Statements in order.
    pub stmts: Vec<Stmt>,
}

/// A statement together with its source position (`TKN_STATEMENT_MARKER` file number and line).
#[derive(Debug, Clone)]
pub struct Stmt {
    /// Index into the module's `SourceFiles`.
    pub file: u32,
    /// Source line.
    pub line: u32,
    /// What the statement does.
    pub kind: StmtKind,
}

/// One `case` branch of a `switch`.
#[derive(Debug, Clone)]
pub struct Case {
    /// Label values; the first matching label (in source order across all branches) wins.
    pub labels: Vec<i32>,
    /// Branch body.
    pub body: Vec<Stmt>,
}

/// Statement kinds.
#[derive(Debug, Clone)]
pub enum StmtKind {
    /// No operation: empty statements and `print`/`assert`/`concat` calls compiled out by directives.
    Nop,
    /// `target = value`.
    Assign {
        /// Assignment target.
        target: VarExpr,
        /// Value.
        value: Expr,
    },
    /// A routine call used as a statement.
    Call {
        /// The call.
        call: Call,
        /// Order-call-site index when the callee is an order (`crunchByte(NumOrderCalls / 32)`/`% 32`).
        order_site: Option<u32>,
    },
    /// `if cond then ... [else ...] endif`.
    If {
        /// Condition (boolean).
        cond: Expr,
        /// Statements of the true branch.
        then_body: Vec<Stmt>,
        /// Statements of the else branch, when present.
        else_body: Option<Vec<Stmt>>,
    },
    /// `while cond do ... endwhile`.
    While {
        /// Loop condition.
        cond: Expr,
        /// Loop body.
        body: Vec<Stmt>,
    },
    /// `repeat ... until cond`.
    Repeat {
        /// Loop body.
        body: Vec<Stmt>,
        /// Exit condition.
        cond: Expr,
    },
    /// `for var = from to limit do ... endfor`.
    For {
        /// Control variable.
        var: VarExpr,
        /// Initial value.
        from: Expr,
        /// Final value (inclusive).
        to: Expr,
        /// Loop body.
        body: Vec<Stmt>,
    },
    /// `switch expr case ... endswitch`.
    Switch {
        /// Switch expression.
        expr: Expr,
        /// Branches in source order.
        cases: Vec<Case>,
    },
    /// `trans state`.
    Trans(SymId),
    /// `transback`.
    TransBack,
}

/// What a call invokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallTarget {
    /// A script-defined routine (function, order or state).
    Declared(SymId),
    /// A built-in (standard function or game native), index into the built-in table.
    Builtin(u16),
}

/// One actual argument.
#[derive(Debug, Clone)]
pub enum CallArg {
    /// Pass by value (expression).
    Val(Expr),
    /// Pass by reference (`@` parameter): the variable itself.
    Ref(VarExpr),
}

/// A routine call.
#[derive(Debug, Clone)]
pub struct Call {
    /// Callee.
    pub target: CallTarget,
    /// Actual arguments in order.
    pub args: Vec<CallArg>,
}

/// Where a variable's storage lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarPlace {
    /// Stack frame slot of the routine at `level` (1 = module frame, 2 = function frame).
    Frame {
        /// Scope level of the declaring routine's frame.
        level: u8,
        /// Slot index within that frame (parameters first, then locals).
        slot: u32,
    },
    /// Static data of a module instance; `lib` is the owning library when the symbol came from a library.
    Static {
        /// Index into the instance's static data.
        idx: u32,
        /// Owning library instance if the symbol was declared by a library.
        lib: Option<crate::symtab::InstId>,
    },
    /// Eternal (global) variable.
    Eternal(u32),
}

/// One array subscript with the geometry of the dimension it indexes.
#[derive(Debug, Clone)]
pub struct Subscript {
    /// Index expression (integer).
    pub expr: Expr,
    /// Number of elements in this dimension (`elementCount`).
    pub count: i32,
    /// Byte size of one element of this dimension.
    pub stride: i32,
}

/// A (possibly subscripted) variable reference.
#[derive(Debug, Clone)]
pub struct VarExpr {
    /// Storage.
    pub place: VarPlace,
    /// True for `@` reference parameters (the slot holds the address of the real variable).
    pub is_ref: bool,
    /// Declared type of the variable.
    pub decl_ty: TypeId,
    /// Subscripts applied, outermost first.
    pub subs: Vec<Subscript>,
    /// Type after all subscripts.
    pub ty: TypeId,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `div`
    IntDiv,
    /// `mod`
    Mod,
    /// `and`
    And,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `or`
    Or,
    /// `==`
    Eq,
    /// `<>`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

/// A binary expression.
#[derive(Debug, Clone)]
pub struct BinExpr {
    /// Operator.
    pub op: BinOp,
    /// Left operand.
    pub l: Expr,
    /// Right operand.
    pub r: Expr,
    /// Static type of the result.
    pub ty: TypeId,
}

/// Expressions.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Integer, boolean or enum constant/literal.
    Int(i32, TypeId),
    /// Real literal.
    Real(f32),
    /// Character literal.
    Char(u8),
    /// String literal or string constant: heap array id and its array type.
    Str(u32, TypeId),
    /// Variable read.
    Var(Box<VarExpr>),
    /// Call used as a value, with the callee's return type.
    Call(Box<Call>, TypeId),
    /// Unary minus.
    Neg(Box<Expr>, TypeId),
    /// `not`.
    Not(Box<Expr>, TypeId),
    /// Binary operation.
    Bin(Box<BinExpr>),
}

impl Expr {
    /// The static type of the expression (`TypePtr` returned by the C++ expression parser).
    pub fn ty(&self) -> TypeId {
        match self {
            Expr::Int(_, t) => *t,
            Expr::Real(_) => REAL_TYPE,
            Expr::Char(_) => CHAR_TYPE,
            Expr::Str(_, t) => *t,
            Expr::Var(v) => v.ty,
            Expr::Call(_, t) => *t,
            Expr::Neg(_, t) => *t,
            Expr::Not(_, t) => *t,
            Expr::Bin(b) => b.ty,
        }
    }

    /// An integer constant of integer type.
    pub fn int(v: i32) -> Expr {
        Expr::Int(v, INTEGER_TYPE)
    }

    /// A boolean constant.
    pub fn boolean(v: bool) -> Expr {
        Expr::Int(v as i32, BOOLEAN_TYPE)
    }
}
