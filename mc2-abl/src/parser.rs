// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Parser and type checker (`ablparse.h`, `ablrtn.cpp`, `abldecl.cpp`, `ablexpr.cpp`, `ablstmt.cpp`, `ablstd.cpp`).
//!
//! This is a direct port of the C++ recursive-descent compiler, with the same follow-sets, the same type-identity
//! rules and the same order of checks. The C++ emits a crunched token stream while parsing; this emits [`ast`]
//! nodes. Because the first syntax error is fatal in the original, every function returns the first error.

use std::rc::Rc;

use crate::ast::*;
use crate::env::Env;
use crate::error::{AblError, AblResult, SyntaxCode};
use crate::scanner::{Literal, Scanner, Tok};
use crate::symtab::*;

type Res<T> = AblResult<T>;

const MAX_NESTING_LEVEL: usize = 3;
const MAX_LIBRARIES_USED: usize = 25;
const MAX_ORDERS: u32 = 65535;
const MAX_LOCAL_SIZE: i32 = 32000;
const MAX_EXPR_DEPTH: usize = 300;

const STATEMENT_START: &[Tok] = &[
    Tok::For,
    Tok::If,
    Tok::Repeat,
    Tok::While,
    Tok::Switch,
    Tok::Trans,
    Tok::TransBack,
    Tok::Identifier,
];
const STATEMENT_END: &[Tok] = &[
    Tok::Semicolon,
    Tok::EndIf,
    Tok::EndWhile,
    Tok::EndFor,
    Tok::EndSwitch,
    Tok::EndFunction,
    Tok::EndOrder,
    Tok::EndState,
    Tok::Else,
    Tok::Elsif,
    Tok::Until,
    Tok::Eof,
];
const DECLARATION_START: &[Tok] = &[Tok::Const, Tok::Var, Tok::Function, Tok::Order, Tok::State];
const RELATIONAL: &[Tok] = &[Tok::Lt, Tok::Le, Tok::EqualEqual, Tok::Ne, Tok::Ge, Tok::Gt];
const ADD_OPS: &[Tok] = &[Tok::Plus, Tok::Minus, Tok::Or];
const MUL_OPS: &[Tok] = &[Tok::Star, Tok::FSlash, Tok::Div, Tok::Mod, Tok::And];
const FOLLOW_HEADER: &[Tok] = &[Tok::Semicolon, Tok::Eof];
const FOLLOW_MODULE_ID: &[Tok] = &[Tok::LParen, Tok::Colon, Tok::Semicolon, Tok::Eof];
const FOLLOW_PARAMS: &[Tok] = &[Tok::RParen, Tok::Comma, Tok::Eof];
const FOLLOW_PARAM: &[Tok] = &[Tok::Comma, Tok::RParen];
const FOLLOW_MODULE_DECLS: &[Tok] = &[Tok::Semicolon, Tok::Code, Tok::Eof];
const FOLLOW_DECLARATION: &[Tok] = &[Tok::Semicolon, Tok::Identifier, Tok::Eof];
const FOLLOW_VARIABLES: &[Tok] = &[Tok::Semicolon, Tok::Identifier, Tok::Eof];
const FOLLOW_VAR_BLOCK: &[Tok] = &[Tok::Function, Tok::Order, Tok::State, Tok::Code, Tok::Eof];
const FOLLOW_DIMENSION: &[Tok] = &[Tok::Comma, Tok::RBracket, Tok::Eof];
const INDEX_TYPE_START: &[Tok] = &[Tok::Identifier, Tok::Number];
const FOLLOW_ROUTINE: &[Tok] = &[Tok::Semicolon, Tok::Eof];
const FOLLOW_SWITCH_EXPR: &[Tok] = &[Tok::Case, Tok::Semicolon];
const FOLLOW_CASE_LABEL: &[Tok] = &[Tok::Colon, Tok::Semicolon];
const CASE_LABEL_START: &[Tok] = &[
    Tok::Identifier,
    Tok::Number,
    Tok::Plus,
    Tok::Minus,
    Tok::String,
];

/// Everything `ABLi_preProcess` needs to register after a successful compile.
pub(crate) struct ParsedModule {
    pub module_sym: SymId,
    pub source_files: Vec<String>,
    pub libraries_used: Vec<InstId>,
    pub static_sizes: Vec<i32>,
    pub num_order_calls: u32,
    pub state_handles: Vec<(String, SymId)>,
    pub lines: usize,
}

/// The compiler.
pub(crate) struct Parser<'a> {
    env: &'a mut Env,
    sc: Scanner,
    level: usize,
    display: [Option<ScopeId>; MAX_NESTING_LEVEL],
    cur_library: Option<InstId>,
    cur_module: Option<SymId>,
    cur_routine: Option<SymId>,
    block_flag: bool,
    code_bytes: usize,
    static_sizes: Vec<i32>,
    num_order_calls: u32,
    state_handles: Vec<(String, SymId)>,
    libs_used: Vec<InstId>,
    /// Nesting depth of `expression()`; the C++ has no guard and would overflow its stack on absurd input.
    expr_depth: usize,
    stmt_depth: usize,
}

fn is_real_ops(t1: TypeId, t2: TypeId) -> bool {
    if t1 == REAL_TYPE {
        t2 == REAL_TYPE || t2 == INTEGER_TYPE
    } else if t2 == REAL_TYPE {
        t1 == INTEGER_TYPE
    } else {
        false
    }
}

fn is_int_ops(t1: TypeId, t2: TypeId) -> bool {
    t1 == INTEGER_TYPE && t2 == INTEGER_TYPE
}

fn is_bool_ops(t1: TypeId, t2: TypeId) -> bool {
    t1 == BOOLEAN_TYPE && t2 == BOOLEAN_TYPE
}

