/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Instruction arguments.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Write;

use itertools::Itertools;

use crate::collections::Hashed;
use crate::collections::SmallMap;
use crate::collections::symbol::symbol::Symbol;
use crate::environment::slots::ModuleSlotId;
use crate::eval::bc::addr::BcAddr;
use crate::eval::bc::addr::BcAddrOffset;
use crate::eval::bc::addr::BcAddrOffsetNeg;
use crate::eval::bc::addr::BcPtrAddr;
use crate::eval::bc::call::BcCallArgsFull;
use crate::eval::bc::call::BcCallArgsPos;
use crate::eval::bc::for_loop::LoopDepth;
use crate::eval::bc::instr::BcInstr;
use crate::eval::bc::instr_impl::InstrDefData;
use crate::eval::bc::native_function::BcNativeFunction;
use crate::eval::bc::opcode::BcOpcode;
use crate::eval::bc::opcode::BcOpcodeHandler;
use crate::eval::bc::slow_arg::BcInstrEndArg;
use crate::eval::bc::stack_ptr::BcSlot;
use crate::eval::bc::stack_ptr::BcSlotIn;
use crate::eval::bc::stack_ptr::BcSlotInRange;
use crate::eval::bc::stack_ptr::BcSlotInRangeFrom;
use crate::eval::bc::stack_ptr::BcSlotOut;
use crate::eval::compiler::def::Def;
use crate::eval::runtime::arguments::ArgSymbol;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::eval::runtime::slots::LocalCapturedSlotId;
use crate::eval::runtime::slots::LocalSlotId;
use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkSerialize;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::any_complex::StarlarkAnyComplex;
use crate::values::function::NativeFunction;
use crate::values::int::pointer_i32::PointerI32;
use crate::values::layout::value_not_special::ValueNotSpecial;
use crate::values::string::StarlarkStr;
use crate::values::types::any_array::AnyArray;
use crate::values::types::any_array::AnyArrayRegistered;
use crate::values::types::known_methods::KnownMethod;
use crate::values::typing::type_compiled::compiled::TypeCompiled;

/// Truncate value if it is too long.
struct TruncateValueRepr<'v>(Value<'v>);

impl<'v> Display for TruncateValueRepr<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let repr = self.0.to_repr();
        // Truncate too long constants (like dicts with hundreds of elements).
        if repr.len() > 100 {
            write!(f, "<{}>", self.0.get_type())
        } else {
            write!(f, "{repr}")
        }
    }
}

/// Instruction fixed argument, at the brand `'v` of the bytecode.
pub(crate) trait BcInstrArg<'v>: StarlarkSerialize + StarlarkDeserialize<'v> {
    /// Append space then append the argument, or append nothing if the argument is empty.
    fn fmt_append(
        param: &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result;
    /// Collect instruction jump addresses.
    fn visit_jump_addr(param: &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr));
}

impl<'v> BcInstrArg<'v> for () {
    fn fmt_append(
        _param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        _f: &mut dyn Write,
    ) -> fmt::Result {
        Ok(())
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for u32 {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {param}")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for i32 {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {param}")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v, A: BcInstrArg<'v>, B: BcInstrArg<'v>> BcInstrArg<'v> for (A, B) {
    fn fmt_append(
        (a, b): &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        A::fmt_append(a, ip, end_arg, f)?;
        B::fmt_append(b, ip, end_arg, f)?;
        Ok(())
    }

    fn visit_jump_addr((a, b): &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        A::visit_jump_addr(a, ip, consumer);
        B::visit_jump_addr(b, ip, consumer);
    }
}

impl<'v, A: BcInstrArg<'v>, B: BcInstrArg<'v>, C: BcInstrArg<'v>> BcInstrArg<'v> for (A, B, C) {
    fn fmt_append(
        (a, b, c): &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        A::fmt_append(a, ip, end_arg, f)?;
        B::fmt_append(b, ip, end_arg, f)?;
        C::fmt_append(c, ip, end_arg, f)?;
        Ok(())
    }

