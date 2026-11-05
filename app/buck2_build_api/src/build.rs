/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use buck2_common::liveliness_observer::LivelinessObserver;
use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::pattern::pattern::Modifiers;
use buck2_core::pattern::pattern::TargetLabelWithModifiers;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorOptionContext;
use buck2_events::dispatch::console_message;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use dice::LinearRecomputeDiceComputations;
use dice::UserComputationData;
use dupe::Dupe;
use dupe::IterDupedExt;
use dupe::OptionDupedExt;
use futures::FutureExt;
use futures::future::Either;
use futures::stream::FuturesUnordered;
use futures::stream::StreamExt;
use itertools::Itertools;
use pagable::Pagable;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::actions::calculation::BuildKey;
use crate::analysis::calculation::RuleAnalysisCalculation;
use crate::artifact_groups::ArtifactGroupValues;
use crate::artifact_groups::ResolvedArtifactGroup;
use crate::artifact_groups::ResolvedArtifactGroupBuildSignalsKey;
use crate::artifact_groups::calculation::EnsureTransitiveSetProjectionKey;
use crate::build::detailed_aggregated_metrics::dice::HasDetailedAggregatedMetrics;
use crate::build::detailed_aggregated_metrics::types::TopLevelTargetSpec;
use crate::build::graph_properties::GraphPropertiesOptions;
use crate::build::graph_properties::GraphPropertiesValues;
use crate::build::outputs::get_outputs_for_top_level_target;
use crate::build_signals::HasBuildSignals;
use crate::interpreter::rule_defs::provider::builtin::run_info::OwnedRunInfo;
use crate::interpreter::rule_defs::provider::builtin::run_info::RunInfo;
use crate::keep_going::KeepGoing;
use crate::materialize::HasMaterializationQueueTracker;
use crate::materialize::MaterializationAndUploadContext;
use crate::materialize::materialize_and_upload_artifact_group;
use crate::validation::validation_impl::VALIDATION_IMPL;

mod action_error;
pub mod build_report;
pub mod build_report_metrics;
pub mod detailed_aggregated_metrics;
pub mod graph_properties;
pub mod outputs;
pub(crate) mod sketch_impl;

/// The types of provider to build on the configured providers label
#[derive(Debug, Clone, Dupe, Copy, Allocative, PartialEq, Eq, Hash, Pagable)]
pub enum BuildProviderType {
    Default,
    DefaultOther,
    Run,
    Test,
}

/// An output or error paired with the wall-clock elapsed time from build start
/// at which it was produced.
#[derive(Clone, Debug, pagable::Pagable)]
pub struct Timed<T> {
    pub inner: T,
    pub elapsed: Duration,
}

// Duration has no heap allocations, so Allocative is trivially empty.
impl<T: Allocative> Allocative for Timed<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        self.inner.visit(visitor);
    }
}

#[derive(Clone, Debug, Allocative)]
pub struct ConfiguredBuildTargetResultGen<T> {
    pub outputs: Vec<Timed<T>>,
    pub run_info: Option<OwnedRunInfo>,
    pub target_rule_type_name: Option<String>,
    pub graph_properties: Option<buck2_error::Result<MaybeCompatible<GraphPropertiesValues>>>,
    pub errors: Vec<Timed<buck2_error::Error>>,
}

impl<T> ConfiguredBuildTargetResultGen<T> {
    /// Wall-clock time from build start at which this target completed (or
    /// timed out), defined as the max elapsed time across all outputs and
    /// errors.
    pub fn wall_clock_completion(&self) -> Option<Duration> {
        self.outputs
            .iter()
            .map(|o| o.elapsed)
            .chain(self.errors.iter().map(|e| e.elapsed))
            .max()
    }

    /// Whether this target hit the `--overall-timeout` deadline, as opposed to building
    /// successfully or failing for some other reason.
    pub fn timed_out(&self) -> bool {
        self.errors
            .iter()
            .any(|e| e.inner.has_tag(buck2_error::ErrorTag::BuildDeadlineExpired))
    }
}

pub type ConfiguredBuildTargetResult =
    ConfiguredBuildTargetResultGen<buck2_error::Result<ProviderArtifacts>>;

pub enum FailFastState {
    Continue,
    /// An error in this target stops a fail-fast build.
    Breakpoint(TargetLabel),
}

pub struct AsyncBuildTargetResultBuilder {
    event_rx: UnboundedReceiver<BuildEvent>,
    builder: BuildTargetResultBuilder,
}

impl AsyncBuildTargetResultBuilder {
    pub fn new(
        mut streaming_build_result_tx: Option<UnboundedSender<BuildTargetResult>>,
        build_start: Instant,
    ) -> (Self, impl BuildEventConsumer + Clone) {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        #[derive(Clone)]
        struct EventConsumer {
            event_tx: UnboundedSender<BuildEvent>,
        }

        impl BuildEventConsumer for EventConsumer {
            fn consume(&self, ev: BuildEvent) {
                let _ignored = self.event_tx.send(ev);
            }
        }

        (
            Self {
                event_rx,
                builder: BuildTargetResultBuilder::new(
                    streaming_build_result_tx.take(),
                    build_start,
                ),
            },
            EventConsumer { event_tx },
        )
    }

