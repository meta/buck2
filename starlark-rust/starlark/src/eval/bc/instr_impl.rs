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

//! Instruction implementations.

use std::cmp::Ordering;
use std::marker;
use std::ptr;

use starlark_derive::StarlarkPagable;
use starlark_syntax::eval_exception::EvalException;
use starlark_syntax::internal_error;

use crate as starlark;
use crate::collections::Hashed;
use crate::collections::SmallMap;
use crate::collections::symbol::symbol::Symbol;
use crate::environment::slots::ModuleSlotId;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::eval::ParametersSpec;
use crate::eval::bc::addr::BcAddrOffset;
use crate::eval::bc::addr::BcAddrOffsetNeg;
use crate::eval::bc::addr::BcPtrAddr;
use crate::eval::bc::bytecode::Bc;
use crate::eval::bc::call::BcCallArgs;
use crate::eval::bc::call::BcCallArgsForDef;
use crate::eval::bc::call::FullArgs;
use crate::eval::bc::call::PosArgs;
use crate::eval::bc::for_loop::LoopDepth;
use crate::eval::bc::frame::BcFramePtr;
use crate::eval::bc::instr::BcInstr;
use crate::eval::bc::instr::InstrControl;
use crate::eval::bc::instr_arg::BcInstrArg;
use crate::eval::bc::native_function::BcNativeFunction;
use crate::eval::bc::slow_arg::BcInstrEndArg;
use crate::eval::bc::stack_ptr::BcSlotIn;
use crate::eval::bc::stack_ptr::BcSlotInRange;
use crate::eval::bc::stack_ptr::BcSlotInRangeFrom;
use crate::eval::bc::stack_ptr::BcSlotOut;
use crate::eval::compiler::add_span_to_expr_error;
use crate::eval::compiler::def::Def;
use crate::eval::compiler::def::DefInfoValue;
use crate::eval::compiler::def::ParameterCompiled;
use crate::eval::compiler::def::ParametersCompiled;
use crate::eval::compiler::expr::EvalError;
use crate::eval::compiler::expr::get_attr_hashed_bind;
use crate::eval::compiler::expr::get_attr_hashed_raw;
use crate::eval::compiler::expr_throw_starlark_result;
use crate::eval::compiler::stmt::AssignError;
use crate::eval::compiler::stmt::add_assign;
use crate::eval::compiler::stmt::bit_or_assign;
use crate::eval::compiler::stmt::possible_gc;
use crate::eval::runtime::arguments::ResolvedArgName;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::eval::runtime::profile::instant::ProfilerInstant;
use crate::eval::runtime::slots::LocalCapturedSlotId;
use crate::eval::runtime::slots::LocalSlotId;
use crate::values::Heap;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::any_complex::StarlarkAnyComplex;
use crate::values::dict::Dict;
use crate::values::int::pointer_i32::PointerI32;
use crate::values::layout::value_not_special::ValueNotSpecial;
use crate::values::string::dot_format::format_one;
use crate::values::string::interpolation::percent_s_one;
use crate::values::types::any_array::AnyArray;
use crate::values::types::known_methods::KnownMethod;
use crate::values::types::list::value::ListData;
use crate::values::typing::type_compiled::compiled::TypeCompiled;

/// Instructions which either fail or proceed to the following instruction,
/// and it returns error with span. See [`BcInstr`] for the brand.
pub(crate) trait InstrNoFlowImpl<'v>: 'static {
    type Arg: BcInstrArg<'v>;

    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr,
        arg: &Self::Arg,
    ) -> crate::Result<()>;
}

pub(crate) struct InstrNoFlow<I>(marker::PhantomData<I>);

impl<'v, I: InstrNoFlowImpl<'v>> BcInstr<'v> for InstrNoFlow<I> {
    type Arg = I::Arg;

    #[inline(always)]
    fn run<'b>(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        arg: &Self::Arg,
    ) -> InstrControl<'v, 'b> {
        match I::run_with_args(eval, frame, ip, arg) {
            Ok(()) => InstrControl::Next(ip.add_instr::<Self>()),
            Err(e) => InstrControl::Err(e),
        }
    }
}

pub(crate) struct InstrConstImpl;
pub(crate) type InstrConst = InstrNoFlow<InstrConstImpl>;

