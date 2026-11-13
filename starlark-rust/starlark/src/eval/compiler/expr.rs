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

//! Evaluation of an expression.

use std::borrow::Cow;
use std::cmp::Ordering;

use dupe::Dupe;
use starlark_derive::StarlarkPagable;
use starlark_derive::VisitSpanMut;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::syntax::ast::AstExprP;
use starlark_syntax::syntax::ast::AstLiteral;
use starlark_syntax::syntax::ast::AstPayload;
use starlark_syntax::syntax::ast::BinOp;
use starlark_syntax::syntax::ast::ExprP;
use starlark_syntax::syntax::ast::FStringP;
use starlark_syntax::syntax::ast::LambdaP;
use starlark_syntax::syntax::ast::StmtP;
use thiserror::Error;

use crate as starlark;
use crate::codemap::Spanned;
use crate::collections::symbol::symbol::Symbol;
use crate::environment::slots::ModuleSlotId;
use crate::errors::did_you_mean::did_you_mean;
use crate::eval::Arguments;
use crate::eval::Evaluator;
use crate::eval::compiler::Compiler;
use crate::eval::compiler::args::ArgsCompiledValue;
use crate::eval::compiler::call::CallCompiled;
use crate::eval::compiler::compr::ComprCompiled;
use crate::eval::compiler::constants::Constants;
use crate::eval::compiler::def::Def;
use crate::eval::compiler::def::DefCompiled;
use crate::eval::compiler::error::CompilerInternalError;
use crate::eval::compiler::expr_bool::ExprCompiledBool;
use crate::eval::compiler::known::list_to_tuple;
use crate::eval::compiler::opt_ctx::OptCtx;
use crate::eval::compiler::scope::AssignCount;
use crate::eval::compiler::scope::Captured;
use crate::eval::compiler::scope::ResolvedIdent;
use crate::eval::compiler::scope::Slot;
use crate::eval::compiler::scope::payload::CstExpr;
use crate::eval::compiler::scope::payload::CstIdent;
use crate::eval::compiler::span::IrSpanned;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::eval::runtime::heap_file_span::HeapFileSpan;
use crate::eval::runtime::slots::LocalCapturedSlotId;
use crate::eval::runtime::slots::LocalSlotId;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::ValueTyped;
use crate::values::bool::StarlarkBool;
use crate::values::function::BoundMethod;
use crate::values::list::ListRef;
use crate::values::range::Range;
use crate::values::string::interpolation::parse_percent_s_one;
use crate::values::tuple::AllocTuple;
use crate::values::types::bytes::value::StarlarkBytes;
use crate::values::types::dict::Dict;
use crate::values::types::ellipsis::Ellipsis;
use crate::values::types::float::StarlarkFloat;
use crate::values::types::int::inline_int::InlineInt;
use crate::values::types::int::int_or_big::StarlarkInt;
use crate::values::types::list::value::ListData;
use crate::values::types::string::dot_format::format_one;
use crate::values::types::string::interpolation::percent_s_one;
use crate::values::types::tuple::value::Tuple;
use crate::values::types::unbound::UnboundValue;

/// `bool` operation.
#[derive(Copy, Clone, Dupe, Eq, PartialEq, Debug)]
pub(crate) enum MaybeNot {
    Id,
    Not,
}

impl MaybeNot {
    pub(crate) fn negate(self) -> MaybeNot {
        match self {
            MaybeNot::Id => MaybeNot::Not,
            MaybeNot::Not => MaybeNot::Id,
        }
    }
}

/// Map result of comparison to boolean.
#[derive(Copy, Clone, Dupe, Debug, StarlarkPagable)]
pub(crate) enum CompareOp {
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
}

impl CompareOp {
    fn apply(self, x: Ordering) -> bool {
        match self {
            CompareOp::Less => x == Ordering::Less,
            CompareOp::Greater => x == Ordering::Greater,
            CompareOp::LessOrEqual => x != Ordering::Greater,
            CompareOp::GreaterOrEqual => x != Ordering::Less,
        }
    }
}

/// Builtin function with one argument.
#[derive(Clone, Debug, VisitSpanMut, StarlarkPagable)]
pub(crate) enum Builtin1<'f> {
    Minus,
    /// `+x`.
    Plus,
    /// `~x`.
    BitNot,
    /// `not x`.
    Not,
    /// `type(arg) == "y"`
    TypeIs(StringValue<'f>),
    /// `"aaa%sbbb" % arg`
    PercentSOne(StringValue<'f>, StringValue<'f>),
    /// `"aaa%sbbb".format(arg)`
    FormatOne(StringValue<'f>, StringValue<'f>),
    /// `x.field`.
    Dot(Symbol),
}

impl<'f> Builtin1<'f> {
    /// Apply to an IR constant, speculatively at the value heap.
    fn eval<'v>(&self, v: Value<'f>, ctx: &mut OptCtx<'v, '_, '_, '_, 'f>) -> Option<Value<'v>> {
        let edge = ctx.edge();
        match self {
            Builtin1::Minus => edge.rebrand(v).minus(ctx.heap()).ok(),
            Builtin1::Plus => edge.rebrand(v).plus(ctx.heap()).ok(),
            Builtin1::BitNot => edge.rebrand(v).bit_not(ctx.heap()).ok(),
            Builtin1::Not => Some(Value::new_bool(!v.to_bool())),
            Builtin1::TypeIs(t) => Some(Value::new_bool(v.get_type_value().at() == *t)),
            Builtin1::FormatOne(before, after) => {
                Some(format_one(before, edge.rebrand(v), after, ctx.heap()).to_value())
            }
            Builtin1::PercentSOne(before, after) => {
                percent_s_one(before, edge.rebrand(v), after, ctx.heap())
                    .map(|s| s.to_value())
                    .ok()
            }
            Builtin1::Dot(field) => {
                Some(edge.rebrand(ExprCompiled::compile_time_getattr(v, field, ctx)?))
            }
        }
    }
}

/// Builtin function with two arguments.
#[derive(Copy, Clone, Dupe, Debug, VisitSpanMut, StarlarkPagable)]
pub(crate) enum Builtin2 {
    /// `a == b`.
    Equals,
    /// `a in b`.
    In,
    /// `a - b`.
    Sub,
    /// `a + b`.
    Add,
    /// `a * b`.
    Multiply,
    /// `a % b`.
    Percent,
    /// `a / b`.
    Divide,
    /// `a // b`.
    FloorDivide,
    /// `a & b`.
    BitAnd,
    /// `a | b`.
    BitOr,
    /// `a ^ b`.
    BitXor,
    /// `a << b`.
    LeftShift,
    /// `a >> b`.
    RightShift,
    /// `a <=> b`.
    Compare(CompareOp),
    /// `a[b]`.
    ArrayIndex,
}