impl<'a> Parser<'a> {
    pub(crate) fn new(env: &'a mut Env, library: Option<InstId>) -> Parser<'a> {
        let provider = env.provider.clone();
        Parser {
            env,
            sc: Scanner::new(provider),
            level: 0,
            display: [Some(ScopeId(0)), None, None],
            cur_library: library,
            cur_module: None,
            cur_routine: None,
            block_flag: false,
            code_bytes: 0,
            static_sizes: Vec::new(),
            num_order_calls: 0,
            state_handles: vec![(String::new(), SymId(0))],
            libs_used: Vec::new(),
            expr_depth: 0,
            stmt_depth: 0,
        }
    }

    // ------------------------------------------------------------------------------------------
    // plumbing
    // ------------------------------------------------------------------------------------------

    fn err(&self, code: SyntaxCode) -> AblError {
        self.sc.syntax_error(code)
    }

    fn crunch(&mut self, bytes: usize) -> Res<()> {
        if self.code_bytes + 100 >= self.env.cfg.max_code_buffer_size {
            return Err(self.err(SyntaxCode::CodeSegmentOverflow));
        }
        self.code_bytes += bytes;
        Ok(())
    }

    fn get_token(&mut self) -> Res<()> {
        self.sc.get_token()?;
        if self.block_flag {
            self.crunch(1)?;
        }
        Ok(())
    }

    /// Account for one more level of expression tree depth (nesting, `not` chains and long operator chains all build
    /// deep trees that the evaluator and the destructor recurse through).
    fn deepen(&mut self, by: &mut usize) -> Res<()> {
        self.expr_depth += 1;
        *by += 1;
        if self.expr_depth > MAX_EXPR_DEPTH {
            return Err(self.err(SyntaxCode::NestingTooDeep));
        }
        Ok(())
    }

    fn tok(&self) -> Tok {
        self.sc.tok
    }

    fn tok_in(&self, list: &[Tok]) -> bool {
        list.contains(&self.sc.tok)
    }

    fn synchronize(&self, l1: &[Tok], l2: &[Tok], l3: &[Tok]) -> Res<()> {
        if !self.tok_in(l1) && !self.tok_in(l2) && !self.tok_in(l3) {
            return Err(self.err(if self.tok() == Tok::Eof {
                SyntaxCode::UnexpectedEof
            } else {
                SyntaxCode::UnexpectedToken
            }));
        }
        Ok(())
    }

    fn if_token_get(&mut self, t: Tok) -> Res<()> {
        if self.tok() == t {
            self.get_token()?;
        }
        Ok(())
    }

    fn if_token_get_else_error(&mut self, t: Tok, code: SyntaxCode) -> Res<()> {
        if self.tok() == t {
            self.get_token()
        } else {
            Err(self.err(code))
        }
    }

    // ------------------------------------------------------------------------------------------
    // symbol table helpers
    // ------------------------------------------------------------------------------------------

    fn new_sym(&mut self, name: &str, level: usize) -> SymId {
        self.env.new_sym(name, level as u8)
    }

    fn display_scope(&self, level: usize) -> ScopeId {
        self.display[level].expect("scope level not entered")
    }

    fn enter_local(&mut self, name: &str) -> SymId {
        let scope = self.display_scope(self.level);
        let id = self.new_sym(name, self.level);
        self.env.scope_insert(scope, name, id);
        id
    }

    fn enter_in_scope(&mut self, scope: ScopeId, name: &str, level: usize) -> SymId {
        let id = self.new_sym(name, level);
        self.env.scope_insert(scope, name, id);
        id
    }

    fn search_scope(&self, scope: ScopeId, name: &str) -> Option<SymId> {
        self.env.scopes[scope.0 as usize]
            .candidates(name)
            .first()
            .copied()
    }

    fn search_local(&self, name: &str) -> Option<SymId> {
        self.search_scope(self.display_scope(self.level), name)
    }

    /// `searchAndEnterLocalSymTable`: redefinition is an error.
    fn search_and_enter_local(&mut self, name: &str) -> Res<SymId> {
        if self.search_local(name).is_some() {
            return Err(self.err(SyntaxCode::RedefinedIdentifier));
        }
        Ok(self.enter_local(name))
    }

    /// `searchSymTableForFunction`: first entry that is a function with no return type.
    fn search_for_function(&self, scope: ScopeId, name: &str) -> Option<SymId> {
        self.env.scopes[scope.0 as usize]
            .candidates(name)
            .iter()
            .copied()
            .find(|&s| {
                let sym = self.env.sym(s);
                sym.ty.is_none() && matches!(sym.defn, Defn::Function(_))
            })
    }

    /// `searchSymTableForState`.
    fn search_for_state(&self, scope: ScopeId, name: &str) -> Option<SymId> {
        self.env.scopes[scope.0 as usize]
            .candidates(name)
            .iter()
            .copied()
            .find(|&s| {
                let sym = self.env.sym(s);
                sym.ty.is_none()
                    && matches!(sym.defn, Defn::Function(_))
                    && sym.flags() & ROUTINE_FLAG_STATE != 0
            })
    }

    fn record_library_used(&mut self, member: SymId) -> Res<()> {
        let Some(lib) = self.env.sym(member).library else {
            return Ok(());
        };
        if self.libs_used.contains(&lib) {
            return Ok(());
        }
        if self.libs_used.len() > MAX_LIBRARIES_USED {
            return Err(AblError::fatal(
                " ABL: Too many libraries referenced from module ",
            ));
        }
        self.libs_used.push(lib);
        Ok(())
    }

    /// `searchSymTableDisplay`.
    fn search_all(&mut self, name: &str) -> Res<Option<SymId>> {
        if let Some(dot) = name.find('.') {
            let (lib_name, member) = (&name[..dot], &name[dot + 1..]);
            let Some(lib_sym) = self.search_scope(ScopeId(0), lib_name) else {
                return Ok(None);
            };
            let Some(scope) = self.env.sym(lib_sym).routine().and_then(|r| r.scope) else {
                return Ok(None);
            };
            let found = self.search_scope(scope, member);
            if let Some(m) = found {
                self.record_library_used(m)?;
            }
            return Ok(found);
        }
        for lvl in (0..=self.level).rev() {
            if let Some(scope) = self.display[lvl] {
                if let Some(s) = self.search_scope(scope, name) {
                    return Ok(Some(s));
                }
            }
        }
        // Not found locally: maybe a member of some library, referenced without the library name.
        let scopes: Vec<ScopeId> = self.env.library_scopes.iter().map(|&(_, s)| s).collect();
        for scope in scopes {
            if let Some(s) = self.search_scope(scope, name) {
                self.record_library_used(s)?;
                return Ok(Some(s));
            }
        }
        Ok(None)
    }

    /// `searchAndFindAllSymTables`: undefined identifiers are errors.
    fn search_and_find_all(&mut self) -> Res<SymId> {
        let word = self.sc.word.clone();
        match self.search_all(&word)? {
            Some(s) => Ok(s),
            None => Err(self.err(SyntaxCode::UndefinedIdentifier)),
        }
    }

    fn enter_scope(&mut self) -> Res<ScopeId> {
        self.level += 1;
        if self.level >= MAX_NESTING_LEVEL {
            return Err(self.err(SyntaxCode::NestingTooDeep));
        }
        let scope = self.env.new_scope();
        self.display[self.level] = Some(scope);
        Ok(scope)
    }

    fn exit_scope(&mut self) -> ScopeId {
        let s = self.display[self.level]
            .take()
            .expect("exit_scope without scope");
        self.level -= 1;
        s
    }

    fn types(&self, t: TypeId) -> &TypeInfo {
        &self.env.types[t.0 as usize]
    }

    fn is_char_array(&self, t: TypeId) -> bool {
        let ti = self.types(t);
        ti.form == Form::Array && ti.elem_ty == CHAR_TYPE
    }

    // ------------------------------------------------------------------------------------------
    // type compatibility
    // ------------------------------------------------------------------------------------------

    fn check_relational_op_types(&self, t1: TypeId, t2: TypeId) -> Res<()> {
        let a = self.types(t1);
        let b = self.types(t2);
        if t1 == t2 && (a.form == Form::Scalar || a.form == Form::Enum) {
            return Ok(());
        }
        if (t1 == INTEGER_TYPE && t2 == REAL_TYPE) || (t2 == INTEGER_TYPE && t1 == REAL_TYPE) {
            return Ok(());
        }
        if a.form == Form::Array
            && b.form == Form::Array
            && a.elem_ty == CHAR_TYPE
            && b.elem_ty == CHAR_TYPE
            && a.elem_count == b.elem_count
        {
            return Ok(());
        }
        Err(self.err(SyntaxCode::IncompatibleTypes))
    }

    fn is_assign_type_compatible(&self, t1: TypeId, t2: TypeId) -> bool {
        if t1 == t2 {
            return true;
        }
        if t1 == REAL_TYPE && t2 == INTEGER_TYPE {
            return true;
        }
        let a = self.types(t1);
        let b = self.types(t2);
        a.form == Form::Array
            && b.form == Form::Array
            && a.elem_ty == CHAR_TYPE
            && b.elem_ty == CHAR_TYPE
            && a.elem_count >= b.elem_count
    }

    // ------------------------------------------------------------------------------------------
    // expressions
    // ------------------------------------------------------------------------------------------

    fn variable(&mut self, sym: SymId) -> Res<VarExpr> {
        let s = self.env.sym(sym).clone();
        let ty = s.ty;
        let (place, is_ref) = match &s.defn {
            Defn::Var { kind, offset } => (
                match kind {
                    VarKind::Normal => VarPlace::Frame {
                        level: s.level,
                        slot: *offset,
                    },
                    VarKind::Static => VarPlace::Static {
                        idx: *offset,
                        lib: s.library,
                    },
                    VarKind::Eternal => VarPlace::Eternal(*offset),
                },
                false,
            ),
            Defn::ValParam { offset } => (
                VarPlace::Frame {
                    level: s.level,
                    slot: *offset,
                },
                false,
            ),
            Defn::RefParam { offset } => (
                VarPlace::Frame {
                    level: s.level,
                    slot: *offset,
                },
                true,
            ),
            _ => return Err(self.err(SyntaxCode::InvalidIdentifierUsage)),
        };
        let decl_ty = ty.unwrap_or(DUMMY_TYPE);
        self.get_token()?;
        if self.tok() == Tok::LParen {
            return Err(self.err(SyntaxCode::UnexpectedToken));
        }
        let mut cur = decl_ty;
        let mut subs = Vec::new();
        while self.tok() == Tok::LBracket {
            cur = self.array_subscript_list(cur, &mut subs)?;
        }
        Ok(VarExpr {
            place,
            is_ref,
            decl_ty,
            subs,
            ty: cur,
        })
    }

    fn array_subscript_list(&mut self, mut ty: TypeId, subs: &mut Vec<Subscript>) -> Res<TypeId> {
        loop {
            let ti = self.types(ty).clone();
            if ti.form == Form::Array {
                self.get_token()?;
                let e = self.expression()?;
                if !self.is_assign_type_compatible(ti.index_ty, e.ty()) {
                    return Err(self.err(SyntaxCode::IncompatibleTypes));
                }
                let stride = self.types(ti.elem_ty).size;
                subs.push(Subscript {
                    expr: e,
                    count: ti.elem_count,
                    stride,
                });
                ty = ti.elem_ty;
            } else {
                return Err(self.err(SyntaxCode::TooManySubscripts));
            }
            if self.tok() != Tok::Comma {
                break;
            }
        }
        self.if_token_get_else_error(Tok::RBracket, SyntaxCode::MissingRBracket)?;
        Ok(ty)
    }

    fn const_expr(&self, sym: SymId) -> Expr {
        let s = self.env.sym(sym);
        let ty = s.ty.unwrap_or(DUMMY_TYPE);
        match s.defn {
            Defn::Const(ConstValue::Int(v)) => Expr::Int(v, ty),
            Defn::Const(ConstValue::Real(v)) => Expr::Real(v),
            Defn::Const(ConstValue::Char(c)) => Expr::Char(c),
            Defn::Const(ConstValue::Str(id)) => Expr::Str(id, ty),
            _ => Expr::Int(0, DUMMY_TYPE),
        }
    }

    fn factor(&mut self) -> Res<Expr> {
        match self.tok() {
            Tok::Identifier => {
                let sym = self.search_and_find_all()?;
                match &self.env.sym(sym).defn {
                    Defn::Function(_) => {
                        self.get_token()?;
                        let (call, ret) = self.routine_call(sym)?;
                        match ret {
                            Some(t) => Ok(Expr::Call(Box::new(call), t)),
                            None => Err(self.err(SyntaxCode::IncompatibleTypes)),
                        }
                    }
                    Defn::Const(_) => {
                        let e = self.const_expr(sym);
                        self.get_token()?;
                        Ok(e)
                    }
                    _ => Ok(Expr::Var(Box::new(self.variable(sym)?))),
                }
            }
            Tok::Number => {
                let e = match self.sc.literal {
                    Literal::Int(v) => Expr::int(v),
                    Literal::Real(v) => Expr::Real(v),
                    Literal::Str(_) => unreachable!("number token with string literal"),
                };
                self.crunch(4)?;
                self.get_token()?;
                Ok(e)
            }
            Tok::String => {
                let bytes = match &self.sc.literal {
                    Literal::Str(b) => b.clone(),
                    _ => unreachable!("string token without string literal"),
                };
                self.crunch(4)?;
                let e = if bytes.len() == 1 {
                    Expr::Char(bytes[0])
                } else {
                    let (id, ty) = self.env.intern_string(&bytes);
                    Expr::Str(id, ty)
                };
                self.get_token()?;
                Ok(e)
            }
            Tok::Not => {
                let mut by = 0;
                self.deepen(&mut by)?;
                self.get_token()?;
                let f = self.factor();
                self.expr_depth -= by;
                let f = f?;
                let ty = f.ty();
                Ok(Expr::Not(Box::new(f), ty))
            }
            Tok::LParen => {
                self.get_token()?;
                let e = self.expression()?;
                self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
                Ok(e)
            }
            _ => Err(self.err(SyntaxCode::InvalidExpression)),
        }
    }

    fn bin(op: BinOp, l: Expr, r: Expr, ty: TypeId) -> Expr {
        Expr::Bin(Box::new(BinExpr { op, l, r, ty }))
    }

    fn term(&mut self) -> Res<Expr> {
        let mut result = self.factor()?;
        let mut chain = 0;
        while self.tok_in(MUL_OPS) {
            let op = self.tok();
            self.deepen(&mut chain)?;
            self.get_token()?;
            let second = self.factor()?;
            let (t1, t2) = (result.ty(), second.ty());
            let (bop, ty) = match op {
                Tok::Star | Tok::FSlash => {
                    let bop = if op == Tok::Star {
                        BinOp::Mul
                    } else {
                        BinOp::Div
                    };
                    if is_int_ops(t1, t2) {
                        (bop, INTEGER_TYPE)
                    } else if is_real_ops(t1, t2) {
                        (bop, REAL_TYPE)
                    } else {
                        return Err(self.err(SyntaxCode::IncompatibleTypes));
                    }
                }
                Tok::Div | Tok::Mod => {
                    if !is_int_ops(t1, t2) {
                        return Err(self.err(SyntaxCode::IncompatibleTypes));
                    }
                    (
                        if op == Tok::Div {
                            BinOp::IntDiv
                        } else {
                            BinOp::Mod
                        },
                        INTEGER_TYPE,
                    )
                }
                _ => {
                    if !is_bool_ops(t1, t2) {
                        return Err(self.err(SyntaxCode::IncompatibleTypes));
                    }
                    (BinOp::And, BOOLEAN_TYPE)
                }
            };
            result = Self::bin(bop, result, second, ty);
        }
        self.expr_depth -= chain;
        Ok(result)
    }

    fn simple_expression(&mut self) -> Res<Expr> {
        let mut unary = None;
        if matches!(self.tok(), Tok::Plus | Tok::Minus) {
            unary = Some(self.tok());
            self.get_token()?;
        }
        let mut result = self.term()?;
        if unary.is_some() && result.ty() != INTEGER_TYPE && result.ty() != REAL_TYPE {
            return Err(self.err(SyntaxCode::IncompatibleTypes));
        }
        if unary == Some(Tok::Minus) {
            let ty = result.ty();
            result = Expr::Neg(Box::new(result), ty);
        }
        let mut chain = 0;
        while self.tok_in(ADD_OPS) {
            let op = self.tok();
            self.deepen(&mut chain)?;
            self.get_token()?;
            let second = self.term()?;
            let (t1, t2) = (result.ty(), second.ty());
            let (bop, ty) = match op {
                Tok::Plus | Tok::Minus => {
                    let bop = if op == Tok::Plus {
                        BinOp::Add
                    } else {
                        BinOp::Sub
                    };
                    if is_int_ops(t1, t2) {
                        (bop, INTEGER_TYPE)
                    } else if is_real_ops(t1, t2) {
                        (bop, REAL_TYPE)
                    } else {
                        return Err(self.err(SyntaxCode::IncompatibleTypes));
                    }
                }
                _ => {
                    if !is_bool_ops(t1, t2) {
                        return Err(self.err(SyntaxCode::IncompatibleTypes));
                    }
                    (BinOp::Or, BOOLEAN_TYPE)
                }
            };
            result = Self::bin(bop, result, second, ty);
        }
        self.expr_depth -= chain;
        Ok(result)
    }

    fn expression(&mut self) -> Res<Expr> {
        self.expr_depth += 1;
        let r = if self.expr_depth > MAX_EXPR_DEPTH {
            Err(self.err(SyntaxCode::NestingTooDeep))
        } else {
            self.expression_inner()
        };
        self.expr_depth -= 1;
        r
    }

    fn expression_inner(&mut self) -> Res<Expr> {
        let first = self.simple_expression()?;
        if self.tok_in(RELATIONAL) {
            let op = self.tok();
            self.get_token()?;
            let second = self.simple_expression()?;
            self.check_relational_op_types(first.ty(), second.ty())?;
            let bop = match op {
                Tok::Lt => BinOp::Lt,
                Tok::Le => BinOp::Le,
                Tok::EqualEqual => BinOp::Eq,
                Tok::Ne => BinOp::Ne,
                Tok::Ge => BinOp::Ge,
                _ => BinOp::Gt,
            };
            return Ok(Self::bin(bop, first, second, BOOLEAN_TYPE));
        }
        Ok(first)
    }

    // ------------------------------------------------------------------------------------------
    // calls
    // ------------------------------------------------------------------------------------------

    /// `routineCall`; returns the call and its result type (`None` for procedures).
    fn routine_call(&mut self, sym: SymId) -> Res<(Call, Option<TypeId>)> {
        let routine_key = self.env.sym(sym).routine().map(|r| r.key);
        match routine_key {
            Some(RoutineKey::Builtin(idx)) => self.standard_routine_call(sym, idx),
            _ => {
                let args = self.actual_param_list(sym)?;
                let ret = self.env.sym(sym).ty;
                Ok((
                    Call {
                        target: CallTarget::Declared(sym),
                        args,
                    },
                    ret,
                ))
            }
        }
    }

    fn actual_param_list(&mut self, routine: SymId) -> Res<Vec<CallArg>> {
        let formals: Vec<SymId> = self
            .env
            .sym(routine)
            .routine()
            .map(|r| r.params.clone())
            .unwrap_or_default();
        let mut idx = 0;
        let mut args = Vec::new();
        if self.tok() == Tok::LParen {
            loop {
                let formal = formals.get(idx).copied();
                self.get_token()?;
                let is_ref = formal
                    .map(|f| matches!(self.env.sym(f).defn, Defn::RefParam { .. }))
                    .unwrap_or(false);
                if !is_ref {
                    let e = self.expression()?;
                    if let Some(f) = formal {
                        let fty = self.env.sym(f).ty.unwrap_or(DUMMY_TYPE);
                        if !self.is_assign_type_compatible(fty, e.ty()) {
                            return Err(self.err(SyntaxCode::IncompatibleTypes));
                        }
                    }
                    args.push(CallArg::Val(e));
                } else if self.tok() == Tok::Identifier {
                    let id = self.search_and_find_all()?;
                    let v = self.variable(id)?;
                    let fty = self.env.sym(formal.unwrap()).ty.unwrap_or(DUMMY_TYPE);
                    if fty != v.ty {
                        return Err(self.err(SyntaxCode::IncompatibleTypes));
                    }
                    args.push(CallArg::Ref(v));
                } else {
                    self.expression()?;
                    return Err(self.err(SyntaxCode::InvalidRefParam));
                }
                if formal.is_none() {
                    return Err(self.err(SyntaxCode::WrongNumberOfParams));
                }
                idx += 1;
                self.synchronize(FOLLOW_PARAM, STATEMENT_END, &[])?;
                if self.tok() != Tok::Comma {
                    break;
                }
            }
            self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
        }
        if idx < formals.len() {
            return Err(self.err(SyntaxCode::WrongNumberOfParams));
        }
        Ok(args)
    }

    fn standard_routine_call(&mut self, sym: SymId, idx: u16) -> Res<(Call, Option<TypeId>)> {
        use crate::env::StdImpl;
        let info = self.env.std_fns[idx as usize].clone();
        let target = CallTarget::Builtin(idx);
        match info.imp {
            StdImpl::Return => {
                let mut args = Vec::new();
                let cur_ty = self.cur_routine.and_then(|r| self.env.sym(r).ty);
                if self.tok() == Tok::LParen {
                    self.get_token()?;
                    let e = self.expression()?;
                    if Some(e.ty()) != cur_ty {
                        return Err(self.err(SyntaxCode::IncompatibleTypes));
                    }
                    self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
                    args.push(CallArg::Val(e));
                } else if cur_ty.is_some() {
                    return Err(self.err(SyntaxCode::WrongNumberOfParams));
                }
                Ok((Call { target, args }, None))
            }
            StdImpl::Print => {
                if self.tok() == Tok::LParen {
                    self.get_token()?;
                } else {
                    return Err(self.err(SyntaxCode::WrongNumberOfParams));
                }
                let e = self.expression()?;
                let t = e.ty();
                if t != INTEGER_TYPE && t != REAL_TYPE && t != CHAR_TYPE && !self.is_char_array(t) {
                    return Err(self.err(SyntaxCode::IncompatibleTypes));
                }
                self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
                Ok((
                    Call {
                        target,
                        args: vec![CallArg::Val(e)],
                    },
                    None,
                ))
            }
            StdImpl::Concat => {
                if self.tok() == Tok::LParen {
                    self.get_token()?;
                } else {
                    return Err(self.err(SyntaxCode::WrongNumberOfParams));
                }
                let dest = self.expression()?;
                if !self.is_char_array(dest.ty()) {
                    return Err(self.err(SyntaxCode::IncompatibleTypes));
                }
                self.if_token_get_else_error(Tok::Comma, SyntaxCode::MissingComma)?;
                let item = self.expression()?;
                let t = item.ty();
                if t != INTEGER_TYPE
                    && t != REAL_TYPE
                    && t != CHAR_TYPE
                    && t != BOOLEAN_TYPE
                    && !self.is_char_array(t)
                {
                    return Err(self.err(SyntaxCode::IncompatibleTypes));
                }
                self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
                Ok((
                    Call {
                        target,
                        args: vec![CallArg::Val(dest), CallArg::Val(item)],
                    },
                    Some(INTEGER_TYPE),
                ))
            }
            StdImpl::GetStateHandle => {
                if self.tok() == Tok::LParen {
                    self.get_token()?;
                } else {
                    return Err(self.err(SyntaxCode::WrongNumberOfParams));
                }
                let e = self.expression()?;
                if !self.is_char_array(e.ty()) {
                    return Err(self.err(SyntaxCode::IncompatibleTypes));
                }
                self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
                Ok((
                    Call {
                        target,
                        args: vec![CallArg::Val(e)],
                    },
                    Some(INTEGER_TYPE),
                ))
            }
            _ => {
                let n = info.params.len();
                let mut args = Vec::new();
                if n == 0 {
                    if self.tok() == Tok::LParen {
                        return Err(self.err(SyntaxCode::WrongNumberOfParams));
                    }
                } else {
                    if self.tok() == Tok::LParen {
                        self.get_token()?;
                    } else {
                        return Err(self.err(SyntaxCode::WrongNumberOfParams));
                    }
                    for (i, kind) in info.params.iter().enumerate() {
                        let e = self.expression()?;
                        let t = e.ty();
                        let ok = match kind {
                            ParamKind::Anything => true,
                            ParamKind::Char => t == CHAR_TYPE,
                            ParamKind::Integer => t == INTEGER_TYPE,
                            ParamKind::Real => t == REAL_TYPE,
                            ParamKind::Boolean => t == BOOLEAN_TYPE,
                            ParamKind::IntegerReal => {
                                if t == INTEGER_TYPE && info.star_int_reinterprets(i) {
                                    self.env.warnings.push(format!(
                                        "{} [line {}]: integer passed to real '*' parameter {} of {} (C++ reads it as float bits)",
                                        self.sc.source_files.first().cloned().unwrap_or_default(),
                                        self.sc.line_number,
                                        i,
                                        info.name
                                    ));
                                }
                                t == INTEGER_TYPE || t == REAL_TYPE
                            }
                            ParamKind::CharArray => self.array_of(t, CHAR_TYPE),
                            ParamKind::IntegerArray => self.array_of(t, INTEGER_TYPE),
                            ParamKind::RealArray => self.array_of(t, REAL_TYPE),
                            ParamKind::BooleanArray => self.array_of(t, BOOLEAN_TYPE),
                        };
                        if !ok {
                            return Err(self.err(SyntaxCode::IncompatibleTypes));
                        }
                        args.push(CallArg::Val(e));
                        if i == n - 1 {
                            self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
                        } else {
                            self.if_token_get_else_error(Tok::Comma, SyntaxCode::MissingComma)?;
                        }
                    }
                }
                let _ = sym;
                Ok((Call { target, args }, info.ret.type_id()))
            }
        }
    }

    fn array_of(&self, t: TypeId, elem: TypeId) -> bool {
        let ti = self.types(t);
        ti.form == Form::Array && ti.elem_ty == elem
    }

    // ------------------------------------------------------------------------------------------
    // statements
    // ------------------------------------------------------------------------------------------

    fn statement_block(&mut self, ends: &[Tok]) -> Res<Vec<Stmt>> {
        let mut out = Vec::new();
        if !self.tok_in(ends) {
            loop {
                if let Some(s) = self.statement()? {
                    out.push(s);
                }
                while self.tok() == Tok::Semicolon {
                    self.get_token()?;
                }
                if self.tok_in(ends) {
                    break;
                }
                if !self.tok_in(STATEMENT_START) {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// The routine/module body loop with `synchronize` after every statement.
    fn body_block(&mut self, end: Tok) -> Res<Vec<Stmt>> {
        let mut out = Vec::new();
        if self.tok() != end {
            loop {
                if let Some(s) = self.statement()? {
                    out.push(s);
                }
                while self.tok() == Tok::Semicolon {
                    self.get_token()?;
                }
                if self.tok() == end {
                    break;
                }
                self.synchronize(STATEMENT_START, &[], &[])?;
                if !self.tok_in(STATEMENT_START) {
                    break;
                }
            }
        }
        Ok(out)
    }

    fn mk_stmt(&self, file: u32, line: u32, kind: StmtKind) -> Stmt {
        Stmt { file, line, kind }
    }

    /// Returns `None` for statements compiled out by `#print_off` / `#assert_off` / `#stringfuncs_off`.
    fn statement(&mut self) -> Res<Option<Stmt>> {
        self.stmt_depth += 1;
        let r = if self.stmt_depth > self.env.cfg.max_statement_depth {
            Err(self.err(SyntaxCode::NestingTooDeep))
        } else {
            self.statement_inner()
        };
        self.stmt_depth -= 1;
        r
    }

    fn statement_inner(&mut self) -> Res<Option<Stmt>> {
        let file = self.sc.file_number.max(0) as u32;
        let line = self.sc.line_number.max(0) as u32;
        self.crunch(6)?;
        let mut result: Option<Stmt> = Some(self.mk_stmt(file, line, StmtKind::Nop));
        match self.tok() {
            Tok::Identifier => {
                let sym = self.search_and_find_all()?;
                if matches!(self.env.sym(sym).defn, Defn::Function(_)) {
                    let flags = self.env.sym(sym).flags();
                    let mut disabled = false;
                    if let Some(RoutineKey::Builtin(idx)) =
                        self.env.sym(sym).routine().map(|r| r.key)
                    {
                        use crate::env::StdImpl;
                        let imp = self.env.std_fns[idx as usize].imp;
                        disabled = (imp == StdImpl::Assert && !self.sc.assert_enabled)
                            || (imp == StdImpl::Print && !self.sc.print_enabled)
                            || (imp == StdImpl::Concat && !self.sc.string_functions_enabled);
                    }
                    let mut order_site = None;
                    if flags & ROUTINE_FLAG_ORDER != 0 {
                        if self.num_order_calls == MAX_ORDERS {
                            return Err(self.err(SyntaxCode::TooManyOrders));
                        }
                        order_site = Some(self.num_order_calls);
                        self.crunch(2)?;
                        self.num_order_calls += 1;
                    }
                    self.crunch(4)?;
                    self.get_token()?;
                    let (call, _ret) = self.routine_call(sym)?;
                    result = if disabled {
                        None
                    } else {
                        Some(self.mk_stmt(file, line, StmtKind::Call { call, order_site }))
                    };
                } else {
                    let target = self.variable(sym)?;
                    self.if_token_get_else_error(Tok::Equal, SyntaxCode::MissingEqual)?;
                    let value = self.expression()?;
                    if !self.is_assign_type_compatible(target.ty, value.ty()) {
                        return Err(self.err(SyntaxCode::IncompatibleAssignment));
                    }
                    result = Some(self.mk_stmt(file, line, StmtKind::Assign { target, value }));
                }
            }
            Tok::Repeat => result = Some(self.repeat_statement(file, line)?),
            Tok::While => result = Some(self.while_statement(file, line)?),
            Tok::If => result = Some(self.if_statement(file, line)?),
            Tok::For => result = Some(self.for_statement(file, line)?),
            Tok::Switch => result = Some(self.switch_statement(file, line)?),
            Tok::Trans => result = Some(self.trans_statement(file, line)?),
            Tok::TransBack => {
                self.get_token()?;
                result = Some(self.mk_stmt(file, line, StmtKind::TransBack));
            }
            _ => {}
        }

        self.synchronize(STATEMENT_END, &[], &[])?;
        if self.tok_in(STATEMENT_START) {
            return Err(self.err(SyntaxCode::MissingSemicolon));
        }
        Ok(result)
    }

    fn boolean_expression(&mut self) -> Res<Expr> {
        let e = self.expression()?;
        if e.ty() != BOOLEAN_TYPE {
            return Err(self.err(SyntaxCode::IncompatibleTypes));
        }
        Ok(e)
    }

    fn repeat_statement(&mut self, file: u32, line: u32) -> Res<Stmt> {
        self.get_token()?;
        let body = if self.tok() != Tok::Until {
            let mut out = Vec::new();
            loop {
                if let Some(s) = self.statement()? {
                    out.push(s);
                }
                while self.tok() == Tok::Semicolon {
                    self.get_token()?;
                }
                if self.tok() == Tok::Until {
                    break;
                }
                if !self.tok_in(STATEMENT_START) {
                    break;
                }
            }
            out
        } else {
            Vec::new()
        };
        self.if_token_get_else_error(Tok::Until, SyntaxCode::MissingUntil)?;
        self.crunch(5)?;
        let cond = self.boolean_expression()?;
        Ok(self.mk_stmt(file, line, StmtKind::Repeat { body, cond }))
    }

    fn while_statement(&mut self, file: u32, line: u32) -> Res<Stmt> {
        self.get_token()?;
        self.crunch(5)?;
        let cond = self.boolean_expression()?;
        self.if_token_get_else_error(Tok::Do, SyntaxCode::MissingDo)?;
        let body = self.statement_block(&[Tok::EndWhile])?;
        self.if_token_get_else_error(Tok::EndWhile, SyntaxCode::MissingEndWhile)?;
        Ok(self.mk_stmt(file, line, StmtKind::While { cond, body }))
    }

    fn if_statement(&mut self, file: u32, line: u32) -> Res<Stmt> {
        self.get_token()?;
        self.crunch(5)?;
        let cond = self.boolean_expression()?;
        self.if_token_get_else_error(Tok::Then, SyntaxCode::MissingThen)?;
        let then_body = self.statement_block(&[Tok::EndIf, Tok::Else])?;
        let mut else_body = None;
        if self.tok() == Tok::Else {
            self.get_token()?;
            self.crunch(5)?;
            else_body = Some(self.statement_block(&[Tok::EndIf])?);
        }
        self.if_token_get_else_error(Tok::EndIf, SyntaxCode::MissingEndIf)?;
        Ok(self.mk_stmt(
            file,
            line,
            StmtKind::If {
                cond,
                then_body,
                else_body,
            },
        ))
    }

    fn for_statement(&mut self, file: u32, line: u32) -> Res<Stmt> {
        self.get_token()?;
        self.crunch(5)?;
        if self.tok() != Tok::Identifier {
            return Err(self.err(SyntaxCode::MissingIdentifier));
        }
        let sym = self.search_and_find_all()?;
        self.crunch(4)?;
        let s = self.env.sym(sym).clone();
        let (place, ty) = match s.defn {
            Defn::Var { kind, offset } => {
                let place = match kind {
                    VarKind::Normal => VarPlace::Frame {
                        level: s.level,
                        slot: offset,
                    },
                    VarKind::Static => VarPlace::Static {
                        idx: offset,
                        lib: s.library,
                    },
                    VarKind::Eternal => VarPlace::Eternal(offset),
                };
                (place, s.ty.unwrap_or(DUMMY_TYPE))
            }
            _ => return Err(self.err(SyntaxCode::InvalidForControl)),
        };
        self.get_token()?;
        if ty != INTEGER_TYPE && self.types(ty).form != Form::Enum {
            return Err(self.err(SyntaxCode::IncompatibleTypes));
        }
        let var = VarExpr {
            place,
            is_ref: false,
            decl_ty: ty,
            subs: Vec::new(),
            ty,
        };
        self.if_token_get_else_error(Tok::Equal, SyntaxCode::MissingEqual)?;
        let from = self.expression()?;
        if !self.is_assign_type_compatible(ty, from.ty()) {
            return Err(self.err(SyntaxCode::IncompatibleTypes));
        }
        if self.tok() == Tok::To {
            self.get_token()?;
        } else {
            return Err(self.err(SyntaxCode::MissingTo));
        }
        let to = self.expression()?;
        if !self.is_assign_type_compatible(ty, to.ty()) {
            return Err(self.err(SyntaxCode::IncompatibleTypes));
        }
        self.if_token_get_else_error(Tok::Do, SyntaxCode::MissingDo)?;
        let body = self.statement_block(&[Tok::EndFor])?;
        self.if_token_get_else_error(Tok::EndFor, SyntaxCode::MissingEndFor)?;
        Ok(self.mk_stmt(
            file,
            line,
            StmtKind::For {
                var,
                from,
                to,
                body,
            },
        ))
    }

    /// `caseLabel`: returns the label's type and value.
    fn case_label(&mut self) -> Res<(TypeId, i32)> {
        self.crunch(8)?;
        let mut sign_minus = false;
        let mut saw_sign = false;
        if matches!(self.tok(), Tok::Plus | Tok::Minus) {
            sign_minus = self.tok() == Tok::Minus;
            saw_sign = true;
            self.get_token()?;
        }
        match self.tok() {
            Tok::Number => {
                self.crunch(4)?;
                match self.sc.literal {
                    Literal::Int(v) => {
                        Ok((INTEGER_TYPE, if sign_minus { v.wrapping_neg() } else { v }))
                    }
                    _ => Err(self.err(SyntaxCode::InvalidConstant)),
                }
            }
            Tok::Identifier => {
                let word = self.sc.word.clone();
                let Some(id) = self.search_all(&word)? else {
                    return Err(self.err(SyntaxCode::UndefinedIdentifier));
                };
                self.crunch(4)?;
                let s = self.env.sym(id).clone();
                let Defn::Const(cv) = s.defn else {
                    return Err(self.err(SyntaxCode::NotAConstantIdentifier));
                };
                let ty = s.ty.unwrap_or(DUMMY_TYPE);
                if ty == INTEGER_TYPE {
                    if let ConstValue::Int(v) = cv {
                        return Ok((INTEGER_TYPE, if sign_minus { v.wrapping_neg() } else { v }));
                    }
                    Ok((DUMMY_TYPE, 0))
                } else if ty == CHAR_TYPE {
                    if saw_sign {
                        return Err(self.err(SyntaxCode::InvalidConstant));
                    }
                    if let ConstValue::Char(c) = cv {
                        return Ok((CHAR_TYPE, c as i32));
                    }
                    Ok((DUMMY_TYPE, 0))
                } else if self.types(ty).form == Form::Enum {
                    if saw_sign {
                        return Err(self.err(SyntaxCode::InvalidConstant));
                    }
                    if let ConstValue::Int(v) = cv {
                        return Ok((ty, v));
                    }
                    Ok((DUMMY_TYPE, 0))
                } else {
                    Ok((DUMMY_TYPE, 0))
                }
            }
            Tok::String => Ok((DUMMY_TYPE, 0)),
            _ => Err(self.err(SyntaxCode::InvalidConstant)),
        }
    }

    fn case_branch(&mut self, expr_ty: TypeId) -> Res<Case> {
        let mut labels = Vec::new();
        loop {
            let (lt, v) = self.case_label()?;
            labels.push(v);
            if expr_ty != lt {
                return Err(self.err(SyntaxCode::IncompatibleTypes));
            }
            self.get_token()?;
            if self.tok() == Tok::Comma {
                self.get_token()?;
                if !self.tok_in(CASE_LABEL_START) {
                    return Err(self.err(SyntaxCode::MissingConstant));
                }
            } else {
                break;
            }
        }
        self.synchronize(FOLLOW_CASE_LABEL, STATEMENT_START, &[])?;
        self.if_token_get_else_error(Tok::Colon, SyntaxCode::MissingColon)?;
        let body = self.statement_block(&[Tok::EndCase])?;
        self.if_token_get_else_error(Tok::EndCase, SyntaxCode::MissingEndCase)?;
        self.if_token_get_else_error(Tok::Semicolon, SyntaxCode::MissingSemicolon)?;
        self.crunch(5)?;
        Ok(Case { labels, body })
    }

    fn switch_statement(&mut self, file: u32, line: u32) -> Res<Stmt> {
        self.get_token()?;
        self.crunch(5)?;
        let expr = self.expression()?;
        let ety = expr.ty();
        let ti = self.types(ety);
        if (ti.form != Form::Scalar && ti.form != Form::Enum) || ety == REAL_TYPE {
            return Err(self.err(SyntaxCode::IncompatibleTypes));
        }
        self.synchronize(FOLLOW_SWITCH_EXPR, &[], &[])?;
        let mut cases = Vec::new();
        while self.tok() == Tok::Case {
            self.get_token()?;
            if self.tok_in(CASE_LABEL_START) {
                cases.push(self.case_branch(ety)?);
            }
        }
        self.crunch(4)?;
        self.if_token_get_else_error(Tok::EndSwitch, SyntaxCode::MissingEndSwitch)?;
        Ok(self.mk_stmt(file, line, StmtKind::Switch { expr, cases }))
    }

    fn forward_state(&mut self, name: &str) -> SymId {
        let id = self.enter_in_scope(self.display_scope(1), name, 1);
        let mut r = Routine::new(RoutineKey::Forward, ROUTINE_FLAG_STATE);
        r.scope = None;
        let s = self.env.sym_mut(id);
        s.defn = Defn::Function(Box::new(r));
        s.library = self.cur_library;
        s.ty = None;
        s.level = 1;
        id
    }

    fn trans_statement(&mut self, file: u32, line: u32) -> Res<Stmt> {
        self.get_token()?;
        if self.tok() != Tok::Identifier {
            return Err(self.err(SyntaxCode::MissingStateIdentifier));
        }
        self.get_token()?;
        // The C++ reads `wordString` only after consuming the identifier, so a `trans x` that is not followed by a
        // non-word token (normally the semicolon) looks up the *next* word instead. Reproduced on purpose.
        let name = self.sc.word.clone();
        let scope = self.display_scope(1);
        let id = match self.search_for_state(scope, &name) {
            Some(s) => s,
            None => self.forward_state(&name),
        };
        let is_state = matches!(self.env.sym(id).defn, Defn::Function(_))
            && self.env.sym(id).flags() & ROUTINE_FLAG_STATE != 0;
        if !is_state {
            return Err(self.err(SyntaxCode::MissingStateIdentifier));
        }
        self.crunch(4)?;
        Ok(self.mk_stmt(file, line, StmtKind::Trans(id)))
    }

    // ------------------------------------------------------------------------------------------
    // declarations
    // ------------------------------------------------------------------------------------------

    fn declarations(&mut self, routine: SymId, allow_functions: bool) -> Res<()> {
        if self.tok() == Tok::Const {
            self.get_token()?;
            self.const_definitions()?;
        }
        if self.tok() == Tok::Type {
            self.get_token()?;
            self.type_definitions()?;
        }
        if self.tok() == Tok::Var {
            self.get_token()?;
            self.var_declarations(routine)?;
        }
        if allow_functions {
            while matches!(self.tok(), Tok::Function | Tok::Order | Tok::State) {
                self.routine()?;
                self.synchronize(FOLLOW_ROUTINE, DECLARATION_START, STATEMENT_START)?;
                if self.tok() == Tok::Semicolon {
                    self.get_token()?;
                } else if self.tok_in(DECLARATION_START) || self.tok_in(STATEMENT_START) {
                    return Err(self.err(SyntaxCode::MissingSemicolon));
                }
            }
        } else if matches!(self.tok(), Tok::Function | Tok::Order | Tok::State) {
            return Err(self.err(SyntaxCode::NoFunctionNesting));
        }
        Ok(())
    }

    fn const_definitions(&mut self) -> Res<()> {
        while self.tok() == Tok::Identifier {
            let name = self.sc.word.clone();
            let id = self.search_and_enter_local(&name)?;
            self.env.sym_mut(id).defn = Defn::Const(ConstValue::Int(0));
            self.env.sym_mut(id).library = self.cur_library;
            self.get_token()?;
            self.if_token_get_else_error(Tok::Equal, SyntaxCode::MissingEqual)?;
            self.do_const(id)?;
            self.synchronize(FOLLOW_DECLARATION, DECLARATION_START, STATEMENT_START)?;
            if self.tok() == Tok::Semicolon {
                self.get_token()?;
            } else if self.tok_in(DECLARATION_START) || self.tok_in(STATEMENT_START) {
                return Err(self.err(SyntaxCode::MissingSemicolon));
            }
        }
        Ok(())
    }

    fn do_const(&mut self, id: SymId) -> Res<()> {
        let mut sign_minus = false;
        let mut saw_sign = false;
        if matches!(self.tok(), Tok::Plus | Tok::Minus) {
            sign_minus = self.tok() == Tok::Minus;
            saw_sign = true;
            self.get_token()?;
        }
        let set = |env: &mut Env, v: ConstValue, ty: TypeId| {
            let s = env.sym_mut(id);
            s.defn = Defn::Const(v);
            s.ty = Some(ty);
        };
        match self.tok() {
            Tok::Number => match self.sc.literal {
                Literal::Int(v) => set(
                    self.env,
                    ConstValue::Int(if sign_minus { v.wrapping_neg() } else { v }),
                    INTEGER_TYPE,
                ),
                Literal::Real(v) => set(
                    self.env,
                    ConstValue::Real(if sign_minus { -v } else { v }),
                    REAL_TYPE,
                ),
                Literal::Str(_) => unreachable!(),
            },
            Tok::Identifier => {
                let word = self.sc.word.clone();
                let Some(other) = self.search_all(&word)? else {
                    return Err(self.err(SyntaxCode::UndefinedIdentifier));
                };
                let o = self.env.sym(other).clone();
                let Defn::Const(cv) = o.defn else {
                    return Err(self.err(SyntaxCode::NotAConstantIdentifier));
                };
                let ty = o.ty.unwrap_or(DUMMY_TYPE);
                if ty == INTEGER_TYPE {
                    if let ConstValue::Int(v) = cv {
                        set(
                            self.env,
                            ConstValue::Int(if sign_minus { v.wrapping_neg() } else { v }),
                            INTEGER_TYPE,
                        );
                    }
                } else if ty == CHAR_TYPE {
                    if saw_sign {
                        return Err(self.err(SyntaxCode::InvalidConstant));
                    }
                    set(self.env, cv, CHAR_TYPE);
                } else if ty == REAL_TYPE {
                    if let ConstValue::Real(v) = cv {
                        set(
                            self.env,
                            ConstValue::Real(if sign_minus { -v } else { v }),
                            REAL_TYPE,
                        );
                    }
                } else if matches!(self.types(ty).form, Form::Enum | Form::Array) {
                    if saw_sign {
                        return Err(self.err(SyntaxCode::InvalidConstant));
                    }
                    set(self.env, cv, ty);
                }
            }
            Tok::String => {
                if saw_sign {
                    return Err(self.err(SyntaxCode::InvalidConstant));
                }
                let bytes = match &self.sc.literal {
                    Literal::Str(b) => b.clone(),
                    _ => unreachable!(),
                };
                if bytes.len() == 1 {
                    set(self.env, ConstValue::Char(bytes[0]), CHAR_TYPE);
                } else {
                    let (arr, ty) = self.env.intern_string(&bytes);
                    set(self.env, ConstValue::Str(arr), ty);
                }
            }
            _ => {
                self.env.sym_mut(id).ty = None;
                return Err(self.err(SyntaxCode::InvalidConstant));
            }
        }
        self.get_token()
    }

    fn type_definitions(&mut self) -> Res<()> {
        while self.tok() == Tok::Identifier {
            let name = self.sc.word.clone();
            let id = self.search_and_enter_local(&name)?;
            self.env.sym_mut(id).defn = Defn::Type;
            self.env.sym_mut(id).library = self.cur_library;
            self.get_token()?;
            self.if_token_get_else_error(Tok::Equal, SyntaxCode::MissingEqual)?;
            let ty = self.do_type()?;
            self.env.sym_mut(id).ty = Some(ty);
            if self.types(ty).type_sym.is_none() {
                self.env.types[ty.0 as usize].type_sym = Some(id);
            }
            self.synchronize(FOLLOW_DECLARATION, DECLARATION_START, STATEMENT_START)?;
            if self.tok() == Tok::Semicolon {
                self.get_token()?;
            } else if self.tok_in(DECLARATION_START) || self.tok_in(STATEMENT_START) {
                return Err(self.err(SyntaxCode::MissingSemicolon));
            }
        }
        Ok(())
    }

    fn do_type(&mut self) -> Res<TypeId> {
        match self.tok() {
            Tok::Identifier => {
                let word = self.sc.word.clone();
                let Some(id) = self.search_all(&word)? else {
                    return Err(self.err(SyntaxCode::UndefinedIdentifier));
                };
                if !matches!(self.env.sym(id).defn, Defn::Type) {
                    return Err(self.err(SyntaxCode::NotATypeIdentifier));
                }
                let element_type = self.env.sym(id).ty.unwrap_or(DUMMY_TYPE);
                self.get_token()?;
                if self.tok() != Tok::LBracket {
                    return Ok(element_type);
                }
                // Array type: `type[n]` or `type[n, m, ...]`.
                let outer = self.env.new_type(TypeInfo::new());
                let mut cur = outer;
                let mut dims = 1usize;
                loop {
                    self.get_token()?;
                    if !self.tok_in(INDEX_TYPE_START) {
                        return Err(self.err(SyntaxCode::InvalidIndexType));
                    }
                    {
                        let t = &mut self.env.types[cur.0 as usize];
                        t.form = Form::Array;
                        t.size = 0;
                        t.index_ty = INTEGER_TYPE;
                    }
                    match self.tok() {
                        Tok::Number => {
                            match self.sc.literal {
                                Literal::Int(v) => self.env.types[cur.0 as usize].elem_count = v,
                                _ => return Err(self.err(SyntaxCode::InvalidIndexType)),
                            }
                            self.get_token()?;
                        }
                        _ => {
                            let word = self.sc.word.clone();
                            match self.search_all(&word)? {
                                None => return Err(self.err(SyntaxCode::UndefinedIdentifier)),
                                Some(cid) => {
                                    let cs = self.env.sym(cid).clone();
                                    match (&cs.defn, cs.ty) {
                                        (Defn::Const(ConstValue::Int(v)), Some(INTEGER_TYPE)) => {
                                            self.env.types[cur.0 as usize].elem_count = *v;
                                        }
                                        _ => return Err(self.err(SyntaxCode::InvalidIndexType)),
                                    }
                                }
                            }
                            self.get_token()?;
                        }
                    }
                    self.synchronize(FOLLOW_DIMENSION, &[], &[])?;
                    if self.tok() == Tok::Comma {
                        dims += 1;
                        if dims > self.env.cfg.max_array_dims {
                            return Err(self.err(SyntaxCode::NestingTooDeep));
                        }
                        let next = self.env.new_type(TypeInfo::new());
                        self.env.types[cur.0 as usize].elem_ty = next;
                        cur = next;
                    } else {
                        break;
                    }
                }
                self.if_token_get_else_error(Tok::RBracket, SyntaxCode::MissingRBracket)?;
                self.env.types[cur.0 as usize].elem_ty = element_type;
                self.array_size(outer)?;
                Ok(outer)
            }
            Tok::LParen => self.enumeration_type(),
            _ => Err(self.err(SyntaxCode::InvalidType)),
        }
    }

    /// `arraySize`, with overflow and memory checks the C++ lacks.
    fn array_size(&mut self, t: TypeId) -> Res<i32> {
        let elem = self.types(t).elem_ty;
        if self.types(elem).size == 0 && self.types(elem).form == Form::Array {
            let s = self.array_size(elem)?;
            self.env.types[elem.0 as usize].size = s;
        }
        let es = self.types(elem).size as i64;
        let count = self.types(t).elem_count;
        if count < -1 {
            return Err(self.err(SyntaxCode::InvalidIndexType));
        }
        let size = if count == -1 { es } else { count as i64 * es };
        if size > self.env.cfg.max_array_bytes as i64 {
            return Err(self.err(SyntaxCode::InvalidIndexType));
        }
        self.env.types[t.0 as usize].size = size as i32;
        Ok(size as i32)
    }

    fn enumeration_type(&mut self) -> Res<TypeId> {
        let mut ti = TypeInfo::new();
        ti.form = Form::Enum;
        ti.size = 4;
        let ty = self.env.new_type(ti);
        let mut value = -1;
        self.get_token()?;
        while self.tok() == Tok::Identifier {
            let name = self.sc.word.clone();
            let id = self.search_and_enter_local(&name)?;
            value += 1;
            let s = self.env.sym_mut(id);
            s.defn = Defn::Const(ConstValue::Int(value));
            s.ty = Some(ty);
            s.library = self.cur_library;
            self.get_token()?;
            self.if_token_get(Tok::Comma)?;
        }
        self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
        self.env.types[ty.0 as usize].enum_max = value;
        Ok(ty)
    }

    fn var_declarations(&mut self, routine: SymId) -> Res<()> {
        let param_count = self
            .env
            .sym(routine)
            .routine()
            .map(|r| r.params.len())
            .unwrap_or(0) as u32;
        let mut offset = param_count;
        let mut total_size: i32 = 0;
        while matches!(self.tok(), Tok::Identifier | Tok::Eternal | Tok::Static) {
            let mut var_kind = VarKind::Normal;
            if matches!(self.tok(), Tok::Eternal | Tok::Static) {
                var_kind = if self.tok() == Tok::Eternal {
                    VarKind::Eternal
                } else {
                    VarKind::Static
                };
                self.get_token()?;
                if self.tok() != Tok::Identifier {
                    return Err(self.err(SyntaxCode::MissingIdentifier));
                }
            }
            let ty = self.do_type()?;
            let size = self.types(ty).size;
            let mut names: Vec<SymId> = Vec::new();
            while self.tok() == Tok::Identifier {
                let name = self.sc.word.clone();
                let id = if var_kind == VarKind::Eternal {
                    // Eternals live in the global (level 0) table.
                    if self.search_scope(ScopeId(0), &name).is_some() {
                        return Err(self.err(SyntaxCode::RedefinedIdentifier));
                    }
                    self.enter_in_scope(ScopeId(0), &name, 0)
                } else {
                    self.search_and_enter_local(&name)?
                };
                self.env.sym_mut(id).library = self.cur_library;
                names.push(id);
                self.get_token()?;
                self.if_token_get(Tok::Comma)?;
            }
            for id in names {
                self.env.sym_mut(id).ty = Some(ty);
                let slot = match var_kind {
                    VarKind::Normal => {
                        total_size = total_size.saturating_add(size);
                        let s = offset;
                        offset += 1;
                        self.env.sym_mut(id).defn = Defn::Var {
                            kind: VarKind::Normal,
                            offset: s,
                        };
                        if let Some(r) = self.env.sym_mut(routine).routine_mut() {
                            r.locals.push(id);
                        }
                        s
                    }
                    VarKind::Eternal => {
                        let idx = self
                            .env
                            .alloc_eternal(self.types(ty).form == Form::Array, size)?;
                        self.env.sym_mut(id).defn = Defn::Var {
                            kind: VarKind::Eternal,
                            offset: idx,
                        };
                        idx
                    }
                    VarKind::Static => {
                        if self.static_sizes.len() >= self.env.cfg.max_static_variables {
                            return Err(self.err(SyntaxCode::TooManyStaticVars));
                        }
                        let idx = self.static_sizes.len() as u32;
                        self.static_sizes
                            .push(if self.types(ty).form == Form::Array {
                                size
                            } else {
                                0
                            });
                        self.env.sym_mut(id).defn = Defn::Var {
                            kind: VarKind::Static,
                            offset: idx,
                        };
                        idx
                    }
                };
                let _ = slot;
            }
            self.synchronize(FOLLOW_VARIABLES, DECLARATION_START, STATEMENT_START)?;
            if self.tok() == Tok::Semicolon {
                self.get_token()?;
            } else if self.tok_in(DECLARATION_START) || self.tok_in(STATEMENT_START) {
                return Err(self.err(SyntaxCode::MissingSemicolon));
            }
        }
        self.synchronize(FOLLOW_VAR_BLOCK, &[], &[])?;
        if total_size > MAX_LOCAL_SIZE {
            return Err(self.err(SyntaxCode::TooManyLocalVariables));
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------
    // routines and modules
    // ------------------------------------------------------------------------------------------

    fn formal_param_list(&mut self) -> Res<Vec<SymId>> {
        let mut params: Vec<SymId> = Vec::new();
        self.get_token()?;
        while matches!(self.tok(), Tok::Identifier | Tok::Ref) {
            let mut is_ref = false;
            if self.tok() == Tok::Ref {
                is_ref = true;
                self.get_token()?;
            }
            let param_ty;
            if self.tok() == Tok::Identifier {
                let id = self.search_and_find_all()?;
                if !matches!(self.env.sym(id).defn, Defn::Type) {
                    return Err(self.err(SyntaxCode::InvalidType));
                }
                param_ty = self.env.sym(id).ty.unwrap_or(DUMMY_TYPE);
                self.get_token()?;
            } else {
                return Err(self.err(SyntaxCode::MissingIdentifier));
            }
            if self.tok() == Tok::Identifier {
                let name = self.sc.word.clone();
                let id = self.search_and_enter_local(&name)?;
                let offset = params.len() as u32;
                let s = self.env.sym_mut(id);
                s.defn = if is_ref {
                    Defn::RefParam { offset }
                } else {
                    Defn::ValParam { offset }
                };
                s.ty = Some(param_ty);
                params.push(id);
                self.get_token()?;
            } else {
                return Err(self.err(SyntaxCode::MissingIdentifier));
            }
            self.synchronize(FOLLOW_PARAMS, &[], &[])?;
            self.if_token_get(Tok::Comma)?;
        }
        self.if_token_get_else_error(Tok::RParen, SyntaxCode::MissingRParen)?;
        if params.len() > 255 {
            return Err(self.err(SyntaxCode::TooManyFormalParameters));
        }
        Ok(params)
    }

    fn module_header(&mut self) -> Res<SymId> {
        let mut is_fsm = false;
        if self.cur_library.is_some() {
            self.if_token_get_else_error(Tok::Library, SyntaxCode::MissingLibrary)?;
        } else if self.tok() == Tok::Fsm {
            is_fsm = true;
            self.get_token()?;
        } else {
            self.if_token_get_else_error(Tok::Module, SyntaxCode::MissingModule)?;
        }
        if self.tok() != Tok::Identifier {
            return Err(self.err(SyntaxCode::MissingIdentifier));
        }
        let name = self.sc.word.clone();
        let id = self.search_and_enter_local(&name)?;
        {
            let mut r = Routine::new(
                RoutineKey::Declared,
                if is_fsm { ROUTINE_FLAG_FSM } else { 0 },
            );
            r.scope = None;
            let s = self.env.sym_mut(id);
            s.defn = Defn::Module(Box::new(r));
            s.library = self.cur_library;
            s.ty = Some(DUMMY_TYPE);
        }
        if is_fsm {
            self.sc.end_word = "endfsm";
        } else if self.cur_library.is_some() {
            self.sc.end_word = "endlibrary";
        }
        self.cur_module = Some(id);
        self.get_token()?;

        self.synchronize(FOLLOW_MODULE_ID, DECLARATION_START, STATEMENT_START)?;
        let scope = self.enter_scope()?;
        if let Some(r) = self.env.sym_mut(id).routine_mut() {
            r.scope = Some(scope);
        }

        if self.tok() == Tok::LParen {
            let params = self.formal_param_list()?;
            if let Some(r) = self.env.sym_mut(id).routine_mut() {
                r.params = params;
            }
        }

        self.env.sym_mut(id).ty = None;
        if self.tok() == Tok::Colon {
            self.get_token()?;
            if self.tok() == Tok::Identifier {
                let tid = self.search_and_find_all()?;
                if !matches!(self.env.sym(tid).defn, Defn::Type) {
                    return Err(self.err(SyntaxCode::InvalidType));
                }
                let t = self.env.sym(tid).ty;
                self.env.sym_mut(id).ty = t;
                self.get_token()?;
            } else {
                return Err(self.err(SyntaxCode::MissingIdentifier));
            }
        }
        Ok(id)
    }

    fn function_header(&mut self) -> Res<(SymId, bool)> {
        let is_state = self.tok() == Tok::State;
        let is_order = self.tok() == Tok::Order;
        self.get_token()?;
        let mut forward_flag = false;
        if self.tok() != Tok::Identifier {
            return Err(self.err(SyntaxCode::MissingIdentifier));
        }
        let name = self.sc.word.clone();
        let scope = self.display_scope(self.level);
        let id = match self.search_for_function(scope, &name) {
            None => {
                let id = self.enter_local(&name);
                let mut flags = 0;
                if is_order {
                    flags |= ROUTINE_FLAG_ORDER;
                }
                if is_state {
                    flags |= ROUTINE_FLAG_STATE;
                }
                let s = self.env.sym_mut(id);
                s.defn = Defn::Function(Box::new(Routine::new(RoutineKey::Declared, flags)));
                s.library = self.cur_library;
                s.ty = Some(DUMMY_TYPE);
                id
            }
            Some(existing) => {
                if self.env.sym(existing).routine().map(|r| r.key) == Some(RoutineKey::Forward) {
                    forward_flag = true;
                    existing
                } else {
                    return Err(self.err(SyntaxCode::RedefinedIdentifier));
                }
            }
        };
        self.get_token()?;

        self.synchronize(FOLLOW_MODULE_ID, DECLARATION_START, STATEMENT_START)?;
        let fscope = self.enter_scope()?;
        if self.tok() == Tok::LParen {
            let params = self.formal_param_list()?;
            if forward_flag {
                return Err(self.err(SyntaxCode::AlreadyForwarded));
            }
            if let Some(r) = self.env.sym_mut(id).routine_mut() {
                r.params = params;
            }
        } else if !forward_flag {
            if let Some(r) = self.env.sym_mut(id).routine_mut() {
                r.params.clear();
            }
        }

        self.env.sym_mut(id).ty = None;
        let mut has_integer_return = false;
        if self.tok() == Tok::Colon {
            self.get_token()?;
            if self.tok() == Tok::Identifier {
                let tid = self.search_and_find_all()?;
                if !matches!(self.env.sym(tid).defn, Defn::Type) {
                    return Err(self.err(SyntaxCode::InvalidType));
                }
                let t = self.env.sym(tid).ty;
                has_integer_return = t == Some(INTEGER_TYPE);
                if !forward_flag {
                    self.env.sym_mut(id).ty = t;
                }
                self.get_token()?;
            } else {
                return Err(self.err(SyntaxCode::MissingIdentifier));
            }
            if forward_flag {
                return Err(self.err(SyntaxCode::AlreadyForwarded));
            }
        }
        if is_order && !has_integer_return {
            return Err(self.err(SyntaxCode::OrderReturnType));
        }
        if let Some(r) = self.env.sym_mut(id).routine_mut() {
            r.scope = Some(fscope);
        }
        Ok((id, forward_flag))
    }

    fn routine(&mut self) -> Res<()> {
        let (id, _forward) = self.function_header()?;
        let prev_routine = self.cur_routine;
        self.cur_routine = Some(id);

        self.synchronize(FOLLOW_HEADER, DECLARATION_START, STATEMENT_START)?;
        if self.tok() == Tok::Semicolon {
            self.get_token()?;
        } else if self.tok_in(DECLARATION_START) || self.tok_in(STATEMENT_START) {
            return Err(self.err(SyntaxCode::MissingSemicolon));
        }

        if self.sc.word != "forward" {
            if let Some(r) = self.env.sym_mut(id).routine_mut() {
                r.key = RoutineKey::Declared;
                r.locals.clear();
            }
            self.declarations(id, false)?;
            self.synchronize(FOLLOW_MODULE_DECLS, &[], &[])?;
            if self.tok() != Tok::Code {
                return Err(self.err(SyntaxCode::MissingCode));
            }
            self.code_bytes = 1;
            self.block_flag = true;
            let order_index = self.num_order_calls;
            self.get_token()?;
            let flags = self.env.sym(id).flags();
            let end_tok = if flags & ROUTINE_FLAG_STATE != 0 {
                Tok::EndState
            } else if flags & ROUTINE_FLAG_ORDER != 0 {
                Tok::EndOrder
            } else {
                Tok::EndFunction
            };
            let stmts = self.body_block(end_tok)?;
            self.if_token_get_else_error(end_tok, SyntaxCode::MissingEndFunction)?;
            self.block_flag = false;
            let r = self.env.sym_mut(id).routine_mut().expect("routine info");
            r.order_call_index = order_index;
            r.num_order_calls = self.num_order_calls - order_index;
            r.body = Some(Rc::new(Body { stmts }));
            r.code_size = self.code_bytes;
            self.code_bytes = 0;
        } else {
            self.get_token()?;
            if let Some(r) = self.env.sym_mut(id).routine_mut() {
                r.key = RoutineKey::Forward;
            }
        }

        if self.env.sym(id).flags() & ROUTINE_FLAG_STATE != 0 {
            if self.state_handles.len() >= self.env.cfg.max_state_handles {
                return Err(AblError::fatal("ABL: too many states in fsm [19 max]"));
            }
            let name = self.env.sym(id).name.clone();
            self.state_handles.push((name, id));
        }

        self.exit_scope();
        self.cur_routine = prev_routine;
        Ok(())
    }

    /// Compile a whole module or library (`ABLi_preProcess` body). The scanner must already be set up.
    pub(crate) fn parse_module(mut self, path: &str) -> Res<ParsedModule> {
        let debugger = self.env.cfg.debugger_active;
        self.sc.reset_for_module(debugger);
        self.sc.open_source_file(path)?;
        self.get_token()?;

        let module_sym = self.module_header()?;
        self.cur_routine = Some(module_sym);
        let is_fsm = self.env.sym(module_sym).flags() & ROUTINE_FLAG_FSM != 0;

        self.synchronize(FOLLOW_HEADER, DECLARATION_START, STATEMENT_START)?;
        if self.tok() == Tok::Semicolon {
            self.get_token()?;
        } else if self.tok_in(DECLARATION_START) || self.tok_in(STATEMENT_START) {
            return Err(self.err(SyntaxCode::MissingSemicolon));
        }

        self.declarations(module_sym, true)?;
        let mut stmts = Vec::new();
        if !is_fsm {
            self.synchronize(FOLLOW_MODULE_DECLS, &[], &[])?;
            if self.tok() != Tok::Code {
                return Err(self.err(SyntaxCode::MissingCode));
            }
            self.code_bytes = 1;
            self.block_flag = true;
            self.get_token()?;
            let end_tok = if self.cur_library.is_some() {
                Tok::EndLibrary
            } else {
                Tok::EndModule
            };
            stmts = self.body_block(end_tok)?;
        }

        if self.cur_library.is_some() {
            self.if_token_get_else_error(Tok::EndLibrary, SyntaxCode::MissingEndLibrary)?;
        } else if is_fsm {
            self.if_token_get_else_error(Tok::EndFsm, SyntaxCode::MissingEndFsm)?;
        } else {
            self.if_token_get_else_error(Tok::EndModule, SyntaxCode::MissingEndModule)?;
        }
        self.block_flag = false;

        let code_size = self.code_bytes;
        let lib_scope = self.exit_scope();
        {
            let r = self
                .env
                .sym_mut(module_sym)
                .routine_mut()
                .expect("module routine");
            r.scope = Some(lib_scope);
            r.body = Some(Rc::new(Body { stmts }));
            r.code_size = code_size;
        }

        self.if_token_get_else_error(Tok::Period, SyntaxCode::MissingPeriod)?;
        if self.tok() != Tok::Eof {
            return Err(self.err(SyntaxCode::UnexpectedToken));
        }
        self.sc.close_source_file();

        Ok(ParsedModule {
            module_sym,
            source_files: self.sc.source_files.clone(),
            libraries_used: self.libs_used.clone(),
            static_sizes: self.static_sizes.clone(),
            num_order_calls: self.num_order_calls,
            state_handles: self.state_handles.clone(),
            lines: self.sc.lines_read,
        })
    }
}