impl<'v> InstrNoFlowImpl<'v> for InstrConstImpl {
    type Arg = (Value<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (constant, target): &(Value<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        frame.set_bc_slot(*target, *constant);
        Ok(())
    }
}

pub(crate) struct InstrLoadLocalImpl;
pub(crate) struct InstrLoadLocalCapturedImpl;
pub(crate) struct InstrLoadModuleImpl;
pub(crate) struct InstrMovImpl;
pub(crate) struct InstrStoreLocalCapturedImpl;
pub(crate) struct InstrStoreModuleImpl;
pub(crate) struct InstrStoreModuleAndExportImpl;
pub(crate) struct InstrUnpackImpl;
pub(crate) struct InstrArrayIndexImpl;
pub(crate) struct InstrSetArrayIndexImpl;
pub(crate) struct InstrArrayIndexSetImpl;
pub(crate) struct InstrObjectFieldImpl;
pub(crate) struct InstrSetObjectFieldImpl;
pub(crate) struct InstrSliceImpl;
pub(crate) struct InstrArrayIndex2Impl;

pub(crate) type InstrLoadLocal = InstrNoFlow<InstrLoadLocalImpl>;
pub(crate) type InstrLoadLocalCaptured = InstrNoFlow<InstrLoadLocalCapturedImpl>;
pub(crate) type InstrLoadModule = InstrNoFlow<InstrLoadModuleImpl>;
pub(crate) type InstrMov = InstrNoFlow<InstrMovImpl>;
pub(crate) type InstrStoreLocalCaptured = InstrNoFlow<InstrStoreLocalCapturedImpl>;
pub(crate) type InstrStoreModule = InstrNoFlow<InstrStoreModuleImpl>;
pub(crate) type InstrStoreModuleAndExport = InstrNoFlow<InstrStoreModuleAndExportImpl>;
pub(crate) type InstrUnpack = InstrNoFlow<InstrUnpackImpl>;
pub(crate) type InstrArrayIndex = InstrNoFlow<InstrArrayIndexImpl>;
pub(crate) type InstrSetArrayIndex = InstrNoFlow<InstrSetArrayIndexImpl>;
pub(crate) type InstrArrayIndexSet = InstrNoFlow<InstrArrayIndexSetImpl>;
pub(crate) type InstrObjectField = InstrNoFlow<InstrObjectFieldImpl>;
pub(crate) type InstrSetObjectField = InstrNoFlow<InstrSetObjectFieldImpl>;
pub(crate) type InstrSlice = InstrNoFlow<InstrSliceImpl>;
pub(crate) type InstrArrayIndex2 = InstrNoFlow<InstrArrayIndex2Impl>;

impl<'v> InstrNoFlowImpl<'v> for InstrLoadLocalImpl {
    type Arg = (LocalSlotId, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, target): &(LocalSlotId, BcSlotOut),
    ) -> crate::Result<()> {
        let value = eval.get_slot_local(frame, *source)?;
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrLoadLocalCapturedImpl {
    type Arg = (LocalCapturedSlotId, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, target): &(LocalCapturedSlotId, BcSlotOut),
    ) -> crate::Result<()> {
        let value = eval.get_slot_local_captured(*source)?;
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrLoadModuleImpl {
    type Arg = (ModuleSlotId, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, target): &(ModuleSlotId, BcSlotOut),
    ) -> crate::Result<()> {
        let value = eval.get_slot_module(*source)?;
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrMovImpl {
    type Arg = (BcSlotIn, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, target): &(BcSlotIn, BcSlotOut),
    ) -> crate::Result<()> {
        let v = frame.get_bc_slot(*source);
        frame.set_bc_slot(*target, v);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrStoreLocalCapturedImpl {
    type Arg = (BcSlotIn, LocalCapturedSlotId);

    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, target): &(BcSlotIn, LocalCapturedSlotId),
    ) -> crate::Result<()> {
        let v = frame.get_bc_slot(*source);
        eval.set_slot_local_captured(*target, v);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrStoreModuleAndExportImpl {
    type Arg = (BcSlotIn, ModuleSlotId, String);

    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, slot, name): &(BcSlotIn, ModuleSlotId, String),
    ) -> crate::Result<()> {
        let v = frame.get_bc_slot(*source);
        v.export_as(name.as_str(), eval)?;
        eval.set_slot_module(*slot, v);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrStoreModuleImpl {
    type Arg = (BcSlotIn, ModuleSlotId);

    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, target): &(BcSlotIn, ModuleSlotId),
    ) -> crate::Result<()> {
        let v = frame.get_bc_slot(*source);
        eval.set_slot_module(*target, v);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrUnpackImpl {
    type Arg = (BcSlotIn, ValueTyped<'v, AnyArray<BcSlotOut>>);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, target): &(BcSlotIn, ValueTyped<'v, AnyArray<BcSlotOut>>),
    ) -> crate::Result<()> {
        let v = frame.get_bc_slot(*source);
        let nvl = v.length()?;
        if nvl != target.len() as i32 {
            return Err(crate::Error::new_other(
                AssignError::IncorrectNumberOfValueToUnpack(target.len() as i32, nvl),
            ));
        }
        let mut i = 0;
        for item in v.iterate(eval.heap())? {
            if i >= target.len() {
                return Err(internal_error!(
                    "iterate() produced more items than length() reported (expected {}, got at least {})",
                    target.len(),
                    i + 1
                ));
            }
            frame.set_bc_slot(target[i], item);
            i += 1;
        }
        if i != target.len() {
            return Err(internal_error!(
                "iterate() produced fewer items than length() reported (expected {}, got {})",
                target.len(),
                i
            ));
        }
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrArrayIndexImpl {
    type Arg = (BcSlotIn, BcSlotIn, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (array, index, target): &(BcSlotIn, BcSlotIn, BcSlotOut),
    ) -> crate::Result<()> {
        let array = frame.get_bc_slot(*array);
        let index = frame.get_bc_slot(*index);
        let value = array.at(index, eval.heap())?;
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrSetArrayIndexImpl {
    type Arg = (BcSlotIn, BcSlotIn, BcSlotIn);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, array, index): &(BcSlotIn, BcSlotIn, BcSlotIn),
    ) -> crate::Result<()> {
        let value = frame.get_bc_slot(*source);
        let array = frame.get_bc_slot(*array);
        let index = frame.get_bc_slot(*index);
        array.set_at(index, value)
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrArrayIndexSetImpl {
    type Arg = (BcSlotIn, BcSlotIn, BcSlotIn);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (array, index, source): &(BcSlotIn, BcSlotIn, BcSlotIn),
    ) -> crate::Result<()> {
        let value = frame.get_bc_slot(*source);
        let array = frame.get_bc_slot(*array);
        let index = frame.get_bc_slot(*index);
        array.set_at(index, value)
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrObjectFieldImpl {
    type Arg = (BcSlotIn, Symbol, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (object, field, target): &(BcSlotIn, Symbol, BcSlotOut),
    ) -> crate::Result<()> {
        let object = frame.get_bc_slot(*object);
        let value = get_attr_hashed_bind(object, field, eval.heap())?;
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrSetObjectFieldImpl {
    type Arg = (BcSlotIn, BcSlotIn, Symbol);

    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, object, field): &(BcSlotIn, BcSlotIn, Symbol),
    ) -> crate::Result<()> {
        let v = frame.get_bc_slot(*source);
        let object = frame.get_bc_slot(*object);
        object.set_attr(field.as_str(), v)
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrSliceImpl {
    type Arg = (
        BcSlotIn,
        Option<BcSlotIn>,
        Option<BcSlotIn>,
        Option<BcSlotIn>,
        BcSlotOut,
    );

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (list, start, stop, step, target): &(
            BcSlotIn,
            Option<BcSlotIn>,
            Option<BcSlotIn>,
            Option<BcSlotIn>,
            BcSlotOut,
        ),
    ) -> crate::Result<()> {
        let list = frame.get_bc_slot(*list);
        let start = start.map(|s| frame.get_bc_slot(s));
        let stop = stop.map(|s| frame.get_bc_slot(s));
        let step = step.map(|s| frame.get_bc_slot(s));
        let value = list.slice(start, stop, step, eval.heap())?;
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrArrayIndex2Impl {
    type Arg = (BcSlotIn, BcSlotIn, BcSlotIn, BcSlotOut);

    #[cold]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (array, index0, index1, target): &(BcSlotIn, BcSlotIn, BcSlotIn, BcSlotOut),
    ) -> crate::Result<()> {
        let array = frame.get_bc_slot(*array);
        let index0 = frame.get_bc_slot(*index0);
        let index1 = frame.get_bc_slot(*index1);
        let value = array.get_ref().at2(index0, index1, eval.heap())?;
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

pub(crate) struct InstrEqImpl;
pub(crate) struct InstrEqConstImpl;
pub(crate) struct InstrEqPtrImpl;
pub(crate) struct InstrEqStrImpl;
pub(crate) struct InstrEqIntImpl;

pub(crate) type InstrEq = InstrBinOp<InstrEqImpl>;
pub(crate) type InstrEqConst = InstrNoFlow<InstrEqConstImpl>;
pub(crate) type InstrEqPtr = InstrNoFlow<InstrEqPtrImpl>;
pub(crate) type InstrEqStr = InstrNoFlow<InstrEqStrImpl>;
pub(crate) type InstrEqInt = InstrNoFlow<InstrEqIntImpl>;

impl InstrBinOpImpl for InstrEqImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.equals(v1).map(Value::new_bool)
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrEqConstImpl {
    type Arg = (BcSlotIn, ValueNotSpecial<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (a, b, target): &(BcSlotIn, ValueNotSpecial<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        let a = frame.get_bc_slot(*a);
        let r = b.equals(a)?;
        frame.set_bc_slot(*target, Value::new_bool(r));
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrEqPtrImpl {
    type Arg = (BcSlotIn, Value<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (a, b, target): &(BcSlotIn, Value<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        let a = frame.get_bc_slot(*a);
        let r = a.ptr_eq(*b);
        frame.set_bc_slot(*target, Value::new_bool(r));
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrEqIntImpl {
    type Arg = (BcSlotIn, ValueTyped<'v, PointerI32>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (a, b, target): &(BcSlotIn, ValueTyped<'v, PointerI32>, BcSlotOut),
    ) -> crate::Result<()> {
        let a = frame.get_bc_slot(*a);
        let r = if let Some(a) = a.unpack_int_value() {
            a.as_ref() == b.as_ref()
        } else {
            b.to_value().equals(a)?
        };
        frame.set_bc_slot(*target, Value::new_bool(r));
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrEqStrImpl {
    type Arg = (BcSlotIn, StringValue<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (a, b, target): &(BcSlotIn, StringValue<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        let a = frame.get_bc_slot(*a);
        let r = if let Some(a) = StringValue::new(a) {
            a == *b
        } else {
            false
        };
        frame.set_bc_slot(*target, Value::new_bool(r));
        Ok(())
    }
}

pub(crate) struct InstrNotImpl;
pub(crate) struct InstrMinusImpl;
pub(crate) struct InstrPlusImpl;
pub(crate) struct InstrBitNotImpl;

pub(crate) type InstrNot = InstrUnOp<InstrNotImpl>;
pub(crate) type InstrMinus = InstrUnOp<InstrMinusImpl>;
pub(crate) type InstrPlus = InstrUnOp<InstrPlusImpl>;
pub(crate) type InstrBitNot = InstrUnOp<InstrBitNotImpl>;

impl InstrUnOpImpl for InstrNotImpl {
    #[inline(always)]
    fn eval<'v>(v: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(Value::new_bool(!v.to_bool()))
    }
}

impl InstrUnOpImpl for InstrPlusImpl {
    #[inline(always)]
    fn eval<'v>(v: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v.plus(heap)
    }
}

impl InstrUnOpImpl for InstrMinusImpl {
    #[inline(always)]
    fn eval<'v>(v: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v.minus(heap)
    }
}

impl InstrUnOpImpl for InstrBitNotImpl {
    #[inline(always)]
    fn eval<'v>(v: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v.bit_not(heap)
    }
}

pub(crate) trait InstrBinOpImpl: 'static {
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>>;
}

pub(crate) trait InstrUnOpImpl: 'static {
    fn eval<'v>(v: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>>;
}

pub(crate) struct InstrBinOpWrapper<I: InstrBinOpImpl>(marker::PhantomData<I>);
pub(crate) struct InstrUnOpWrapper<I: InstrUnOpImpl>(marker::PhantomData<I>);
pub(crate) type InstrBinOp<I> = InstrNoFlow<InstrBinOpWrapper<I>>;
pub(crate) type InstrUnOp<I> = InstrNoFlow<InstrUnOpWrapper<I>>;

impl<'v, I: InstrBinOpImpl> InstrNoFlowImpl<'v> for InstrBinOpWrapper<I> {
    type Arg = (BcSlotIn, BcSlotIn, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (v0, v1, target): &(BcSlotIn, BcSlotIn, BcSlotOut),
    ) -> crate::Result<()> {
        let v0 = frame.get_bc_slot(*v0);
        let v1 = frame.get_bc_slot(*v1);
        let v = I::eval(v0, v1, eval.heap())?;
        frame.set_bc_slot(*target, v);
        Ok(())
    }
}

impl<'v, I: InstrUnOpImpl> InstrNoFlowImpl<'v> for InstrUnOpWrapper<I> {
    type Arg = (BcSlotIn, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (source, target): &(BcSlotIn, BcSlotOut),
    ) -> crate::Result<()> {
        let source = frame.get_bc_slot(*source);
        let value = I::eval(source, eval.heap())?;
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

pub(crate) struct InstrAddImpl;
pub(crate) struct InstrAddAssignImpl;
pub(crate) struct InstrSubImpl;
pub(crate) struct InstrMultiplyImpl;
pub(crate) struct InstrPercentImpl;
pub(crate) struct InstrDivideImpl;
pub(crate) struct InstrFloorDivideImpl;
pub(crate) struct InstrBitAndImpl;
pub(crate) struct InstrBitOrImpl;
pub(crate) struct InstrBitOrAssignImpl;
pub(crate) struct InstrBitXorImpl;
pub(crate) struct InstrLeftShiftImpl;
pub(crate) struct InstrRightShiftImpl;
pub(crate) struct InstrInImpl;

pub(crate) type InstrAdd = InstrBinOp<InstrAddImpl>;
pub(crate) type InstrAddAssign = InstrBinOp<InstrAddAssignImpl>;
pub(crate) type InstrSub = InstrBinOp<InstrSubImpl>;
pub(crate) type InstrMultiply = InstrBinOp<InstrMultiplyImpl>;
pub(crate) type InstrPercent = InstrBinOp<InstrPercentImpl>;
pub(crate) type InstrDivide = InstrBinOp<InstrDivideImpl>;
pub(crate) type InstrFloorDivide = InstrBinOp<InstrFloorDivideImpl>;
pub(crate) type InstrBitAnd = InstrBinOp<InstrBitAndImpl>;
pub(crate) type InstrBitOr = InstrBinOp<InstrBitOrImpl>;
pub(crate) type InstrBitOrAssign = InstrBinOp<InstrBitOrAssignImpl>;
pub(crate) type InstrBitXor = InstrBinOp<InstrBitXorImpl>;
pub(crate) type InstrLeftShift = InstrBinOp<InstrLeftShiftImpl>;
pub(crate) type InstrRightShift = InstrBinOp<InstrRightShiftImpl>;
pub(crate) type InstrIn = InstrBinOp<InstrInImpl>;

impl InstrBinOpImpl for InstrAddImpl {
    #[inline(always)]
    fn eval<'v>(l: Value<'v>, r: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        l.add(r, heap)
    }
}

impl InstrBinOpImpl for InstrAddAssignImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        add_assign(v0, v1, heap)
    }
}

impl InstrBinOpImpl for InstrSubImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.sub(v1, heap)
    }
}

impl InstrBinOpImpl for InstrMultiplyImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.mul(v1, heap)
    }
}

impl InstrBinOpImpl for InstrPercentImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.percent(v1, heap)
    }
}

impl InstrBinOpImpl for InstrFloorDivideImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.floor_div(v1, heap)
    }
}

impl InstrBinOpImpl for InstrDivideImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.div(v1, heap)
    }
}

impl InstrBinOpImpl for InstrBitAndImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.bit_and(v1, heap)
    }
}

impl InstrBinOpImpl for InstrBitOrImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.bit_or(v1, heap)
    }
}

