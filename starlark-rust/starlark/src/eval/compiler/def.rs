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

//! Implementation of `def`.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Display;
use std::fmt::Write;
use std::ptr;

use allocative::Allocative;
use derivative::Derivative;
use dupe::Dupe;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::VisitSpanMut;
use starlark_derive::starlark_value;
use starlark_map::StarlarkHasher;
use starlark_syntax::eval_exception::EvalException;
use starlark_syntax::internal_error;
use starlark_syntax::slice_vec_ext::SliceExt;
use starlark_syntax::syntax::def::DefParam;
use starlark_syntax::syntax::def::DefParamIndices;
use starlark_syntax::syntax::def::DefParamKind;
use starlark_syntax::syntax::def::DefParams;
use triomphe::Arc;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::codemap::CodeMap;
use crate::codemap::Spanned;
use crate::collections::Hashed;
use crate::const_frozen_string;
use crate::docs::DocFunction;
use crate::docs::DocItem;
use crate::docs::DocMember;
use crate::docs::DocString;
use crate::docs::DocStringKind;
use crate::environment::FrozenModuleData;
use crate::environment::FrozenModuleValue;
use crate::environment::Globals;
use crate::eval::Arguments;
use crate::eval::bc::bytecode::Bc;
use crate::eval::bc::frame::alloca_frame;
use crate::eval::compiler::Compiler;
use crate::eval::compiler::def_inline::InlineDefBody;
use crate::eval::compiler::def_inline::inline_def_body;
use crate::eval::compiler::error::CompilerInternalError;
use crate::eval::compiler::expr::ExprCompiled;
use crate::eval::compiler::opt_ctx::OptCtx;
use crate::eval::compiler::scope::Captured;
use crate::eval::compiler::scope::ScopeId;
use crate::eval::compiler::scope::payload::CstAssignIdent;
use crate::eval::compiler::scope::payload::CstParameter;
use crate::eval::compiler::scope::payload::CstPayload;
use crate::eval::compiler::scope::payload::CstStmt;
use crate::eval::compiler::scope::payload::CstTypeExpr;
use crate::eval::compiler::span::IrSpanned;
use crate::eval::compiler::stmt::OptimizeOnFreezeContext;
use crate::eval::compiler::stmt::StmtCompileContext;
use crate::eval::compiler::stmt::StmtsCompiled;
use crate::eval::runtime::arguments::ArgumentsImpl;
use crate::eval::runtime::arguments::ResolvedArgName;
use crate::eval::runtime::evaluator::Evaluator;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::eval::runtime::heap_file_span::HeapFileSpan;
use crate::eval::runtime::params::spec::ParametersSpec;
use crate::eval::runtime::params::spec::ParametersSpecPrototype;
use crate::eval::runtime::profile::instant::ProfilerInstant;
use crate::eval::runtime::slots::LocalSlotId;
use crate::eval::runtime::slots::LocalSlotIdCapturedOrNot;
use crate::register_starlark_any;
use crate::register_starlark_any_complex;
use crate::static_starlark_value;
use crate::typing::ParamSpec;
use crate::typing::Ty;
use crate::typing::callable_param::ParamIsRequired;
use crate::util::arc_str::ArcStr;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::FrozenValueTyped;
use crate::values::Heap;
use crate::values::HeapEdge;
use crate::values::SealEdge;
use crate::values::StarlarkValue;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::any::StarlarkAny;
use crate::values::any_complex::StarlarkAnyComplex;
use crate::values::function::FUNCTION_TYPE;
use crate::values::layout::typed::AtomicFrozenValueTypedOption;
use crate::values::types::any_array::AnyArray;
use crate::values::typing::type_compiled::compiled::TypeCompiled;

static_starlark_value!(VALUE_EMPTY_PARAMETER_CAPTURES: AnyArray<LocalSlotId> = AnyArray::empty());

#[derive(thiserror::Error, Debug)]
enum DefError {
    #[error("Function has no type, while function was compiled with return type (internal error)")]
    CheckReturnTypeNoType,
}

/// The body a frozen def runs: [`DefInfo::stmt_compiled`] re-optimized against the fully frozen
/// module. `post_freeze` fills it. While it is empty - before freeze, or for a def frozen by a
/// [`Freezer`] that never runs `post_freeze` - the def runs [`DefInfo::stmt_compiled`] itself.
#[derive(ProvidesStaticType)]
struct StmtCompiledCell<'v> {
    cell: UnsafeCell<Option<Bc<'v>>>,
}

