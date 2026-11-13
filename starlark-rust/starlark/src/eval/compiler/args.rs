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

use starlark_derive::StarlarkPagable;
use starlark_derive::VisitSpanMut;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::syntax::ast::ArgumentP;
use starlark_syntax::syntax::ast::CallArgsP;

use crate as starlark;
use crate::collections::symbol::symbol::Symbol;
use crate::eval::Arguments;
use crate::eval::compiler::Compiler;
use crate::eval::compiler::error::CompilerInternalError;
use crate::eval::compiler::expr::ExprCompiled;
use crate::eval::compiler::opt_ctx::OptCtx;
use crate::eval::compiler::scope::payload::CstPayload;
use crate::eval::compiler::span::IrSpanned;
use crate::eval::runtime::arguments::ArgNames;
use crate::eval::runtime::arguments::ArgumentsFull;
use crate::values::HeapEdge;
use crate::values::StringValue;
use crate::values::Value;

#[derive(Default, Clone, Debug, VisitSpanMut, StarlarkPagable)]
pub(crate) struct ArgsCompiledValue<'f> {
    pub(crate) pos_named: Vec<IrSpanned<'f, ExprCompiled<'f>>>,
    /// Named arguments compiled.
    ///
    /// Note names are guaranteed to be unique here because names are validated in AST:
    /// named arguments in [`Expr::Call`] are unique.
    pub(crate) names: Vec<(Symbol, StringValue<'f>)>,
    pub(crate) args: Option<IrSpanned<'f, ExprCompiled<'f>>>,
    pub(crate) kwargs: Option<IrSpanned<'f, ExprCompiled<'f>>>,
}

impl<'f> ArgsCompiledValue<'f> {
    /// Check if arguments is one positional argument.
    pub(crate) fn one_pos(&self) -> Option<&IrSpanned<'f, ExprCompiled<'f>>> {
        let ArgsCompiledValue {
            pos_named,
            names,
            args,
            kwargs,
        } = self;
        match (pos_named.as_slice(), names.as_slice(), args, kwargs) {
            ([pos], [], None, None) => Some(pos),
            _ => None,
        }
    }

    /// Check if arguments is two positional arguments.
    pub(crate) fn two_pos(
        &self,
    ) -> Option<(
        &IrSpanned<'f, ExprCompiled<'f>>,
        &IrSpanned<'f, ExprCompiled<'f>>,
    )> {
        let ArgsCompiledValue {
            pos_named,
            names,
            args,
            kwargs,
        } = self;
        match (pos_named.as_slice(), names.as_slice(), args, kwargs) {
            ([pos0, pos1], [], None, None) => Some((pos0, pos1)),
            _ => None,
        }
    }

    pub(crate) fn pos_only(&self) -> Option<&[IrSpanned<'f, ExprCompiled<'f>>]> {
        if self.names.is_empty() && self.args.is_none() && self.kwargs.is_none() {
            Some(&self.pos_named)
        } else {
            None
        }
    }

    fn split_pos_names(
        &self,
    ) -> (
        &[IrSpanned<'f, ExprCompiled<'f>>],
        &[IrSpanned<'f, ExprCompiled<'f>>],
    ) {
        self.pos_named
            .as_slice()
            .split_at(self.pos_named.len() - self.names.len())
    }