impl InstrBinOpImpl for InstrBitOrAssignImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        bit_or_assign(v0, v1, heap)
    }
}

impl InstrBinOpImpl for InstrBitXorImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.bit_xor(v1, heap)
    }
}

impl InstrBinOpImpl for InstrLeftShiftImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.left_shift(v1, heap)
    }
}

impl InstrBinOpImpl for InstrRightShiftImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        v0.right_shift(v1, heap)
    }
}

impl InstrBinOpImpl for InstrInImpl {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(Value::new_bool(v1.is_in(v0)?))
    }
}

pub(crate) struct InstrPercentSOneImpl;
pub(crate) type InstrPercentSOne = InstrNoFlow<InstrPercentSOneImpl>;
pub(crate) struct InstrFormatOneImpl;
pub(crate) type InstrFormatOne = InstrNoFlow<InstrFormatOneImpl>;

impl<'v> InstrNoFlowImpl<'v> for InstrPercentSOneImpl {
    type Arg = (StringValue<'v>, BcSlotIn, StringValue<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (before, arg, after, target): &(StringValue<'v>, BcSlotIn, StringValue<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        let arg = frame.get_bc_slot(*arg);
        let r = percent_s_one(before.as_str(), arg, after.as_str(), eval.heap())?;
        frame.set_bc_slot(*target, r.to_value());
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrFormatOneImpl {
    type Arg = (StringValue<'v>, BcSlotIn, StringValue<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (before, arg, after, target): &(StringValue<'v>, BcSlotIn, StringValue<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        let arg = frame.get_bc_slot(*arg);
        let r = format_one(before.as_str(), arg, after.as_str(), eval.heap());
        frame.set_bc_slot(*target, r.to_value());
        Ok(())
    }
}

pub(crate) trait InstrCompareImpl: 'static {
    fn eval_compare(ordering: Ordering) -> bool;
}

pub(crate) struct InstrCompare<I: InstrCompareImpl>(marker::PhantomData<I>);

impl<I: InstrCompareImpl> InstrBinOpImpl for InstrCompare<I> {
    #[inline(always)]
    fn eval<'v>(v0: Value<'v>, v1: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(Value::new_bool(I::eval_compare(v0.compare(v1)?)))
    }
}