// `Option<Bc>` uses a niche in `Bc`, so the empty state costs the def no space.
static_assertions::assert_eq_size!(Option<Bc<'static>>, Bc<'static>);

unsafe impl<'v> Trace<'v> for StmtCompiledCell<'v> {
    fn trace(&mut self, _: &Tracer<'v>) {
        // Bytecode contains only frozen values.
    }
}

// SAFETY: The cell is written once, by `post_freeze`, before the frozen def is reachable from any
// other thread (see `set` and `get`), so sharing it is sharing the `Bc<'static>` inside, which is
// `Sync` like the contents of every frozen value. At any other brand the def is unfrozen and
// belongs to one thread, like every `Value<'v>`.
unsafe impl Sync for StmtCompiledCell<'static> {}

impl<'v> StmtCompiledCell<'v> {
    fn new() -> StmtCompiledCell<'v> {
        StmtCompiledCell {
            cell: UnsafeCell::new(None),
        }
    }

    /// This function is unsafe if other thread is executing the stmt.
    unsafe fn set(&self, value: Bc<'v>) {
        unsafe {
            ptr::drop_in_place(self.cell.get());
            ptr::write(self.cell.get(), Some(value));
        }
    }

    fn get(&self) -> Option<&Bc<'v>> {
        // SAFETY: The only writer is `set`, which runs once, in `post_freeze`, before the frozen
        // def is reachable by anything that could call `get`; so no write overlaps this read.
        unsafe { (*self.cell.get()).as_ref() }
    }
}

// Only `post_freeze` fills the cell, after the def has been frozen, so the cell of a def being
// frozen is empty; a frozen def is never frozen again, only re-typed at another brand.
impl<'v> Freeze<'v> for StmtCompiledCell<'v> {
    type Frozen<'fv> = StmtCompiledCell<'fv>;

    fn freeze<'fv>(self, _freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        debug_assert!(self.get().is_none());
        Ok(StmtCompiledCell::new())
    }
}

#[derive(Clone, Debug, VisitSpanMut, StarlarkPagable)]
pub(crate) struct ParameterName {
    pub(crate) name: String,
    captured: Captured,
}

#[derive(Clone, Debug, VisitSpanMut, StarlarkPagable)]
pub(crate) enum ParameterCompiled<'f, T> {
    Normal(
        /// Name.
        ParameterName,
        /// Type.
        Option<TypeCompiled<'f>>,
        /// Default value.
        Option<T>,
    ),
    Args(ParameterName, Option<TypeCompiled<'f>>),
    KwArgs(ParameterName, Option<TypeCompiled<'f>>),
}

impl<'f, T> ParameterCompiled<'f, T> {
    pub(crate) fn map_expr<U>(&self, f: impl FnMut(&T) -> U) -> ParameterCompiled<'f, U> {
        match self {
            ParameterCompiled::Normal(n, o, t) => {
                ParameterCompiled::Normal(n.clone(), *o, t.as_ref().map(f))
            }
            ParameterCompiled::Args(n, o) => ParameterCompiled::Args(n.clone(), *o),
            ParameterCompiled::KwArgs(n, o) => ParameterCompiled::KwArgs(n.clone(), *o),
        }
    }

    pub(crate) fn captured(&self) -> Captured {
        self.name_ty().0.captured
    }

    pub(crate) fn name_ty(&self) -> (&ParameterName, Option<TypeCompiled<'f>>) {
        match self {
            Self::Normal(n, t, _) => (n, *t),
            Self::Args(n, t) => (n, *t),
            Self::KwArgs(n, t) => (n, *t),
        }
    }

    pub(crate) fn has_type(&self) -> bool {
        match self.name_ty() {
            (_, Some(_)) => true,
            _ => false,
        }
    }

    pub(crate) fn ty(&self) -> Ty {
        match self.name_ty() {
            (_, Some(t)) => t.as_ty().clone(),
            (_, None) => Ty::any(),
        }
    }

    pub(crate) fn required(&self) -> ParamIsRequired {
        match self {
            ParameterCompiled::Normal(_, _, None) => ParamIsRequired::Yes,
            ParameterCompiled::Normal(_, _, Some(_)) => ParamIsRequired::No,
            ParameterCompiled::Args(..) => ParamIsRequired::No,
            ParameterCompiled::KwArgs(..) => ParamIsRequired::No,
        }
    }

    pub(crate) fn is_star_or_star_star(&self) -> bool {
        matches!(
            self,
            ParameterCompiled::Args(_, _) | ParameterCompiled::KwArgs(_, _)
        )
    }
}