    /// Invoke a callback if all arguments are constants, with the arguments brought to the heap
    /// `edge` leads to.
    pub(crate) fn all_values<'v, R>(
        &self,
        edge: HeapEdge<'v, 'f>,
        handler: impl FnOnce(&Arguments<'v, '_>) -> R,
    ) -> Option<R> {
        self.all_values_generic(edge, |e| e.as_value().map(|v| edge.rebrand(v)), handler)
    }

    /// Invoke a callback if all arguments are values under `expr_to_value`; `edge` brings the
    /// argument names to the heap the values are at.
    pub(crate) fn all_values_generic<'v, R>(
        &self,
        edge: HeapEdge<'v, 'f>,
        expr_to_value: impl Fn(&ExprCompiled<'f>) -> Option<Value<'v>>,
        handler: impl FnOnce(&Arguments<'v, '_>) -> R,
    ) -> Option<R> {
        let (pos, named) = self.split_pos_names();
        let pos = pos.try_map(|e| expr_to_value(e).ok_or(())).ok()?;
        let named = named.try_map(|e| expr_to_value(e).ok_or(())).ok()?;
        let args = self
            .args
            .as_ref()
            .map(|args| expr_to_value(args).ok_or(()))
            .transpose()
            .ok()?;
        let kwargs = self
            .kwargs
            .as_ref()
            .map(|kwargs| expr_to_value(kwargs).ok_or(()))
            .transpose()
            .ok()?;
        Some(handler(&Arguments(ArgumentsFull {
            pos: &pos,
            named: &named,
            names: ArgNames::new_unique(edge.rebrand_ref(&self.names)),
            args,
            kwargs,
        })))
    }

    /// Expressions of all arguments: positional, named, star-args, star-star-args.
    pub(crate) fn arg_exprs(&self) -> impl Iterator<Item = &IrSpanned<'f, ExprCompiled<'f>>> {
        self.pos_named
            .iter()
            .chain(self.args.iter())
            .chain(self.kwargs.iter())
    }

    pub(crate) fn map_exprs<E>(
        &self,
        mut f: impl FnMut(
            &IrSpanned<'f, ExprCompiled<'f>>,
        ) -> Result<IrSpanned<'f, ExprCompiled<'f>>, E>,
    ) -> Result<ArgsCompiledValue<'f>, E> {
        let ArgsCompiledValue {
            pos_named,
            names,
            args,
            kwargs,
        } = self;
        Ok(ArgsCompiledValue {
            pos_named: pos_named.try_map(&mut f)?,
            names: names.clone(),
            args: args.as_ref().map(&mut f).transpose()?,
            kwargs: kwargs.as_ref().map(&mut f).transpose()?,
        })
    }

    pub(crate) fn optimize(&self, ctx: &mut OptCtx<'_, '_, '_, '_, 'f>) -> ArgsCompiledValue<'f> {
        enum Never {}
        self.map_exprs(|e| Ok(e.optimize(ctx)))
            .unwrap_or_else(|e: Never| match e {})
    }

    pub(crate) fn push_pos(&mut self, expr: IrSpanned<'f, ExprCompiled<'f>>) {
        self.pos_named.push(expr)
    }

    pub(crate) fn reserve_pos_exact(&mut self, n: usize) {
        self.pos_named.reserve_exact(n);
    }
}

impl<'fm> Compiler<'_, '_, '_, '_, 'fm> {
    pub(crate) fn args(
        &mut self,
        args: &CallArgsP<CstPayload<'fm>>,
    ) -> Result<ArgsCompiledValue<'fm>, CompilerInternalError> {
        let (pos_named_count, named_count) =
            args.args
                .iter()
                .fold((0, 0), |(pos_named, named), arg| match &arg.node {
                    ArgumentP::Positional(_) => (pos_named + 1, named),
                    ArgumentP::Named(..) => (pos_named + 1, named + 1),
                    ArgumentP::Args(_) | ArgumentP::KwArgs(_) => (pos_named, named),
                });
        let mut res = ArgsCompiledValue::default();
        res.pos_named.reserve_exact(pos_named_count);
        res.names.reserve_exact(named_count);
        for x in &args.args {
            match &x.node {
                ArgumentP::Positional(x) => res.pos_named.push(self.expr(x)?),
                ArgumentP::Named(name, value) => {
                    let fv = self.fh.alloc(name.node.as_str());
                    res.names.push((Symbol::new(&name.node), fv));
                    res.pos_named.push(self.expr(value)?);
                }
                ArgumentP::Args(x) => res.args = Some(self.expr(x)?),
                ArgumentP::KwArgs(x) => res.kwargs = Some(self.expr(x)?),
            }
        }
        // Any drift between the pre-counts and the pushes above silently
        // reintroduces doubling slack.
        debug_assert_eq!(pos_named_count, res.pos_named.len());
        debug_assert_eq!(named_count, res.names.len());
        Ok(res)
    }
}