    pub async fn wait_for(
        mut self,
        fail_fast: bool,
        fut: impl Future<Output = ()>,
    ) -> buck2_error::Result<BuildTargetResult> {
        let mut fut = pin!(fut);
        loop {
            tokio::select! {
                event = self.event_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let FailFastState::Breakpoint(target) = self.builder.event(event)? {
                                if fail_fast {
                                    // Analysis errors and validation failures write no streaming
                                    // line of their own.
                                    self.builder.send_snapshot_for(target);
                                    break;
                                }
                            }
                        }
                        None => {
                            // Intentionally don't break early in this case.
                            // The None indicates that the event sender has been dropped, but a caller is going to expect
                            // the future to be driven to completion except for in the fail_fast case.
                        }
                    }
                }
                _ = &mut fut => {
                    // The future is done, but make sure to drain the queue of events.
                    // Unlike poll_recv, try_recv never spuriously returns empty.
                    while let Ok(event) = self.event_rx.try_recv() {
                        self.builder.event(event)?;
                    }
                    break;
                }
            }
        }

        Ok(self.builder.build())
    }
}

pub struct BuildTargetResultBuilder {
    res: BuckMutMap<
        ConfiguredProvidersLabel,
        Option<ConfiguredBuildTargetResultGen<(usize, buck2_error::Result<ProviderArtifacts>)>>,
    >,
    configured_to_pattern_modifiers: BuckMutMap<ConfiguredProvidersLabel, Vec<Modifiers>>,
    other_errors: BTreeMap<Option<ProvidersLabel>, Vec<buck2_error::Error>>,
    build_failed: bool,
    incompatible_targets: BuckMutInsertionSet<ConfiguredTargetLabel>,
    labels_by_target: BuckMutMap<TargetLabel, BuckMutInsertionSet<ConfiguredProvidersLabel>>,
    /// Kept by target so that a streaming line takes one target's entries without scanning the
    /// rest, which would make streaming quadratic in the number of targets.
    finished_builds_by_target: BuckMutMap<TargetLabel, FinishedBuilds>,
    streaming_build_result_tx: Option<UnboundedSender<BuildTargetResult>>,
    build_start: Instant,
}

impl BuildTargetResultBuilder {
    pub fn new(
        mut streaming_build_result_tx: Option<UnboundedSender<BuildTargetResult>>,
        build_start: Instant,
    ) -> Self {
        Self {
            res: BuckMutMap::default(),
            configured_to_pattern_modifiers: BuckMutMap::default(),
            other_errors: BTreeMap::new(),
            incompatible_targets: BuckMutInsertionSet::new(),
            labels_by_target: BuckMutMap::default(),
            finished_builds_by_target: BuckMutMap::default(),
            build_failed: false,
            streaming_build_result_tx: streaming_build_result_tx.take(),
            build_start,
        }
    }

    fn send_snapshot_for(&self, target: TargetLabel) {
        if let Some(tx) = &self.streaming_build_result_tx {
            let _ignored = tx.send(self.build_for_target(target));
        }
    }