impl Builtin2 {
    fn eval<'v>(self, a: Value<'v>, b: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match self {
            Builtin2::Equals => a.equals(b).map(Value::new_bool),
            Builtin2::Compare(cmp) => a.compare(b).map(|c| Value::new_bool(cmp.apply(c))),
            Builtin2::In => b.is_in(a).map(Value::new_bool),
            Builtin2::Sub => a.sub(b, heap),
            Builtin2::Add => a.add(b, heap),
            Builtin2::Multiply => a.mul(b, heap),
            Builtin2::Percent => a.percent(b, heap),
            Builtin2::Divide => a.div(b, heap),
            Builtin2::FloorDivide => a.floor_div(b, heap),
            Builtin2::BitAnd => a.bit_and(b, heap),
            Builtin2::BitOr => a.bit_or(b, heap),
            Builtin2::BitXor => a.bit_xor(b, heap),
            Builtin2::LeftShift => a.left_shift(b, heap),
            Builtin2::RightShift => a.right_shift(b, heap),
            Builtin2::ArrayIndex => a.at(b, heap),
        }
    }
}

/// Logical binary operator.
#[derive(Copy, Clone, Dupe, Debug, VisitSpanMut, Eq, PartialEq, StarlarkPagable)]
pub(crate) enum ExprLogicalBinOp {
    And,
    Or,
}

/// An expression of the IR, at the brand `'f` of the frozen heap its constants live in.
#[derive(Clone, Debug, VisitSpanMut, StarlarkPagable)]
pub(crate) enum ExprCompiled<'f> {
    Value(Value<'f>),
    /// Read local non-captured variable.
    Local(LocalSlotId),
    /// Read local captured variable.
    LocalCaptured(LocalCapturedSlotId),
    Module(ModuleSlotId),
    Tuple(Vec<IrSpanned<'f, ExprCompiled<'f>>>),
    List(Vec<IrSpanned<'f, ExprCompiled<'f>>>),
    Dict(
        Vec<(
            IrSpanned<'f, ExprCompiled<'f>>,
            IrSpanned<'f, ExprCompiled<'f>>,
        )>,
    ),
    /// Comprehension.
    Compr(ComprCompiled<'f>),
    If(
        Box<(
            // Condition.
            IrSpanned<'f, ExprCompiled<'f>>,
            // Then branch.
            IrSpanned<'f, ExprCompiled<'f>>,
            // Else branch.
            IrSpanned<'f, ExprCompiled<'f>>,
        )>,
    ),
    Slice(
        Box<(
            IrSpanned<'f, ExprCompiled<'f>>,
            Option<IrSpanned<'f, ExprCompiled<'f>>>,
            Option<IrSpanned<'f, ExprCompiled<'f>>>,
            Option<IrSpanned<'f, ExprCompiled<'f>>>,
        )>,
    ),
    Builtin1(Builtin1<'f>, Box<IrSpanned<'f, ExprCompiled<'f>>>),
    LogicalBinOp(
        ExprLogicalBinOp,
        Box<(
            IrSpanned<'f, ExprCompiled<'f>>,
            IrSpanned<'f, ExprCompiled<'f>>,
        )>,
    ),
    /// Expression equivalent to `(x, y)[1]`: evaluate `x`, discard the result,
    /// then evaluate `y` and use its result.
    Seq(
        Box<(
            IrSpanned<'f, ExprCompiled<'f>>,
            IrSpanned<'f, ExprCompiled<'f>>,
        )>,
    ),
    Builtin2(
        Builtin2,
        Box<(
            IrSpanned<'f, ExprCompiled<'f>>,
            IrSpanned<'f, ExprCompiled<'f>>,
        )>,
    ),
    Index2(
        Box<(
            IrSpanned<'f, ExprCompiled<'f>>,
            IrSpanned<'f, ExprCompiled<'f>>,
            IrSpanned<'f, ExprCompiled<'f>>,
        )>,
    ),
    Call(Box<IrSpanned<'f, CallCompiled<'f>>>),
    Def(DefCompiled<'f>),
}

impl<'f> ExprCompiled<'f> {
    pub fn as_value(&self) -> Option<Value<'f>> {
        match self {
            Self::Value(x) => Some(*x),
            _ => None,
        }
    }

    /// Expression is known to be a constant which is a `def`.
    pub(crate) fn as_def(&self) -> Option<ValueTyped<'f, Def<'f>>> {
        ValueTyped::new(self.as_value()?)
    }

    /// Expression is known to be a constant bound method.
    pub(crate) fn as_bound_method(&self) -> Option<ValueTyped<'f, BoundMethod<'f>>> {
        ValueTyped::new(self.as_value()?)
    }

    /// Expression is builtin `len` function.
    pub(crate) fn is_fn_len(&self) -> bool {
        match self.as_value() {
            Some(value) => Constants::get().fn_len.is(value),
            None => false,
        }
    }

    /// Expression is builtin `type` function.
    pub(crate) fn is_fn_type(&self) -> bool {
        match self.as_value() {
            Some(value) => Constants::get().fn_type.is(value),
            None => false,
        }
    }

    /// Expression is builtin `isinstance` function.
    pub(crate) fn is_fn_isinstance(&self) -> bool {
        match self.as_value() {
            Some(value) => Constants::get().fn_isinstance.is(value),
            None => false,
        }
    }

    /// If expression is `type(x)`, return `x`.
    pub(crate) fn as_type(&self) -> Option<&IrSpanned<'f, ExprCompiled<'f>>> {
        match self {
            Self::Call(c) => c.as_type(),
            _ => None,
        }
    }

    /// If expression if `type(x) == t`, return `x` and `t`.
    pub(crate) fn as_type_is(&self) -> Option<(&IrSpanned<'f, ExprCompiled<'f>>, StringValue<'f>)> {
        match self {
            ExprCompiled::Builtin1(Builtin1::TypeIs(t), x) => Some((x, *t)),
            _ => None,
        }
    }

    /// Expression is a constant which is builtin.
    pub(crate) fn as_builtin_value(&self) -> Option<Value<'f>> {
        match self {
            Self::Value(x) if x.is_builtin() => Some(*x),
            _ => None,
        }
    }

    /// Is expression a constant string?
    pub(crate) fn as_string(&self) -> Option<StringValue<'f>> {
        StringValue::new(self.as_value()?)
    }

    /// Iterable produced by this expression results in empty.
    pub(crate) fn is_iterable_empty(&self) -> bool {
        match self {
            ExprCompiled::List(xs) => xs.is_empty(),
            ExprCompiled::Tuple(xs) => xs.is_empty(),
            ExprCompiled::Dict(xs) => xs.is_empty(),
            // Strings have a length but are not iterable.
            ExprCompiled::Value(v) if v.is_builtin() && !v.is_str() => {
                v.length().is_ok_and(|l| l == 0)
            }
            _ => false,
        }
    }

    /// Result of this expression is definitely `bool`
    /// (if `false` it may also be `bool`).
    fn is_definitely_bool(&self) -> bool {
        match self {
            Self::Value(v) => v.unpack_bool().is_some(),
            Self::Builtin1(Builtin1::Not | Builtin1::TypeIs(_), _)
            | Self::Builtin2(Builtin2::In | Builtin2::Equals | Builtin2::Compare(_), ..) => true,
            _ => false,
        }
    }

    /// This expression is definitely:
    /// * infallible
    /// * has no effects
    pub(crate) fn is_pure_infallible(&self) -> bool {
        match self {
            Self::Value(..) => true,
            Self::List(xs) | Self::Tuple(xs) => xs.iter().all(|x| x.is_pure_infallible()),
            Self::Dict(xs) => xs.is_empty(),
            Self::Builtin1(Builtin1::Not | Builtin1::TypeIs(_), x) => x.is_pure_infallible(),
            Self::Seq(x_y) => {
                let (x, y) = &**x_y;
                x.is_pure_infallible() && y.is_pure_infallible()
            }
            Self::LogicalBinOp(_op, x_y) => {
                let (x, y) = &**x_y;
                x.is_pure_infallible() && y.is_pure_infallible()
            }
            Self::If(cond_x_y) => {
                let (cond, x, y) = &**cond_x_y;
                cond.is_pure_infallible() && x.is_pure_infallible() && y.is_pure_infallible()
            }
            Self::Call(call) => call.is_pure_infallible(),
            _ => false,
        }
    }

    /// If this expression is pure, infallible, and known to produce a value,
    /// return truth of that value.
    pub(crate) fn is_pure_infallible_to_bool(&self) -> Option<bool> {
        match self {
            ExprCompiled::Value(v) => Some(v.to_bool()),
            ExprCompiled::List(xs) | ExprCompiled::Tuple(xs)
                if xs.iter().all(|x| x.is_pure_infallible()) =>
            {
                Some(!xs.is_empty())
            }
            // TODO(nga): if keys are unique hashable constants, we can fold this to constant too.
            ExprCompiled::Dict(xs) if xs.is_empty() => Some(false),
            ExprCompiled::Builtin1(Builtin1::Not, x) => x.is_pure_infallible_to_bool().map(|x| !x),
            ExprCompiled::LogicalBinOp(op, x_y) => {
                let (x, y) = &**x_y;
                match (
                    op,
                    x.is_pure_infallible_to_bool(),
                    y.is_pure_infallible_to_bool(),
                ) {
                    (ExprLogicalBinOp::And, Some(true), y) => y,
                    (ExprLogicalBinOp::Or, Some(false), y) => y,
                    (ExprLogicalBinOp::And, Some(false), _) => Some(false),
                    (ExprLogicalBinOp::Or, Some(true), _) => Some(true),
                    (_, None, _) => None,
                }
            }
            _ => None,
        }
    }

    /// This expression is local slot.
    pub(crate) fn as_local_non_captured(&self) -> Option<LocalSlotId> {
        match self {
            ExprCompiled::Local(slot) => Some(*slot),
            _ => None,
        }
    }
}