    fn visit_jump_addr((a, b, c): &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        A::visit_jump_addr(a, ip, consumer);
        B::visit_jump_addr(b, ip, consumer);
        C::visit_jump_addr(c, ip, consumer);
    }
}

#[allow(clippy::many_single_char_names)]
impl<'v, A: BcInstrArg<'v>, B: BcInstrArg<'v>, C: BcInstrArg<'v>, D: BcInstrArg<'v>> BcInstrArg<'v>
    for (A, B, C, D)
{
    fn fmt_append(
        (a, b, c, d): &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        A::fmt_append(a, ip, end_arg, f)?;
        B::fmt_append(b, ip, end_arg, f)?;
        C::fmt_append(c, ip, end_arg, f)?;
        D::fmt_append(d, ip, end_arg, f)?;
        Ok(())
    }

    fn visit_jump_addr((a, b, c, d): &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        A::visit_jump_addr(a, ip, consumer);
        B::visit_jump_addr(b, ip, consumer);
        C::visit_jump_addr(c, ip, consumer);
        D::visit_jump_addr(d, ip, consumer);
    }
}

#[allow(clippy::many_single_char_names)]
impl<
    'v,
    A: BcInstrArg<'v>,
    B: BcInstrArg<'v>,
    C: BcInstrArg<'v>,
    D: BcInstrArg<'v>,
    E: BcInstrArg<'v>,
> BcInstrArg<'v> for (A, B, C, D, E)
{
    fn fmt_append(
        (a, b, c, d, e): &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        A::fmt_append(a, ip, end_arg, f)?;
        B::fmt_append(b, ip, end_arg, f)?;
        C::fmt_append(c, ip, end_arg, f)?;
        D::fmt_append(d, ip, end_arg, f)?;
        E::fmt_append(e, ip, end_arg, f)?;
        Ok(())
    }

    fn visit_jump_addr((a, b, c, d, e): &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        A::visit_jump_addr(a, ip, consumer);
        B::visit_jump_addr(b, ip, consumer);
        C::visit_jump_addr(c, ip, consumer);
        D::visit_jump_addr(d, ip, consumer);
        E::visit_jump_addr(e, ip, consumer);
    }
}

#[allow(clippy::many_single_char_names)]
impl<
    'v,
    A: BcInstrArg<'v>,
    B: BcInstrArg<'v>,
    C: BcInstrArg<'v>,
    D: BcInstrArg<'v>,
    E: BcInstrArg<'v>,
    F: BcInstrArg<'v>,
> BcInstrArg<'v> for (A, B, C, D, E, F)
{
    fn fmt_append(
        (a, b, c, d, e, f): &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        w: &mut dyn Write,
    ) -> fmt::Result {
        A::fmt_append(a, ip, end_arg, w)?;
        B::fmt_append(b, ip, end_arg, w)?;
        C::fmt_append(c, ip, end_arg, w)?;
        D::fmt_append(d, ip, end_arg, w)?;
        E::fmt_append(e, ip, end_arg, w)?;
        F::fmt_append(f, ip, end_arg, w)?;
        Ok(())
    }

    fn visit_jump_addr((a, b, c, d, e, f): &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        A::visit_jump_addr(a, ip, consumer);
        B::visit_jump_addr(b, ip, consumer);
        C::visit_jump_addr(c, ip, consumer);
        D::visit_jump_addr(d, ip, consumer);
        E::visit_jump_addr(e, ip, consumer);
        F::visit_jump_addr(f, ip, consumer);
    }
}

impl<'v, A: BcInstrArg<'v>, const N: usize> BcInstrArg<'v> for [A; N] {
    fn fmt_append(
        param: &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        for a in param {
            A::fmt_append(a, ip, end_arg, f)?;
        }
        Ok(())
    }

    fn visit_jump_addr(param: &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        for a in param {
            A::visit_jump_addr(a, ip, consumer);
        }
    }
}