#[derive(Debug, Clone, VisitSpanMut, StarlarkPagable)]
pub(crate) struct ParametersCompiled<'f, T> {
    pub(crate) params: Vec<IrSpanned<'f, ParameterCompiled<'f, T>>>,
    #[starlark_pagable(pagable)]
    pub(crate) indices: DefParamIndices,
    #[starlark_pagable(pagable)]
    param_spec_prototype: Arc<ParametersSpecPrototype>,
}

impl<'f, T> ParametersCompiled<'f, T> {
    pub(crate) fn new(
        function_name: ArcStr,
        params: Vec<IrSpanned<'f, ParameterCompiled<'f, T>>>,
        indices: DefParamIndices,
    ) -> ParametersCompiled<'f, T> {
        let mut builder = ParametersSpec::<()>::with_capacity(function_name, params.len());
        for (i, x) in params.iter().enumerate() {
            let i = i as u32;

            if i == indices.num_positional_only && !x.is_star_or_star_star() {
                builder.no_more_positional_only_args();
            }

            if i == indices.num_positional && !x.is_star_or_star_star() {
                builder.no_more_positional_args();
            }

            match &x.node {
                ParameterCompiled::Normal(n, _, None) => builder.required(&n.name),
                ParameterCompiled::Normal(n, _, Some(_)) => builder.defaulted(&n.name, ()),
                ParameterCompiled::Args(n, _) => builder.args(&n.name),
                ParameterCompiled::KwArgs(n, _) => builder.kwargs(&n.name),
            }
        }
        ParametersCompiled {
            params,
            indices,
            param_spec_prototype: builder.finish_prototype(),
        }
    }

    pub(crate) fn param_spec_prototype(&self) -> Arc<ParametersSpecPrototype> {
        self.param_spec_prototype.clone()
    }

    pub(crate) fn map_exprs<U>(&self, mut f: impl FnMut(&T) -> U) -> ParametersCompiled<'f, U> {
        ParametersCompiled {
            params: self.params.map(|p| p.map(|p| p.map_expr(&mut f))),
            indices: self.indices,
            param_spec_prototype: self.param_spec_prototype.clone(),
        }
    }

    /// How many expressions this parameters references (default values and types).
    pub(crate) fn count_exprs(&self) -> u32 {
        let mut count = 0;
        for p in &self.params {
            p.map_expr(|_e| count += 1);
        }
        count
    }

    /// How many parameter variables?
    pub(crate) fn count_param_variables(&self) -> u32 {
        self.params.len().try_into().unwrap()
    }

    /// Any parameter has type annotation?
    pub(crate) fn has_types(&self) -> bool {
        self.params.iter().any(|p| p.has_type())
    }

    /// Has `*args` or `*kwargs` parameter? `*` is fine.
    pub(crate) fn has_args_or_kwargs(&self) -> bool {
        self.params.iter().any(|p| {
            matches!(
                p.node,
                ParameterCompiled::Args(..) | ParameterCompiled::KwArgs(..)
            )
        })
    }

    /// The type-annotated parameters, for [`DefInfo::parameter_types`].
    pub(crate) fn parameter_types(&self, heap: FrozenHeap<'f>) -> Vec<ParameterTypeCompiled<'f>> {
        self.params
            .iter()
            .enumerate()
            .filter_map(|(i, x)| {
                let (name, ty) = x.name_ty();
                ty.map(|ty| ParameterTypeCompiled {
                    slot: LocalSlotId(i as u32),
                    name: heap.alloc(name.name.as_str()),
                    ty,
                })
            })
            .collect()
    }

    pub(crate) fn parameter_captures(&self) -> Vec<LocalSlotId> {
        self.params
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                if p.captured() == Captured::Yes {
                    Some(LocalSlotId(i as u32))
                } else {
                    None
                }
            })
            .collect()
    }

    pub(crate) fn to_ty_params(&self) -> ParamSpec {
        ParamSpec::new_parts(
            self.indices.pos_only().map(|i| {
                let p = &self.params[i].node;
                (p.required(), p.ty())
            }),
            self.indices.pos_or_named().map(|i| {
                let p = &self.params[i].node;
                (
                    ArcStr::from(p.name_ty().0.name.as_str()),
                    p.required(),
                    p.ty(),
                )
            }),
            self.indices.args.map(|i| {
                let p = &self.params[i as usize].node;
                p.ty()
            }),
            self.indices.named_only(self.params.len()).map(|i| {
                let p = &self.params[i].node;
                (
                    ArcStr::from(p.name_ty().0.name.as_str()),
                    p.required(),
                    p.ty(),
                )
            }),
            self.indices.kwargs.map(|i| {
                let p = &self.params[i as usize].node;
                p.ty()
            }),
        )
        // TODO(nga): do not unwrap.
        .unwrap()
    }
}