enum ExprShortList<'a, 'f> {
    Exprs(&'a [IrSpanned<'f, ExprCompiled<'f>>]),
    Constants(&'a [Value<'f>]),
}

impl<'a, 'f> IrSpanned<'f, ExprShortList<'a, 'f>> {
    fn as_exprs(&self) -> Vec<IrSpanned<'f, ExprCompiled<'f>>> {
        match &self.node {
            ExprShortList::Exprs(exprs) => exprs.to_vec(),
            ExprShortList::Constants(constants) => constants
                .iter()
                .map(|c| IrSpanned {
                    node: ExprCompiled::Value(*c),
                    span: self.span,
                })
                .collect(),
        }
    }
}

impl<'f> IrSpanned<'f, ExprCompiled<'f>> {
    /// Try to extract `[e0, e1, ..., en]` from this expression.
    fn as_short_list(&self) -> Option<IrSpanned<'f, ExprShortList<'_, 'f>>> {
        // Prevent exponential explosion during optimization.
        const MAX_LEN: usize = 1000;
        match &self.node {
            ExprCompiled::List(xs) if xs.len() <= MAX_LEN => Some(ExprShortList::Exprs(xs)),
            ExprCompiled::Value(v) => {
                let list = ListRef::from_value(*v)?;
                if list.len() <= MAX_LEN {
                    Some(ExprShortList::Constants(list.content()))
                } else {
                    None
                }
            }
            _ => None,
        }
        .map(|node| IrSpanned {
            node,
            span: self.span,
        })
    }

    pub(crate) fn optimize(
        &self,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> IrSpanned<'f, ExprCompiled<'f>> {
        let span = self.span;
        let expr = match &self.node {
            e @ (ExprCompiled::Value(..)
            | ExprCompiled::Local(..)
            | ExprCompiled::LocalCaptured(..)) => e.clone(),
            ExprCompiled::Module(slot) => {
                match ctx.frozen_module().and_then(|m| m.get_slot(*slot)) {
                    None => {
                        // Let if fail at runtime.
                        ExprCompiled::Module(*slot)
                    }
                    Some(v) => ExprCompiled::Value(v),
                }
            }
            ExprCompiled::Tuple(xs) => {
                ExprCompiled::tuple(xs.map(|e| e.optimize(ctx)), ctx.frozen_heap())
            }
            ExprCompiled::List(xs) => ExprCompiled::List(xs.map(|e| e.optimize(ctx))),
            ExprCompiled::Dict(kvs) => {
                ExprCompiled::Dict(kvs.map(|(k, v)| (k.optimize(ctx), v.optimize(ctx))))
            }
            ExprCompiled::Compr(compr) => compr.optimize(ctx),
            ExprCompiled::If(cond_t_f) => {
                let (cond, t, f) = &**cond_t_f;
                let cond = cond.optimize(ctx);
                let t = t.optimize(ctx);
                let f = f.optimize(ctx);
                return ExprCompiled::if_expr(cond, t, f);
            }
            ExprCompiled::Slice(v_start_stop_step) => {
                let (v, start, stop, step) = &**v_start_stop_step;
                let v = v.optimize(ctx);
                let start = start.as_ref().map(|x| x.optimize(ctx));
                let stop = stop.as_ref().map(|x| x.optimize(ctx));
                let step = step.as_ref().map(|x| x.optimize(ctx));
                ExprCompiled::slice(span, v, start, stop, step, ctx)
            }
            ExprCompiled::Builtin1(op, e) => {
                let e = e.optimize(ctx);
                ExprCompiled::un_op(span, op, e, ctx)
            }
            ExprCompiled::LogicalBinOp(op, l_r) => {
                let (l, r) = &**l_r;
                let l = l.optimize(ctx);
                let r = r.optimize(ctx);
                return ExprCompiled::logical_bin_op(*op, l, r);
            }
            ExprCompiled::Seq(l_r) => {
                let (l, r) = &**l_r;
                let l = l.optimize(ctx);
                let r = r.optimize(ctx);
                return ExprCompiled::seq(l, r);
            }
            ExprCompiled::Builtin2(op, l_r) => {
                let (l, r) = &**l_r;
                let l = l.optimize(ctx);
                let r = r.optimize(ctx);
                ExprCompiled::bin_op(*op, l, r, ctx)
            }
            ExprCompiled::Index2(a_i0_i1) => {
                let (a, i0, i1) = &**a_i0_i1;
                let a = a.optimize(ctx);
                let i0 = i0.optimize(ctx);
                let i1 = i1.optimize(ctx);
                ExprCompiled::index2(a, i0, i1)
            }
            d @ ExprCompiled::Def(..) => (*d).clone(),
            ExprCompiled::Call(call) => call.optimize(ctx),
        };
        IrSpanned { node: expr, span }
    }
}