impl<'v> BcInstrArg<'v> for BcAddrOffset {
    fn fmt_append(
        param: &Self,
        ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", ip.offset(*param).0)
    }

    fn visit_jump_addr(param: &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        consumer(ip.offset(*param));
    }
}

impl<'v> BcInstrArg<'v> for BcAddrOffsetNeg {
    fn fmt_append(
        param: &Self,
        ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", ip.offset_neg(*param).0)
    }

    fn visit_jump_addr(param: &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        consumer(ip.offset_neg(*param));
    }
}

impl<'v> BcInstrArg<'v> for Value<'v> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", TruncateValueRepr(*param))
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for ValueNotSpecial<'v> {
    fn fmt_append(
        param: &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        Value::fmt_append(&param.to_value(), ip, end_arg, f)
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for TypeCompiled<'v> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {param}")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v, T: BcInstrArg<'v>> BcInstrArg<'v> for Option<T> {
    fn fmt_append(
        param: &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        match param {
            None => write!(f, " ()"),
            Some(v) => T::fmt_append(v, ip, end_arg, f),
        }
    }

    fn visit_jump_addr(param: &Self, ip: BcAddr, consumer: &mut dyn FnMut(BcAddr)) {
        if let Some(param) = param {
            T::visit_jump_addr(param, ip, consumer);
        }
    }
}

impl<'v> BcInstrArg<'v> for String {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, "{param:?}")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

/// Typed values print as the value they are.
macro_rules! impl_bc_instr_arg_for_value_typed {
    ($($t:ty),* $(,)?) => {
        $(
            impl<'v> BcInstrArg<'v> for ValueTyped<'v, $t> {
                fn fmt_append(
                    param: &Self,
                    _ip: BcAddr,
                    _end_arg: Option<&BcInstrEndArg<'v>>,
                    f: &mut dyn Write,
                ) -> fmt::Result {
                    write!(f, " {}", TruncateValueRepr(param.to_value()))
                }

                fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
            }
        )*
    };
}

impl_bc_instr_arg_for_value_typed!(StarlarkStr, PointerI32, Def<'v>, NativeFunction<'v>);

impl<'v> BcInstrArg<'v> for ValueOf<'v, StarlarkStr> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", TruncateValueRepr(param.to_value()))
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v, T: AnyArrayRegistered> BcInstrArg<'v> for ValueTyped<'v, AnyArray<T>> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", TruncateValueRepr(param.to_value()))
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

/// The span of a call instruction prints as the span.
impl<'v> BcInstrArg<'v> for ValueTyped<'v, StarlarkAnyComplex<FrameSpan<'v>>> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", param.value)
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for BcNativeFunction<'v> {
    fn fmt_append(
        param: &Self,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        BcInstrArg::fmt_append(&param.fun(), ip, end_arg, f)
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

struct BcSlotDisplay<'a, 'v>(BcSlot, Option<&'a BcInstrEndArg<'v>>);

impl<'a, 'v> Display for BcSlotDisplay<'a, 'v> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let name = self
            .1
            .and_then(|end_arg| end_arg.local_names.get(self.0.0 as usize));
        match name {
            Some(name) => write!(f, "&{}", name.as_str()),
            None => write!(f, "&{}", self.0.0),
        }
    }
}