/// Type annotation on a parameter: shared per def site in
/// [`DefInfo::parameter_types`], sparse, in parameter order.
#[derive(Debug, Clone, Copy, Dupe, StarlarkPagable)]
pub(crate) struct ParameterTypeCompiled<'f> {
    /// The parameter's local slot, which equals its parameter index.
    pub(crate) slot: LocalSlotId,
    /// Parameter name, for error messages.
    pub(crate) name: StringValue<'f>,
    pub(crate) ty: TypeCompiled<'f>,
}

/// Copy local variable slot to nested function.
#[derive(
    Debug,
    Clone,
    Dupe,
    pagable::Pagable,
    starlark_derive::StarlarkPagableViaPagable
)]
pub(crate) struct CopySlotFromParent {
    /// Slot in the outer function.
    pub(crate) parent: LocalSlotIdCapturedOrNot,
    /// Slot in the nested function.
    pub(crate) child: LocalSlotIdCapturedOrNot,
}

/// Static info for `def`, `lambda` or module: a compiler product in the module's frozen heap, at
/// its brand, allocated as a `StarlarkAnyComplex` (see [`DefInfoValue`]).
#[derive(Derivative, Allocative, ProvidesStaticType, StarlarkPagable)]
#[derivative(Debug)]
pub(crate) struct DefInfo<'f> {
    pub(crate) name: StringValue<'f>,
    /// Span of function signature.
    pub(crate) signature_span: HeapFileSpan<'f>,
    /// Indices of parameters, which are captured in nested defs.
    ///
    /// A heap array rather than a box: every [`Def`] copies the handle, see
    /// [`Def::parameter_captures`].
    parameter_captures: ValueTyped<'f, AnyArray<LocalSlotId>>,
    /// Type annotations on parameters, sparse, in parameter order.
    #[allocative(skip)]
    pub(crate) parameter_types: Box<[ParameterTypeCompiled<'f>]>,
    /// Type of this function, for the typechecker.
    #[starlark_pagable(pagable)]
    ty: Ty,
    /// Codemap of the file where the function is declared.
    pub(crate) codemap: ValueTyped<'f, StarlarkAny<CodeMap>>,
    /// The raw docstring pulled out of the AST.
    pub(crate) docstring: Option<String>,
    /// Slots this scope uses, including for parameters and `parent`.
    /// Indexed by [`LocalSlotId`], values are variable names.
    pub(crate) used: Box<[StringValue<'f>]>,
    /// Slots to copy from the parent.
    /// Module-level identifiers are not copied over, to avoid excess copying.
    #[allocative(skip)]
    pub(crate) parent: Box<[CopySlotFromParent]>,
    /// Statement compiled for non-frozen def.
    #[derivative(Debug = "ignore")]
    #[allocative(skip)]
    stmt_compiled: Bc<'f>,
    // The compiled expression for the body of this definition, to be run
    // after the parameters are evaluated.
    #[derivative(Debug = "ignore")]
    #[allocative(skip)]
    body_stmts: StmtsCompiled<'f>,
    /// How to compile the statement on freeze.
    #[allocative(skip)]
    stmt_compile_context: StmtCompileContext,
    /// Function can be inlined.
    #[allocative(skip)]
    pub(crate) inline_def_body: Option<InlineDefBody<'f>>,
    /// Globals captured during function or module creation.
    /// Only needed for debugger evaluation.
    pub(crate) globals: ValueTyped<'f, StarlarkAny<Globals>>,
}

register_starlark_any_complex!(frozen DefInfo<'_>);

/// A [`DefInfo`] as the value it is allocated as, at the brand of the heap it lives in.
pub(crate) type DefInfoValue<'f> = FrozenValueTyped<'f, StarlarkAnyComplex<DefInfo<'f>>>;

impl<'f> DefInfo<'f> {
    pub(crate) fn for_module(
        codemap: ValueTyped<'f, StarlarkAny<CodeMap>>,
        local_names: Box<[StringValue<'f>]>,
        parent: Box<[CopySlotFromParent]>,
        globals: ValueTyped<'f, StarlarkAny<Globals>>,
    ) -> DefInfo<'f> {
        DefInfo {
            name: const_frozen_string!("<module>").at(),
            signature_span: HeapFileSpan::default(),
            parameter_captures: VALUE_EMPTY_PARAMETER_CAPTURES.at(),
            parameter_types: Box::default(),
            ty: Ty::any(),
            codemap,
            docstring: None,
            used: local_names,
            parent,
            stmt_compiled: Bc::default(),
            body_stmts: StmtsCompiled::empty(),
            stmt_compile_context: StmtCompileContext::default(),
            inline_def_body: None,
            globals,
        }
    }
}

#[derive(Clone, Debug, VisitSpanMut, StarlarkPagable)]
pub(crate) struct DefCompiled<'f> {
    pub(crate) params: ParametersCompiled<'f, IrSpanned<'f, ExprCompiled<'f>>>,
    pub(crate) return_type: Option<TypeCompiled<'f>>,
    pub(crate) info: DefInfoValue<'f>,
}

impl<'fm> Compiler<'_, '_, '_, '_, 'fm> {
    fn parameter_name(&mut self, ident: &CstAssignIdent<'fm>) -> ParameterName {
        let binding_id = ident.payload.expect("no binding for parameter");
        let binding = self.scope_data.get_binding(binding_id);
        ParameterName {
            name: ident.node.ident.clone(),
            captured: binding.captured,
        }
    }

    fn parameter(
        &mut self,
        x: &Spanned<DefParam<'_, CstPayload<'fm>>>,
    ) -> Result<
        IrSpanned<'fm, ParameterCompiled<'fm, IrSpanned<'fm, ExprCompiled<'fm>>>>,
        CompilerInternalError,
    > {
        let span = FrameSpan::new(HeapFileSpan::new(self.codemap, x.span));
        let parameter_name = self.parameter_name(x.ident);
        Ok(IrSpanned {
            span,
            node: match &x.node.kind {
                DefParamKind::Regular(_mode, default_value) => ParameterCompiled::Normal(
                    parameter_name,
                    self.expr_for_type(x.ty).map(|t| t.node),
                    default_value.as_ref().map(|d| self.expr(d)).transpose()?,
                ),
                DefParamKind::Args => ParameterCompiled::Args(
                    parameter_name,
                    self.expr_for_type(x.ty).map(|t| t.node),
                ),
                DefParamKind::Kwargs => ParameterCompiled::KwArgs(
                    parameter_name,
                    self.expr_for_type(x.ty).map(|t| t.node),
                ),
            },
        })
    }

    pub fn function(
        &mut self,
        name: &str,
        signature_span: HeapFileSpan<'fm>,
        scope_id: ScopeId,
        params: &[CstParameter<'fm>],
        return_type: Option<&CstTypeExpr<'fm>>,
        suite: &CstStmt<'fm>,
    ) -> Result<ExprCompiled<'fm>, CompilerInternalError> {
        let file = self.codemap.file_span(suite.span);
        let function_name = ArcStr::from(format!("{}.{}", file.file.filename(), name).as_str());
        let name = self.fh.alloc(name);

        let DefParams { params, indices } = match DefParams::unpack(params, &self.codemap) {
            Ok(def_params) => def_params,
            Err(e) => return Err(CompilerInternalError::from_eval_exception(e)),
        };

        // The parameters run in the scope of the parent, so compile them with the outer
        // scope
        let params: Vec<_> = params.try_map(|x| self.parameter(x))?;
        let params = ParametersCompiled::new(function_name, params, indices);
        let return_type = self.expr_for_type(return_type).map(|t| t.node);

        let ty = Ty::function(
            params.to_ty_params(),
            return_type.map_or(Ty::any(), |t| t.as_ty().clone()),
        );

        self.enter_scope(scope_id);

        let docstring = DocString::extract_raw_starlark_docstring(suite);
        let mut body = self.stmt(suite, false)?;
        body.shrink_to_fit();
        let scope_id = self.exit_scope();
        let scope_names = self.scope_data.get_scope(scope_id);

        let has_types = return_type.is_some() || params.has_types();

        let inline_def_body = if has_types {
            // It is harder to inline if a function declares parameter types or return type.
            None
        } else {
            inline_def_body(&params, &body)
        };

        let param_count = params.count_param_variables();

        let used: Box<[StringValue<'fm>]> = scope_names.used.clone().into_boxed_slice();
        let stmt_compiled = body.as_bc(
            &self.compile_context(return_type.is_some()),
            &used,
            param_count,
            self.fh,
        );
        let info = self.fh.alloc_simple_typed(StarlarkAnyComplex::new(DefInfo {
            name,
            signature_span,
            parameter_captures: self.fh.alloc_any_array_value(&params.parameter_captures()),
            parameter_types: params.parameter_types(self.fh).into_boxed_slice(),
            ty,
            codemap: self.codemap,
            docstring,
            used,
            parent: scope_names.parent.clone().into_boxed_slice(),
            stmt_compiled,
            body_stmts: body,
            inline_def_body,
            stmt_compile_context: self.compile_context(return_type.is_some()),
            globals: self.globals,
        }));
        let info = FrozenValueTyped::from_typed(info).expect("allocated in a frozen heap");

        Ok(ExprCompiled::Def(DefCompiled {
            params,
            return_type,
            info,
        }))
    }
}

/// Starlark function internal representation and implementation of
/// [`StarlarkValue`].
///
/// Every field is at the brand: the parameter defaults and the captured variables are values of
/// the heap the def lives in, and the compiler products (the [`DefInfo`], the return type, the
/// captured parameter slots) are values of the frozen heap that heap depends on.
#[derive(
    Derivative,
    NoSerialize,
    ProvidesStaticType,
    Trace,
    Allocative,
    Freeze,
    starlark_derive::StarlarkPagable
)]
#[derivative(Debug)]
pub(crate) struct Def<'v> {
    pub(crate) parameters: ParametersSpec<Value<'v>>, // The parameters, **kwargs etc including defaults (which are evaluated afresh each time)
    /// Indices of parameters, which are captured in nested defs.
    /// This is a copy of `DefInfo.parameter_captures`.
    parameter_captures: ValueTyped<'v, AnyArray<LocalSlotId>>,
    pub(crate) return_type: Option<TypeCompiled<'v>>, // The return type annotation for the function
    /// Data created during function compilation but before function instantiation.
    /// `DefInfo` can be shared by multiple `def` instances, for example,
    /// `lambda` functions can be instantiated multiple times.
    pub(crate) def_info: DefInfoValue<'v>,
    /// Any variables captured from the outer scope (nested def/lambda).
    /// Each points to a [`ValueCaptured`] or [`FrozenValueCaptured`].
    captured: Box<[Value<'v>]>,
    // Important to ignore these field as it probably references Def in a cycle
    #[derivative(Debug = "ignore")]
    /// A reference to the module where the function is defined after the module has been frozen.
    /// When the module is not frozen yet, this field contains `None`, and function's module
    /// can be accessed from evaluator's module.
    #[allocative(skip)]
    pub(crate) module: AtomicFrozenValueTypedOption<'v, StarlarkAnyComplex<FrozenModuleData<'v>>>,
    /// See [`StmtCompiledCell`].
    #[derivative(Debug = "ignore")]
    #[allocative(skip)]
    #[starlark_pagable(
        serialize_with = "serialize_optimized_on_freeze_stmt",
        deserialize_with = "deserialize_optimized_on_freeze_stmt"
    )]
    optimized_on_freeze_stmt: StmtCompiledCell<'v>,
}

