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

use buck2_analysis::analysis::env::RuleAnalysisAttrResolutionContext;
use buck2_analysis::attrs::resolve::attr_type::arg::ConfiguredStringWithMacrosExt;
use buck2_analysis::attrs::resolve::attr_type::dep::DepAttrTypeExt;
use buck2_analysis::attrs::resolve::ctx::AttrResolutionContext;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::anon_target::AnonTargetDependentAnalysisResults;
use buck2_build_api::artifact_groups::promise::PromiseArtifact;
use buck2_build_api::artifact_groups::promise::PromiseArtifactAttr;
use buck2_build_api::artifact_groups::promise::PromiseArtifactResolveError;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_promise_artifact::StarlarkPromiseArtifact;
use buck2_build_api::keep_going::KeepGoing;
use buck2_core::package::PackageLabel;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_interpreter::types::configured_providers_label::StarlarkProvidersLabel;
use buck2_node::attrs::attr_type::dep::DepAttrType;
use buck2_node::attrs::attr_type::query::ResolvedQueryLiterals;
use buck2_node::attrs::configured_traversal::ConfiguredAttrTraversal;
use dice::DiceComputations;
use dupe::Dupe;
use starlark::values::Value;
use starlark::values::tuple::AllocTuple;

use crate::anon_target_attr::AnonTargetAttr;
use crate::anon_targets::AnonTargetKey;
use crate::anon_targets::AnonTargetsError;
use crate::anon_targets::get_artifact_from_anon_target_analysis;

// No macros in anon targets, so query results are empty. Execution platform resolution should
// always be inherited from the anon target.
pub(crate) struct AnonTargetAttrResolutionContext<'a, 'v> {
    pub(crate) promised_artifacts_map: BuckMutMap<&'a PromiseArtifactAttr, Artifact>,
    pub(crate) rule_analysis_attr_resolution_ctx: RuleAnalysisAttrResolutionContext<'a, 'v>,
}

pub(crate) trait AnonTargetAttrResolution {
    fn resolve<'a, 'v>(
        &self,
        pkg: PackageLabel,
        ctx: &AnonTargetAttrResolutionContext<'a, 'v>,
    ) -> buck2_error::Result<Vec<Value<'v>>>;

    fn resolve_single<'a, 'v>(
        &self,
        pkg: PackageLabel,
        ctx: &AnonTargetAttrResolutionContext<'a, 'v>,
    ) -> buck2_error::Result<Value<'v>>;
}

impl AnonTargetAttrResolution for AnonTargetAttr {
    /// "Resolves" the anon target attr value to the resolved value provided to the rule implementation.
    ///
    /// `resolve` may return multiple values. It is up to the caller to fail if
    /// an inappropriate number of elements is returned. e.g. `attrs.list()` might
    /// accept and merge multiple returned values from `attrs.source()`, but
    /// `attrs.optional()` might only accept a single value, and fail otherwise.
    fn resolve<'a, 'v>(
        &self,
        pkg: PackageLabel,
        ctx: &AnonTargetAttrResolutionContext<'a, 'v>,
    ) -> buck2_error::Result<Vec<Value<'v>>> {
        Ok(vec![self.resolve_single(pkg, ctx)?])
    }

    /// Resolving a single value is common, so `resolve_single` will validate
    /// this function's output, and return a single value or an error.
    fn resolve_single<'a, 'v>(
        &self,
        pkg: PackageLabel,
        anon_resolution_ctx: &AnonTargetAttrResolutionContext<'a, 'v>,
    ) -> buck2_error::Result<Value<'v>> {
        let mut ctx = &anon_resolution_ctx.rule_analysis_attr_resolution_ctx;
        match self {
            AnonTargetAttr::Bool(v) => Ok(Value::new_bool(v.0)),
            AnonTargetAttr::Int(v) => Ok(ctx.heap().alloc(*v)),
            AnonTargetAttr::String(v) | AnonTargetAttr::EnumVariant(v) => {
                Ok(ctx.heap().alloc(v.as_str()))
            }
            AnonTargetAttr::List(list) => {
                let mut values = Vec::with_capacity(list.len());
                for v in list.iter() {
                    values.append(&mut v.resolve(pkg, anon_resolution_ctx)?);
                }
                Ok(ctx.heap().alloc(values))
            }
            AnonTargetAttr::Tuple(list) => {
                let mut values = Vec::with_capacity(list.len());
                for v in list.iter() {
                    values.append(&mut v.resolve(pkg, anon_resolution_ctx)?);
                }
                Ok(ctx.heap().alloc(AllocTuple(values)))
            }
            AnonTargetAttr::Dict(dict) => {
                let mut res = BuckMutInsertionMap::with_capacity(dict.len());
                for (k, v) in dict.iter() {
                    res.insert_hashed(
                        k.resolve_single(pkg, anon_resolution_ctx)?.get_hashed()?,
                        v.resolve_single(pkg, anon_resolution_ctx)?,
                    );
                }
                Ok(ctx.heap().alloc(res))
            }
            AnonTargetAttr::None => Ok(Value::new_none()),
            AnonTargetAttr::OneOf(box l, _) => l.resolve_single(pkg, anon_resolution_ctx),
            AnonTargetAttr::Dep(d) => Ok(DepAttrType::resolve_single(&mut ctx, d)?),
            AnonTargetAttr::Artifact(d) => Ok(ctx
                .heap()
                .alloc_representation(StarlarkArtifact::new(d.dupe()))
                .to_value()),
            AnonTargetAttr::Arg(a) => Ok(a.resolve(&mut ctx, pkg)?),
            AnonTargetAttr::PromiseArtifact(box promise_artifact_attr) => {
                let promise_id = promise_artifact_attr.id.dupe();
                // We validated that the analysis contains the promise artifact id earlier
                let artifact = anon_resolution_ctx
                    .promised_artifacts_map
                    .get(&promise_artifact_attr)
                    .unwrap();

                let promise_has_content_based_path = promise_artifact_attr.has_content_based_path;
                let artifact_has_content_based_path =
                    artifact.path_resolution_requires_artifact_value();
                if artifact_has_content_based_path && !promise_has_content_based_path {
                    return Err(PromiseArtifactResolveError::UsesContentBasedPath(
                        promise_id.clone(),
                        format!("{}", artifact),
                    )
                    .into());
                } else if !artifact_has_content_based_path && promise_has_content_based_path {
                    return Err(PromiseArtifactResolveError::DoesNotUseContentBasedPath(
                        promise_id.clone(),
                        format!("{}", artifact),
                    )
                    .into());
                }

                // Assert the short path, since we have the real artifact now
                if let Some(expected_short_path) = &promise_artifact_attr.short_path {
                    artifact.get_path().with_short_path(|artifact_short_path| {
                        if artifact_short_path != expected_short_path {
                            Err(buck2_error::Error::from(
                                PromiseArtifactResolveError::ShortPathMismatch(
                                    expected_short_path.clone(),
                                    artifact_short_path.to_string(),
                                ),
                            ))
                        } else {
                            Ok(())
                        }
                    })?;
                }

                let fulfilled_promise_inner =
                    PromiseArtifact::new(Arc::new(artifact.dupe().into()), promise_id);

                let fulfilled_promise_artifact = StarlarkPromiseArtifact::new(
                    None,
                    fulfilled_promise_inner,
                    promise_artifact_attr.short_path.clone(),
                    promise_artifact_attr.has_content_based_path,
                );
                // To resolve the promise artifact attr, we end up creating a new `StarlarkPromiseArtifact` with the `OnceLock` set
                // with the artifact that was found from the upstream analysis.
                Ok(ctx
                    .heap()
                    .alloc_representation(fulfilled_promise_artifact)
                    .to_value())
            }
            AnonTargetAttr::Label(label) => Ok(ctx
                .heap()
                .alloc_representation(StarlarkProvidersLabel::new(label.dupe()))
                .to_value()),
        }
    }
}