impl<'f> ExprCompiled<'f> {
    fn equals(
        l: IrSpanned<'f, ExprCompiled<'f>>,
        r: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> IrSpanned<'f, ExprCompiled<'f>> {
        let span = l.span.merge(&r.span);
        if let (Some(l), Some(r)) = (l.as_value(), r.as_value()) {
            // If comparison fails, let it fail in runtime.
            if let Ok(r) = l.equals(r) {
                return IrSpanned {
                    span,
                    node: ExprCompiled::Value(Value::new_bool(r)),
                };
            }
        }

        let (l, r) = match try_eval_type_is(l, r) {
            Ok(e) => return e,
            Err((l, r)) => (l, r),
        };

        let (r, l) = match try_eval_type_is(r, l) {
            Ok(e) => return e,
            Err((r, l)) => (r, l),
        };

        IrSpanned {
            span,
            node: ExprCompiled::Builtin2(Builtin2::Equals, Box::new((l, r))),
        }
    }

    pub(crate) fn not(
        span: FrameSpan<'f>,
        expr: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> IrSpanned<'f, ExprCompiled<'f>> {
        match expr.node {
            ExprCompiled::Value(x) => IrSpanned {
                node: ExprCompiled::Value(Value::new_bool(!x.to_bool())),
                span,
            },
            // Collapse `not not e` to `e` only if `e` is known to produce a boolean.
            ExprCompiled::Builtin1(Builtin1::Not, e) if e.is_definitely_bool() => (*e).clone(),
            _ => IrSpanned {
                node: ExprCompiled::Builtin1(Builtin1::Not, Box::new(expr)),
                span,
            },
        }
    }

    fn or(
        l: IrSpanned<'f, ExprCompiled<'f>>,
        r: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> IrSpanned<'f, ExprCompiled<'f>> {
        Self::logical_bin_op(ExprLogicalBinOp::Or, l, r)
    }

    fn and(
        l: IrSpanned<'f, ExprCompiled<'f>>,
        r: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> IrSpanned<'f, ExprCompiled<'f>> {
        Self::logical_bin_op(ExprLogicalBinOp::And, l, r)
    }

    pub(crate) fn logical_bin_op(
        op: ExprLogicalBinOp,
        l: IrSpanned<'f, ExprCompiled<'f>>,
        r: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> IrSpanned<'f, ExprCompiled<'f>> {
        if let Some(l_v) = l.is_pure_infallible_to_bool() {
            if l_v == (op == ExprLogicalBinOp::Or) {
                l
            } else {
                r
            }
        } else {
            let span = l.span.merge(&r.span);
            IrSpanned {
                node: ExprCompiled::LogicalBinOp(op, Box::new((l, r))),
                span,
            }
        }
    }

    pub(crate) fn seq(
        l: IrSpanned<'f, ExprCompiled<'f>>,
        r: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> IrSpanned<'f, ExprCompiled<'f>> {
        if l.is_pure_infallible() {
            r
        } else {
            let span = l.span.merge(&r.span);
            IrSpanned {
                node: ExprCompiled::Seq(Box::new((l, r))),
                span,
            }
        }
    }

    fn percent(
        l: IrSpanned<'f, ExprCompiled<'f>>,
        r: IrSpanned<'f, ExprCompiled<'f>>,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> ExprCompiled<'f> {
        if let Some(v) = l.as_string() {
            if let Some((before, after)) = parse_percent_s_one(&v) {
                let before = ctx.frozen_heap().alloc_str(&before);
                let after = ctx.frozen_heap().alloc_str(&after);
                return ExprCompiled::percent_s_one(before, r, after, ctx);
            }
        }
        ExprCompiled::Builtin2(Builtin2::Percent, Box::new((l, r)))
    }

    fn percent_s_one(
        before: StringValue<'f>,
        arg: IrSpanned<'f, ExprCompiled<'f>>,
        after: StringValue<'f>,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> ExprCompiled<'f> {
        if let Some(arg) = arg.as_value() {
            if let Ok(value) = percent_s_one(
                before.as_str(),
                ctx.edge().rebrand(arg),
                after.as_str(),
                ctx.heap(),
            ) {
                let value = ctx.frozen_heap().alloc_str(value.as_str());
                return ExprCompiled::Value(value.to_value());
            }
        }

        ExprCompiled::Builtin1(Builtin1::PercentSOne(before, after), Box::new(arg))
    }

    pub(crate) fn format_one(
        before: StringValue<'f>,
        arg: IrSpanned<'f, ExprCompiled<'f>>,
        after: StringValue<'f>,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> ExprCompiled<'f> {
        if let Some(arg) = arg.as_value() {
            let value = format_one(&before, ctx.edge().rebrand(arg), &after, ctx.heap());
            let value = ctx.frozen_heap().alloc_str(value.as_str());
            return ExprCompiled::Value(value.to_value());
        }

        ExprCompiled::Builtin1(Builtin1::FormatOne(before, after), Box::new(arg))
    }

    fn add(
        l: IrSpanned<'f, ExprCompiled<'f>>,
        r: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> ExprCompiled<'f> {
        if let (Some(l), Some(r)) = (l.as_short_list(), r.as_short_list()) {
            return ExprCompiled::List(l.as_exprs().into_iter().chain(r.as_exprs()).collect());
        }
        ExprCompiled::Builtin2(Builtin2::Add, Box::new((l, r)))
    }

    pub(crate) fn bin_op(
        bin_op: Builtin2,
        l: IrSpanned<'f, ExprCompiled<'f>>,
        r: IrSpanned<'f, ExprCompiled<'f>>,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> ExprCompiled<'f> {
        let span = l.span.merge(&r.span);
        // Binary operators should have no side effects,
        // but to avoid possible problems, we only fold binary operators on builtin types.
        if let (Some(l), Some(r)) = (l.as_builtin_value(), r.as_builtin_value()) {
            let edge = ctx.edge();
            if let Ok(v) = bin_op.eval(edge.rebrand(l), edge.rebrand(r), ctx.heap()) {
                if let Some(v) = ExprCompiled::try_value(span, v, ctx) {
                    return v;
                }
            }
        }

        match bin_op {
            Builtin2::Percent => ExprCompiled::percent(l, r, ctx),
            Builtin2::Add => ExprCompiled::add(l, r),
            Builtin2::Equals => ExprCompiled::equals(l, r).node,
            Builtin2::ArrayIndex => ExprCompiled::index(l, r, ctx),
            bin_op => ExprCompiled::Builtin2(bin_op, Box::new((l, r))),
        }
    }

    pub(crate) fn if_expr(
        cond: IrSpanned<'f, ExprCompiled<'f>>,
        t: IrSpanned<'f, ExprCompiled<'f>>,
        f: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> IrSpanned<'f, ExprCompiled<'f>> {
        let cond_span = cond.span;
        let cond = ExprCompiledBool::new(cond);
        match cond.node {
            ExprCompiledBool::Const(true) => t,
            ExprCompiledBool::Const(false) => f,
            ExprCompiledBool::Expr(cond) => match cond {
                ExprCompiled::Builtin1(Builtin1::Not, cond) => ExprCompiled::if_expr(*cond, f, t),
                ExprCompiled::Seq(x_cond) => {
                    let (x, cond) = *x_cond;
                    ExprCompiled::seq(x, ExprCompiled::if_expr(cond, t, f))
                }
                cond => {
                    let cond = IrSpanned {
                        node: cond,
                        span: cond_span,
                    };
                    let span = cond.span.merge(&t.span).merge(&f.span);
                    IrSpanned {
                        node: ExprCompiled::If(Box::new((cond, t, f))),
                        span,
                    }
                }
            },
        }
    }

    pub(crate) fn un_op(
        span: FrameSpan<'f>,
        op: &Builtin1<'f>,
        expr: IrSpanned<'f, ExprCompiled<'f>>,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> ExprCompiled<'f> {
        if let Some(v) = expr.as_builtin_value() {
            if let Some(v) = op.eval(v, ctx) {
                if let Some(v) = ExprCompiled::try_value(expr.span, v, ctx) {
                    return v;
                }
            }
        }
        match op {
            Builtin1::FormatOne(before, after) => {
                ExprCompiled::format_one(*before, expr, *after, ctx)
            }
            Builtin1::PercentSOne(before, after) => {
                ExprCompiled::percent_s_one(*before, expr, *after, ctx)
            }
            Builtin1::Dot(field) => ExprCompiled::dot(expr, field, ctx),
            Builtin1::TypeIs(t) => ExprCompiled::type_is(expr, *t),
            Builtin1::Not => ExprCompiled::not(span, expr).node,
            op => ExprCompiled::Builtin1(op.clone(), Box::new(expr)),
        }
    }

    fn try_values<'v>(
        span: FrameSpan<'f>,
        values: &[Value<'v>],
        ctx: &OptCtx<'v, '_, '_, '_, 'f>,
    ) -> Option<Vec<IrSpanned<'f, ExprCompiled<'f>>>> {
        values
            .try_map(|v| {
                Self::try_value(span, *v, ctx)
                    .map(|expr| IrSpanned { span, node: expr })
                    .ok_or(())
            })
            .ok()
    }

    /// Try convert a maybe not frozen value to an expression, or discard it.
    pub(crate) fn try_value<'v>(
        span: FrameSpan<'f>,
        v: Value<'v>,
        ctx: &OptCtx<'v, '_, '_, '_, 'f>,
    ) -> Option<ExprCompiled<'f>> {
        let heap = ctx.frozen_heap();
        if let Some(v) = ctx.demote(v) {
            // If frozen, we are lucky.
            Some(ExprCompiled::Value(v))
        } else if let Some(v) = v.unpack_str() {
            if v.len() <= 1000 {
                // If string, copy it to frozen heap.
                Some(ExprCompiled::Value(heap.alloc_str(v).to_value()))
            } else {
                // Long strings may lead to exponential explosion in the optimizer,
                // so skips optimizations for them.
                None
            }
        } else if let Some(v) = v.downcast_ref::<StarlarkFloat>() {
            Some(ExprCompiled::Value(
                heap.alloc_representation(*v).to_value(),
            ))
        } else if let Some(v) = v.downcast_ref::<Range>() {
            Some(ExprCompiled::Value(
                heap.alloc_representation(*v).to_value(),
            ))
        } else if let Some(v) = ListRef::from_value(v) {
            // When spec-safe function returned a non-frozen list,
            // we try to convert that list to a list of constants instruction.
            let items = Self::try_values(span, v.content(), ctx)?;
            Some(ExprCompiled::List(items))
        } else if let Some(v) = Tuple::from_value(v) {
            let items = Self::try_values(span, v.content(), ctx)?;
            Some(Self::tuple(items, heap))
        } else {
            None
        }
    }

    pub(crate) fn compr(compr: ComprCompiled<'f>) -> ExprCompiled<'f> {
        match compr {
            ComprCompiled::List(x, clauses) => {
                if clauses.is_nop() {
                    ExprCompiled::List(Vec::new())
                } else {
                    ExprCompiled::Compr(ComprCompiled::List(x, clauses))
                }
            }
            ComprCompiled::Dict(k_v, clauses) => {
                let (k, v) = *k_v;
                if clauses.is_nop() {
                    ExprCompiled::Dict(Vec::new())
                } else {
                    ExprCompiled::Compr(ComprCompiled::Dict(Box::new((k, v)), clauses))
                }
            }
        }
    }

    /// Construct tuple expression from elements optimizing to frozen tuple value when possible.
    pub(crate) fn tuple(
        elems: Vec<IrSpanned<'f, ExprCompiled<'f>>>,
        heap: FrozenHeap<'f>,
    ) -> ExprCompiled<'f> {
        if let Ok(elems) = elems.try_map(|e| e.as_value().ok_or(())) {
            ExprCompiled::Value(heap.alloc(AllocTuple(elems)).to_value())
        } else {
            ExprCompiled::Tuple(elems)
        }
    }

    pub(crate) fn compile_time_getattr(
        left: Value<'f>,
        attr: &Symbol,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> Option<Value<'f>> {
        // We assume `getattr` has no side effects.
        let v = get_attr_hashed_raw(ctx.edge().rebrand(left), attr, ctx.heap()).ok()?;
        match v {
            MemberOrValue::Member(m) => match m.at() {
                UnboundValue::Method(m) => Some(
                    ctx.frozen_heap()
                        .alloc_representation(BoundMethod::new(left, m))
                        .to_value(),
                ),
                UnboundValue::Attr(..) => None,
            },
            MemberOrValue::Value(v) => ctx.demote(v),
        }
    }

    pub(crate) fn dot(
        object: IrSpanned<'f, ExprCompiled<'f>>,
        field: &Symbol,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> ExprCompiled<'f> {
        if let Some(left) = object.as_value() {
            if let Some(v) = Self::compile_time_getattr(left, field, ctx) {
                return ExprCompiled::Value(v);
            }
        }

        ExprCompiled::Builtin1(Builtin1::Dot(field.clone()), Box::new(object))
    }

    fn slice(
        span: FrameSpan<'f>,
        array: IrSpanned<'f, ExprCompiled<'f>>,
        start: Option<IrSpanned<'f, ExprCompiled<'f>>>,
        stop: Option<IrSpanned<'f, ExprCompiled<'f>>>,
        step: Option<IrSpanned<'f, ExprCompiled<'f>>>,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> ExprCompiled<'f> {
        // An omitted part is `Some(None)`; a part that is present but not constant is `None`,
        // which disables folding.
        fn const_part<'f>(
            e: &Option<IrSpanned<'f, ExprCompiled<'f>>>,
        ) -> Option<Option<Value<'f>>> {
            match e {
                None => Some(None),
                Some(e) => e.as_value().map(Some),
            }
        }
        if let (Some(array), Some(start), Some(stop), Some(step)) = (
            array.as_builtin_value(),
            const_part(&start),
            const_part(&stop),
            const_part(&step),
        ) {
            let edge = ctx.edge();
            if let Ok(v) = edge.rebrand(array).slice(
                start.map(|v| edge.rebrand(v)),
                stop.map(|v| edge.rebrand(v)),
                step.map(|v| edge.rebrand(v)),
                ctx.heap(),
            ) {
                if let Some(v) = ExprCompiled::try_value(span, v, ctx) {
                    return v;
                }
            }
        }
        ExprCompiled::Slice(Box::new((array, start, stop, step)))
    }

    pub(crate) fn index(
        array: IrSpanned<'f, ExprCompiled<'f>>,
        index: IrSpanned<'f, ExprCompiled<'f>>,
        ctx: &mut OptCtx<'_, '_, '_, '_, 'f>,
    ) -> ExprCompiled<'f> {
        let span = array.span.merge(&index.span);
        if let (Some(array), Some(index)) = (array.as_builtin_value(), index.as_value()) {
            let edge = ctx.edge();
            if let Ok(v) = edge.rebrand(array).at(edge.rebrand(index), ctx.heap()) {
                if let Some(expr) = ExprCompiled::try_value(span, v, ctx) {
                    return expr;
                }
            }
        }
        ExprCompiled::Builtin2(Builtin2::ArrayIndex, Box::new((array, index)))
    }

    pub(crate) fn index2(
        array: IrSpanned<'f, ExprCompiled<'f>>,
        index0: IrSpanned<'f, ExprCompiled<'f>>,
        index1: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> ExprCompiled<'f> {
        ExprCompiled::Index2(Box::new((array, index0, index1)))
    }

    pub(crate) fn typ(span: FrameSpan<'f>, v: IrSpanned<'f, ExprCompiled<'f>>) -> ExprCompiled<'f> {
        match &v.node {
            ExprCompiled::Value(v) => ExprCompiled::Value(v.get_type_value().at().to_value()),
            ExprCompiled::Tuple(xs) if xs.iter().all(|e| e.is_pure_infallible()) => {
                ExprCompiled::Value(Tuple::get_type_value_static().at().to_value())
            }
            ExprCompiled::List(xs) if xs.iter().all(|e| e.is_pure_infallible()) => {
                ExprCompiled::Value(ListData::get_type_value_static().at().to_value())
            }
            ExprCompiled::Dict(xs) if xs.is_empty() => {
                ExprCompiled::Value(Dict::get_type_value_static().at().to_value())
            }
            ExprCompiled::Builtin1(Builtin1::Not | Builtin1::TypeIs(_), x)
                if x.is_pure_infallible() =>
            {
                ExprCompiled::Value(StarlarkBool::get_type_value_static().at().to_value())
            }
            _ => ExprCompiled::Call(Box::new(IrSpanned {
                span,
                node: CallCompiled {
                    fun: IrSpanned {
                        span,
                        node: ExprCompiled::Value(Constants::get().fn_type.at()),
                    },
                    args: ArgsCompiledValue {
                        pos_named: vec![v],
                        ..ArgsCompiledValue::default()
                    },
                },
            })),
        }
    }

    pub(crate) fn type_is(
        v: IrSpanned<'f, ExprCompiled<'f>>,
        t: StringValue<'f>,
    ) -> ExprCompiled<'f> {
        if let Some(v) = v.as_value() {
            return ExprCompiled::Value(Value::new_bool(v.get_type() == t.as_str()));
        }
        ExprCompiled::Builtin1(Builtin1::TypeIs(t), Box::new(v))
    }

    pub(crate) fn len(
        span: FrameSpan<'f>,
        arg: IrSpanned<'f, ExprCompiled<'f>>,
    ) -> ExprCompiled<'f> {
        if let Some(arg) = arg.as_value() {
            if let Ok(len) = arg.length() {
                if let Ok(len) = InlineInt::try_from(len) {
                    return ExprCompiled::Value(Value::new_int(len));
                }
            }
        }
        ExprCompiled::Call(Box::new(IrSpanned {
            span,
            node: CallCompiled {
                fun: IrSpanned {
                    span,
                    node: ExprCompiled::Value(Constants::get().fn_len.at()),
                },
                args: ArgsCompiledValue {
                    pos_named: vec![arg],
                    ..ArgsCompiledValue::default()
                },
            },
        }))
    }
}

#[derive(Debug, Clone, Error)]
pub(crate) enum EvalError {
    #[error("Dictionary key repeated for `{0}`")]
    DuplicateDictionaryKey(String),
}

/// Try fold expression `cmp(l == r)` into `cmp(type(x) == "y")`.
/// Return original `l` and `r` arguments if fold was unsuccessful.
#[allow(clippy::result_large_err)]
fn try_eval_type_is<'f>(
    l: IrSpanned<'f, ExprCompiled<'f>>,
    r: IrSpanned<'f, ExprCompiled<'f>>,
) -> Result<
    IrSpanned<'f, ExprCompiled<'f>>,
    (
        IrSpanned<'f, ExprCompiled<'f>>,
        IrSpanned<'f, ExprCompiled<'f>>,
    ),
> {
    let span = l.span.merge(&r.span);
    if let (Some(l), Some(r)) = (l.as_type(), r.as_string()) {
        Ok(IrSpanned {
            span,
            node: ExprCompiled::type_is(l.clone(), r),
        })
    } else {
        Err((l, r))
    }
}

trait AstLiteralCompile {
    fn compile<'f>(&self, heap: FrozenHeap<'f>) -> Value<'f>;
}

impl AstLiteralCompile for AstLiteral {
    fn compile<'f>(&self, heap: FrozenHeap<'f>) -> Value<'f> {
        match self {
            AstLiteral::Int(i) => heap.alloc(StarlarkInt::from(i.node.clone())).to_value(),
            AstLiteral::Float(f) => heap.alloc(f.node).to_value(),
            AstLiteral::String(x) => heap.alloc(x.node.as_str()).to_value(),
            AstLiteral::Bytes(b) => heap
                .alloc_representation(StarlarkBytes::new(&b.node))
                .to_value(),
            AstLiteral::Ellipsis => heap.alloc(Ellipsis).to_value(),
        }
    }
}