fn serialize_optimized_on_freeze_stmt(
    cell: &StmtCompiledCell,
    ctx: &mut dyn crate::pagable::StarlarkSerializeContext,
) -> crate::Result<()> {
    // `StmtCompiledCell::set` is only called from `post_freeze`, which runs as part of the
    // module freeze before the frozen def is exposed to any caller. Pagable serialization is
    // downstream of that, so the cell is stable here, and empty only if a `Freezer` other than
    // the module's froze this def.
    let bc = cell
        .get()
        .ok_or_else(|| internal_error!("frozen def paged out before `post_freeze`"))?;
    <Bc as crate::pagable::StarlarkSerialize>::starlark_serialize(bc, ctx)
}

fn deserialize_optimized_on_freeze_stmt<'v>(
    ctx: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'v>,
) -> crate::Result<StmtCompiledCell<'v>> {
    Ok(StmtCompiledCell {
        cell: UnsafeCell::new(Some(
            <Bc as crate::pagable::StarlarkDeserialize<'v>>::starlark_deserialize(ctx)?,
        )),
    })
}

impl Display for Def<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.parameters.signature())
    }
}

impl<'v> Def<'v> {
    pub(crate) fn new(
        parameters: ParametersSpec<Value<'v>>,
        return_type: Option<TypeCompiled<'v>>,
        stmt: DefInfoValue<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let captured = stmt
            .value
            .parent
            .as_ref()
            .map(|copy| eval.clone_slot_capture(copy, &stmt.value))
            .into_boxed_slice();
        Ok(eval
            .heap()
            .alloc(Self {
                parameters,
                parameter_captures: stmt.value.parameter_captures,
                return_type,
                captured,
                module: AtomicFrozenValueTypedOption::new(eval.top_frame_def_frozen_module(false)?),
                optimized_on_freeze_stmt: StmtCompiledCell::new(),
                def_info: stmt,
            })
            .to_value())
    }
}