    pub fn event(&mut self, event: BuildEvent) -> buck2_error::Result<FailFastState> {
        let ConfiguredBuildEvent { variant, label } = match event {
            BuildEvent::Configured(variant) => variant,
            BuildEvent::OtherError { label: target, err } => {
                self.other_errors.entry(target).or_default().push(err);
                self.build_failed = true;
                // TODO(cjhopman): Why don't we break here?
                return Ok(FailFastState::Continue);
            }
            BuildEvent::ConfiguredTargetBuildFinished { target, modifiers } => {
                let unconfigured = target.unconfigured().dupe();
                self.finished_builds_by_target
                    .entry(unconfigured.dupe())
                    .or_default()
                    .configured_targets
                    .insert((target, modifiers));
                self.send_snapshot_for(unconfigured);
                return Ok(FailFastState::Continue);
            }
            BuildEvent::TargetBuildFinished { target } => {
                let unconfigured = target.target_label.dupe();
                self.finished_builds_by_target
                    .entry(unconfigured.dupe())
                    .or_default()
                    .targets
                    .insert(target);
                self.send_snapshot_for(unconfigured);
                return Ok(FailFastState::Continue);
            }
        };
        self.labels_by_target
            .entry(label.target().unconfigured().dupe())
            .or_default()
            .insert(label.dupe());
        let elapsed = Instant::now() - self.build_start;
        match variant {
            ConfiguredBuildEventVariant::SkippedIncompatible => {
                self.incompatible_targets.insert(label.target().dupe());
                self.res.entry(label.dupe()).or_insert(None);
            }
            ConfiguredBuildEventVariant::MapModifiers { modifiers } => {
                self.configured_to_pattern_modifiers
                    .entry(label.dupe())
                    .or_default()
                    .push(modifiers);
            }
            ConfiguredBuildEventVariant::Prepared {
                run_info,
                target_rule_type_name,
            } => {
                self.res
                    .entry(label.dupe())
                    .or_insert(Some(ConfiguredBuildTargetResultGen {
                        outputs: Vec::new(),
                        run_info,
                        target_rule_type_name: Some(target_rule_type_name),
                        graph_properties: None,
                        errors: Vec::new(),
                    }));
            }
            ConfiguredBuildEventVariant::Execution(execution_variant) => {
                let is_err = {
                    let results = self.res.get_mut(&label)
                        .with_internal_error(|| format!("ConfiguredBuildEventVariant::Execution before ConfiguredBuildEventVariant::Prepared for {label}"))?
                        .as_mut()
                        .with_internal_error(|| format!("ConfiguredBuildEventVariant::Execution for a skipped target: `{label}`"))?;
                    match execution_variant {
                        ConfiguredBuildEventExecutionVariant::Validation { result } => {
                            if let Err(e) = result {
                                results.errors.push(Timed { inner: e, elapsed });
                                true
                            } else {
                                false
                            }
                        }
                        ConfiguredBuildEventExecutionVariant::BuildOutput { index, output } => {
                            let is_err = output.is_err();
                            results.outputs.push(Timed {
                                inner: (index, output),
                                elapsed,
                            });
                            self.send_snapshot_for(label.target().unconfigured().dupe());

                            is_err
                        }
                    }
                };
                if is_err {
                    self.build_failed = true;
                    return Ok(FailFastState::Breakpoint(
                        label.target().unconfigured().dupe(),
                    ));
                }
            }
            ConfiguredBuildEventVariant::GraphProperties { graph_properties } => {
                self.res.get_mut(&label)
                     .with_internal_error(|| format!("ConfiguredBuildEventVariant::GraphProperties before ConfiguredBuildEventVariant::Prepared for {label}"))?
                     .as_mut()
                     .with_internal_error(|| format!("ConfiguredBuildEventVariant::GraphProperties for a skipped target: `{label}`"))?
                     .graph_properties = Some(graph_properties);
            }
            ConfiguredBuildEventVariant::Timeout => {
                let results = self.res.get_mut(&label)
                     .with_internal_error(|| format!("ConfiguredBuildEventVariant::Timeout before ConfiguredBuildEventVariant::Prepared for {label}"))?
                     .as_mut()
                     .with_internal_error(|| format!("ConfiguredBuildEventVariant::Timeout for a skipped target: `{label}`"))?;
                results.errors.push(Timed {
                    inner: buck2_error::Error::from(BuildDeadlineExpired),
                    elapsed,
                });
                // TODO(cjhopman): Why don't we break here?
                self.build_failed = true;
            }
            ConfiguredBuildEventVariant::Error { err } => {
                self.build_failed = true;
                self.res
                    .entry(label.dupe())
                    .or_insert(Some(ConfiguredBuildTargetResultGen {
                        outputs: Vec::new(),
                        run_info: None,
                        target_rule_type_name: None,
                        graph_properties: None,
                        errors: Vec::new(),
                    }))
                    .as_mut()
                    .unwrap()
                    .errors
                    .push(Timed {
                        inner: err,
                        elapsed,
                    });
                return Ok(FailFastState::Breakpoint(
                    label.target().unconfigured().dupe(),
                ));
            }
        }
        Ok(FailFastState::Continue)
    }

    pub fn build(&self) -> BuildTargetResult {
        // The whole result, for the final build report; streaming build report lines come from
        // `build_for_target`. It intentionally does not consume self and copies the arrays in the
        // return object.

        if !self.incompatible_targets.is_empty() {
            // TODO(cjhopman): Probably better to return this in the result and let the caller decide what to do with it.
            console_message(IncompatiblePlatformReason::skipping_message_for_multiple(
                &self.incompatible_targets,
            ));
        }

        // Sort our outputs within each individual BuildTargetResult, then return those.
        // Also, turn our BuckMutMap into a BTreeMap.
        let res = self
            .res
            .iter()
            .map(|(label, result)| (label.clone(), result.as_ref().map(Self::configured_result)))
            .collect();

        let configured_to_pattern_modifiers = self
            .configured_to_pattern_modifiers
            .iter()
            .map(|(label, modifiers)| {
                (
                    label.clone(),
                    BTreeSet::from_iter(modifiers.iter().cloned()),
                )
            })
            .collect();

        let finished_builds = FinishedBuilds {
            configured_targets: self
                .finished_builds_by_target
                .values()
                .flat_map(|finished| finished.configured_targets.iter().cloned())
                .collect(),
            targets: self
                .finished_builds_by_target
                .values()
                .flat_map(|finished| finished.targets.iter().cloned())
                .collect(),
        };

        BuildTargetResult {
            configured: res,
            configured_to_pattern_modifiers,
            other_errors: self.other_errors.clone(),
            finished_builds,
            build_failed: self.build_failed,
        }
    }

    fn configured_result(
        result: &ConfiguredBuildTargetResultGen<(usize, buck2_error::Result<ProviderArtifacts>)>,
    ) -> ConfiguredBuildTargetResult {
        // TODO: This whole building thing needs quite a bit of
        // refactoring. We might request the same targets multiple
        // times here, but since we know that ConfiguredTargetLabel
        // -> Output is going to be deterministic, we just dedupe
        // them using the index, keeping the min elapsed time (this
        // is somewhat arbitrary but the outputs are all secretly
        // the "same" output anyway, and keeping the min elapsed
        // time ensures we don't report a time then update it to
        // "later" in another call).
        let mut indexed: Vec<_> = result
            .outputs
            .iter()
            .map(|timed| {
                let (index, output) = &timed.inner;
                (*index, output.clone(), timed.elapsed)
            })
            .collect();
        indexed.sort_unstable_by_key(|(index, _, _)| *index);

        let outputs: Vec<_> = indexed
            .into_iter()
            .chunk_by(|(index, _, _)| *index)
            .into_iter()
            .map(|(_index, group)| {
                let (_, output, elapsed) = group.min_by_key(|(_, _, elapsed)| *elapsed).unwrap();
                Timed {
                    inner: output.clone(),
                    elapsed,
                }
            })
            .collect();

        ConfiguredBuildTargetResult {
            outputs,
            run_info: result.run_info.dupe(),
            target_rule_type_name: result.target_rule_type_name.clone(),
            graph_properties: result.graph_properties.clone(),
            errors: result.errors.clone(),
        }
    }