// Container for things that require looking up analysis results in order to resolve the attribute.
pub(crate) struct AnonTargetDependents {
    pub(crate) deps: Vec<ConfiguredTargetLabel>,
    pub(crate) promise_artifacts: Vec<PromiseArtifactAttr>,
}

pub(crate) trait AnonTargetAttrTraversal {
    fn promise_artifact(
        &mut self,
        promise_artifact: &PromiseArtifactAttr,
    ) -> buck2_error::Result<()>;
}

impl AnonTargetDependents {
    pub(crate) fn get_dependents(
        anon_target: &AnonTargetKey,
    ) -> buck2_error::Result<AnonTargetDependents> {
        struct DepTraversal(Vec<ConfiguredTargetLabel>);
        struct PromiseArtifactTraversal(Vec<PromiseArtifactAttr>);

        impl ConfiguredAttrTraversal for DepTraversal {
            fn dep(&mut self, dep: &ConfiguredProvidersLabel) -> buck2_error::Result<()> {
                self.0.push(dep.target().dupe());
                Ok(())
            }

            fn query(
                &mut self,
                _query: &str,
                _resolved_literals: &ResolvedQueryLiterals<ConfiguredProvidersLabel>,
            ) -> buck2_error::Result<()> {
                Err(AnonTargetsError::QueryMacroNotSupported.into())
            }
        }

        impl AnonTargetAttrTraversal for PromiseArtifactTraversal {
            fn promise_artifact(
                &mut self,
                promise_artifact: &PromiseArtifactAttr,
            ) -> buck2_error::Result<()> {
                self.0.push(promise_artifact.clone());
                Ok(())
            }
        }

        let mut dep_traversal = DepTraversal(Vec::new());
        let mut promise_artifact_traversal = PromiseArtifactTraversal(Vec::new());
        for x in anon_target.0.attrs().values() {
            x.traverse(anon_target.0.name().pkg(), &mut dep_traversal)?;
            x.traverse_anon_attr(&mut promise_artifact_traversal)?;
        }
        Ok(AnonTargetDependents {
            deps: dep_traversal.0,
            promise_artifacts: promise_artifact_traversal.0,
        })
    }

    pub(crate) async fn get_analysis_results<'v>(
        &'v self,
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<AnonTargetDependentAnalysisResults<'v>> {
        let dep_analysis_results =
            KeepGoing::try_compute_join_all(dice, self.deps.iter(), async |ctx, dep| {
                ctx.get_analysis_result(dep)
                    .await
                    .require_compatible()
                    .map(|r| (dep, r.dupe()))
            })
            .await?;
        let promised_artifacts: BuckMutMap<_, _> = {
            KeepGoing::try_compute_join_all(
                dice,
                self.promise_artifacts.iter(),
                async |ctx, promise_artifact_attr| {
                    get_artifact_from_anon_target_analysis(&promise_artifact_attr.id, ctx)
                        .await
                        .map(|artifact| (promise_artifact_attr, artifact.dupe()))
                },
            )
        }
        .await?
        .into_iter()
        .collect();

        Ok(AnonTargetDependentAnalysisResults {
            dep_analysis_results,
            promised_artifacts,
        })
    }
}