#[starlark_value(type = FUNCTION_TYPE, frozen_vtable, alloc = complex)]
impl<'v> StarlarkValue<'v> for Def<'v> {
    fn name_for_call_stack(&self, _me: Value<'v>) -> String {
        self.def_info.value.name.as_str().to_owned()
    }

    fn invoke(
        &self,
        me: Value<'v>,
        args: &Arguments<'v, '_>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        self.invoke_impl(me, &args.0, eval)
    }

    fn documentation(&self) -> DocItem {
        let mut parameter_types = vec![Ty::any(); self.parameters.len()];
        for pt in self.def_info.value.parameter_types.iter() {
            // Local slot number for parameter is the same as parameter index.
            parameter_types[pt.slot.0 as usize] = pt.ty.as_ty().clone();
        }

        let return_type = self.return_type.map_or(Ty::any(), |r| r.as_ty().clone());

        let function_docs = DocFunction::from_docstring(
            DocStringKind::Starlark,
            self.parameters
                .documentation(parameter_types, HashMap::new()),
            return_type,
            self.def_info.value.docstring.as_ref().map(String::as_ref),
        );

        DocItem::Member(DocMember::Function(function_docs))
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(self.def_info.value.ty.clone())
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        // It's hard to come up with a good hash here, but let's at least make an effort.
        self.def_info.value.name.write_hash(hasher)
    }
}

