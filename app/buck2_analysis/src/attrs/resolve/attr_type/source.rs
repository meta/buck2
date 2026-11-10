/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::source_artifact::SourceArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_core::package::source_path::SourcePath;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_node::attrs::attr_type::source::SourceAttrType;
use starlark::values::Value;

use crate::attrs::resolve::ctx::AttrResolutionContext;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum SourceLabelResolutionError {
    #[error("Expected a single artifact from {0}, but it returned {1} artifacts")]
    ExpectedSingleValue(String, usize),
}

pub(crate) trait SourceAttrTypeExt {
    fn resolve_single_file<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        path: SourcePath,
    ) -> Value<'v> {
        ctx.heap()
            .alloc_representation(StarlarkArtifact::new(SourceArtifact::new(path).into()))
            .to_value()
    }

    fn resolve_label<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        label: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<Vec<Value<'v>>> {
        let dep = ctx.get_dep(label)?;
        dep.default_info()?
            .default_outputs_raw()
            .iter()
            .map(|v| Ok(v?.to_value()))
            .collect()
    }

    fn resolve_single_label<'v>(
        ctx: &mut dyn AttrResolutionContext<'v>,
        value: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<Value<'v>> {
        let mut resolved = Self::resolve_label(ctx, value)?;
        if resolved.len() == 1 {
            Ok(resolved.pop().unwrap())
        } else {
            Err(
                SourceLabelResolutionError::ExpectedSingleValue(value.to_string(), resolved.len())
                    .into(),
            )
        }
    }
}

impl SourceAttrTypeExt for SourceAttrType {}
