/*
 * Copyright 2018 The Starlark in Rust Authors.
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

use crate::typing::OperatorSignature;
use crate::typing::Ty;
use crate::typing::TypingBinOp;

pub(crate) enum NumTy {
    Int,
    Float,
}

fn int_or_float() -> Ty {
    Ty::union2(Ty::int(), Ty::float())
}

/// Group of operators sharing the typing behavior.
enum BinOpClass {
    /// If any operand is a float, the result is a float.
    Add,
    /// Result is always a float.
    Div,
    /// Only supported for integers.
    BitAnd,
    /// Not supported.
    In,
    /// Supported.
    Less,
}

/// The signatures of `lhs op rhs` for a number `lhs`.
///
/// `int` and `float` are the types whose operators take more than one type of operand with
/// a result that depends on which, so they write their signatures here instead of having them
/// read off their operator methods, which take any value.
pub(crate) fn num_bin_op_signatures(lhs: NumTy, op: TypingBinOp) -> Vec<OperatorSignature> {
    let op = match op {
        TypingBinOp::Add
        | TypingBinOp::Sub
        | TypingBinOp::Mul
        | TypingBinOp::FloorDiv
        | TypingBinOp::Percent => BinOpClass::Add,
        TypingBinOp::Div => BinOpClass::Div,
        TypingBinOp::BitOr
        | TypingBinOp::BitXor
        | TypingBinOp::BitAnd
        | TypingBinOp::LeftShift
        | TypingBinOp::RightShift => BinOpClass::BitAnd,
        TypingBinOp::In => BinOpClass::In,
        TypingBinOp::Less => BinOpClass::Less,
    };

    let signature = |operand, result| OperatorSignature { operand, result };
    match (lhs, op) {
        (_, BinOpClass::In) => Vec::new(),
        (_, BinOpClass::Less) => vec![signature(int_or_float(), Ty::bool())],
        (NumTy::Int, BinOpClass::Add) => vec![
            signature(Ty::int(), Ty::int()),
            signature(Ty::float(), Ty::float()),
        ],
        (NumTy::Float, BinOpClass::Add) => vec![signature(int_or_float(), Ty::float())],
        (_, BinOpClass::Div) => vec![signature(int_or_float(), Ty::float())],
        (NumTy::Int, BinOpClass::BitAnd) => vec![signature(Ty::int(), Ty::int())],
        (NumTy::Float, BinOpClass::BitAnd) => Vec::new(),
    }
}
