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

pub(crate) mod args;
pub(crate) mod call;
pub(crate) mod compr;
pub(crate) mod constants;
pub(crate) mod def;
pub(crate) mod def_inline;
pub(crate) mod error;
pub(crate) mod expr;
pub(crate) mod expr_bool;
pub(crate) mod known;
pub(crate) mod module;
pub(crate) mod opt_ctx;
pub(crate) mod scope;
pub(crate) mod small_vec_1;
pub(crate) mod span;
pub(crate) mod stmt;
pub(crate) mod types;

use starlark_syntax::eval_exception::EvalException;

use crate::codemap::CodeMap;
use crate::environment::Globals;
use crate::eval::Evaluator;
use crate::eval::compiler::def_inline::local_as_value::LocalAsValue;
use crate::eval::compiler::scope::ModuleScopeData;
use crate::eval::compiler::scope::ScopeId;
use crate::eval::compiler::scope::ScopeNames;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::values::FrozenHeap;
use crate::values::HeapEdge;
use crate::values::SealEdge;
use crate::values::ValueTyped;
use crate::values::any::StarlarkAny;

#[cold]
#[inline(never)]
pub(crate) fn add_span_to_expr_error<'v>(
    e: crate::Error,
    span: FrameSpan<'v>,
    eval: &Evaluator<'v, '_, '_>,
) -> EvalException {
    EvalException::new_with_callstack(e, span.span.span(), &span.span.file(), || {
        eval.call_stack.to_diagnostic_frames(span.inlined_frames)
    })
}

/// Convert syntax error to spanned evaluation exception
#[inline(always)]
pub(crate) fn expr_throw<'v, T>(
    r: crate::Result<T>,
    span: FrameSpan<'v>,
    eval: &Evaluator<'v, '_, '_>,
) -> Result<T, EvalException> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => Err(add_span_to_expr_error(e, span, eval)),
    }
}

/// Convert syntax error to spanned evaluation exception
#[inline(always)]
pub(crate) fn expr_throw_starlark_result<'v, T>(
    r: crate::Result<T>,
    span: FrameSpan<'v>,
    eval: &Evaluator<'v, '_, '_>,
) -> Result<T, EvalException> {
    match r {
        Ok(v) => Ok(v),
        Err(e) => Err(add_span_to_expr_error(e, span, eval)),
    }
}

/// The compiler of one module: it compiles the module's statements one at a time and executes
/// each before compiling the next.
///
/// Its products - the IR, the bytecode, constants, names - are allocated on the module's frozen
/// heap at its brand `'fm`; the evaluator runs at the value heap's brand `'v`. `edge` brings a
/// product to `'v` when it is handed to execution, which happens once per top-level statement,
/// see `eval_regular_top_level_stmt`; `seal_edge` brings frozen values the optimizer observes at
/// `'v` into the IR, see [`OptCtx::demote`](crate::eval::compiler::opt_ctx::OptCtx::demote).
pub(crate) struct Compiler<'v, 'a, 'e, 'x, 'fm> {
    pub(crate) eval: &'x mut Evaluator<'v, 'a, 'e>,
    pub(crate) fh: FrozenHeap<'fm>,
    pub(crate) edge: HeapEdge<'v, 'fm>,
    pub(crate) seal_edge: SealEdge<'fm, 'v>,
    pub(crate) scope_data: ModuleScopeData<'fm>,
    pub(crate) locals: Vec<ScopeId>,
    pub(crate) globals: ValueTyped<'fm, StarlarkAny<Globals>>,
    pub(crate) codemap: ValueTyped<'fm, StarlarkAny<CodeMap>>,
    pub(crate) check_types: bool,
    pub(crate) top_level_stmt_count: usize,
    /// Set with `@starlark-rust: typecheck`.
    pub(crate) typecheck: bool,
    /// See [`OptCtx::local_as_values`](crate::eval::compiler::opt_ctx::OptCtx::local_as_values).
    pub(crate) local_as_values: Vec<ValueTyped<'fm, LocalAsValue>>,
}

impl<'fm> Compiler<'_, '_, '_, '_, 'fm> {
    pub(crate) fn enter_scope(&mut self, scope_id: ScopeId) {
        self.locals.push(scope_id);
    }

    pub(crate) fn exit_scope(&mut self) -> ScopeId {
        self.locals.pop().unwrap()
    }

    pub(crate) fn current_scope(&self) -> &ScopeNames<'fm> {
        self.scope_data.get_scope(*self.locals.last().unwrap())
    }
}
