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

//! Define the bytecode instruction.

use crate::eval::Evaluator;
use crate::eval::bc::addr::BcPtrAddr;
use crate::eval::bc::frame::BcFramePtr;
use crate::eval::bc::instr_arg::BcInstrArg;
use crate::values::Value;

/// Result of instruction evaluation.
// This is more efficient than `Result<R, EvalException>`,
// see in compiler explorer: https://rust.godbolt.org/z/81rj4jb4M
#[must_use]
pub(crate) enum InstrControl<'v, 'b> {
    /// Go to address.
    Next(BcPtrAddr<'b>),
    /// Return from the function.
    Return(Value<'v>),
    /// Error. This can be either any `anyhow::Error` or `Diagnostics`.
    /// If it is the former, error span will be added from instruction metadata.
    Err(crate::Error),
}

/// An instruction of bytecode at the brand `'v`: its operands are values there, and it runs
/// against an evaluator there. Instruction types themselves carry no brand (they are `'static`,
/// which lets the opcode be found by type), so each implements this for every `'v`.
pub(crate) trait BcInstr<'v>: Sized + 'static {
    /// Fixed instruction argument (which may encode additional arguments
    /// pushed or popped from the stack by the instruction implementation).
    type Arg: BcInstrArg<'v>;

    /// Execute the instruction.
    fn run<'b>(
        eval: &mut Evaluator<'v, '_, '_>,
        frame: BcFramePtr<'v>,
        ip: BcPtrAddr<'b>,
        arg: &Self::Arg,
    ) -> InstrControl<'v, 'b>;
}
