// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The interface between scripts and the game: argument values, array access and the context handed to
//! [`GameBindings`](crate::natives::GameBindings) methods (the safe replacement for `ABLi_pop*` / `ABLi_push*`).

use crate::env::Env;
use crate::value::ArrRef;

/// A reference to script array data (an `integer[]`, `real[]`, `char[]` string or a sub-array of a multi-dimensional
/// array). Access it through [`NativeCtx`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrayRef(pub(crate) ArrRef);

/// The value of a `?` parameter (`ABLStackItem` in `ablexec.h`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnyValue {
    /// `char`
    Char(u8),
    /// `integer`
    Int(i32),
    /// `real`
    Real(f32),
    /// `boolean`
    Bool(bool),
    /// `char[]`
    CharArray(ArrayRef),
    /// `integer[]`
    IntArray(ArrayRef),
    /// `real[]`
    RealArray(ArrayRef),
    /// `boolean[]`
    BoolArray(ArrayRef),
}

/// An evaluated argument of a native call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeArg {
    /// `i`
    Int(i32),
    /// `r` and `*` (after the integer-to-float policy of the function was applied)
    Real(f32),
    /// `b`
    Bool(bool),
    /// `c`
    Char(u8),
    /// `C I R B`
    Array(ArrayRef),
    /// `?`
    Any(AnyValue),
}

impl NativeArg {
    /// Integer value (0 for other kinds).
    pub fn int(self) -> i32 {
        match self {
            NativeArg::Int(v) => v,
            NativeArg::Bool(b) => b as i32,
            NativeArg::Char(c) => c as i32,
            _ => 0,
        }
    }
    /// Real value.
    pub fn real(self) -> f32 {
        match self {
            NativeArg::Real(v) => v,
            NativeArg::Int(v) => v as f32,
            _ => 0.0,
        }
    }
    /// Boolean value (`ABLi_popBoolean`: true only for exactly 1).
    pub fn boolean(self) -> bool {
        match self {
            NativeArg::Bool(b) => b,
            NativeArg::Int(v) => v == 1,
            _ => false,
        }
    }
    /// Char value.
    pub fn char_(self) -> u8 {
        match self {
            NativeArg::Char(c) => c,
            NativeArg::Int(v) => v as u8,
            _ => 0,
        }
    }
    /// Array reference. Panics if the argument is not an array (the type checker guarantees it is).
    pub fn array(self) -> ArrayRef {
        match self {
            NativeArg::Array(a) => a,
            other => panic!("native argument is not an array: {other:?}"),
        }
    }
    /// The `?` value.
    pub fn any(self) -> AnyValue {
        match self {
            NativeArg::Any(a) => a,
            NativeArg::Int(v) => AnyValue::Int(v),
            NativeArg::Real(v) => AnyValue::Real(v),
            NativeArg::Bool(v) => AnyValue::Bool(v),
            NativeArg::Char(v) => AnyValue::Char(v),
            NativeArg::Array(a) => AnyValue::IntArray(a),
        }
    }
}

/// The value a native call produces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeRet {
    /// No value.
    None,
    /// Integer.
    Int(i32),
    /// Real.
    Real(f32),
    /// Boolean.
    Bool(bool),
}

/// Signature of a host closure registered with [`Env::add_function`].
pub type NativeFn = dyn FnMut(&mut NativeCtx<'_>, &[NativeArg]) -> NativeRet;

/// Context passed to every game function: array access, the current module's FSM/order state and the host RNG.
pub struct NativeCtx<'a> {
    pub(crate) env: &'a mut Env,
}

impl<'a> NativeCtx<'a> {
    pub(crate) fn new(env: &'a mut Env) -> NativeCtx<'a> {
        NativeCtx { env }
    }

    fn elem(&self, a: ArrayRef, index: usize, size: usize) -> ArrRef {
        ArrRef {
            id: a.0.id,
            off: a.0.off.wrapping_add((index * size) as u32),
        }
    }

    /// Bytes available from the start of the array reference to the end of its storage.
    pub fn len_bytes(&self, a: ArrayRef) -> usize {
        self.env.heap.tail(a.0).len()
    }

    /// Element `index` of an `integer[]` (or `boolean[]`); out-of-range reads give 0.
    pub fn get_i32(&self, a: ArrayRef, index: usize) -> i32 {
        self.env.heap.read_u32(self.elem(a, index, 4)) as i32
    }

    /// Store element `index` of an `integer[]`; out-of-range writes are ignored.
    pub fn set_i32(&mut self, a: ArrayRef, index: usize, v: i32) {
        let r = self.elem(a, index, 4);
        self.env.heap.write_u32(r, v as u32);
    }

    /// Element `index` of a `real[]`.
    pub fn get_f32(&self, a: ArrayRef, index: usize) -> f32 {
        f32::from_bits(self.env.heap.read_u32(self.elem(a, index, 4)))
    }

    /// Store element `index` of a `real[]`.
    pub fn set_f32(&mut self, a: ArrayRef, index: usize, v: f32) {
        let r = self.elem(a, index, 4);
        self.env.heap.write_u32(r, v.to_bits());
    }

    /// Byte `index` of a `char[]`.
    pub fn get_u8(&self, a: ArrayRef, index: usize) -> u8 {
        self.env.heap.read_u8(self.elem(a, index, 1))
    }

    /// Store byte `index` of a `char[]`.
    pub fn set_u8(&mut self, a: ArrayRef, index: usize, v: u8) {
        let r = self.elem(a, index, 1);
        self.env.heap.write_u8(r, v);
    }

    /// Read a NUL-terminated string from a `char[]`.
    pub fn string(&self, a: ArrayRef) -> String {
        String::from_utf8_lossy(self.env.heap.cstr(a.0)).into_owned()
    }

    /// `strcpy` into a `char[]` (truncated to the array size, always NUL terminated when it fits).
    pub fn write_string(&mut self, a: ArrayRef, s: &str) {
        let dst = self.env.heap.tail_mut(a.0);
        if dst.is_empty() {
            return;
        }
        let n = s.len().min(dst.len() - 1);
        dst[..n].copy_from_slice(&s.as_bytes()[..n]);
        dst[n] = 0;
    }

    /// Name of the instance currently executing (`CurModule->getName()`).
    pub fn module_name(&self) -> &str {
        self.env.instances[self.env.rt.cur_inst.0 as usize]
            .name
            .as_str()
    }

    /// Source line of the statement being executed.
    pub fn current_line(&self) -> u32 {
        self.env.rt.exec_line
    }

    /// `ABLi_getCurrentState`: state handle of the running FSM, 0 when the running module is not an FSM.
    pub fn current_state_handle(&self) -> i32 {
        match self.env.rt.cur_fsm {
            Some(f) => self.env.state_handle(f),
            None => 0,
        }
    }

    /// `ABLi_transState`: switch the running FSM to the state with this handle.
    pub fn trans_state(&mut self, handle: i32) {
        self.env.trans_state_handle(handle);
    }

    /// `ABLi_resetOrders`: clear the order-completion bits of the running module.
    pub fn reset_orders(&mut self) {
        let inst = self.env.rt.cur_inst;
        self.env.reset_orders(inst);
    }

    /// `ABLi_getSkipOrder`. Natives registered as orders are never called with this set (the interpreter answers 1
    /// itself, as every C++ order callback does).
    pub fn skip_order(&self) -> bool {
        self.env.rt.skip_order
    }

    /// The host's random number generator (`ABLRandomCallback`).
    pub fn random(&mut self, range: i32) -> i32 {
        self.env.host.random(range)
    }
}
