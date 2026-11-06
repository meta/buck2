/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::bxl::starlark_defs::lazy_ctx::operation::StarlarkLazy;
use crate::bxl::starlark_defs::lazy_ctx::operation::cquery::LazyCqueryOperation;
use crate::bxl::starlark_defs::uquery::UnpackUnconfiguredQueryArgs;

/// Context for lazy cquery operations.
#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative,
    Debug,
    pagable::Pagable,
    starlark::StarlarkPagableViaPagable
)]
#[display("bxl.LazyCqueryContext")]
#[allocative(skip)]
pub(crate) struct StarlarkLazyCqueryCtx {
    global_cfg_options_override: GlobalCfgOptions,
}

impl StarlarkLazyCqueryCtx {
    pub(crate) fn new(global_cfg_options_override: GlobalCfgOptions) -> Self {
        Self {
            global_cfg_options_override,
        }
    }
}

starlark_simple_value!(StarlarkLazyCqueryCtx);

starlark::methods_static!(LAZY_CQUERY_METHODS = lazy_cquery_methods);

#[starlark_value(type = "bxl.LazyCqueryContext")]
impl<'v> StarlarkValue<'v> for StarlarkLazyCqueryCtx {
    fn get_methods() -> Option<&'static Methods> {
        Some(LAZY_CQUERY_METHODS.methods())
    }
}

/// The context for performing lazy `cquery` operations in bxl. The functions offered on this ctx are
/// the same behaviour as the query functions available within cquery command.
///
/// An instance may be obtained with [`bxl.LazyContext.cquery()`](../LazyContext#lazycontextcquery).
#[starlark_module]
fn lazy_cquery_methods(builder: &mut MethodsBuilder) {
    /// Evaluates some general query string. `query_args` can be a target_set of unconfigured nodes, or
    /// a list of strings. Returns a `dict` of target labels mapped to their `target_set` results if `query_args`
    /// was passed in, otherwise returns a single `target_set`.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_eval(ctx):
    ///     result1 = ctx.lazy.cquery().eval("inputs(root//bin:the_binary)").resolve()
    ///     ctx.output.print(result1)
    ///
    ///     result2 = ctx.lazy.cquery().eval("inputs(%s)", query_args = ["cell//path/to/file:target"]).resolve()
    ///     ctx.output.print(result2)
    /// ```
    fn eval<'v>(
        this: &StarlarkLazyCqueryCtx,
        #[starlark(require = pos)] query: &'v str,
        #[starlark(require = named, default = NoneOr::None)] query_args: NoneOr<
            UnpackUnconfiguredQueryArgs<'v>,
        >,
        #[starlark(require = named, default = NoneOr::None)] target_universe: NoneOr<
            ValueOf<'v, ListOrTupleType<String>>,
        >,
    ) -> starlark::Result<StarlarkLazy> {
        let query_args = match query_args {
            NoneOr::None => Vec::new(),
            NoneOr::Other(query_args) => query_args.into_strings()?,
        };
        let target_universe = target_universe
            .into_option()
            .map(|list| list.try_to_vec())
            .transpose()?;
        Ok(StarlarkLazy::new_cquery(LazyCqueryOperation::new_eval(
            this.global_cfg_options_override.dupe(),
            query.to_owned(),
            query_args,
            target_universe,
        )))
    }
}
