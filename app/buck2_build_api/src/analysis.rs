/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::sync::Arc;

use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_build_signals::env::WaitingData;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_events::span::SpanId;
use buck2_hash::BuckFixedSortedMap;
use buck2_interpreter::starlark_profiler::data::StarlarkProfileDataAndStats;
use buck2_node::rule_type::RuleType;
use buck2_util::time_span::TimeSpan;
use pagable::DeferredValue;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use smallvec::SmallVec;
use starlark::pagable::DeferredFieldReadsEnabled;
use starlark::values::FrozenValueTyped;
use starlark::values::OwnedFrozen;
use starlark::values::ValueTyped;

use crate::analysis::registry::RecordedAnalysisValues;
use crate::artifact_groups::promise::PromiseArtifactId;

// TODO(@wendyy) move into `buck2_node`
pub mod anon_promises_dyn;
// TODO(@wendyy) move into `buck2_interpreter_for_build`
pub mod anon_targets_registry;
pub mod calculation;
pub mod extra_v;
pub mod registry;

use allocative::Allocative;
use dupe::Dupe;

use crate::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use crate::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValueRef;
use crate::interpreter::rule_defs::provider::collection::ProviderCollection;
use crate::validation::transitive_validations::TransitiveValidations;

/// Timestamps around promise resolution, for splitting analysis nodes.
pub struct AnalysisSplitInstants {
    pub pre_promises: std::time::Instant,
    pub post_promises: std::time::Instant,
}

/// DICE activation data recorded by analysis-like computations (rule analysis, anon
/// targets, dynamic outputs) and consumed by build signals.
pub struct AnalysisKeyActivationData {
    pub waiting_data: WaitingData,
    pub time_span: TimeSpan,
    pub spans: SmallVec<[SpanId; 1]>,
    pub analysis_with_extra_data: AnalysisWithExtraData,
    /// If this analysis resolved anon targets, contains timing data for the
    /// post-anon-targets portion. When present, `time_span` represents only
    /// the pre-anon-targets portion (part 1).
    pub anon_target_split: Option<AnonTargetSplitData>,
}

/// Timing data for the portion of analysis after anon target resolution.
pub struct AnonTargetSplitData {
    pub part2_time_span: TimeSpan,
}

#[derive(Clone)]
pub struct AnalysisWithExtraData {
    pub target_rule_type_name: Option<String>,
}

#[derive(Debug, Clone, Dupe, Allocative, pagable::Pagable)]
pub struct AnalysisResult {
    /// Shares the rule definition without retaining its configured dependency graph.
    /// Forward nodes reuse the underlying analysis result, including this metadata.
    rule_type: RuleType,
    /// With deferred field reads enabled, provider-only reads leave this
    /// payload unloaded, avoiding restoration of unrelated registered actions.
    analysis_values: DeferredAnalysisValues,
    /// Shares the collection and owning frozen heap with the provider root
    /// in `RecordedAnalysisValues`. Keeping this root independently accessible
    /// lets provider reads bypass `analysis_values`.
    /// `None` when the recorded values have no analysis storage.
    provider_collection: Option<OwnedFrozen<ValueTyped<'static, ProviderCollection<'static>>>>,
    /// Profiling data after running analysis, for this analysis only, without dependencies.
    /// `None` when profiling is disabled.
    /// For forward node, this value is shared with underlying analysis (including this field).
    #[pagable(discard = "None")]
    pub profile_data: Option<Arc<StarlarkProfileDataAndStats>>,
    promise_artifact_map: Arc<BuckFixedSortedMap<PromiseArtifactId, Artifact>>,
    pub num_declared_actions: u64,
    pub num_declared_artifacts: u64,
    /// `None` means there are no `ValidationInfo` providers in transitive dependencies.
    pub validations: Option<TransitiveValidations>,
    /// The anon targets requested (and therefore analyzed) during this analysis, deduplicated,
    /// in first-request order. Always `BaseDeferredKey::AnonTarget` entries holding the key
    /// allocation DICE shares between every requester of the same anon target.
    requested_anon_targets: Arc<[BaseDeferredKey]>,
}

