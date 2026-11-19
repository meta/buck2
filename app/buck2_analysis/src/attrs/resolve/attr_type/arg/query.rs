/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_build_api::interpreter::rule_defs::resolve_query_macro::ResolvedQueryMacro;
use buck2_build_api::interpreter::rule_defs::resolve_query_macro::ResolvedQueryMacroTargetAndOutputs;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_node::attrs::attr_type::arg::QueryExpansion;
use buck2_node::attrs::attr_type::query::QueryMacroBase;
use dupe::Dupe;

use crate::attrs::resolve::ctx::AnalysisQueryResult;
use crate::attrs::resolve::ctx::AttrResolutionContext;

pub(crate) trait ConfiguredQueryMacroBaseExt {
    fn resolve(
        &self,
        ctx: &mut dyn AttrResolutionContext,
    ) -> buck2_error::Result<ResolvedQueryMacro>;
}

impl ConfiguredQueryMacroBaseExt for QueryMacroBase<ConfiguredProvidersLabel> {
    fn resolve(
        &self,
        ctx: &mut dyn AttrResolutionContext,
    ) -> buck2_error::Result<ResolvedQueryMacro> {
        let query_result: Arc<AnalysisQueryResult> = ctx.resolve_query(&self.query.query)?;

        match &self.expansion_type {
            QueryExpansion::Output => Ok(ResolvedQueryMacro::Outputs(
                query_result
                    .result
                    .iter()
                    .map(|(_, providers)| {
                        Ok(providers
                            .provider_collection()
                            .default_info()?
                            .default_outputs()?
                            .into())
                    })
                    .collect::<buck2_error::Result<_>>()?,
            )),
            QueryExpansion::Target => Ok(ResolvedQueryMacro::Targets(
                query_result
                    .result
                    .iter()
                    .map(|(target, _)| target.dupe())
                    .collect(),
            )),
            QueryExpansion::TargetAndOutput(separator) => {
                Ok(ResolvedQueryMacro::TargetsAndOutputs(Box::new(
                    ResolvedQueryMacroTargetAndOutputs {
                        sep: match separator {
                            Some(separator) => separator.to_owned().into_boxed_str(),
                            None => " ".to_owned().into_boxed_str(),
                        },
                        list: query_result
                            .result
                            .iter()
                            .map(|(target, providers)| {
                                Ok((
                                    target.dupe(),
                                    providers
                                        .provider_collection()
                                        .default_info()?
                                        .default_outputs()?
                                        .into(),
                                ))
                            })
                            .collect::<buck2_error::Result<_>>()?,
                    },
                )))
            }
        }
    }
}
