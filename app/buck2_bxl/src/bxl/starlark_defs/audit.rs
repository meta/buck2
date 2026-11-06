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
use buck2_build_api::audit_cell::audit_cell;
use buck2_build_api::audit_output::AuditOutputResult;
use buck2_build_api::audit_output::audit_output;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::internal_error;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use derivative::Derivative;
use derive_more::Display;
use dupe::Dupe;
use futures::FutureExt;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::AllocDict;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::bxl::starlark_defs::context::BxlContext;
use crate::bxl::starlark_defs::nodes::action::StarlarkAction;
use crate::bxl::value_as_starlark_target_label::ValueAsStarlarkTargetLabel;

#[derive(
    ProvidesStaticType,
    Derivative,
    Display,
    Trace,
    NoSerialize,
    Allocative
)]
#[derivative(Debug)]
#[display("{:?}", self)]
#[allocative(skip)]
pub(crate) struct StarlarkAuditCtx<'v> {
    #[derivative(Debug = "ignore")]
    ctx: ValueTyped<'v, BxlContext<'v>>,
    #[trace(static)]
    #[derivative(Debug = "ignore")]
    working_dir: ProjectRelativePathBuf,
}

starlark::methods_static!(AUDIT_METHODS = audit_methods);

#[starlark_value(type = "bxl.AuditContext", StarlarkTypeRepr, UnpackValue)]
impl<'v> StarlarkValue<'v> for StarlarkAuditCtx<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(AUDIT_METHODS.methods())
    }
}

impl<'v> AllocValue<'v> for StarlarkAuditCtx<'v> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        heap.alloc_complex_no_freeze(self)
    }
}

impl<'v> StarlarkAuditCtx<'v> {
    pub(crate) fn new(
        ctx: ValueTyped<'v, BxlContext<'v>>,
        working_dir: ProjectRelativePathBuf,
    ) -> buck2_error::Result<Self> {
        Ok(Self { ctx, working_dir })
    }
}

/// The context for performing `audit` operations in bxl. The functions offered on this ctx are
/// the same behaviour as the audit functions available within audit command.
///
/// An instance may be obtained with [`bxl.Context.audit()`](../Context/#contextaudit).
#[starlark_module]
fn audit_methods(builder: &mut MethodsBuilder) {
    /// Returns either:
    ///  - The `action` which created the buck-out path, if exists.
    ///  - The `unconfigured_target_label` constructed from the buck-out path, if the configuration hashes do not match.
    ///  - None, if the configuration hash of the buck-out path matches the one passed into this function, or the default target
    /// configuration, but no action could be found that generated the buck-out path.
    ///
    /// Takes in an optional target platform, otherwise will use the default target platform.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_audit_output(ctx):
    ///     target_platform = "foo"
    ///     result = ctx.audit().output("buck-out/v2/art/fbcode/some_cfg_hash/path/to/__target__/artifact", target_platform)
    ///     ctx.output.print(result)
    /// ```
    fn output<'v>(
        this: &StarlarkAuditCtx<'v>,
        // TODO(nga): parameters should be either positional or named, not both.
        output_path: &'v str,
        #[starlark(default = ValueAsStarlarkTargetLabel::NONE)]
        target_platform: ValueAsStarlarkTargetLabel<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<
        // TODO(nga): used precise type.
        NoneOr<Value<'v>>,
    > {
        let heap = eval.heap();
        let global_cfg_options = this
            .ctx
            .resolve_global_cfg_options(target_platform, vec![])?;

        Ok(this.ctx.via_dice(eval, |ctx| {
            ctx.via(|ctx| {
                async move {
                    let output = audit_output(
                        output_path,
                        &this.working_dir,
                        this.ctx.cell_resolver(),
                        ctx,
                        &global_cfg_options,
                    )
                    .await?;
                    match output {
                        None => Ok(NoneOr::None),
                        Some(result) => buck2_error::Ok(NoneOr::Other(match result {
                            AuditOutputResult::Match(action) => heap.alloc(StarlarkAction(
                                action
                                    .action()
                                    .ok_or_else(|| {
                                        internal_error!("audit_output did not return an action")
                                    })?
                                    .dupe(),
                            )),
                            AuditOutputResult::MaybeRelevantForConfigurationHashPath(label) => {
                                heap.alloc(StarlarkTargetLabel::new(label))
                            }
                            AuditOutputResult::MatchContentBasedPath(label) => {
                                heap.alloc(StarlarkTargetLabel::new(label))
                            }
                        })),
                    }
                }
                .boxed_local()
            })
        })?)
    }

    /// Query information about the [cells] list in .buckconfig.
    ///
    /// Takes the following parameters:
    /// * `aliases_to_resolve` - list of cell aliases to query. These aliases will be resolved in the root cell of the BXL script.
    /// * optional `aliases` flag - if enabled, and no explicit aliases are passed, will query for all aliases in the root cell of the BXL script.
    ///
    /// Returns a dict of cell name to absolute path mappings.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_audit_cell(ctx):
    ///     result = ctx.audit().cell(aliases = True)
    ///     ctx.output.print(result)
    /// ```
    fn cell<'v>(
        this: &StarlarkAuditCtx<'v>,
        // TODO(nga): parameters should be either positional or named, not both.
        #[starlark(default = ValueOf::empty_list())] aliases_to_resolve: ValueOf<
            'v,
            ListOrTupleType<String>,
        >,
        #[starlark(require = named, default = false)] aliases: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<AllocDict<impl Iterator<Item = (String, String)> + use<>>> {
        let aliases_to_resolve = aliases_to_resolve.try_to_vec()?;
        Ok(this.ctx.via_dice(eval, |ctx| {
            ctx.via(|ctx| {
                async {
                    let result = audit_cell(
                        ctx,
                        &aliases_to_resolve,
                        aliases,
                        &this.working_dir,
                        this.ctx.project_root(),
                    )?
                    .await?;
                    Ok(AllocDict(
                        result.into_iter().map(|(k, v)| (k, v.to_string())),
                    ))
                }
                .boxed_local()
            })
        })?)
    }
}