    /// The part of the result that belongs to `target`. Errors that name no target are included
    /// too, since they have no target of their own to be reported with.
    fn build_for_target(&self, target: TargetLabel) -> BuildTargetResult {
        let labels = self.labels_by_target.get(&target).into_iter().flatten();

        let configured = labels
            .clone()
            .filter_map(|label| {
                let result = self.res.get(label)?;
                Some((label.clone(), result.as_ref().map(Self::configured_result)))
            })
            .collect();

        let configured_to_pattern_modifiers = labels
            .filter_map(|label| {
                let modifiers = self.configured_to_pattern_modifiers.get(label)?;
                Some((
                    label.clone(),
                    BTreeSet::from_iter(modifiers.iter().cloned()),
                ))
            })
            .collect();

        let other_errors = self
            .other_errors
            .iter()
            .filter(|(label, _)| label.as_ref().is_none_or(|l| *l.target() == target))
            .map(|(label, errors)| (label.clone(), errors.clone()))
            .collect();

        BuildTargetResult {
            configured,
            configured_to_pattern_modifiers,
            other_errors,
            finished_builds: self
                .finished_builds_by_target
                .get(&target)
                .cloned()
                .unwrap_or_default(),
            build_failed: self.build_failed,
        }
    }
}

/// Targets whose builds have ended, as reported by the driver that owns their futures.
///
/// The builder cannot work this out itself: it sees each provider label separately, and a
/// target's labels can register after some of them have already finished.
#[derive(Clone, Default)]
pub struct FinishedBuilds {
    /// Every provider label of the configured target has finished building for one requested
    /// pattern (target and modifiers). Patterns whose modifiers resolve to the same configuration
    /// are built separately, so each one finishes on its own.
    pub configured_targets: BTreeSet<(ConfiguredTargetLabel, Modifiers)>,
    /// Every provider label of the target has finished building or failed to configure.
    pub targets: BTreeSet<TargetLabelWithModifiers>,
}

pub struct BuildTargetResult {
    pub configured: BTreeMap<ConfiguredProvidersLabel, Option<ConfiguredBuildTargetResult>>,
    pub configured_to_pattern_modifiers: BuckMutMap<ConfiguredProvidersLabel, BTreeSet<Modifiers>>,
    /// Errors that could not be associated with a specific configured target. These errors may be
    /// associated with a providers label, or might not be associated with any target at all.
    pub other_errors: BTreeMap<Option<ProvidersLabel>, Vec<buck2_error::Error>>,
    pub finished_builds: FinishedBuilds,
    pub build_failed: bool,
}

impl BuildTargetResult {
    pub fn new() -> Self {
        Self {
            configured: BTreeMap::new(),
            configured_to_pattern_modifiers: BuckMutMap::default(),
            other_errors: BTreeMap::new(),
            finished_builds: FinishedBuilds::default(),
            build_failed: false,
        }
    }

    pub fn extend(&mut self, other: BuildTargetResult) {
        self.configured.extend(other.configured);
        self.other_errors.extend(other.other_errors);
        self.finished_builds
            .configured_targets
            .extend(other.finished_builds.configured_targets);
        self.finished_builds
            .targets
            .extend(other.finished_builds.targets);

        for (label, modifiers_set) in other.configured_to_pattern_modifiers {
            self.configured_to_pattern_modifiers
                .entry(label)
                .or_default()
                .extend(modifiers_set);
        }
    }

    /// Ignores `finished_builds`: `buck test` reads an empty result as "nothing was built", which
    /// finish events alone don't change.
    pub fn is_empty(&self) -> bool {
        self.configured.is_empty() && self.other_errors.is_empty()
    }
}

pub enum ConfiguredBuildEventExecutionVariant {
    BuildOutput {
        output: buck2_error::Result<ProviderArtifacts>,
        /// Ensure a stable ordering of outputs.
        index: usize,
    },
    Validation {
        result: buck2_error::Result<()>,
    },
}

pub enum ConfiguredBuildEventVariant {
    SkippedIncompatible,
    MapModifiers {
        modifiers: Modifiers,
    },
    Prepared {
        run_info: Option<OwnedRunInfo>,
        target_rule_type_name: String,
    },
    Execution(ConfiguredBuildEventExecutionVariant),
    GraphProperties {
        graph_properties: buck2_error::Result<MaybeCompatible<GraphPropertiesValues>>,
    },
    Error {
        /// An error that can't be associated with a single artifact.
        err: buck2_error::Error,
    },
    // This target did not build within the allocated time.
    Timeout,
}

/// Events to be accumulated using BuildTargetResult::collect_stream.
pub struct ConfiguredBuildEvent {
    label: ConfiguredProvidersLabel,
    variant: ConfiguredBuildEventVariant,
}

pub enum BuildEvent {
    Configured(ConfiguredBuildEvent),
    // An error that cannot be associated with a specific configured target
    OtherError {
        label: Option<ProvidersLabel>,
        err: buck2_error::Error,
    },
    /// Every provider label of this configured target has finished building, successfully or not,
    /// for one requested pattern (target and modifiers).
    ConfiguredTargetBuildFinished {
        target: ConfiguredTargetLabel,
        modifiers: Modifiers,
    },
    /// Every provider label of this target has finished building or failed to configure.
    TargetBuildFinished {
        target: TargetLabelWithModifiers,
    },
}