trait CompilerExprUtil<P: AstPayload> {
    fn unpack_string_literal(&self) -> Option<&str>;
    fn reduces_to_string<'a>(
        op: BinOp,
        left: &'a AstExprP<P>,
        right: &'a AstExprP<P>,
    ) -> Option<String>;
}

impl<P: AstPayload> CompilerExprUtil<P> for ExprP<P> {
    fn unpack_string_literal(&self) -> Option<&str> {
        match self {
            ExprP::Literal(AstLiteral::String(i)) => Some(&i.node),
            _ => None,
        }
    }

    // Does an entire sequence of additions reduce to a string literal
    fn reduces_to_string<'a>(
        mut op: BinOp,
        mut left: &'a AstExprP<P>,
        mut right: &'a AstExprP<P>,
    ) -> Option<String> {
        let mut results = Vec::new();
        loop {
            if op != BinOp::Add {
                return None;
            }
            // a + b + c  associates as  (a + b) + c
            let x = right.unpack_string_literal()?;
            results.push(x.to_owned());
            match &left.node {
                ExprP::Op(left2, op2, right2) => {
                    op = *op2;
                    left = left2;
                    right = right2;
                }
                _ => {
                    let x = left.unpack_string_literal()?;
                    results.push(x.to_owned());
                    break;
                }
            }
        }
        results.reverse();
        Some(results.concat())
    }
}

