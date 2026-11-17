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

// This makes for a better API.
#![allow(clippy::result_unit_err)]

use dupe::Dupe;

use crate::typing::Ty;

/// Unary operator for typechecker.
#[derive(Copy, Clone, Dupe, Eq, PartialEq, derive_more::Display, Debug)]
pub enum TypingUnOp {
    /// `+`.
    #[display("+")]
    Plus,
    /// `+`.
    #[display("-")]
    Minus,
    /// `~`.
    #[display("~")]
    BitNot,
}

/// Binary operator for typechecker.
#[derive(Copy, Clone, Dupe, Eq, PartialEq, derive_more::Display, Debug)]
pub enum TypingBinOp {
    /// `+`.
    #[display("+")]
    Add,
    /// `-`.
    #[display("-")]
    Sub,
    /// `/`.
    #[display("/")]
    Div,
    /// `//`.
    #[display("/")]
    FloorDiv,
    /// `*`.
    #[display("*")]
    Mul,
    /// `%`.
    #[display("%")]
    Percent,
    /// `y in x`.
    #[display("in")]
    In,
    /// `|`.
    #[display("|")]
    BitOr,
    /// `^`.
    #[display("^")]
    BitXor,
    /// `&`.
    #[display("&")]
    BitAnd,
    /// `<`.
    #[display("<")]
    Less,
    /// `<<`.
    #[display("<<")]
    LeftShift,
    /// `>>`.
    #[display(">>")]
    RightShift,
}

impl TypingBinOp {
    /// Result type is always `bool`.
    pub(crate) fn always_bool(self) -> bool {
        matches!(self, TypingBinOp::In | TypingBinOp::Less)
    }
}

/// One way an operator of a type applies: what the other operand is, and what the result is
/// then.
///
/// A type lists these per operator
/// ([`StarlarkType::bin_op_signatures`](crate::values::StarlarkType::bin_op_signatures)), and
/// the typechecker does the rest: an operand that intersects the operand type of one or more
/// of them gives the union of their results, and an operand that intersects none is an error.
/// A type has more than one for an operator when the result depends on the operand, as
/// `int + int` and `int + float` do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorSignature {
    /// The type of the other operand.
    pub operand: Ty,
    /// The type of the result.
    pub result: Ty,
}

impl OperatorSignature {
    /// What an operator written without types says: any operand, and a result that is not
    /// known.
    pub fn any() -> OperatorSignature {
        OperatorSignature {
            operand: Ty::any(),
            result: Ty::any(),
        }
    }
}