impl BuildEvent {
    pub fn new_configured(
        label: ConfiguredProvidersLabel,
        variant: ConfiguredBuildEventVariant,
    ) -> Self {
        Self::Configured(ConfiguredBuildEvent { label, variant })
    }
}

pub trait BuildEventConsumer: Sync {
    fn consume(&self, ev: BuildEvent);
    fn consume_configured(&self, ev: ConfiguredBuildEvent) {
        self.consume(BuildEvent::Configured(ev))
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = BuildDeadlineExpired)]
#[error("Build timed out")]
struct BuildDeadlineExpired;

#[derive(Copy, Clone, Dupe, Debug)]
pub struct BuildConfiguredLabelOptions {
    pub skippable: bool,
    pub graph_properties: GraphPropertiesOptions,
    /// Resolve the target's run command line (`run_args`). Set only by `buck run`;
    pub return_run_args: bool,
}

pub async fn build_configured_label(
    event_consumer: &dyn BuildEventConsumer,
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    materialization_and_upload: MaterializationAndUploadContext,
    providers_label: ConfiguredProvidersLabel,
    providers_to_build: &ProvidersToBuild,
    opts: BuildConfiguredLabelOptions,
    timeout_observer: Option<&Arc<dyn LivelinessObserver>>,
) {
    if let Err(e) = build_configured_label_inner(
        event_consumer,
        ctx,
        materialization_and_upload,
        providers_label.dupe(),
        providers_to_build,
        opts,
        timeout_observer,
    )
    .await
    {
        event_consumer.consume_configured(ConfiguredBuildEvent {
            label: providers_label,
            variant: ConfiguredBuildEventVariant::Error { err: e },
        });
    }
}

async fn build_configured_label_inner(
    event_consumer: &dyn BuildEventConsumer,
    ctx: LinearRecomputeDiceComputations<'_, '_>,
    materialization_and_upload: MaterializationAndUploadContext,
    providers_label: ConfiguredProvidersLabel,
    providers_to_build: &ProvidersToBuild,
    opts: BuildConfiguredLabelOptions,
    timeout_observer: Option<&'_ Arc<dyn LivelinessObserver>>,
) -> buck2_error::Result<()> {
    let outputs = match get_outputs_for_top_level_target(
        &mut ctx.get(),
        &providers_label,
        providers_to_build,
    )
    .await?
    {
        MaybeCompatible::Incompatible(reason) => {
            if opts.skippable {
                tracing::debug!("{}", reason.skipping_message(providers_label.target()));
                event_consumer.consume_configured(ConfiguredBuildEvent {
                    label: providers_label.dupe(),
                    variant: ConfiguredBuildEventVariant::SkippedIncompatible,
                });
                return Ok(());
            } else {
                return Err(reason.to_err());
            };
        }
        MaybeCompatible::Compatible(v) => v,
    };

    let target_rule_type_name = outputs.rule_type.name().to_owned();
    let outputs = outputs.outputs;
    ctx.get().top_level_target(TopLevelTargetSpec {
        label: providers_label.dupe(),
        outputs: outputs.dupe(),
    })?;

    let run_info: Option<OwnedRunInfo> = if opts.return_run_args {
        ctx.get()
            .get_providers(&providers_label)
            .await?
            .require_compatible()?
            .builtin_provider_value::<RunInfo>()?
    } else {
        None
    };
    if let Some(signals) = ctx
        .get()
        .per_transaction_data()
        .get_build_signals()
        .cloned()
    {
        let resolved_artifacts: Vec<_> =
            tokio::task::unconstrained(KeepGoing::try_compute_join_all(
                &mut ctx.get(),
                outputs.iter(),
                async |ctx, (output, _type)| output.resolved_artifact(ctx).await,
            ))
            .await?;
        let node_keys = resolved_artifacts
            .iter()
            .filter_map(|resolved| match resolved.dupe() {
                ResolvedArtifactGroup::Artifact(artifact) => artifact
                    .action_key()
                    .duped()
                    .map(BuildKey)
                    .map(ResolvedArtifactGroupBuildSignalsKey::BuildKey),
                ResolvedArtifactGroup::TransitiveSetProjection(key) => Some(
                    ResolvedArtifactGroupBuildSignalsKey::EnsureTransitiveSetProjectionKey(
                        EnsureTransitiveSetProjectionKey(key.dupe().dupe()),
                    ),
                ),
            })
            .collect();

        signals.top_level_target(providers_label.target().dupe(), node_keys);
    }

    if !opts.skippable && outputs.is_empty() {
        let docs = "https://buck2.build/docs/users/faq/common_issues/#why-does-my-target-not-have-any-outputs"; // @oss-enable
        // @oss-disable: let docs = "https://www.internalfb.com/intern/staticdocs/buck2/docs/users/faq/common_issues/#why-does-my-target-not-have-any-outputs";
        console_message(format!(
            "Target {} does not have any outputs. This means the rule did not define any outputs. See {} for more information",
            providers_label.target(),
            docs,
        ));
    }

    let queue_tracker = ctx
        .get()
        .per_transaction_data()
        .get_materialization_queue_tracker();

    let mut outputs: Vec<_> = outputs
        .iter()
        .duped()
        .enumerate()
        .map(|(index, (output, provider_type))| {
            let queue_tracker = queue_tracker.dupe();

            let fut = ctx.spawned(move |ctx, _cancellations| {
                async move {
                    materialize_and_upload_artifact_group(
                        ctx,
                        &output,
                        materialization_and_upload,
                        &queue_tracker,
                    )
                    .await
                }
                .boxed()
            });

            Either::Left(fut.map(move |v| {
                let res = match v {
                    Ok(values) => Ok(ProviderArtifacts {
                        values,
                        provider_type,
                    }),
                    Err(e) => Err(e),
                };
                ConfiguredBuildEventExecutionVariant::BuildOutput { index, output: res }
            }))
        })
        .collect();

    let validation_result = {
        let validation_impl = VALIDATION_IMPL.get()?;
        let mut ctx = ctx.get();
        let target = providers_label.target().dupe();
        Either::Right(async move {
            let result = validation_impl
                .validate_target_node_transitively(&mut ctx, target)
                .await;
            ConfiguredBuildEventExecutionVariant::Validation { result }
        })
    };

    outputs.push(validation_result);

    event_consumer.consume_configured(ConfiguredBuildEvent {
        label: providers_label.dupe(),
        variant: ConfiguredBuildEventVariant::Prepared {
            run_info,
            target_rule_type_name,
        },
    });

    let mut outputs: FuturesUnordered<_> = outputs
        .into_iter()
        .map(|build| async move {
            let build = build.map(ConfiguredBuildEventVariant::Execution);
            match timeout_observer {
                Some(timeout_observer) => {
                    let alive = timeout_observer
                        .while_alive()
                        .map(|()| ConfiguredBuildEventVariant::Timeout);
                    futures::pin_mut!(alive);
                    futures::pin_mut!(build);
                    futures::future::select(alive, build)
                        .map(|r| r.factor_first().0)
                        .await
                }
                None => build.await,
            }
        })
        .collect();

    while let Some(variant) = tokio::task::unconstrained(outputs.next()).await {
        event_consumer.consume_configured(ConfiguredBuildEvent {
            label: providers_label.dupe(),
            variant,
        })
    }

    if !opts.graph_properties.is_empty() {
        let graph_properties = graph_properties::get_graph_properties(
            &mut ctx.get(),
            providers_label.target(),
            opts.graph_properties
                .should_compute_configured_graph_sketch(),
            opts.graph_properties.retained_analysis_memory_sketch,
            opts.graph_properties.peak_analysis_memory_sketch,
            opts.graph_properties.peak_load_memory_sketch,
        )
        .await
        .ok();

        event_consumer.consume_configured(ConfiguredBuildEvent {
            label: providers_label,
            variant: ConfiguredBuildEventVariant::GraphProperties { graph_properties },
        });
    }

    Ok(())
}