pub(crate) struct InstrLessImpl;
pub(crate) struct InstrGreaterImpl;
pub(crate) struct InstrLessOrEqualImpl;
pub(crate) struct InstrGreaterOrEqualImpl;

pub(crate) type InstrLess = InstrBinOp<InstrCompare<InstrLessImpl>>;
pub(crate) type InstrGreater = InstrBinOp<InstrCompare<InstrGreaterImpl>>;
pub(crate) type InstrLessOrEqual = InstrBinOp<InstrCompare<InstrLessOrEqualImpl>>;
pub(crate) type InstrGreaterOrEqual = InstrBinOp<InstrCompare<InstrGreaterOrEqualImpl>>;

impl InstrCompareImpl for InstrLessImpl {
    #[inline(always)]
    fn eval_compare(ordering: Ordering) -> bool {
        ordering == Ordering::Less
    }
}

impl InstrCompareImpl for InstrGreaterImpl {
    #[inline(always)]
    fn eval_compare(ordering: Ordering) -> bool {
        ordering == Ordering::Greater
    }
}

impl InstrCompareImpl for InstrLessOrEqualImpl {
    #[inline(always)]
    fn eval_compare(ordering: Ordering) -> bool {
        ordering != Ordering::Greater
    }
}

impl InstrCompareImpl for InstrGreaterOrEqualImpl {
    #[inline(always)]
    fn eval_compare(ordering: Ordering) -> bool {
        ordering != Ordering::Less
    }
}

pub(crate) struct InstrTypeImpl;
pub(crate) type InstrType = InstrUnOp<InstrTypeImpl>;

impl InstrUnOpImpl for InstrTypeImpl {
    #[inline(always)]
    fn eval<'v>(v: Value<'v>, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(v.get_type_value().at().to_value())
    }
}

pub(crate) struct InstrTypeIsImpl;
pub(crate) type InstrTypeIs = InstrNoFlow<InstrTypeIsImpl>;

