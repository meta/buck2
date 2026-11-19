/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(non_upper_case_globals)]

use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::unconfigured::TargetNode;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::starlark_value_as_type::StarlarkValueAsType;

use super::nodes::action::StarlarkActionQueryNode;
use crate::bxl::starlark_defs::analysis_result::StarlarkAnalysisResult;
use crate::bxl::starlark_defs::aquery::StarlarkAQueryCtx;
use crate::bxl::starlark_defs::artifacts::EnsuredArtifact;
use crate::bxl::starlark_defs::audit::StarlarkAuditCtx;
use crate::bxl::starlark_defs::build_result::StarlarkBxlBuildResult;
use crate::bxl::starlark_defs::cli_args::CliArgs;
use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::context::actions::BxlActions;
use crate::bxl::starlark_defs::context::build::StarlarkFailedArtifactIterable;
use crate::bxl::starlark_defs::context::build::StarlarkProvidersArtifactIterable;
use crate::bxl::starlark_defs::context::fs::BxlFilesystem;
use crate::bxl::starlark_defs::context::output::StarlarkOutputStream;
use crate::bxl::starlark_defs::cquery::StarlarkCQueryCtx;
use crate::bxl::starlark_defs::file_set::StarlarkFileNode;
use crate::bxl::starlark_defs::lazy_ctx::StarlarkLazyCtx;
use crate::bxl::starlark_defs::lazy_ctx::lazy_cquery_ctx::StarlarkLazyCqueryCtx;
use crate::bxl::starlark_defs::lazy_ctx::lazy_uquery_ctx::StarlarkLazyUqueryCtx;
use crate::bxl::starlark_defs::lazy_ctx::operation::StarlarkLazy;
use crate::bxl::starlark_defs::nodes::action::StarlarkActionAttr;
use crate::bxl::starlark_defs::nodes::configured::StarlarkConfiguredAttr;
use crate::bxl::starlark_defs::nodes::configured::StarlarkConfiguredTargetNode;
use crate::bxl::starlark_defs::nodes::configured::StarlarkLazyAttrs;
use crate::bxl::starlark_defs::nodes::configured::StarlarkLazyResolvedAttrs;
use crate::bxl::starlark_defs::nodes::unconfigured::StarlarkTargetNode;
use crate::bxl::starlark_defs::result::StarlarkActionError;
use crate::bxl::starlark_defs::result::StarlarkError;
use crate::bxl::starlark_defs::result::StarlarkResult;
use crate::bxl::starlark_defs::target_universe::StarlarkTargetUniverse;
use crate::bxl::starlark_defs::targetset::CONFIGURED_TARGET_SET_TYPE;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSet;
use crate::bxl::starlark_defs::targetset::StarlarkTargetSetType;
use crate::bxl::starlark_defs::targetset::UNCONFIGURED_TARGET_SET_TYPE;
use crate::bxl::starlark_defs::uquery::StarlarkUQueryCtx;
use crate::bxl::starlark_defs::visibility_intersection::StarlarkVisibilityIntersectionLayer;

#[starlark_module]
#[starlark_types(
    CliArgs as CliArgs,
    BxlContext<'_> as Context,
    StarlarkActionAttr as ActionAttr,
    StarlarkAuditCtx<'_> as AuditContext,
    StarlarkAQueryCtx<'_> as AqueryContext,
    StarlarkCQueryCtx<'_> as CqueryContext,
    StarlarkUQueryCtx<'_> as UqueryContext,
    BxlActions<'_> as Actions,
    BxlFilesystem<'_> as Filesystem,
    StarlarkBxlBuildResult as BuildResult,
    StarlarkAnalysisResult as AnalysisResult,
    EnsuredArtifact as EnsuredArtifact,
    StarlarkFileNode as FileNode,
    StarlarkActionQueryNode as ActionQueryNode,
    StarlarkTargetNode as UnconfiguredTargetNode,
    StarlarkConfiguredTargetNode as ConfiguredTargetNode,
    StarlarkLazyAttrs<'_> as LazyAttrs,
    StarlarkLazyResolvedAttrs<'_> as LazyResolvedAttrs,
    StarlarkConfiguredAttr as ConfiguredAttr,
    StarlarkTargetUniverse<'_> as TargetUniverse,
    StarlarkOutputStream as OutputStream,
    StarlarkLazyCtx<'_> as LazyContext,
    StarlarkLazy as Lazy,
    StarlarkError as Error,
    StarlarkActionError as ActionError,
    StarlarkResult<'_> as Result,
    StarlarkLazyUqueryCtx as LazyUqueryContext,
    StarlarkLazyCqueryCtx as LazyCqueryContext,
    StarlarkFailedArtifactIterable<'_> as FailedArtifactsIterable,
    StarlarkProvidersArtifactIterable<'_> as BuiltArtifactsIterable,
    StarlarkVisibilityIntersectionLayer as VisibilityIntersectionLayer
)]
pub(crate) fn register_bxl_type_names_in_bxl_namespace(globals: &mut GlobalsBuilder) {
    const UnconfiguredTargetSet: StarlarkValueAsType<StarlarkTargetSet<TargetNode>> =
        UNCONFIGURED_TARGET_SET_TYPE;
    const ConfiguredTargetSet: StarlarkValueAsType<StarlarkTargetSet<ConfiguredTargetNode>> =
        CONFIGURED_TARGET_SET_TYPE;
    const TargetSet: StarlarkTargetSetType = StarlarkTargetSetType;
}
