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
use derivative::Derivative;
use derive_more::Display;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::bxl::starlark_defs::file_set::FileSetExpr;
use crate::bxl::starlark_defs::file_set::OwnedFileSetExpr;
use crate::bxl::starlark_defs::lazy_ctx::operation::StarlarkLazy;
use crate::bxl::starlark_defs::lazy_ctx::operation::uquery::LazyUqueryOperation;
use crate::bxl::starlark_defs::target_list_expr::OwnedTargetListExprArg;
use crate::bxl::starlark_defs::target_list_expr::TargetListExprArg;
use crate::bxl::starlark_defs::uquery::UnpackUnconfiguredQueryArgs;

#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative,
    Debug,
    starlark::StarlarkPagable
)]
#[display("bxl.LazyUqueryContext")]
pub(crate) struct StarlarkLazyUqueryCtx {
    allow_partial_graph: bool,
}

impl StarlarkLazyUqueryCtx {
    pub(crate) fn new(allow_partial_graph: bool) -> Self {
        Self {
            allow_partial_graph,
        }
    }
}

starlark_simple_value!(StarlarkLazyUqueryCtx);

starlark::methods_static!(LAZY_UQUERY_METHODS = lazy_uquery_methods);

#[starlark_value(type = "bxl.LazyUqueryContext")]
impl<'v> StarlarkValue<'v> for StarlarkLazyUqueryCtx {
    fn get_methods() -> Option<&'static Methods> {
        Some(LAZY_UQUERY_METHODS.methods())
    }
}