impl<'v> InstrNoFlowImpl<'v> for InstrTypeIsImpl {
    type Arg = (BcSlotIn, StringValue<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        (arg, t, target): &(BcSlotIn, StringValue<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        let arg = frame.get_bc_slot(*arg);
        let r = arg.get_type_value().at() == *t;
        frame.set_bc_slot(*target, Value::new_bool(r));
        Ok(())
    }
}

pub(crate) struct InstrIsInstanceImpl;
pub(crate) type InstrIsInstance = InstrNoFlow<InstrIsInstanceImpl>;

impl<'v> InstrNoFlowImpl<'v> for InstrIsInstanceImpl {
    type Arg = (BcSlotIn, TypeCompiled<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        (arg, t, target): &(BcSlotIn, TypeCompiled<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        let arg = frame.get_bc_slot(*arg);
        let r = t.matches(arg);
        frame.set_bc_slot(*target, Value::new_bool(r));
        Ok(())
    }
}

pub(crate) struct InstrLenImpl;
pub(crate) type InstrLen = InstrUnOp<InstrLenImpl>;

impl InstrUnOpImpl for InstrLenImpl {
    #[inline(always)]
    fn eval<'v>(v: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(heap.alloc(v.length()?).to_value())
    }
}

pub(crate) struct InstrTupleNPopImpl;
pub(crate) struct InstrListNPopImpl;
pub(crate) struct InstrListOfConstsImpl;
pub(crate) struct InstrDictOfConstsImpl;
pub(crate) struct InstrDictConstKeysImpl;
pub(crate) struct InstrDictNPopImpl;
pub(crate) struct InstrListNewImpl;
pub(crate) struct InstrDictNewImpl;

pub(crate) type InstrTupleNPop = InstrNoFlow<InstrTupleNPopImpl>;
pub(crate) type InstrListNew = InstrNoFlow<InstrListNewImpl>;
pub(crate) type InstrListNPop = InstrNoFlow<InstrListNPopImpl>;
pub(crate) type InstrListOfConsts = InstrNoFlow<InstrListOfConstsImpl>;
pub(crate) type InstrDictNew = InstrNoFlow<InstrDictNewImpl>;
pub(crate) type InstrDictOfConsts = InstrNoFlow<InstrDictOfConstsImpl>;
pub(crate) type InstrDictConstKeys = InstrNoFlow<InstrDictConstKeysImpl>;
pub(crate) type InstrDictNPop = InstrNoFlow<InstrDictNPopImpl>;

impl<'v> InstrNoFlowImpl<'v> for InstrTupleNPopImpl {
    type Arg = (BcSlotInRange, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        (values, target): &(BcSlotInRange, BcSlotOut),
    ) -> crate::Result<()> {
        let items = frame.get_bc_slot_range(*values);
        let value = eval.heap().alloc_tuple(items);
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrListNPopImpl {
    type Arg = (BcSlotInRange, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        (values, target): &(BcSlotInRange, BcSlotOut),
    ) -> crate::Result<()> {
        let items = frame.get_bc_slot_range(*values);
        let value = eval.heap().alloc_list(items);
        frame.set_bc_slot(*target, value);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrListOfConstsImpl {
    type Arg = (Box<[Value<'v>]>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        (values, target): &(Box<[Value<'v>]>, BcSlotOut),
    ) -> crate::Result<()> {
        let list = eval.heap().alloc_list(values);
        frame.set_bc_slot(*target, list);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrDictOfConstsImpl {
    type Arg = (SmallMap<Value<'v>, Value<'v>>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        (values, target): &(SmallMap<Value<'v>, Value<'v>>, BcSlotOut),
    ) -> crate::Result<()> {
        let dict = eval.heap().alloc(Dict::new(values.clone())).to_value();
        frame.set_bc_slot(*target, dict);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrDictNPopImpl {
    type Arg = (BcSlotInRange, BcSlotOut, u32);

    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr,
        (npops, target, spans_offset): &(BcSlotInRange, BcSlotOut, u32),
    ) -> crate::Result<()> {
        let items = frame.get_bc_slot_range(*npops);
        debug_assert!(items.len().is_multiple_of(2));
        let mut dict = SmallMap::with_capacity(items.len() / 2);
        for i in 0..items.len() / 2 {
            let k = items[i * 2];
            let v = items[i * 2 + 1];
            let k = match k.get_hashed() {
                Ok(k) => k,
                Err(e) => {
                    let (_, end_arg) = Bc::end_arg_at_ptr(ip);
                    let span = end_arg.dictnpop_spans[*spans_offset as usize + i];
                    return Err(add_span_to_expr_error(e, span, eval).into_error());
                }
            };
            let prev = dict.insert_hashed(k, v);
            if prev.is_some() {
                let e =
                    crate::Error::new_other(EvalError::DuplicateDictionaryKey(k.key().to_string()));
                let (_, end_arg) = Bc::end_arg_at_ptr(ip);
                let span = end_arg.dictnpop_spans[*spans_offset as usize + i];
                return Err(add_span_to_expr_error(e, span, eval).into_error());
            }
        }
        let dict = eval.heap().alloc(Dict::new(dict)).to_value();
        frame.set_bc_slot(*target, dict);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrDictConstKeysImpl {
    type Arg = (Box<[Hashed<Value<'v>>]>, BcSlotInRangeFrom, BcSlotOut);

    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        (keys, values, target): &(Box<[Hashed<Value<'v>>]>, BcSlotInRangeFrom, BcSlotOut),
    ) -> crate::Result<()> {
        let values = frame.get_bc_slot_range(values.to_range(keys.len() as u32));
        let mut dict = SmallMap::with_capacity(keys.len());
        for (k, v) in keys.iter().zip(values) {
            let prev = dict.insert_hashed(*k, *v);
            debug_assert!(prev.is_none());
        }
        let dict = eval.heap().alloc(Dict::new(dict)).to_value();
        frame.set_bc_slot(*target, dict);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrListNewImpl {
    type Arg = BcSlotOut;

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        target: &BcSlotOut,
    ) -> crate::Result<()> {
        let list = eval.heap().alloc_list(&[]);
        frame.set_bc_slot(*target, list);
        Ok(())
    }
}

impl<'v> InstrNoFlowImpl<'v> for InstrDictNewImpl {
    type Arg = BcSlotOut;

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _: BcPtrAddr,
        target: &BcSlotOut,
    ) -> crate::Result<()> {
        let dict = eval.heap().alloc(Dict::default());
        frame.set_bc_slot(*target, dict.to_value());
        Ok(())
    }
}

pub(crate) struct InstrComprListAppend;
pub(crate) struct InstrComprDictInsert;

impl<'v> BcInstr<'v> for InstrComprListAppend {
    type Arg = (BcSlotIn, BcSlotIn);

    #[inline(always)]
    fn run<'b>(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        (list, item): &(BcSlotIn, BcSlotIn),
    ) -> InstrControl<'v, 'b> {
        let list = frame.get_bc_slot(*list);
        let item = frame.get_bc_slot(*item);
        // SAFETY: in generated bytecode this slot can be only occupied by a mutable list.
        let list = unsafe { ListData::from_value_unchecked_mut(list) };
        list.push(item, eval.heap());
        // TODO(nga): call continue routine here.
        InstrControl::Next(ip.add_instr::<Self>())
    }
}

impl<'v> BcInstr<'v> for InstrComprDictInsert {
    type Arg = (BcSlotIn, BcSlotIn, BcSlotIn);

    #[inline(always)]
    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr<'b>,
        (dict, key, value): &(BcSlotIn, BcSlotIn, BcSlotIn),
    ) -> InstrControl<'v, 'b> {
        let dict = frame.get_bc_slot(*dict);
        let key = frame.get_bc_slot(*key);
        let value = frame.get_bc_slot(*value);
        let key = match key.get_hashed() {
            Ok(key) => key,
            Err(e) => return InstrControl::Err(e),
        };
        // SAFETY: in generated bytecode this slot can be only occupied by a mutable dict.
        let mut dict = unsafe { Dict::from_value_unchecked_mut(dict) };
        dict.insert_hashed(key, value);
        // TODO(nga): call continue routine here.
        InstrControl::Next(_ip.add_instr::<Self>())
    }
}

pub(crate) struct InstrCheckTypeImpl;
pub(crate) type InstrCheckType = InstrNoFlow<InstrCheckTypeImpl>;

impl<'v> InstrNoFlowImpl<'v> for InstrCheckTypeImpl {
    type Arg = (BcSlotIn, TypeCompiled<'v>);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (expr, ty): &(BcSlotIn, TypeCompiled<'v>),
    ) -> crate::Result<()> {
        let expr = frame.get_bc_slot(*expr);
        let start = if eval.typecheck_profile.enabled {
            Some(ProfilerInstant::now())
        } else {
            None
        };
        let res = ty.check_type(expr, None);
        if let Some(start) = start {
            eval.typecheck_profile.add("assignment", start.elapsed());
        }
        res
    }
}

pub(crate) struct InstrBr;
pub(crate) struct InstrIfBr;
pub(crate) struct InstrIfNotBr;

impl<'v> BcInstr<'v> for InstrBr {
    type Arg = BcAddrOffset;

    #[inline(always)]
    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        _frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        target: &BcAddrOffset,
    ) -> InstrControl<'v, 'b> {
        InstrControl::Next(ip.add_rel(*target))
    }
}

impl<'v> BcInstr<'v> for InstrIfBr {
    type Arg = (BcSlotIn, BcAddrOffset);

    #[inline(always)]
    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        (cond, target): &(BcSlotIn, BcAddrOffset),
    ) -> InstrControl<'v, 'b> {
        let cond = frame.get_bc_slot(*cond);
        if cond.to_bool() {
            InstrControl::Next(ip.add_rel(*target))
        } else {
            InstrControl::Next(ip.add_instr::<Self>())
        }
    }
}

impl<'v> BcInstr<'v> for InstrIfNotBr {
    type Arg = (BcSlotIn, BcAddrOffset);

    #[inline(always)]
    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        (cond, target): &(BcSlotIn, BcAddrOffset),
    ) -> InstrControl<'v, 'b> {
        let cond = frame.get_bc_slot(*cond);
        if !cond.to_bool() {
            InstrControl::Next(ip.add_rel(*target))
        } else {
            InstrControl::Next(ip.add_instr::<Self>())
        }
    }
}

/// Setup `for` loop.
pub(crate) struct InstrIter;
/// `continue` statement.
pub(crate) struct InstrContinue;
/// `break` statement.
pub(crate) struct InstrBreak;
/// Stop all the iterations to release mutation locks before `return`.
pub(crate) struct InstrIterStop;

impl<'v> BcInstr<'v> for InstrIter {
    type Arg = (BcSlotIn, LoopDepth, BcSlotOut, BcSlotOut, BcAddrOffset);

    #[inline(always)]
    fn run<'b>(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        (over, loop_depth, iter_slot, var, end): &(
            BcSlotIn,
            LoopDepth,
            BcSlotOut,
            BcSlotOut,
            BcAddrOffset,
        ),
    ) -> InstrControl<'v, 'b> {
        let over = frame.get_bc_slot(*over);
        let iter = match over.get_ref().iterate(over, eval.heap()) {
            Ok(iter) => iter,
            Err(e) => return InstrControl::Err(e),
        };
        match iter.get_ref().iter_next(0, eval.heap()) {
            Some(next) => {
                frame.set_bc_slot(*iter_slot, iter);
                frame.set_bc_slot(*var, next);
                frame.set_iter_index(*loop_depth, 1);
                InstrControl::Next(ip.add_instr::<Self>())
            }
            None => {
                iter.get_ref().iter_stop();
                InstrControl::Next(ip.add_rel(*end))
            }
        }
    }
}

impl<'v> BcInstr<'v> for InstrContinue {
    type Arg = (
        BcSlotIn,
        LoopDepth,
        BcSlotOut,
        BcAddrOffsetNeg,
        BcAddrOffset,
    );

    #[inline(always)]
    fn run<'b>(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        (iter, loop_depth, var, begin, end): &(
            BcSlotIn,
            LoopDepth,
            BcSlotOut,
            BcAddrOffsetNeg,
            BcAddrOffset,
        ),
    ) -> InstrControl<'v, 'b> {
        if let Err(e) = eval.report_forward_progress() {
            return InstrControl::Err(e);
        }
        let iter = frame.get_bc_slot(*iter);
        let loop_depth = *loop_depth;
        let i = frame.get_iter_index(loop_depth);
        match iter.get_ref().iter_next(i, eval.heap()) {
            Some(next) => {
                frame.set_iter_index(loop_depth, i + 1);
                frame.set_bc_slot(*var, next);
                InstrControl::Next(ip.add_rel_neg(*begin))
            }
            None => {
                iter.get_ref().iter_stop();
                InstrControl::Next(ip.add_rel(*end))
            }
        }
    }
}

impl<'v> BcInstr<'v> for InstrBreak {
    type Arg = (BcSlotIn, BcAddrOffset);