#[derive(Clone, Allocative)]
pub struct ProviderArtifacts {
    pub values: ArtifactGroupValues,
    pub provider_type: BuildProviderType,
}

// what type of artifacts to build based on the provider it came from
#[derive(Default, Allocative, Debug, Clone, Dupe, Eq, PartialEq, Hash, Pagable)]
pub struct ProvidersToBuild {
    pub default: bool,
    pub default_other: bool,
    /// Build the target's `RunInfo` input artifacts, so it is runnable. The `RunInfo` member of the
    /// "which provider outputs to build" family ({default, default_other, run, tests}); distinct from
    /// `BuildConfiguredLabelOptions::return_run_args`, which resolves the run command line.
    pub run: bool,
    pub tests: bool,
}

impl Debug for ProviderArtifacts {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderArtifacts")
            .field("values", &self.values.iter().collect::<Vec<_>>())
            .field("provider_type", &self.provider_type)
            .finish()
    }
}

pub trait HasCreateUnhashedSymlinkLock {
    fn set_create_unhashed_symlink_lock(&mut self, lock: Arc<Mutex<()>>);

    fn get_create_unhashed_symlink_lock(&self) -> Arc<Mutex<()>>;
}

impl HasCreateUnhashedSymlinkLock for UserComputationData {
    fn set_create_unhashed_symlink_lock(&mut self, lock: Arc<Mutex<()>>) {
        self.data.set(lock);
    }