#[cold]
#[inline(never)]
fn get_attr_no_attr_error<'v>(x: Value<'v>, attribute: &Symbol) -> crate::Error {
    match did_you_mean(attribute.as_str(), x.dir_attr().iter().map(|s| s.as_str())) {
        None => ValueError::NoAttr(x.get_type().to_owned(), attribute.as_str().to_owned()).into(),
        Some(better) => ValueError::NoAttrDidYouMean(
            x.get_type().to_owned(),
            attribute.as_str().to_owned(),
            better.to_owned(),
        )
        .into(),
    }
}

pub(crate) enum MemberOrValue<'v> {
    Member(&'static UnboundValue<'static>),
    Value(Value<'v>),
}

impl<'v> MemberOrValue<'v> {
    #[inline]
    pub(crate) fn invoke(
        &self,
        this: Value<'v>,
        span: &'v FrameSpan<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        match self {
            MemberOrValue::Member(member) => member.at().invoke_method(this, span, args, eval),
            MemberOrValue::Value(value) => value.invoke_with_loc(Some(span), args, eval),
        }
    }
}

#[inline(always)]
pub(crate) fn get_attr_hashed_raw<'v>(
    x: Value<'v>,
    attribute: &Symbol,
    heap: Heap<'v>,
) -> crate::Result<MemberOrValue<'v>> {
    let aref = x.get_ref();
    if let Some(methods) = aref.vtable().methods() {
        if let Some(v) = methods.get_frozen_symbol(attribute) {
            return Ok(MemberOrValue::Member(v));
        }
    }
    match aref.get_attr_hashed(attribute.as_str_hashed(), heap) {
        None => Err(get_attr_no_attr_error(x, attribute)),
        Some(x) => Ok(MemberOrValue::Value(x)),
    }
}