    #[inline(always)]
    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        (iter, end): &(BcSlotIn, BcAddrOffset),
    ) -> InstrControl<'v, 'b> {
        let iter = frame.get_bc_slot(*iter);
        iter.get_ref().iter_stop();
        InstrControl::Next(ip.add_rel(*end))
    }
}

impl<'v> BcInstr<'v> for InstrIterStop {
    type Arg = BcSlotIn;

    #[inline(always)]
    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        iter: &BcSlotIn,
    ) -> InstrControl<'v, 'b> {
        let iter = frame.get_bc_slot(*iter);
        iter.get_ref().iter_stop();
        InstrControl::Next(ip.add_instr::<Self>())
    }
}

pub(crate) struct InstrReturnConst;
pub(crate) struct InstrReturn;
pub(crate) struct InstrReturnCheckType;

impl<'v> BcInstr<'v> for InstrReturnConst {
    type Arg = Value<'v>;

    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        _frame: BcFramePtr<'v>,
        _ip: BcPtrAddr<'b>,
        value: &Value<'v>,
    ) -> InstrControl<'v, 'b> {
        InstrControl::Return(*value)
    }
}

impl<'v> BcInstr<'v> for InstrReturn {
    type Arg = BcSlotIn;

    #[inline(always)]
    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr<'b>,
        &slot: &BcSlotIn,
    ) -> InstrControl<'v, 'b> {
        let v = frame.get_bc_slot(slot);
        InstrControl::Return(v)
    }
}