    fn get_create_unhashed_symlink_lock(&self) -> Arc<Mutex<()>> {
        self.data
            .get::<Arc<Mutex<()>>>()
            .expect("Lock for creating unhashed symlinks should be set")
            .dupe()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Instant;

    use buck2_core::configuration::data::ConfigurationData;
    use buck2_core::pattern::pattern::Modifiers;
    use buck2_core::pattern::pattern::TargetLabelWithModifiers;
    use buck2_core::provider::label::ConfiguredProvidersLabel;
    use buck2_core::provider::label::ProvidersName;
    use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
    use buck2_error::ErrorTag;
    use buck2_error::buck2_error;
    use dupe::Dupe;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::AsyncBuildTargetResultBuilder;
    use super::BuildEvent;
    use super::BuildEventConsumer;
    use super::BuildTargetResult;
    use super::BuildTargetResultBuilder;
    use super::ConfiguredBuildEventVariant;
    use super::FailFastState;

    fn configured_target(name: &str) -> ConfiguredTargetLabel {
        ConfiguredTargetLabel::testing_parse(name, ConfigurationData::testing_new())
    }

    fn macos_arm() -> Modifiers {
        Modifiers::parse("root//:macos+root//:arm")
    }

    fn target(
        configured: &ConfiguredTargetLabel,
        modifiers: &Modifiers,
    ) -> TargetLabelWithModifiers {
        TargetLabelWithModifiers {
            target_label: configured.unconfigured().dupe(),
            modifiers: modifiers.dupe(),
        }
    }

    fn builder_with_sender() -> (
        BuildTargetResultBuilder,
        UnboundedReceiver<BuildTargetResult>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (BuildTargetResultBuilder::new(Some(tx), Instant::now()), rx)
    }

    fn feed(builder: &mut BuildTargetResultBuilder, event: BuildEvent) {
        let state = builder.event(event).unwrap();
        assert!(
            matches!(state, FailFastState::Continue),
            "only build errors stop a fail-fast build"
        );
    }

    fn prepared(label: &ConfiguredProvidersLabel) -> BuildEvent {
        BuildEvent::new_configured(
            label.dupe(),
            ConfiguredBuildEventVariant::Prepared {
                run_info: None,
                target_rule_type_name: "genrule".to_owned(),
            },
        )
    }

    fn labels(result: &BuildTargetResult) -> Vec<&ConfiguredProvidersLabel> {
        result.configured.keys().collect()
    }

    fn configured_finished(
        configured: &ConfiguredTargetLabel,
        modifiers: &Modifiers,
    ) -> BuildEvent {
        BuildEvent::ConfiguredTargetBuildFinished {
            target: configured.dupe(),
            modifiers: modifiers.dupe(),
        }
    }

    fn feed_finish_events(
        builder: &mut BuildTargetResultBuilder,
        configured: &ConfiguredTargetLabel,
        modifiers: &Modifiers,
    ) {
        feed(builder, configured_finished(configured, modifiers));
        feed(
            builder,
            BuildEvent::TargetBuildFinished {
                target: target(configured, modifiers),
            },
        );
    }

    fn sent(rx: &mut UnboundedReceiver<BuildTargetResult>) -> Vec<BuildTargetResult> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn assert_recorded(
        result: &BuildTargetResult,
        configured: &ConfiguredTargetLabel,
        modifiers: &Modifiers,
    ) {
        assert_eq!(
            result.finished_builds.configured_targets,
            BTreeSet::from([(configured.dupe(), modifiers.dupe())])
        );
        assert_eq!(
            result.finished_builds.targets,
            BTreeSet::from([target(configured, modifiers)])
        );
        assert!(
            result.is_empty(),
            "finish events alone add no build results or errors"
        );
    }

    #[test]
    fn test_finish_events_send_snapshots() {
        let configured = configured_target("root//pkg:a");
        let (mut builder, mut rx) = builder_with_sender();

        feed_finish_events(&mut builder, &configured, &macos_arm());

        let snapshots = sent(&mut rx);
        assert_eq!(snapshots.len(), 2, "one snapshot per finish event");
        assert_eq!(
            snapshots[0].finished_builds.configured_targets,
            BTreeSet::from([(configured.dupe(), macos_arm())])
        );
        assert!(
            snapshots[0].finished_builds.targets.is_empty(),
            "the first snapshot predates TargetBuildFinished"
        );
        assert_recorded(&snapshots[1], &configured, &macos_arm());
        assert_recorded(&builder.build(), &configured, &macos_arm());
    }

    #[test]
    fn test_finish_snapshots_hold_only_the_finishing_target() {
        let a = configured_target("root//pkg:a");
        let b = configured_target("root//pkg:b");
        let a_label = ConfiguredProvidersLabel::new(a.dupe(), ProvidersName::Default);
        let b_label = ConfiguredProvidersLabel::new(b.dupe(), ProvidersName::Default);
        let (mut builder, mut rx) = builder_with_sender();
        feed(&mut builder, prepared(&a_label));
        feed(&mut builder, prepared(&b_label));
        feed(
            &mut builder,
            BuildEvent::OtherError {
                label: Some(b_label.unconfigured()),
                err: buck2_error!(ErrorTag::Input, "b failed"),
            },
        );
        feed(
            &mut builder,
            BuildEvent::OtherError {
                label: None,
                err: buck2_error!(ErrorTag::Input, "no target"),
            },
        );
        feed_finish_events(&mut builder, &b, &macos_arm());
        let b_snapshots = sent(&mut rx);
        assert_eq!(b_snapshots.len(), 2, "one snapshot per finish event");
        for snapshot in &b_snapshots {
            assert_eq!(labels(snapshot), [&b_label]);
        }

        feed_finish_events(&mut builder, &a, &macos_arm());

        let a_snapshots = sent(&mut rx);
        assert_eq!(a_snapshots.len(), 2, "one snapshot per finish event");
        for snapshot in &a_snapshots {
            assert_eq!(
                labels(snapshot),
                [&a_label],
                "b's results stay off a's lines"
            );
            assert_eq!(
                snapshot.other_errors.keys().collect::<Vec<_>>(),
                [&None],
                "b's error stays off a's lines; an error naming no target is on every line"
            );
            assert_eq!(
                snapshot.finished_builds.configured_targets,
                BTreeSet::from([(a.dupe(), macos_arm())]),
                "b's finish events stay off a's lines"
            );
        }
        assert!(
            a_snapshots[0].finished_builds.targets.is_empty(),
            "the first snapshot predates a's TargetBuildFinished, and b's is not a's"
        );
        assert_eq!(
            a_snapshots[1].finished_builds.targets,
            BTreeSet::from([target(&a, &macos_arm())])
        );

        let result = builder.build();
        assert_eq!(labels(&result), [&a_label, &b_label]);
        assert_eq!(
            result.finished_builds.configured_targets,
            BTreeSet::from([(a.dupe(), macos_arm()), (b.dupe(), macos_arm())]),
            "the final result still holds every target's finish events"
        );
        assert_eq!(
            result.finished_builds.targets,
            BTreeSet::from([target(&a, &macos_arm()), target(&b, &macos_arm())])
        );
    }

    async fn wait_for_analysis_error(
        fail_fast: bool,
        label: &ConfiguredProvidersLabel,
    ) -> (BuildTargetResult, Vec<BuildTargetResult>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut builder, consumer) = AsyncBuildTargetResultBuilder::new(Some(tx), Instant::now());
        // Fed directly: a second queued event would be handled after `yield_now` is ready, and
        // `select!` could then finish the build future first.
        feed(&mut builder.builder, prepared(&other_label()));
        consumer.consume(BuildEvent::new_configured(
            label.dupe(),
            ConfiguredBuildEventVariant::Error {
                err: buck2_error!(ErrorTag::Input, "analysis failed"),
            },
        ));

        // `yield_now` is pending on its first poll, so `wait_for` handles the error before the
        // build future completes.
        let result = builder
            .wait_for(fail_fast, tokio::task::yield_now())
            .await
            .unwrap();
        (result, sent(&mut rx))
    }