pub(crate) fn get_attr_hashed_bind<'v>(
    x: Value<'v>,
    attribute: &Symbol,
    heap: Heap<'v>,
) -> crate::Result<Value<'v>> {
    let aref = x.get_ref();
    if let Some(methods) = aref.vtable().methods() {
        if let Some(v) = methods.get_frozen_symbol(attribute) {
            return v.at().bind(x, heap);
        }
    }
    match aref.get_attr_hashed(attribute.as_str_hashed(), heap) {
        None => Err(get_attr_no_attr_error(x, attribute)),
        Some(x) => {
            // Only `get_methods` is allowed to return unbound methods or attributes.
            // Both types are crate private, so we assume `get_attr` never returns them.
            Ok(x)
        }
    }
}

impl<'v, 'a, 'e, 'fm> Compiler<'v, 'a, 'e, '_, 'fm> {
    fn expr_ident(&mut self, ident: &CstIdent<'fm>) -> ExprCompiled<'fm> {
        let resolved_ident = ident
            .node
            .payload
            .as_ref()
            .unwrap_or_else(|| panic!("variable not resolved: `{}`", ident.node.ident));
        match resolved_ident {
            ResolvedIdent::Slot(Slot::Local(slot), binding_id) => {
                let binding = self.scope_data.get_binding(*binding_id);

                // We can't look up the local variabless in advance, because they are different each time
                // we go through a new function call.
                match binding.captured {
                    Captured::Yes => ExprCompiled::LocalCaptured(LocalCapturedSlotId(slot.0)),
                    Captured::No => ExprCompiled::Local(LocalSlotId(slot.0)),
                }
            }
            ResolvedIdent::Slot(Slot::Module(slot), binding_id) => {
                let binding = self.scope_data.get_binding(*binding_id);

                // We can only inline variables if they were assigned once
                // otherwise we might inline the wrong value.
                if binding.assign_count == AssignCount::AtMostOnce {
                    if let Some(v) = self.eval.module_env.slots().get_slot(*slot) {
                        // We could inline non-frozen values, but these values
                        // can be garbage-collected, so it is somewhat harder to implement.
                        if let Some(v) = self.opt_ctx().demote(v) {
                            return ExprCompiled::Value(v);
                        }
                    }
                }

                ExprCompiled::Module(*slot)
            }
            ResolvedIdent::Global(v) => ExprCompiled::Value(*v),
        }
    }

    fn opt_ctx<'s>(&'s mut self) -> OptCtx<'v, 'a, 'e, 's, 'fm> {
        let param_count = self.current_scope().param_count();
        OptCtx::new(self, param_count)
    }

    pub(crate) fn expr(
        &mut self,
        expr: &CstExpr<'fm>,
    ) -> Result<IrSpanned<'fm, ExprCompiled<'fm>>, CompilerInternalError> {
        // println!("compile {}", expr.node);
        let span = FrameSpan::new(HeapFileSpan::new(self.codemap, expr.span));
        let expr = match &expr.node {
            ExprP::Identifier(ident) => self.expr_ident(ident),
            ExprP::Lambda(l) => {
                let signature_span = l.signature_span();
                let signature_span = HeapFileSpan::new(self.codemap, signature_span);
                let LambdaP {
                    params,
                    body,
                    payload: scope_id,
                } = l;
                let suite = Spanned {
                    span: expr.span,
                    // TODO(nga): unnecessary clone.
                    node: StmtP::Return(Some(*body.clone())),
                };
                self.function("lambda", signature_span, *scope_id, params, None, &suite)?
            }
            ExprP::Tuple(exprs) => {
                let xs = self.exprs(exprs)?;
                ExprCompiled::tuple(xs, self.fh)
            }
            ExprP::List(exprs) => {
                let xs = self.exprs(exprs)?;
                ExprCompiled::List(xs)
            }
            ExprP::Dict(exprs) => {
                let xs = exprs.try_map(|(k, v)| Ok((self.expr(k)?, self.expr(v)?)))?;
                ExprCompiled::Dict(xs)
            }
            ExprP::If(cond_then_expr_else_expr) => {
                let (cond, then_expr, else_expr) = &**cond_then_expr_else_expr;
                let cond = self.expr(cond)?;
                let then_expr = self.expr(then_expr)?;
                let else_expr = self.expr(else_expr)?;
                return Ok(ExprCompiled::if_expr(cond, then_expr, else_expr));
            }
            ExprP::Dot(left, right) => {
                let left = self.expr(left)?;
                let s = Symbol::new(&right.node);

                ExprCompiled::dot(left, &s, &mut self.opt_ctx())
            }
            ExprP::Call(left, args) => {
                let left = self.expr(left)?;
                let args = self.args(args)?;
                CallCompiled::call(span, left, args, &mut self.opt_ctx())
            }
            ExprP::Index(array_index) => {
                let (array, index) = &**array_index;
                let array = self.expr(array)?;
                let index = self.expr(index)?;
                ExprCompiled::index(array, index, &mut self.opt_ctx())
            }
            ExprP::Index2(array_index0_index1) => {
                let (array, index0, index1) = &**array_index0_index1;
                let array = self.expr(array)?;
                let index0 = self.expr(index0)?;
                let index1 = self.expr(index1)?;
                ExprCompiled::index2(array, index0, index1)
            }
            ExprP::Slice(collection, start, stop, stride) => {
                let collection = self.expr(collection)?;
                let start = start.as_ref().map(|x| self.expr(x)).transpose()?;
                let stop = stop.as_ref().map(|x| self.expr(x)).transpose()?;
                let stride = stride.as_ref().map(|x| self.expr(x)).transpose()?;
                ExprCompiled::slice(span, collection, start, stop, stride, &mut self.opt_ctx())
            }
            ExprP::Not(expr) => {
                let expr = self.expr(expr)?;
                return Ok(ExprCompiled::not(span, expr));
            }
            ExprP::Minus(expr) => {
                let expr = self.expr(expr)?;
                ExprCompiled::un_op(span, &Builtin1::Minus, expr, &mut self.opt_ctx())
            }
            ExprP::Plus(expr) => {
                let expr = self.expr(expr)?;
                ExprCompiled::un_op(span, &Builtin1::Plus, expr, &mut self.opt_ctx())
            }
            ExprP::BitNot(expr) => {
                let expr = self.expr(expr)?;
                ExprCompiled::un_op(span, &Builtin1::BitNot, expr, &mut self.opt_ctx())
            }
            ExprP::Op(left, op, right) => {
                if let Some(x) = ExprP::reduces_to_string(*op, left, right) {
                    // Note there's const propagation for `+` on compiled expressions,
                    // but special handling of `+` on AST might be slightly more efficient
                    // (no unnecessary allocations on the heap). So keep it.
                    let val = self.fh.alloc(x).to_value();
                    ExprCompiled::Value(val)
                } else {
                    let right = if *op == BinOp::In || *op == BinOp::NotIn {
                        list_to_tuple(right)
                    } else {
                        Cow::Borrowed(&**right)
                    };

                    let l = self.expr(left)?;
                    let r = self.expr(&right)?;
                    match op {
                        BinOp::Or => return Ok(ExprCompiled::or(l, r)),
                        BinOp::And => return Ok(ExprCompiled::and(l, r)),
                        BinOp::Equal => return Ok(ExprCompiled::equals(l, r)),
                        BinOp::NotEqual => {
                            return Ok(ExprCompiled::not(span, ExprCompiled::equals(l, r)));
                        }
                        BinOp::Less => ExprCompiled::bin_op(
                            Builtin2::Compare(CompareOp::Less),
                            l,
                            r,
                            &mut self.opt_ctx(),
                        ),
                        BinOp::Greater => ExprCompiled::bin_op(
                            Builtin2::Compare(CompareOp::Greater),
                            l,
                            r,
                            &mut self.opt_ctx(),
                        ),
                        BinOp::LessOrEqual => ExprCompiled::bin_op(
                            Builtin2::Compare(CompareOp::LessOrEqual),
                            l,
                            r,
                            &mut self.opt_ctx(),
                        ),
                        BinOp::GreaterOrEqual => ExprCompiled::bin_op(
                            Builtin2::Compare(CompareOp::GreaterOrEqual),
                            l,
                            r,
                            &mut self.opt_ctx(),
                        ),
                        BinOp::In => ExprCompiled::bin_op(Builtin2::In, l, r, &mut self.opt_ctx()),
                        BinOp::NotIn => {
                            ExprCompiled::not(
                                span,
                                IrSpanned {
                                    span,
                                    node: ExprCompiled::bin_op(
                                        Builtin2::In,
                                        l,
                                        r,
                                        &mut self.opt_ctx(),
                                    ),
                                },
                            )
                            .node
                        }
                        BinOp::Subtract => {
                            ExprCompiled::bin_op(Builtin2::Sub, l, r, &mut self.opt_ctx())
                        }
                        BinOp::Add => {
                            ExprCompiled::bin_op(Builtin2::Add, l, r, &mut self.opt_ctx())
                        }
                        BinOp::Multiply => {
                            ExprCompiled::bin_op(Builtin2::Multiply, l, r, &mut self.opt_ctx())
                        }
                        BinOp::Percent => {
                            ExprCompiled::bin_op(Builtin2::Percent, l, r, &mut self.opt_ctx())
                        }
                        BinOp::Divide => {
                            ExprCompiled::bin_op(Builtin2::Divide, l, r, &mut self.opt_ctx())
                        }
                        BinOp::FloorDivide => {
                            ExprCompiled::bin_op(Builtin2::FloorDivide, l, r, &mut self.opt_ctx())
                        }
                        BinOp::BitAnd => {
                            ExprCompiled::bin_op(Builtin2::BitAnd, l, r, &mut self.opt_ctx())
                        }
                        BinOp::BitOr => {
                            ExprCompiled::bin_op(Builtin2::BitOr, l, r, &mut self.opt_ctx())
                        }
                        BinOp::BitXor => {
                            ExprCompiled::bin_op(Builtin2::BitXor, l, r, &mut self.opt_ctx())
                        }
                        BinOp::LeftShift => {
                            ExprCompiled::bin_op(Builtin2::LeftShift, l, r, &mut self.opt_ctx())
                        }
                        BinOp::RightShift => {
                            ExprCompiled::bin_op(Builtin2::RightShift, l, r, &mut self.opt_ctx())
                        }
                    }
                }
            }
            ExprP::ListComprehension(x, for_, clauses) => {
                self.list_comprehension(x, for_, clauses)?
            }
            ExprP::DictComprehension(k_v, for_, clauses) => {
                let (k, v) = &**k_v;
                self.dict_comprehension(k, v, for_, clauses)?
            }
            ExprP::Literal(x) => {
                let val = x.compile(self.fh);
                ExprCompiled::Value(val)
            }
            ExprP::FString(fstring) => {
                let Spanned {
                    node:
                        FStringP {
                            format,
                            expressions,
                        },
                    span: fstring_span,
                } = fstring;

                let fstring_span = FrameSpan::new(HeapFileSpan::new(self.codemap, *fstring_span));

                // Desugar f"foo{x}bar{y}" to "foo{}bar{}.format(x, y)"
                let format = IrSpanned {
                    node: ExprCompiled::Value(self.fh.alloc(format.node.as_str()).to_value()),
                    span: fstring_span,
                };
                let method = IrSpanned {
                    node: ExprCompiled::dot(format, &Symbol::new("format"), &mut self.opt_ctx()),
                    span: fstring_span,
                };

                let mut args = ArgsCompiledValue::default();
                args.reserve_pos_exact(expressions.len());
                for expr in expressions {
                    args.push_pos(self.expr(expr)?);
                }

                CallCompiled::call(span, method, args, &mut self.opt_ctx())
            }
        };
        Ok(IrSpanned { node: expr, span })
    }

    /// Like `expr` but returns an expression optimized assuming
    /// only the truth of the result is needed.
    pub(crate) fn expr_truth(
        &mut self,
        expr: &CstExpr<'fm>,
    ) -> Result<IrSpanned<'fm, ExprCompiledBool<'fm>>, CompilerInternalError> {
        let expr = self.expr(expr)?;
        Ok(ExprCompiledBool::new(expr))
    }

    pub(crate) fn exprs(
        &mut self,
        exprs: &[CstExpr<'fm>],
    ) -> Result<Vec<IrSpanned<'fm, ExprCompiled<'fm>>>, CompilerInternalError> {
        exprs.try_map(|e| self.expr(e))
    }
}