impl<'v> Def<'v> {
    pub(crate) fn bc(&self) -> &Bc<'v> {
        self.optimized_on_freeze_stmt
            .get()
            .unwrap_or(&self.def_info.value.stmt_compiled)
    }

    fn check_parameter_types(&self, eval: &mut Evaluator<'v, '_, '_>) -> crate::Result<()> {
        let start = if eval.typecheck_profile.enabled {
            Some(ProfilerInstant::now())
        } else {
            None
        };
        for pt in self.def_info.value.parameter_types.iter() {
            match eval.current_frame.get_slot(pt.slot.to_captured_or_not()) {
                None => {
                    panic!("Not allowed optional unassigned with type annotations on them")
                }
                Some(v) => pt.ty.check_type(v, Some(pt.name.as_str()))?,
            }
        }
        if let Some(start) = start {
            eval.typecheck_profile
                .add(self.def_info.value.name.as_str(), start.elapsed());
        }
        Ok(())
    }

    pub(crate) fn check_return_type(
        &self,
        ret: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<()> {
        let return_type_ty: TypeCompiled<'v> = self
            .return_type
            .ok_or_else(|| crate::Error::new_other(DefError::CheckReturnTypeNoType))?;
        let start = if eval.typecheck_profile.enabled {
            Some(ProfilerInstant::now())
        } else {
            None
        };
        return_type_ty.check_type(ret, None)?;
        if let Some(start) = start {
            eval.typecheck_profile
                .add(self.def_info.value.name.as_str(), start.elapsed());
        }
        Ok(())
    }

    #[inline(always)]
    fn invoke_impl<'a, A: ArgumentsImpl<'v, 'a>>(
        &self,
        me: Value<'v>,
        args: &A,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>>
    where
        'v: 'a,
    {
        let bc = self.bc();
        alloca_frame(
            eval,
            bc.local_count,
            bc.max_stack_size,
            bc.max_loop_depth,
            |eval| {
                // SAFETY: `slots` is unique: `alloca_frame` just allocated the frame,
                //   so there are no references to the frame except `eval.current_frame`.
                //   We use `slots` only in `collect_inline`,
                //   which does not have access to `eval` thus cannot access the frame indirectly.
                let slots = unsafe { eval.current_frame.locals_mut() };
                self.parameters.collect_inline(args, slots, eval.heap())?;
                self.invoke_raw(me, eval)
            },
        )
    }

    pub(crate) fn invoke_with_args<'a, A: ArgumentsImpl<'v, 'a>>(
        &self,
        me: Value<'v>,
        args: &A,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>>
    where
        'v: 'a,
    {
        // This is trivial function which delegates to `invoke_impl`.
        // `invoke_impl` is called from two places,
        // giving this function different name makes this function easier to see in profiler.
        self.invoke_impl(me, args, eval)
    }

    /// Invoke the function, assuming that:
    /// * the frame has been allocated and stored in `eval.current_frame`
    /// * the arguments have been collected into the frame
    #[inline(always)]
    fn invoke_raw(
        &self,
        me: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> crate::Result<Value<'v>> {
        // println!("invoking {}", self.def.stmt.name.node);

        if !self.def_info.value.parameter_types.is_empty() {
            self.check_parameter_types(eval)?;
        }

        // Parameters are collected into local slots without captures
        // (to avoid even more branches in parameter capture),
        // and this loop wraps captured parameters.
        for &captured in self.parameter_captures.as_slice() {
            eval.wrap_local_slot_captured(captured);
        }

        // Copy over the parent slots.
        // Explicitly check `self.captured` is not empty to avoid accessing
        // self.def_info.scope_names which is two indirections.
        if !self.captured.is_empty() {
            for (copy, captured) in self.def_info.value.parent.iter().zip(self.captured.iter()) {
                eval.current_frame.set_slot(copy.child, *captured);
            }
        }

        debug_assert!(!me.is_frozen() || self.module.load_relaxed().is_some());

        eval.eval_bc(me, self.bc())
            .map_err(EvalException::into_error)
    }

    pub(crate) fn resolve_arg_name(&self, name: Hashed<&str>) -> ResolvedArgName {
        self.parameters.resolve_name(name)
    }

    pub(crate) fn dump_debug(&self) -> String {
        let mut w = String::new();
        writeln!(w, "Bytecode:").unwrap();
        self.bc()
            .dump_debug()
            .lines()
            .for_each(|l| writeln!(w, "  {l}").unwrap());
        w
    }

    /// Re-optimize the body of this frozen def against its frozen module, see
    /// [`StmtCompiledCell`]. `heap` is the value heap the module was frozen from; `edge` leads
    /// from it to `frozen_heap`, the heap this def lives in, and `seal_edge` back.
    pub(crate) fn post_freeze<'h>(
        &self,
        module: FrozenModuleValue<'v>,
        heap: Heap<'h>,
        frozen_heap: FrozenHeap<'v>,
        edge: HeapEdge<'h, 'v>,
        seal_edge: SealEdge<'v, 'h>,
    ) {
        // Module passed to this function is not always module where the function is declared:
        // A function can be created in a frozen module and frozen later in another module.
        // `def_module` variable contains a module where this `def` is declared.
        let def_module = match self.module.load_relaxed() {
            None => {
                self.module.store_relaxed(module);
                module
            }
            Some(module) => module,
        };

        // Now perform the optimization of function body with fully frozen module:
        // all module variables are frozen, so we can inline more aggressively.
        let def_info = &self.def_info.value;
        let body_optimized = def_info
            .body_stmts
            .optimize(&mut OptCtx::new(
                &mut OptimizeOnFreezeContext {
                    module: &def_module.as_ref().value,
                    heap,
                    frozen_heap,
                    edge,
                    seal_edge,
                    local_as_values: Vec::new(),
                },
                self.parameters.len().try_into().unwrap(),
            ))
            .as_bc(
                &def_info.stmt_compile_context,
                &def_info.used,
                self.parameters.len() as u32,
                frozen_heap,
            );

        // Store the optimized body.
        // This is (relatively) safe because we know that during freeze
        // nobody has a reference to stmt: nobody is executing this `def`.
        unsafe {
            self.optimized_on_freeze_stmt.set(body_optimized);
        }
    }
}

register_starlark_any!(CopySlotFromParent);