    /// A target that is still building when the analysis error stops the build.
    fn other_label() -> ConfiguredProvidersLabel {
        ConfiguredProvidersLabel::new(configured_target("root//pkg:b"), ProvidersName::Default)
    }

    fn error_messages(result: &BuildTargetResult, label: &ConfiguredProvidersLabel) -> Vec<String> {
        result.configured[label]
            .as_ref()
            .expect("an analysis error records a result for its label")
            .errors
            .iter()
            .map(|e| e.inner.to_string())
            .collect()
    }

    #[tokio::test]
    async fn test_fail_fast_breakpoint_sends_snapshot() {
        let label =
            ConfiguredProvidersLabel::new(configured_target("root//pkg:a"), ProvidersName::Default);

        let (result, snapshots) = wait_for_analysis_error(true, &label).await;
        assert_eq!(
            snapshots.len(),
            1,
            "the fail-fast breakpoint sends exactly one snapshot"
        );
        assert_eq!(
            labels(&snapshots[0]),
            [&label],
            "the snapshot carries only the target that stopped the build"
        );
        assert_eq!(error_messages(&snapshots[0], &label), ["analysis failed"]);
        assert_eq!(labels(&result), [&label, &other_label()]);
        assert_eq!(error_messages(&result, &label), ["analysis failed"]);

        let (result, snapshots) = wait_for_analysis_error(false, &label).await;
        assert!(
            snapshots.is_empty(),
            "without fail-fast, an analysis error writes no streaming line of its own"
        );
        assert_eq!(error_messages(&result, &label), ["analysis failed"]);
    }

    #[test]
    fn test_finish_events_recorded_without_sender() {
        let configured = configured_target("root//pkg:a");
        let mut builder = BuildTargetResultBuilder::new(None, Instant::now());

        feed_finish_events(&mut builder, &configured, &macos_arm());

        assert_recorded(&builder.build(), &configured, &macos_arm());
    }

    #[test]
    fn test_configured_finish_is_per_pattern() {
        // Both orderings resolve to the same configuration but are requested as separate patterns.
        let configured = configured_target("root//pkg:a");
        let arm_macos = Modifiers::parse("root//:arm+root//:macos");
        let mut builder = BuildTargetResultBuilder::new(None, Instant::now());

        feed(&mut builder, configured_finished(&configured, &macos_arm()));

        let finished = builder.build().finished_builds.configured_targets;
        assert!(finished.contains(&(configured.dupe(), macos_arm())));
        assert!(
            !finished.contains(&(configured.dupe(), arm_macos.dupe())),
            "?macos+arm finishing must not mark ?arm+macos finished"
        );

        feed(&mut builder, configured_finished(&configured, &arm_macos));

        assert_eq!(
            builder.build().finished_builds.configured_targets,
            BTreeSet::from([(configured.dupe(), macos_arm()), (configured, arm_macos)])
        );
    }

    #[test]
    fn test_extend_merges_finished_builds() {
        let a = configured_target("root//pkg:a");
        let b = configured_target("root//pkg:b");
        let modifiers = macos_arm();
        let mut result = BuildTargetResult::new();
        result
            .finished_builds
            .configured_targets
            .insert((a.dupe(), modifiers.dupe()));
        let mut other = BuildTargetResult::new();
        other
            .finished_builds
            .configured_targets
            .insert((b.dupe(), modifiers.dupe()));
        other.finished_builds.targets.insert(target(&b, &modifiers));

        result.extend(other);

        assert_eq!(
            result.finished_builds.configured_targets,
            BTreeSet::from([(a, modifiers.dupe()), (b.dupe(), modifiers.dupe())])
        );
        assert_eq!(
            result.finished_builds.targets,
            BTreeSet::from([target(&b, &modifiers)])
        );
        assert!(
            result.is_empty(),
            "finish events alone add no build results or errors"
        );
    }
}