impl<'v> BcInstrArg<'v> for LocalSlotId {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", BcSlotDisplay(param.to_bc_slot(), end_arg))
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for LocalCapturedSlotId {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", BcSlotDisplay(param.to_bc_slot(), end_arg))
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for BcSlotIn {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", BcSlotDisplay(param.get(), end_arg))
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for BcSlotOut {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " ->{}", BcSlotDisplay(param.get(), end_arg))
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for BcSlotInRange {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(
            f,
            " [{}]",
            param
                .iter()
                .map(|s| BcSlotDisplay(s.get(), end_arg).to_string())
                .join(", ")
        )
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for BcSlotInRangeFrom {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}..", param.0)
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for ModuleSlotId {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " m{}", param.0)
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for FrameSpan<'v> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {param}")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

/// Opcode as instruction argument.
impl<'v> BcInstrArg<'v> for BcOpcode {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {param:?}")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for LoopDepth {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {param}")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for KnownMethod {
    fn fmt_append(
        _param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " <m>")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for Symbol {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", param.as_str())
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for Box<[Value<'v>]> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " [")?;
        for (i, v) in param.iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", TruncateValueRepr(*v))?;
        }
        write!(f, "]")?;
        Ok(())
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for Box<[Hashed<Value<'v>>]> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " [")?;
        for (i, v) in param.iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", TruncateValueRepr(*v.key()))?;
        }
        write!(f, "]")?;
        Ok(())
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for SmallMap<Value<'v>, Value<'v>> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {{")?;
        for (i, (k, v)) in param.iter().enumerate() {
            if i != 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}: {}", TruncateValueRepr(*k), TruncateValueRepr(*v))?;
        }
        write!(f, "}}")?;
        Ok(())
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for InstrDefData<'v> {
    fn fmt_append(
        _param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " InstrDefData")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v, S: ArgSymbol> BcInstrArg<'v> for BcCallArgsFull<'v, S> {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {{{param}}}")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for BcCallArgsPos {
    fn fmt_append(
        param: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " {}", param.pos)
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl<'v> BcInstrArg<'v> for BcInstrEndArg<'v> {
    fn fmt_append(
        _: &Self,
        _ip: BcAddr,
        _end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        write!(f, " BcInstrEndArg")
    }

    fn visit_jump_addr(_param: &Self, _ip: BcAddr, _consumer: &mut dyn FnMut(BcAddr)) {}
}

impl BcOpcode {
    /// Format instruction argument.
    pub(crate) fn fmt_append_arg<'v>(
        self,
        ptr: BcPtrAddr,
        ip: BcAddr,
        end_arg: Option<&BcInstrEndArg<'v>>,
        f: &mut dyn Write,
    ) -> fmt::Result {
        struct HandlerImpl<'b, 'g, 'v> {
            ptr: BcPtrAddr<'b>,
            ip: BcAddr,
            end_arg: Option<&'b BcInstrEndArg<'v>>,
            f: &'g mut dyn Write,
        }

        impl<'v> BcOpcodeHandler<'v, fmt::Result> for HandlerImpl<'_, '_, 'v> {
            fn handle<I: BcInstr<'v>>(self) -> fmt::Result {
                let HandlerImpl {
                    ptr,
                    ip,
                    end_arg,
                    f,
                } = self;
                let instr = ptr.get_instr::<'v, I>();
                I::Arg::fmt_append(&instr.arg, ip, end_arg, f)
            }
        }

        self.dispatch(HandlerImpl {
            ptr,
            ip,
            end_arg,
            f,
        })
    }

    pub(crate) fn visit_jump_addr(
        self,
        ptr: BcPtrAddr,
        addr: BcAddr,
        consumer: &mut dyn FnMut(BcAddr),
    ) {
        struct HandlerImpl<'b, 'c> {
            ptr: BcPtrAddr<'b>,
            addr: BcAddr,
            consumer: &'c mut dyn FnMut(BcAddr),
        }

        // Jump addresses do not depend on the brand.
        impl BcOpcodeHandler<'static, ()> for HandlerImpl<'_, '_> {
            fn handle<I: BcInstr<'static>>(self) {
                let HandlerImpl {
                    ptr,
                    addr,
                    consumer,
                } = self;
                let instr = ptr.get_instr::<'static, I>();
                I::Arg::visit_jump_addr(&instr.arg, addr, consumer);
            }
        }

        self.dispatch(HandlerImpl {
            ptr,
            addr,
            consumer,
        });
    }
}