impl<'v> BcInstr<'v> for InstrReturnCheckType {
    type Arg = BcSlotIn;

    #[inline(always)]
    fn run<'b>(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr<'b>,
        &slot: &BcSlotIn,
    ) -> InstrControl<'v, 'b> {
        let v = frame.get_bc_slot(slot);
        if let Err(e) = eval.check_return_type(v) {
            return InstrControl::Err(e);
        }
        InstrControl::Return(v)
    }
}

pub(crate) struct InstrDefImpl;
pub(crate) type InstrDef = InstrNoFlow<InstrDefImpl>;

#[derive(Debug, StarlarkPagable)]
pub(crate) struct InstrDefData<'v> {
    pub(crate) params: ParametersCompiled<'v, u32>,
    pub(crate) return_type: Option<TypeCompiled<'v>>,
    pub(crate) info: DefInfoValue<'v>,
}

impl<'v> InstrNoFlowImpl<'v> for InstrDefImpl {
    type Arg = (BcSlotInRange, InstrDefData<'v>, BcSlotOut);

    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (pops, def_data, target): &(BcSlotInRange, InstrDefData<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        let pop = frame.get_bc_slot_range(*pops);

        let mut defaults = Vec::with_capacity(pop.len());

        let mut pop_index = 0;

        for x in def_data.params.params.iter() {
            if let ParameterCompiled::Normal(n, ty, Some(v)) = &x.node {
                assert!(*v == pop_index);
                let value = pop[pop_index as usize];
                pop_index += 1;

                if let Some(ty_compiled) = ty {
                    // Check the type of the default
                    expr_throw_starlark_result(
                        ty_compiled.check_type(value, Some(&n.name)),
                        x.span,
                        eval,
                    )
                    .map_err(EvalException::into_error)?;
                }
                defaults.push(value);
            }
        }
        let return_type = def_data.return_type;
        assert!(pop_index as usize == pop.len());
        let def = eval.heap().alloc(Def::new(
            ParametersSpec::from_prototype(def_data.params.param_spec_prototype(), defaults),
            return_type,
            def_data.info,
            eval,
        )?);
        frame.set_bc_slot(*target, def.to_value());
        Ok(())
    }
}

/// A constant callable argument of a call instruction, see [`BcCallArgs`] for why a marker
/// implements this rather than the operand type.
pub(crate) trait BcFrozenCallable: 'static {
    /// The operand.
    type Callable<'v>: BcInstrArg<'v> + Copy;

    fn bc_invoke<'v>(
        callable: Self::Callable<'v>,
        location: &'v FrameSpan<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>>;
}

/// Any constant callable.
pub(crate) struct AnyCallable;

/// A constant native function.
pub(crate) struct NativeCallable;

impl BcFrozenCallable for AnyCallable {
    type Callable<'v> = Value<'v>;

    #[inline(always)]
    fn bc_invoke<'v>(
        callable: Value<'v>,
        location: &'v FrameSpan<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        callable.invoke_with_loc(Some(location), args, eval)
    }
}

impl BcFrozenCallable for NativeCallable {
    type Callable<'v> = BcNativeFunction<'v>;

    #[inline(always)]
    fn bc_invoke<'v>(
        callable: BcNativeFunction<'v>,
        location: &'v FrameSpan<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        eval.with_call_stack(callable.to_value(), Some(location), |eval| {
            callable.invoke(args, eval)
        })
    }
}

/// The span operand of a call instruction, see `BcWriter::alloc_file_span`.
type CallSpan<'v> = ValueTyped<'v, StarlarkAnyComplex<FrameSpan<'v>>>;

pub(crate) struct InstrCallImpl<A: BcCallArgs<Symbol>>(marker::PhantomData<fn(A)>);
pub(crate) struct InstrCallFrozenGenericImpl<F: BcFrozenCallable, A: BcCallArgs<Symbol>>(
    marker::PhantomData<(F, A)>,
);
pub(crate) struct InstrCallFrozenDefImpl<A: BcCallArgsForDef>(marker::PhantomData<A>);
pub(crate) struct InstrCallMethodImpl<A: BcCallArgs<Symbol>>(marker::PhantomData<A>);
pub(crate) struct InstrCallMaybeKnownMethodImpl<A: BcCallArgs<Symbol>>(marker::PhantomData<A>);

pub(crate) type InstrCall = InstrNoFlow<InstrCallImpl<FullArgs<Symbol>>>;
pub(crate) type InstrCallPos = InstrNoFlow<InstrCallImpl<PosArgs>>;
pub(crate) type InstrCallFrozenDef = InstrNoFlow<InstrCallFrozenDefImpl<FullArgs<ResolvedArgName>>>;
pub(crate) type InstrCallFrozenDefPos = InstrNoFlow<InstrCallFrozenDefImpl<PosArgs>>;
pub(crate) type InstrCallFrozenNative =
    InstrNoFlow<InstrCallFrozenGenericImpl<NativeCallable, FullArgs<Symbol>>>;
pub(crate) type InstrCallFrozenNativePos =
    InstrNoFlow<InstrCallFrozenGenericImpl<NativeCallable, PosArgs>>;
pub(crate) type InstrCallFrozen =
    InstrNoFlow<InstrCallFrozenGenericImpl<AnyCallable, FullArgs<Symbol>>>;
pub(crate) type InstrCallFrozenPos = InstrNoFlow<InstrCallFrozenGenericImpl<AnyCallable, PosArgs>>;
pub(crate) type InstrCallMethod = InstrNoFlow<InstrCallMethodImpl<FullArgs<Symbol>>>;
pub(crate) type InstrCallMethodPos = InstrNoFlow<InstrCallMethodImpl<PosArgs>>;
pub(crate) type InstrCallMaybeKnownMethod =
    InstrNoFlow<InstrCallMaybeKnownMethodImpl<FullArgs<Symbol>>>;
pub(crate) type InstrCallMaybeKnownMethodPos = InstrNoFlow<InstrCallMaybeKnownMethodImpl<PosArgs>>;