/// The context for performing lazy `uquery` operations in bxl. The functions offered on this ctx are
/// the same behaviour as the query functions available within uquery command.
///
/// An instance may be obtained with [`bxl.LazyContext.uquery()`](../LazyContext#lazycontextuquery).
#[starlark_module]
fn lazy_uquery_methods(builder: &mut MethodsBuilder) {
    /// Computes all dependency paths from `from` to `to`, with optional filter.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().allpaths("//:foo", "//:bar", filter = "attrfilter('name', 'some_name', target_deps()")).catch().resolve()
    /// ```
    fn allpaths<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] from: TargetListExprArg<'v>,
        #[starlark(require = pos)] to: TargetListExprArg<'v>,
        #[starlark(require = named, default = NoneOr::None)] filter: NoneOr<&'v str>,
    ) -> starlark::Result<StarlarkLazy> {
        let from = OwnedTargetListExprArg::from_ref(&from)?;
        let to = OwnedTargetListExprArg::from_ref(&to)?;
        let filter = filter.into_option().map(|s| s.to_owned());
        let op = LazyUqueryOperation::AllPaths { from, to, filter };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Computes some dependency path from `from` to `to`, with optional filter.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().somepath("//:foo", "//:bar", filter = "attrfilter('name', 'some_name', target_deps()")).catch().resolve()
    /// ```
    fn somepath<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] from: TargetListExprArg<'v>,
        #[starlark(require = pos)] to: TargetListExprArg<'v>,
        #[starlark(require = named, default = NoneOr::None)] filter: NoneOr<&'v str>,
    ) -> starlark::Result<StarlarkLazy> {
        let from = OwnedTargetListExprArg::from_ref(&from)?;
        let to = OwnedTargetListExprArg::from_ref(&to)?;
        let filter = filter.into_option().map(|s| s.to_owned());
        let op = LazyUqueryOperation::SomePath { from, to, filter };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Filters targets by attribute value.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().attrfilter("name", "some_name", "//:foo").catch().resolve()
    /// ```
    fn attrfilter<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] attr: &'v str,
        #[starlark(require = pos)] value: &'v str,
        #[starlark(require = pos)] targets: TargetListExprArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let attr = attr.to_owned();
        let value = value.to_owned();
        let targets = OwnedTargetListExprArg::from_ref(&targets)?;
        let op = LazyUqueryOperation::AttrFilter {
            attr,
            value,
            targets,
        };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Finds input files for the given targets.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().inputs("//:foo").catch().resolve()
    /// ```
    fn inputs<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] targets: TargetListExprArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let targets = OwnedTargetListExprArg::from_ref(&targets)?;
        let op = LazyUqueryOperation::Inputs(targets);
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Filters targets by rule type using a regex pattern.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().kind("cpp.*", "//:foo").catch().resolve()
    /// ```
    fn kind<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] regex: &'v str,
        #[starlark(require = pos)] targets: TargetListExprArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let regex = regex.to_owned();
        let targets = OwnedTargetListExprArg::from_ref(&targets)?;
        let op = LazyUqueryOperation::Kind { regex, targets };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Finds the [transitive closure](https://en.wikipedia.org/wiki/Transitive_closure) of dependencies.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().deps("//:foo", 1, filter = "attrfilter('name', 'some_name', target_deps())").catch().resolve()
    /// ```
    fn deps<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] universe: TargetListExprArg<'v>,
        #[starlark(require = pos, default = NoneOr::None)] depth: NoneOr<i32>,
        #[starlark(require = named, default = NoneOr::None)] filter: NoneOr<&'v str>,
    ) -> starlark::Result<StarlarkLazy> {
        let universe = OwnedTargetListExprArg::from_ref(&universe)?;
        let depth = depth.into_option().into();
        let filter = filter.into_option().map(|s| s.to_owned());
        let op = LazyUqueryOperation::Deps {
            universe,
            depth,
            filter,
        };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Finds the [transitive closure](https://en.wikipedia.org/wiki/Transitive_closure) of reverse dependencies.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().rdeps("//:universe", "//:from", 1, filter = "attrfilter('name', 'some_name', target_deps())").catch().resolve()
    /// ```
    fn rdeps<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] universe: TargetListExprArg<'v>,
        #[starlark(require = pos)] from: TargetListExprArg<'v>,
        #[starlark(require = pos, default = NoneOr::None)] depth: NoneOr<i32>,
        #[starlark(require = named, default = NoneOr::None)] filter: NoneOr<&'v str>,
    ) -> starlark::Result<StarlarkLazy> {
        let universe = OwnedTargetListExprArg::from_ref(&universe)?;
        let from = OwnedTargetListExprArg::from_ref(&from)?;
        let depth = depth.into_option().into();
        let filter = filter.into_option().map(|s| s.to_owned());
        let op = LazyUqueryOperation::Rdeps {
            universe,
            from,
            depth,
            filter,
        };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Filters targets by name using a regex pattern.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().filter(".*the_binary", "//:foo").catch().resolve()
    /// ```
    fn filter<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] regex: &'v str,
        #[starlark(require = pos)] targets: TargetListExprArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let regex = regex.to_owned();
        let targets = OwnedTargetListExprArg::from_ref(&targets)?;
        let op = LazyUqueryOperation::Filter { regex, targets };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Querying the test targets of the given target.
    /// It returns `UnconfiguredTargetSet`
    ///
    /// Example:
    /// ```python
    /// def _impl(ctx: bxl.Context):
    ///     res = ctx.lazy.uquery().testsof("//:foo_lib").catch().resolve()
    ///     if res.is_ok():
    ///         nodes = res.get()
    ///         ctx.output.print(nodes)
    ///     else:
    ///         err = res.unwrap_err()
    ///         ctx.output.print(err)
    /// ```
    fn testsof<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] expr: TargetListExprArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let expr = OwnedTargetListExprArg::from_ref(&expr)?;
        let op = LazyUqueryOperation::TestsOf(expr);
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Finds the build file(s) that define the given targets.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().buildfile("//:foo").catch().resolve()
    /// ```
    fn buildfile<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] targets: TargetListExprArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let targets = OwnedTargetListExprArg::from_ref(&targets)?;
        let op = LazyUqueryOperation::Buildfile(targets);
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Finds the build files of the given targets and their transitive imports.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().allbuildfiles("//:foo").catch().resolve()
    /// ```
    fn allbuildfiles<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] universe: TargetListExprArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let universe = OwnedTargetListExprArg::from_ref(&universe)?;
        let op = LazyUqueryOperation::AllBuildfiles(universe);
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Finds all build files that transitively depend on the given files.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().rbuildfiles("bin/TARGETS", "bin/defs.bzl").catch().resolve()
    /// ```
    fn rbuildfiles<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] universe: FileSetExpr<'v>,
        #[starlark(require = pos)] argset: FileSetExpr<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let universe = OwnedFileSetExpr::from_ref(&universe)?;
        let argset = OwnedFileSetExpr::from_ref(&argset)?;
        let op = LazyUqueryOperation::Rbuildfiles { universe, argset };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Finds targets that own the specified files.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().owner("bin/TARGETS.fixture").catch().resolve()
    /// res = ctx.lazy.uquery().owner(["bin/TARGET", "bin/kind"]).catch().resolve()
    /// ```
    fn owner<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] files: FileSetExpr<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let files = OwnedFileSetExpr::from_ref(&files)?;
        let op = LazyUqueryOperation::Owner { files };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Finds targets defined in the specified build files.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().targets_in_buildfile("bin/TARGETS.fixture").catch().resolve()
    /// res = ctx.lazy.uquery().targets_in_buildfile(["bin/TARGETS", "lib/TARGETS"]).catch().resolve()
    /// ```
    fn targets_in_buildfile<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] files: FileSetExpr<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let files = OwnedFileSetExpr::from_ref(&files)?;
        let op = LazyUqueryOperation::TargetsInBuildfile { files };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Filters targets by attribute value using regex matching.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().attrregexfilter("name", "he.*lo", "//:foo").catch().resolve()
    /// ```
    fn attrregexfilter<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] attr: &'v str,
        #[starlark(require = pos)] value: &'v str,
        #[starlark(require = pos)] targets: TargetListExprArg<'v>,
    ) -> starlark::Result<StarlarkLazy> {
        let attr = attr.to_owned();
        let value = value.to_owned();
        let targets = OwnedTargetListExprArg::from_ref(&targets)?;
        let op = LazyUqueryOperation::AttrRegexFilter {
            attr,
            value,
            targets,
        };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }

    /// Evaluates a general query string with optional query arguments.
    ///
    /// Example:
    /// ```python
    /// res = ctx.lazy.uquery().eval("inputs(cell//path/to/file:target)").catch().resolve()
    /// res = ctx.lazy.uquery().eval("inputs(%s)", query_args = ["cell//path/to/file:target"]).catch().resolve()
    /// ```
    fn eval<'v>(
        #[starlark(this)] this: &'v StarlarkLazyUqueryCtx,
        #[starlark(require = pos)] query: &'v str,
        #[starlark(require = named, default = NoneOr::None)] query_args: NoneOr<
            UnpackUnconfiguredQueryArgs<'v>,
        >,
    ) -> starlark::Result<StarlarkLazy> {
        let query = query.to_owned();
        let query_args = match query_args {
            NoneOr::None => Vec::new(),
            NoneOr::Other(query_args) => query_args.into_strings()?,
        };
        let op = LazyUqueryOperation::Eval { query, query_args };
        Ok(StarlarkLazy::new_uquery(op, this.allow_partial_graph))
    }
}