#[derive(Debug, Clone, Allocative, pagable::PagableSerialize)]
struct DeferredAnalysisValues(DeferredValue<Arc<RecordedAnalysisValues>>);

impl Dupe for DeferredAnalysisValues {}

impl<'de> PagableDeserialize<'de> for DeferredAnalysisValues {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let enabled = deserializer
            .storage()
            .storage_context()
            .get::<DeferredFieldReadsEnabled>()
            .is_some();
        Ok(Self(DeferredValue::deserialize_arc(deserializer, enabled)?))
    }
}

impl AnalysisResult {
    /// Create a new AnalysisResult
    pub fn new(
        analysis_values: RecordedAnalysisValues,
        profile_data: Option<Arc<StarlarkProfileDataAndStats>>,
        promise_artifact_map: BuckFixedSortedMap<PromiseArtifactId, Artifact>,
        num_declared_actions: u64,
        num_declared_artifacts: u64,
        validations: Option<TransitiveValidations>,
        requested_anon_targets: Vec<BaseDeferredKey>,
        rule_type: RuleType,
    ) -> buck2_error::Result<Self> {
        let provider_collection = analysis_values
            .provider_collection_if_present()?
            .map(|providers| providers.to_owned().value);
        Ok(Self {
            rule_type,
            analysis_values: DeferredAnalysisValues(DeferredValue::new(Arc::new(analysis_values))),
            provider_collection,
            profile_data,
            promise_artifact_map: Arc::new(promise_artifact_map),
            num_declared_actions,
            num_declared_artifacts,
            validations,
            requested_anon_targets: requested_anon_targets.into(),
        })
    }

    pub fn requested_anon_targets(&self) -> &[BaseDeferredKey] {
        &self.requested_anon_targets
    }

    pub fn rule_type(&self) -> &RuleType {
        &self.rule_type
    }

    pub fn providers(&self) -> buck2_error::Result<FrozenProviderCollectionValueRef<'_>> {
        let value = self
            .provider_collection
            .as_ref()
            .ok_or_else(|| buck2_error::internal_error!("missing provider collection"))?;
        Ok(FrozenProviderCollectionValueRef::from_inner(
            value
                .as_ref()
                .map::<FrozenValueTyped<'static, ProviderCollection<'static>>, _>(|v| {
                    FrozenValueTyped::new(v.to_value()).expect("provider collection is frozen")
                }),
        ))
    }

    pub fn promise_artifact_map(&self) -> &BuckFixedSortedMap<PromiseArtifactId, Artifact> {
        &self.promise_artifact_map
    }

    /// Used to lookup an inner named provider result.
    pub fn lookup_inner(
        &self,
        label: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<FrozenProviderCollectionValue> {
        Ok(self.providers()?.lookup_inner(label)?.to_owned())
    }

    /// Resolves the providers `label` names, reading the providers root and
    /// one `DefaultInfo` per subtarget hop. Fails if a subtarget is missing.
    pub fn lookup(&self, label: &ConfiguredProvidersLabel) -> buck2_error::Result<ProvidersLookup> {
        Ok(ProvidersLookup {
            providers: self.lookup_inner(label)?,
            label: label.dupe(),
        })
    }

    pub fn analysis_values(&self) -> buck2_error::Result<&RecordedAnalysisValues> {
        Ok(self.analysis_values.0.read()?)
    }
}

/// The providers a label names, resolved once by [`AnalysisResult::lookup`].
/// Holding one proves the subtarget path exists and the collection is
/// resident. It reaches nothing else in the analysis result, so a holder
/// cannot read the result's other deferred fields.
#[derive(Debug, Clone, Dupe, Allocative, starlark::StarlarkPagable)]
pub struct ProvidersLookup {
    #[starlark_pagable(pagable)]
    label: ConfiguredProvidersLabel,
    providers: FrozenProviderCollectionValue,
}

impl ProvidersLookup {
    pub fn label(&self) -> &ConfiguredProvidersLabel {
        &self.label
    }

    pub fn providers(&self) -> &FrozenProviderCollectionValue {
        &self.providers
    }
}