impl<'v, A: BcCallArgs<Symbol>> InstrNoFlowImpl<'v> for InstrCallImpl<A> {
    type Arg = (BcSlotIn, A::Arg<'v>, CallSpan<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (this, args, span, target): &(BcSlotIn, A::Arg<'v>, CallSpan<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        eval.report_forward_progress()?;
        let f = frame.get_bc_slot(*this);
        let arguments = Arguments(A::pop_from_stack(args, frame));
        let r = f.invoke_with_loc(Some(&span.as_ref().value), &arguments, eval)?;
        frame.set_bc_slot(*target, r);
        Ok(())
    }
}

impl<'v, F: BcFrozenCallable, A: BcCallArgs<Symbol>> InstrNoFlowImpl<'v>
    for InstrCallFrozenGenericImpl<F, A>
{
    type Arg = (F::Callable<'v>, A::Arg<'v>, CallSpan<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (fun, args, span, target): &(F::Callable<'v>, A::Arg<'v>, CallSpan<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        eval.report_forward_progress()?;
        let arguments = Arguments(A::pop_from_stack(args, frame));
        let r = F::bc_invoke(*fun, &span.as_ref().value, &arguments, eval)?;
        frame.set_bc_slot(*target, r);
        Ok(())
    }
}

impl<'v, A: BcCallArgsForDef> InstrNoFlowImpl<'v> for InstrCallFrozenDefImpl<A> {
    type Arg = (ValueTyped<'v, Def<'v>>, A::Arg<'v>, CallSpan<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (fun, args, span, target): &(ValueTyped<'v, Def<'v>>, A::Arg<'v>, CallSpan<'v>, BcSlotOut),
    ) -> crate::Result<()> {
        eval.report_forward_progress()?;
        let arguments = A::pop_from_stack(args, frame);
        let r = eval.with_call_stack(fun.to_value(), Some(&span.as_ref().value), |eval| {
            fun.as_ref()
                .invoke_with_args(fun.to_value(), &arguments, eval)
        })?;
        frame.set_bc_slot(*target, r);
        Ok(())
    }
}

/// Common of method invocation instructions.
#[inline(always)]
fn call_method_common<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    frame: BcFramePtr<'v>,
    this: Value<'v>,
    symbol: &Symbol,
    arguments: &Arguments<'v, '_>,
    span: &'v FrameSpan<'v>,
    target: BcSlotOut,
) -> crate::Result<()> {
    eval.report_forward_progress()?;
    // TODO: wrong span: should be span of `object.method`, not of the whole expression
    let method = get_attr_hashed_raw(this, symbol, eval.heap())?;
    let r = method.invoke(this, span, arguments, eval)?;
    frame.set_bc_slot(target, r);
    Ok(())
}

/// Common of method invocation instructions where a method is likely stdlib method.
#[inline(always)]
fn call_maybe_known_method_common<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    frame: BcFramePtr<'v>,
    this: Value<'v>,
    symbol: &Symbol,
    known_method: &KnownMethod,
    arguments: &Arguments<'v, '_>,
    span: &'v FrameSpan<'v>,
    target: BcSlotOut,
) -> crate::Result<()> {
    if let Some(methods) = this.vtable().methods() {
        // Instead of method lookup by name, we compare `Methods` pointers.
        // If pointers are equal, getattr would return the same method
        // we already have.
        if ptr::eq(methods, known_method.type_methods) {
            let r = eval.with_call_stack(known_method.to_value(), Some(span), |eval| {
                known_method.invoke_method(this, arguments, eval)
            })?;
            frame.set_bc_slot(target, r);
            return Ok(());
        }
    }

    call_method_common(eval, frame, this, symbol, arguments, span, target)
}

impl<'v, A: BcCallArgs<Symbol>> InstrNoFlowImpl<'v> for InstrCallMethodImpl<A> {
    type Arg = (BcSlotIn, Symbol, A::Arg<'v>, CallSpan<'v>, BcSlotOut);

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (this, symbol, args, span, target): &(
            BcSlotIn,
            Symbol,
            A::Arg<'v>,
            CallSpan<'v>,
            BcSlotOut,
        ),
    ) -> crate::Result<()> {
        let this = frame.get_bc_slot(*this);
        let arguments = Arguments(A::pop_from_stack(args, frame));
        call_method_common(
            eval,
            frame,
            this,
            symbol,
            &arguments,
            &span.as_ref().value,
            *target,
        )
    }
}

impl<'v, A: BcCallArgs<Symbol>> InstrNoFlowImpl<'v> for InstrCallMaybeKnownMethodImpl<A> {
    type Arg = (
        BcSlotIn,
        Symbol,
        KnownMethod,
        A::Arg<'v>,
        CallSpan<'v>,
        BcSlotOut,
    );

    #[inline(always)]
    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (this, symbol, known_method, args, span, target): &(
            BcSlotIn,
            Symbol,
            KnownMethod,
            A::Arg<'v>,
            CallSpan<'v>,
            BcSlotOut,
        ),
    ) -> crate::Result<()> {
        let this = frame.get_bc_slot(*this);
        let arguments = Arguments(A::pop_from_stack(args, frame));
        call_maybe_known_method_common(
            eval,
            frame,
            this,
            symbol,
            known_method,
            &arguments,
            &span.as_ref().value,
            *target,
        )
    }
}

pub(crate) struct InstrPossibleGcImpl;

pub(crate) type InstrPossibleGc = InstrNoFlow<InstrPossibleGcImpl>;

impl<'v> InstrNoFlowImpl<'v> for InstrPossibleGcImpl {
    type Arg = ();

    fn run_with_args(
        eval: &mut Evaluator<'v, '_, '_>,
        _frame: BcFramePtr<'v>,
        _ip: BcPtrAddr,
        (): &(),
    ) -> crate::Result<()> {
        possible_gc(eval);
        Ok(())
    }
}

/// Pseudo-instruction:
/// * to store bytecode metadata (i.e. spans): when bytecode is evaluated, we only have IP,
///   we don't have a pointer to bytecode object. To obtain spans by IP, we scroll
///   through the instruction until we encounter this pseudo-instruction.
/// * as a safety against memory overruns. Function block must terminate with return instruction,
///   but if return was missed, this instruction is executed and it panics.
pub(crate) struct InstrEnd;

impl<'v> BcInstr<'v> for InstrEnd {
    type Arg = BcInstrEndArg<'v>;

    fn run<'b>(
        _eval: &mut Evaluator<'v, '_, '_>,
        _frame: BcFramePtr<'v>,
        _ip: BcPtrAddr<'b>,
        _: &Self::Arg,
    ) -> InstrControl<'v, 'b> {
        unreachable!("this instruction is not meant to be executed");
    }
}
