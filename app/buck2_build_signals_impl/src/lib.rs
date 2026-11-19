/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::fmt;
use std::hash::Hash;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use async_trait::async_trait;
use buck2_analysis::analysis::calculation::AnalysisKey;
use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::RegisteredAction;
use buck2_build_api::actions::calculation::ActionExtraData;
use buck2_build_api::actions::calculation::ActionWithExtraData;
use buck2_build_api::actions::calculation::BuildKey;
use buck2_build_api::actions::calculation::BuildKeyActivationData;
use buck2_build_api::analysis::AnalysisKeyActivationData;
use buck2_build_api::analysis::AnalysisWithExtraData;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::artifact_groups::ResolvedArtifactGroupBuildSignalsKey;
use buck2_build_api::artifact_groups::calculation::EnsureProjectedArtifactKey;
use buck2_build_api::artifact_groups::calculation::EnsureTransitiveSetProjectionKey;
use buck2_build_api::artifact_groups::calculation::EnsureTransitiveSetProjectionKeyActivationData;
use buck2_build_api::build_signals::BuildSignals;
use buck2_build_api::build_signals::BuildSignalsInstaller;
use buck2_build_api::build_signals::CREATE_BUILD_SIGNALS;
use buck2_build_signals::env::BuildSignalsContext;
use buck2_build_signals::env::CriticalPathBackendName;
use buck2_build_signals::env::DeferredBuildSignals;
use buck2_build_signals::env::FinishBuildSignals;
use buck2_build_signals::env::NodeDuration;
use buck2_build_signals::env::WaitingData;
use buck2_build_signals::error::CriticalPathError;
use buck2_build_signals::node_key::BuildSignalsNodeKey;
use buck2_build_signals::node_key::BuildSignalsNodeKeyImpl;
use buck2_common::package_listing::dice::PackageListingKey;
use buck2_common::package_listing::dice::PackageListingKeyActivationData;
use buck2_core::package::PackageLabel;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_data::ToProtoMessage;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::instant_event;
use buck2_events::dispatch::with_dispatcher_async;
use buck2_events::span::SpanId;
use buck2_hash::BuckDashMap;
use buck2_hash::BuckDashSet;
use buck2_hash::BuckMutMap;
use buck2_interpreter_for_build::interpreter::calculation::InterpreterResultsKey;
use buck2_interpreter_for_build::interpreter::calculation::InterpreterResultsKeyActivationData;
use buck2_util::early_command_timing::EarlyCommandTiming;
use buck2_util::size_assert;
use buck2_util::time_span::TimeSpan;
use dice::ActivationData;
use dice::ActivationTracker;
use dice::DynKey;
use dice::PageInPhase;
use dupe::Dupe;
use ref_cast::RefCast;
use smallvec::SmallVec;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;
use variant_name::VariantName;

use crate::backend::backend::BuildListenerBackend;
use crate::backend::logging::LoggingBackend;
use crate::backend::longest_path_graph::LongestPathGraphBackend;
use crate::enhancement::CriticalPathProtoEnhancer;
use crate::test_signals::InstallBuildSignalKey;
use crate::test_signals::InstallSignal;
use crate::test_signals::TestExecutionBuildSignalKey;
use crate::test_signals::TestExecutionSignal;
use crate::test_signals::TestListingBuildSignalKey;
use crate::test_signals::TestListingSignal;

mod backend;
mod enhancement;
mod error;
mod test_signals;

/// A node in our critical path graph.
#[derive(Hash, Eq, PartialEq, Clone, Dupe, Debug, VariantName)]
enum NodeKey {
    // Those are DICE keys.
    BuildKey(BuildKey),
    AnalysisKey(AnalysisKey),
    EnsureProjectedArtifactKey(EnsureProjectedArtifactKey),
    EnsureTransitiveSetProjectionKey(EnsureTransitiveSetProjectionKey),
    InterpreterResultsKey(InterpreterResultsKey),
    PackageListingKey(PackageListingKey),

    // This one is not a DICE key.
    FinalMaterialization(BuildArtifact),

    // Keys for test not on DICE.
    TestExecution(TestExecutionBuildSignalKey),
    TestListing(TestListingBuildSignalKey),
    Install(InstallBuildSignalKey),

    // Dynamically-typed.
    Dyn(&'static str, BuildSignalsNodeKey),

    // A DICE key that is normally omitted from the graph, retained as a zero-duration connector
    // because it paged in or depends on a key that did.
    PageInConnector(DynKey),

    // Evaluation work separated from the key's completion because hydration happened after it.
    EvaluationWork(Arc<NodeKey>),

    // The page-in of the wrapped key, represented as a separate graph phase.
    PageIn(Arc<NodeKey>),
}

// Explain the sizeof this struct (and avoid regressing it since we store it in the longest path
// graph implementation).

size_assert::words_of_type!(BuildKey, 4);
size_assert::words_of_type!(AnalysisKey, 2);
size_assert::words_of_type!(EnsureTransitiveSetProjectionKey, 5);
size_assert::words_of_type!(EnsureProjectedArtifactKey, 7);
size_assert::words_of_type!(InterpreterResultsKey, 1);
size_assert::words_of_type!(PackageListingKey, 1);
size_assert::words_of_type!(BuildArtifact, 6);
size_assert::words_of_type!(NodeKey, 7);

impl NodeKey {
    /// The package this computation belongs to: the loaded package itself, or the package of
    /// the target a configured-node, analysis, or configuration-node computation runs on
    /// behalf of. Only computations with a package take part in load discovery.
    fn package(&self) -> Option<PackageLabel> {
        match self {
            NodeKey::InterpreterResultsKey(InterpreterResultsKey(pkg)) => Some(pkg.dupe()),
            NodeKey::AnalysisKey(key) => Some(key.0.unconfigured().pkg()),
            NodeKey::Dyn(_, key) => key.package(),
            _ => None,
        }
    }

    fn from_dyn_key(key: &DynKey) -> Option<Self> {
        let key = if let Some(key) = key.downcast_ref::<BuildKey>() {
            Self::BuildKey(key.dupe())
        } else if let Some(key) = key.downcast_ref::<AnalysisKey>() {
            Self::AnalysisKey(key.dupe())
        } else if let Some(key) = key.downcast_ref::<EnsureProjectedArtifactKey>() {
            Self::EnsureProjectedArtifactKey(key.dupe())
        } else if let Some(key) = key.downcast_ref::<EnsureTransitiveSetProjectionKey>() {
            Self::EnsureTransitiveSetProjectionKey(key.dupe())
        } else if let Some(key) = key.downcast_ref::<InterpreterResultsKey>() {
            Self::InterpreterResultsKey(key.dupe())
        } else if let Some(key) = key.downcast_ref::<PackageListingKey>() {
            Self::PackageListingKey(key.dupe())
        } else if let Some(node_key) = key.request_value::<BuildSignalsNodeKey>() {
            Self::Dyn(key.key_type_name(), node_key)
        } else {
            return None;
        };

        Some(key)
    }

    fn into_critical_path_entry_data(
        self,
        extra_data: &NodeExtraData,
    ) -> buck2_data::critical_path_entry2::Entry {
        match self {
            NodeKey::BuildKey(ref key) => {
                let owner = key.0.owner().to_proto().into();

                // If we have a NodeKey that's an ActionKey we'd expect to have `action`
                // extra data (unless we didn't actually run it because of e.g. early
                // cutoff, in which case omitting it is what we want).
                match extra_data {
                    NodeExtraData::Action(ActionNodeData {
                        action,
                        execution_kind,
                        target_rule_type_name,
                        action_digest,
                        invalidation_info,
                    }) => buck2_data::critical_path_entry2::ActionExecution {
                        owner: Some(owner),
                        name: Some(buck2_data::ActionName {
                            category: action.category().as_str().to_owned(),
                            identifier: action.identifier().unwrap_or("").to_owned(),
                        }),
                        execution_kind: (*execution_kind).into(),
                        target_rule_type_name: target_rule_type_name.to_owned(),
                        action_digest: action_digest.to_owned(),
                        invalidation_info: invalidation_info.to_owned(),
                    }
                    .into(),
                    _ => self.into_generic_entry(),
                }
            }
            NodeKey::AnalysisKey(key) => buck2_data::critical_path_entry2::Analysis {
                target: Some(key.0.as_proto().into()),
                target_rule_type_name: match &extra_data {
                    NodeExtraData::Analysis(node_data) => node_data.target_rule_type_name.clone(),
                    _ => None,
                },
                part: match &extra_data {
                    NodeExtraData::Analysis(node_data) => node_data.part,
                    _ => None,
                },
            }
            .into(),

            NodeKey::FinalMaterialization(key) => {
                let owner = key.key().owner().to_proto().into();

                buck2_data::critical_path_entry2::FinalMaterialization {
                    owner: Some(owner),
                    path: key.get_path().path().to_string(),
                }
                .into()
            }
            NodeKey::InterpreterResultsKey(key) => buck2_data::critical_path_entry2::Load {
                package: key.0.to_string(),
            }
            .into(),

            NodeKey::PackageListingKey(key) => buck2_data::critical_path_entry2::Listing {
                package: key.0.to_string(),
            }
            .into(),

            NodeKey::EnsureProjectedArtifactKey(..) => self.into_generic_entry(),
            NodeKey::EnsureTransitiveSetProjectionKey(_key) => {
                buck2_data::critical_path_entry2::EnsureTransitiveSetProjection {}.into()
            }
            NodeKey::Dyn(_, ref d) => match d.critical_path_entry_proto() {
                Some(mut entry) => {
                    // For Dyn keys that produce AnonAnalysis entries and have been
                    // split, inject the part number from the analysis extra_data.
                    if let (
                        buck2_data::critical_path_entry2::Entry::AnonAnalysis(anon),
                        NodeExtraData::Analysis(node_data),
                    ) = (&mut entry, extra_data)
                    {
                        if anon.part.is_none() {
                            anon.part = node_data.part;
                        }
                    }
                    entry
                }
                None => self.into_generic_entry(),
            },
            NodeKey::TestExecution(t) => buck2_data::critical_path_entry2::TestExecution {
                target_label: Some(t.target.as_proto()),
                suite: t.suite.to_string(),
                testcases: t.testcases.to_vec(),
                variant: t.variant.map(|v| v.to_string()),
            }
            .into(),

            NodeKey::TestListing(t) => buck2_data::critical_path_entry2::TestListing {
                target_label: Some(t.target.as_proto()),
                suite: t.suite.to_string(),
            }
            .into(),

            NodeKey::Install(t) => buck2_data::critical_path_entry2::Install {
                installer_label: Some(t.installer.target().as_proto()),
                target_labels: t.targets.iter().map(|t| t.as_proto()).collect(),
            }
            .into(),

            key @ NodeKey::PageInConnector(..) => key.into_generic_entry(),

            NodeKey::EvaluationWork(inner) => inner
                .as_ref()
                .dupe()
                .into_critical_path_entry_data(extra_data),

            NodeKey::PageIn(inner) => buck2_data::critical_path_entry2::PageIn {
                key_type: match &*inner {
                    NodeKey::PageInConnector(key) => key.key_type_name().to_owned(),
                    _ => inner.variant_name_lowercase().to_owned(),
                },
                count: 1,
                key_type_counts: Default::default(),
            }
            .into(),
        }
    }

    fn into_generic_entry(self) -> buck2_data::critical_path_entry2::Entry {
        buck2_data::critical_path_entry2::GenericEntry {
            kind: match self {
                NodeKey::Dyn(_, d) => d.kind(),
                _ => self.variant_name_lowercase(),
            }
            .to_owned(),
        }
        .into()
    }
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuildKey(k) => write!(f, "BuildKey({k})"),
            Self::AnalysisKey(k) => write!(f, "AnalysisKey({k})"),
            Self::EnsureProjectedArtifactKey(k) => write!(f, "EnsureProjectedArtifactKey({k})"),
            Self::EnsureTransitiveSetProjectionKey(k) => {
                write!(f, "EnsureTransitiveSetProjectionKey({k})")
            }
            Self::InterpreterResultsKey(k) => write!(f, "InterpreterResultsKey({k})"),
            Self::PackageListingKey(k) => write!(f, "PackageListingKey({k})"),
            Self::FinalMaterialization(k) => write!(f, "FinalMaterialization({k})"),
            Self::TestExecution(k) => write!(f, "TestExecution({k:?})"),
            Self::TestListing(k) => write!(f, "TestListing({k:?})"),
            Self::Install(k) => write!(f, "Install({k:?})"),
            Self::Dyn(name, k) => write!(f, "{name}({k})"),
            Self::PageInConnector(key) => write!(f, "{}({key})", key.key_type_name()),
            Self::EvaluationWork(k) => write!(f, "EvaluationWork({k})"),
            Self::PageIn(k) => write!(f, "PageIn({k})"),
        }
    }
}

struct TopLevelTargetSignal {
    pub(crate) label: ConfiguredTargetLabel,
    pub(crate) artifacts: Vec<ResolvedArtifactGroupBuildSignalsKey>,
}

struct FinalMaterializationSignal {
    pub(crate) artifact: BuildArtifact,
    pub(crate) from_group: ArtifactGroup,
    pub(crate) duration: NodeDuration,
    pub(crate) span_id: Option<SpanId>,
    pub(crate) waiting_data: WaitingData,
}

/// A paged-out DICE key that was paged back in. `duration` is a synthetic span covering the
/// page-in (backend fetch + deserialize) so its cost lands on the critical path as its own node.
struct PageInSignal {
    key: NodeKey,
    duration: NodeDuration,
    phase: PageInPhase,
}

struct PageInAssociation {
    key: Arc<NodeKey>,
    phase: PageInPhase,
}

/* These signals are distinct from the main Buck event bus because some
 * analysis needs access to the entire build graph, and serializing the
 * entire build graph isn't feasible - therefore, we have these signals
 * with an unserializable but lightweight handle on a RegisteredAction.
 */
enum BuildSignal {
    Evaluation(Evaluation),
    TopLevelTarget(TopLevelTargetSignal),
    FinalMaterialization(FinalMaterializationSignal),
    TestExecution(TestExecutionSignal),
    TestListing(TestListingSignal),
    Install(InstallSignal),
    PageIn(PageInSignal),
    LoadDiscovery(LoadDiscoverySignal),
    BuildFinished,
}

/// A package load requested on behalf of a node of another package by code
/// outside any tracked computation; see `BuildSignals::load_discovered`.
struct LoadDiscoverySignal {
    package: PackageLabel,
    discovered_by: PackageLabel,
    requested_at: Instant,
}

/// Data for a BuildSignal that is the result of a DICE key evaluation.
pub(crate) struct Evaluation {
    /// The key we evaluated.
    key: NodeKey,
    /// The duration. By default this'll be zero, unless activation data says otherwise.
    duration: NodeDuration,
    /// The dependencies.
    dep_keys: Vec<NodeKey>,
    /// Spans that correspond to this key. We use this when producing a chrome trace.
    spans: SmallVec<[SpanId; 1]>,

    /// Data about time spent waiting (not on critical path) during this evaluation.
    waiting_data: WaitingData,

    /// Node-type-specific extra data (action data for BuildKey, load result for InterpreterResultsKey, etc.).
    extra_data: NodeExtraData,

    /// If this is Part 2 of a split analysis, contains info about the discovering
    /// analysis Part 1 and the anon targets that were discovered.
    split_discovery: Option<SplitDiscoveryData>,

    /// A page-in that completed after this evaluation phase.
    page_in: Option<PageInAssociation>,
}

pub(crate) struct BuildSignalSender {
    sender: UnboundedSender<BuildSignal>,
    pending_page_in_phases: BuckDashMap<NodeKey, PageInPhase>,
    // `None` until the first page-in is recorded (via `key_paged_in`), so builds that never
    // page in allocate nothing here and keep the fast early-return for unmapped keys. Never
    // cleared once set, matching the monotonic nature of page-in tracking within a build.
    page_in_reachability: OnceLock<PageInReachability>,
}

/// Tracks which keys reach a page-in (themselves, or transitively through a dependency), so
/// otherwise-filtered DICE keys on the chain between a page-in and the graph can be retained
/// as `PageInConnector` connectors. Only exists once a page-in has occurred (see
/// [`BuildSignalSender::page_in_reachability`]).
#[derive(Default)]
struct PageInReachability {
    keys: BuckDashSet<DynKey>,
}

impl PageInReachability {
    fn reaches_page_in(&self, key: &DynKey) -> bool {
        self.keys.contains(key)
    }

    fn record_page_in(&self, key: &DynKey) {
        self.keys.insert(key.dupe());
    }

    /// Resolve an activation into its graph representation while tracking page-in reachability:
    /// walk `deps`, substituting `PageInConnector` connectors for unmapped deps that reach a page-in;
    /// mark `key` if it (or a dep) reaches one; and return the graph key plus dep keys, or `None`
    /// when `key` is unmapped and doesn't reach a page-in (so it is dropped).
    fn resolve_graph_keys(
        &self,
        key: &DynKey,
        mapped_key: Option<NodeKey>,
        deps: &mut dyn Iterator<Item = &DynKey>,
    ) -> Option<(NodeKey, Vec<NodeKey>)> {
        let mut has_dep_reaching_page_in = false;
        let dep_keys = deps
            .filter_map(|dep| {
                let mapped_dep = NodeKey::from_dyn_key(dep);
                if self.reaches_page_in(dep) {
                    has_dep_reaching_page_in = true;
                    Some(mapped_dep.unwrap_or_else(|| NodeKey::PageInConnector(dep.dupe())))
                } else {
                    mapped_dep
                }
            })
            .collect();

        // Mark `key` (even when mapped) so its reachability propagates to its parents. Dependency
        // callbacks finish before the parent's, so the mark is visible as activations complete.
        let reaches_page_in = has_dep_reaching_page_in || self.reaches_page_in(key);
        if reaches_page_in {
            self.keys.insert(key.dupe());
        }

        let key = match mapped_key {
            Some(key) => key,
            None if reaches_page_in => NodeKey::PageInConnector(key.dupe()),
            None => return None,
        };
        Some((key, dep_keys))
    }
}

impl BuildSignalSender {
    /// Create a finish NodeKey for Part 2 of a split analysis.
    fn make_finish_key(part1_key: &NodeKey, analysis_data: &AnalysisNodeData) -> NodeKey {
        match part1_key {
            NodeKey::AnalysisKey(key) => NodeKey::Dyn(
                "AnalysisFinishKey",
                BuildSignalsNodeKey::new(AnalysisFinishNodeKey {
                    target: key.0.dupe(),
                    target_rule_type_name: analysis_data.target_rule_type_name.clone(),
                }),
            ),
            NodeKey::Dyn(name, dyn_key) if *name == "AnonTargetKey" => {
                // Extract the anon target proto from the Part 1 key's critical path entry.
                let anon_target_proto = dyn_key
                    .critical_path_entry_proto()
                    .and_then(|entry| match entry {
                        buck2_data::critical_path_entry2::Entry::AnonAnalysis(a) => a.anon_target,
                        _ => None,
                    })
                    .unwrap_or_default();
                NodeKey::Dyn(
                    "AnonTargetFinishKey",
                    BuildSignalsNodeKey::new(AnonTargetFinishNodeKey {
                        display_name: format!("{}", dyn_key),
                        anon_target_proto,
                    }),
                )
            }
            _ => {
                // Shouldn't happen - only AnalysisKey and AnonTargetKey can be split
                part1_key.dupe()
            }
        }
    }
}

impl BuildSignals for BuildSignalSender {
    fn load_discovered(
        &self,
        package: PackageLabel,
        discovered_by: PackageLabel,
        requested_at: Instant,
    ) {
        let _ignored = self
            .sender
            .send(BuildSignal::LoadDiscovery(LoadDiscoverySignal {
                package,
                discovered_by,
                requested_at,
            }));
    }

    fn top_level_target(
        &self,
        label: ConfiguredTargetLabel,
        artifacts: Vec<ResolvedArtifactGroupBuildSignalsKey>,
    ) {
        let _ignored = self
            .sender
            .send(BuildSignal::TopLevelTarget(TopLevelTargetSignal {
                label,
                artifacts,
            }));
    }

    fn final_materialization(
        &self,
        artifact: BuildArtifact,
        from_group: ArtifactGroup,
        duration: NodeDuration,
        span_id: Option<SpanId>,
        waiting_data: WaitingData,
    ) {
        let _ignored = self.sender.send(BuildSignal::FinalMaterialization(
            FinalMaterializationSignal {
                artifact,
                from_group,
                duration,
                span_id,
                waiting_data,
            },
        ));
    }

    fn test_listing(
        &self,
        target: ConfiguredTargetLabel,
        suite: String,
        duration: NodeDuration,
        deps: &[ActionKey],
        waiting_data: WaitingData,
    ) {
        let _ignored = self
            .sender
            .send(BuildSignal::TestListing(TestListingSignal {
                target,
                suite,
                deps: deps.to_vec(),
                duration,
                waiting_data,
            }));
    }

    fn test_execution(
        &self,
        target: ConfiguredTargetLabel,
        suite: String,
        testcases: &[String],
        variant: Option<String>,
        duration: NodeDuration,
        deps: &[ActionKey],
        waiting_data: WaitingData,
    ) {
        let _ignored = self
            .sender
            .send(BuildSignal::TestExecution(TestExecutionSignal {
                target,
                suite,
                testcases: testcases.to_vec(),
                variant,
                deps: deps.to_vec(),
                duration,
                waiting_data,
            }));
    }

    fn install(
        &self,
        installer: ConfiguredProvidersLabel,
        targets: Vec<ConfiguredTargetLabel>,
        duration: NodeDuration,
        deps: &[ActionKey],
        waiting_data: WaitingData,
    ) {
        let _ignored = self.sender.send(BuildSignal::Install(InstallSignal {
            installer,
            targets,
            deps: deps.to_vec(),
            duration,
            waiting_data,
        }));
    }
}

impl ActivationTracker for BuildSignalSender {
    /// We received a DICE key. Check if it's one of the keys we care about (i.e. can we downcast
    /// it to NodeKey?), and then if that's the case, extract its dependencies and activation data
    /// (if any).
    fn key_activated(
        &self,
        key: &DynKey,
        deps: &mut dyn Iterator<Item = &DynKey>,
        activation_data: ActivationData,
    ) {
        let mapped_key = NodeKey::from_dyn_key(key);
        let (key, dep_keys) = match self.page_in_reachability.get() {
            Some(reachability) => match reachability.resolve_graph_keys(key, mapped_key, deps) {
                Some(resolved) => resolved,
                None => return,
            },
            None => {
                // No page-in has happened yet: keep only mapped keys, exactly as before paging.
                let Some(key) = mapped_key else {
                    return;
                };
                (key, deps.filter_map(NodeKey::from_dyn_key).collect())
            }
        };
        let page_in =
            self.pending_page_in_phases
                .remove(&key)
                .map(|(_, phase)| PageInAssociation {
                    key: Arc::new(key.dupe()),
                    phase,
                });

        let mut signal = Evaluation {
            key,
            extra_data: NodeExtraData::None,
            duration: NodeDuration::zero(),
            dep_keys,
            spans: Default::default(),
            waiting_data: WaitingData::new(),
            split_discovery: None,
            page_in,
        };

        /// Given an Option containing an Any, take it if and only if it contains a T.
        fn downcast_and_take<T: 'static>(
            data: &mut Option<Box<dyn Any + Send + Sync + 'static>>,
        ) -> Option<T> {
            if data.as_ref().map(|d| d.is::<T>()) != Some(true) {
                return None;
            }

            // Unwrap safety: we just checked that the option is occupied and the type matches
            Some(*data.take().unwrap().downcast().ok().unwrap())
        }

        if let ActivationData::Evaluated(mut activation_data) = activation_data {
            if let Some(BuildKeyActivationData {
                action_with_extra_data,
                duration,
                spans,
                waiting_data,
            }) = downcast_and_take(&mut activation_data)
            {
                signal.extra_data =
                    NodeExtraData::Action(ActionNodeData::from_extra_data(action_with_extra_data));
                signal.duration = duration;
                signal.spans = spans;
                signal.waiting_data = waiting_data;
            } else if let Some(AnalysisKeyActivationData {
                time_span,
                spans,
                analysis_with_extra_data,
                waiting_data,
                anon_target_split,
            }) = downcast_and_take(&mut activation_data)
            {
                let analysis_node_data =
                    AnalysisNodeData::from_extra_data(analysis_with_extra_data);

                if let Some(split) = anon_target_split {
                    // Split this analysis into Part 1 (before anon targets) and Part 2 (after).
                    // Partition deps into anon target deps and other deps.
                    let (anon_deps, other_deps): (Vec<_>, Vec<_>) =
                        signal.dep_keys.drain(..).partition(
                            |k| matches!(k, NodeKey::Dyn(_, key) if key.kind() == "anon_target"),
                        );

                    // Part 1: the main key with pre-anon-target duration and non-anon deps.
                    signal.duration = NodeDuration {
                        user: time_span.duration(),
                        total: time_span,
                        queue: None,
                    };
                    signal.dep_keys = other_deps;
                    signal.spans = spans;
                    signal.extra_data =
                        NodeExtraData::Analysis(analysis_node_data.clone().with_part(Some(1)));
                    signal.waiting_data = waiting_data;

                    let part1_key = signal.key.dupe();
                    let page_in = signal.page_in.take();

                    // Part 2: finish key with post-anon-target duration
                    let finish_key = Self::make_finish_key(&part1_key, &analysis_node_data);
                    let part2_deps: Vec<NodeKey> = std::iter::once(part1_key.dupe())
                        .chain(anon_deps.iter().cloned())
                        .collect();

                    let part2_signal = Evaluation {
                        key: finish_key,
                        duration: NodeDuration {
                            user: split.part2_time_span.duration(),
                            total: split.part2_time_span,
                            queue: None,
                        },
                        dep_keys: part2_deps,
                        spans: Default::default(),
                        extra_data: NodeExtraData::Analysis(analysis_node_data.with_part(Some(2))),
                        waiting_data: WaitingData::new(),
                        split_discovery: Some(SplitDiscoveryData {
                            discovering_analysis: part1_key,
                            discovering_analysis_end: time_span.end(),
                            anon_targets: anon_deps,
                        }),
                        page_in,
                    };

                    // Send Part 1 and Part 2 as separate signals
                    let _ignored = self.sender.send(BuildSignal::Evaluation(signal));
                    let _ignored = self.sender.send(BuildSignal::Evaluation(part2_signal));
                    return;
                }

                signal.duration = NodeDuration {
                    user: time_span.duration(),
                    total: time_span,
                    queue: None,
                };
                signal.spans = spans;
                signal.extra_data = NodeExtraData::Analysis(analysis_node_data);
                signal.waiting_data = waiting_data;
            } else if let Some(InterpreterResultsKeyActivationData {
                time_span,
                result,
                spans,
            }) = downcast_and_take(&mut activation_data)
            {
                signal.duration = NodeDuration {
                    user: time_span.duration(),
                    total: time_span,
                    queue: None,
                };
                let _ignored = result;

                signal.spans = spans;
            } else if let Some(PackageListingKeyActivationData { time_span, spans }) =
                downcast_and_take(&mut activation_data)
            {
                signal.duration = NodeDuration {
                    user: time_span.duration(),
                    total: time_span,
                    queue: None,
                };
                signal.spans = spans;
            } else if let Some(EnsureTransitiveSetProjectionKeyActivationData { time_span }) =
                downcast_and_take(&mut activation_data)
            {
                signal.duration = NodeDuration {
                    user: time_span.duration(),
                    total: time_span,
                    queue: None,
                };
            }
        } else {
            signal.extra_data = NodeExtraData::Reused;
        }

        let _ignored = self.sender.send(BuildSignal::Evaluation(signal));
    }

    fn key_paged_in(&self, key: &DynKey, start: Instant, duration: Duration, phase: PageInPhase) {
        if phase != PageInPhase::ValueDemand {
            self.page_in_reachability
                .get_or_init(PageInReachability::default)
                .record_page_in(key);
        }
        let key =
            NodeKey::from_dyn_key(key).unwrap_or_else(|| NodeKey::PageInConnector(key.dupe()));
        if phase != PageInPhase::ValueDemand {
            self.pending_page_in_phases.insert(key.dupe(), phase);
        }

        let duration = NodeDuration {
            user: duration,
            total: TimeSpan::from_start_and_duration(start, duration),
            queue: None,
        };

        let _ignored = self.sender.send(BuildSignal::PageIn(PageInSignal {
            key,
            duration,
            phase,
        }));
    }
}

pub(crate) struct DeferredBuildSignalsImpl {
    sender: Arc<BuildSignalSender>,
    receiver: UnboundedReceiver<BuildSignal>,
}

impl DeferredBuildSignals for DeferredBuildSignalsImpl {
    fn start(
        self: Box<Self>,
        events: EventDispatcher,
        backend: CriticalPathBackendName,
        ctx: BuildSignalsContext,
    ) -> Box<dyn FinishBuildSignals> {
        let handle = match backend {
            CriticalPathBackendName::LongestPathGraph => {
                start_backend(events, self.receiver, LongestPathGraphBackend::new(), ctx)
            }
            CriticalPathBackendName::Logging => start_backend(
                events.dupe(),
                self.receiver,
                LoggingBackend::new(events),
                ctx,
            ),
        };

        Box::new(FinishBuildSignalsImpl {
            sender: self.sender,
            handle,
        }) as _
    }
}

pub(crate) struct FinishBuildSignalsImpl {
    sender: Arc<BuildSignalSender>,
    handle: JoinHandle<Result<(), CriticalPathError>>,
}

#[async_trait]
impl FinishBuildSignals for FinishBuildSignalsImpl {
    async fn finish(self: Box<Self>) -> Result<(), CriticalPathError> {
        let _ignored = self.sender.sender.send(BuildSignal::BuildFinished);

        self.handle.await.map_err(CriticalPathError::JoinError)?
    }
}

fn start_backend(
    events: EventDispatcher,
    receiver: UnboundedReceiver<BuildSignal>,
    backend: impl BuildListenerBackend + Send + 'static,
    ctx: BuildSignalsContext,
) -> JoinHandle<Result<(), CriticalPathError>> {
    let listener = BuildSignalReceiver::new(receiver, backend);
    tokio::spawn(with_dispatcher_async(events.dupe(), async move {
        listener.run_and_log(ctx).await
    }))
}

struct BuildSignalReceiver<T> {
    receiver: UnboundedReceiverStream<BuildSignal>,
    // Discovered package load -> the package load credited with discovering it, and when;
    // applied as discovery edges in finish(). See `record_load_discovery`.
    first_requester_for_load: BuckMutMap<NodeKey, (NodeKey, Instant)>,
    // Loads made by a computation with no target of its own, until a packaged dependent
    // claims them. See `record_load_discovery`.
    discoveries_awaiting_package: BuckMutMap<NodeKey, SmallVec<[PackageLabel; 4]>>,
    // Span of each package load this command evaluated.
    load_spans: BuckMutMap<PackageLabel, TimeSpan>,
    // Maps an anon target NodeKey to the analysis Part 1 NodeKey that discovered it
    // (the one whose Part 1 finished earliest). Used to add discovery edges in finish().
    first_analysis_for_anon_target: BuckMutMap<NodeKey, (NodeKey, Instant)>,
    // Maps a Part 1 key (e.g. AnalysisKey) to its finish key (Part 2) for split analyses.
    // When a node depends on a split analysis, the dep should point to the finish key
    // (representing full completion) rather than the Part 1 key.
    split_analysis_finish_keys: BuckMutMap<NodeKey, NodeKey>,
    // Hydration within evaluation precedes `key_activated`, which supplies its dependencies
    // and evaluation data. Hold those signals until the associated evaluation arrives;
    // independent value-demand reads are emitted immediately.
    pending_page_ins: BuckMutMap<NodeKey, PageInSignal>,
    backend: T,

    // TODO(rajneeshl): When Test listing and execution are on DICE, we can remove this and use
    // DICE keys instead.
    test_listing_keys: BuckMutMap<String, NodeKey>,
}

impl<T> BuildSignalReceiver<T>
where
    T: BuildListenerBackend,
{
    fn new(receiver: UnboundedReceiver<BuildSignal>, backend: T) -> Self {
        Self {
            receiver: UnboundedReceiverStream::new(receiver),
            backend,
            first_requester_for_load: BuckMutMap::default(),
            discoveries_awaiting_package: BuckMutMap::default(),
            load_spans: BuckMutMap::default(),
            first_analysis_for_anon_target: BuckMutMap::default(),
            split_analysis_finish_keys: BuckMutMap::default(),
            pending_page_ins: BuckMutMap::default(),
            test_listing_keys: BuckMutMap::default(),
        }
    }

    pub(crate) async fn run_and_log(
        mut self,
        ctx: BuildSignalsContext,
    ) -> Result<(), CriticalPathError> {
        while let Some(event) = self.receiver.next().await {
            match event {
                BuildSignal::Evaluation(eval) => self.process_evaluation(eval),
                BuildSignal::TopLevelTarget(top_level) => self.process_top_level_target(top_level),
                BuildSignal::FinalMaterialization(final_materialization) => {
                    self.process_final_materialization(final_materialization)
                }
                BuildSignal::TestExecution(test_execution) => {
                    self.process_test_execution(test_execution)
                }
                BuildSignal::TestListing(test_listing) => self.process_test_listing(test_listing),
                BuildSignal::Install(install) => self.process_install(install),
                BuildSignal::PageIn(page_in) => self.process_page_in(page_in),
                BuildSignal::LoadDiscovery(discovery) => self.process_load_discovery(discovery),
                BuildSignal::BuildFinished => {
                    self.flush_unmatched_page_ins();
                    break;
                }
            }
        }

        let now = Instant::now();

        let discovery_edges: BuckMutMap<NodeKey, NodeKey> = self
            .first_analysis_for_anon_target
            .into_iter()
            .chain(self.first_requester_for_load)
            .map(|(k, (v, _instant))| (k, v))
            .collect();
        let BuildInfo {
            critical_path,
            slowest_path,
            num_nodes,
            num_edges,
            top_level_targets,
        } = self.backend.finish(discovery_edges)?;

        let critical_path2 = critical_path.into_critical_path_proto(&ctx.early_command_timing, now);

        let slowest_path = slowest_path.into_critical_path_proto(&ctx.early_command_timing, now);

        let top_level_targets = top_level_targets
            .iter()
            .map(|(key, duration)| buck2_data::TopLevelTargetCriticalPath {
                target: Some(key.as_proto()),
                duration: Some((*duration).try_into().unwrap_or(prost_types::Duration {
                    seconds: i64::MAX,
                    nanos: 0,
                })),
            })
            .collect::<Vec<_>>();

        instant_event(buck2_data::BuildGraphExecutionInfo {
            critical_path2,
            slowest_path,
            metadata: ctx.metadata,
            command_name: Some(ctx.command_name),
            isolation_dir: Some(ctx.isolation_prefix.into_inner().into()),
            num_nodes,
            num_edges,
            backend_name: Some(T::name().to_string()),
            top_level_targets,
            client_ids: ctx.client_ids,
        });
        Ok(())
    }

    /// Receive an Evaluation. Do a little enrichment if it's a load, then pass through to the
    /// underlying backend.
    fn process_evaluation(&mut self, mut evaluation: Evaluation) {
        if let NodeKey::InterpreterResultsKey(InterpreterResultsKey(pkg)) = &evaluation.key
            && !matches!(evaluation.extra_data, NodeExtraData::Reused)
        {
            self.load_spans
                .insert(pkg.dupe(), evaluation.duration.total);
        }
        self.record_load_discovery(&evaluation);

        // Remap deps that point to Part 1 of a split analysis to the finish key (Part 2),
        // since nodes depending on a split analysis can only proceed after it fully completes.
        // This must happen before recording new split mappings below, so that the Part 2
        // node's own dep on Part 1 is not incorrectly remapped to itself.
        for dep_key in &mut evaluation.dep_keys {
            if let Some(finish_key) = self.split_analysis_finish_keys.get(dep_key) {
                *dep_key = finish_key.dupe();
            }
        }

        // Track discovery edges for anon target splits
        if let Some(discovery) = &evaluation.split_discovery {
            // Record the mapping from Part 1 key to the finish key (Part 2).
            // Other nodes that depend on this analysis via DICE will have the Part 1
            // key in their deps, but should depend on the finish key instead.
            self.split_analysis_finish_keys
                .insert(discovery.discovering_analysis.dupe(), evaluation.key.dupe());

            for anon_key in &discovery.anon_targets {
                let new_end = discovery.discovering_analysis_end;
                match self.first_analysis_for_anon_target.entry(anon_key.dupe()) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert((discovery.discovering_analysis.dupe(), new_end));
                    }
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if new_end < e.get().1 {
                            e.insert((discovery.discovering_analysis.dupe(), new_end));
                        }
                    }
                }
            }
        }

        let page_in = evaluation.page_in.take().and_then(|association| {
            self.pending_page_ins
                .remove(association.key.as_ref())
                .inspect(|page_in| {
                    debug_assert_eq!(page_in.phase, association.phase);
                })
        });

        match page_in {
            Some(page_in) => self.process_evaluation_with_page_in(evaluation, page_in),
            None => self.process_evaluation_node(evaluation),
        }
    }

    fn process_evaluation_node(&mut self, evaluation: Evaluation) {
        self.backend.process_node(
            evaluation.key,
            evaluation.extra_data,
            evaluation.duration,
            evaluation.dep_keys,
            evaluation.spans,
            evaluation.waiting_data,
        );
    }

    fn process_evaluation_with_page_in(&mut self, evaluation: Evaluation, page_in: PageInSignal) {
        let page_in_key = NodeKey::PageIn(Arc::new(page_in.key));

        match page_in.phase {
            PageInPhase::ValueDemand => {
                unreachable!("value-demand reads are independent of activation")
            }
            PageInPhase::AfterRecompute => {
                // Recalculation completed before hydration. Keep the original key as a
                // zero-duration completion node so callers see:
                // `key -> PageIn(key) -> EvaluationWork(key) -> dependencies`.
                let completion_key = evaluation.key.dupe();
                let work_key = NodeKey::EvaluationWork(Arc::new(evaluation.key));
                // Activation is reported after hydration, so an evaluation that carries no
                // timing of its own is stamped later than the page-in it precedes here.
                let hydrate_start = page_in.duration.total.start();
                let mut work_duration = evaluation.duration;
                if work_duration.total.end() > hydrate_start {
                    work_duration.total = TimeSpan::new_saturating(
                        work_duration.total.start().min(hydrate_start),
                        hydrate_start,
                    );
                    work_duration.user = work_duration.user.min(work_duration.total.duration());
                }
                self.backend.process_node(
                    work_key.dupe(),
                    evaluation.extra_data,
                    work_duration,
                    evaluation.dep_keys,
                    evaluation.spans,
                    evaluation.waiting_data,
                );
                self.backend.process_node(
                    page_in_key.dupe(),
                    NodeExtraData::None,
                    page_in.duration,
                    [work_key],
                    Default::default(),
                    WaitingData::new(),
                );

                let completion = page_in.duration.total.end();
                self.backend.process_node(
                    completion_key,
                    NodeExtraData::None,
                    NodeDuration {
                        user: Duration::ZERO,
                        total: TimeSpan::new_saturating(completion, completion),
                        queue: None,
                    },
                    [page_in_key],
                    Default::default(),
                    WaitingData::new(),
                );
            }
        }
    }

    /// A package is discovered by the earliest-finishing computation about a target of
    /// another package that depends on it, on its load directly or on one of its nodes; the
    /// discovered load gets a discovery edge to that target's package load. Matching a
    /// configuration against a set of select keys is a computation with no target of its own
    /// (keyed by configuration and key set, shared by every target using those keys), so the
    /// key-target packages it loads wait in `discoveries_awaiting_package` until a packaged
    /// computation depends on it. Loads no tracked computation depended on have no
    /// discoverer here; query traversals report theirs through `process_load_discovery`,
    /// and target-pattern loads get no edge.
    ///
    /// Only a package whose load finished before the discovered load began can have
    /// discovered it; see `load_could_discover`. Requests from packages that loaded later
    /// (a package-level cycle, or a load some other request had already started), and
    /// requests involving a load this command did not evaluate, do not compete.
    fn record_load_discovery(&mut self, evaluation: &Evaluation) {
        let end = evaluation.duration.total.end();
        let from_pkg = evaluation.key.package();
        let mut discovered: SmallVec<[PackageLabel; 4]> = SmallVec::new();
        for dep in &evaluation.dep_keys {
            if let Some(pkg) = dep.package() {
                if !discovered.contains(&pkg) {
                    discovered.push(pkg);
                }
            } else if from_pkg.is_some()
                && let Some(awaiting) = self.discoveries_awaiting_package.get(dep)
            {
                for pkg in awaiting {
                    if !discovered.contains(pkg) {
                        discovered.push(pkg.dupe());
                    }
                }
            }
        }
        if discovered.is_empty() {
            return;
        }
        let Some(from_pkg) = from_pkg else {
            self.discoveries_awaiting_package
                .insert(evaluation.key.dupe(), discovered);
            return;
        };
        for pkg in discovered {
            if self.load_could_discover(from_pkg, pkg) == Some(true) {
                self.credit_load_discovery(pkg, from_pkg.dupe(), end);
            }
        }
    }

    /// A query traversal asked for `package` while expanding a node of `discovered_by`;
    /// nothing tracked encloses that request, so the traversal reports it once the request
    /// returns. The load has been signaled by then if this command evaluated it, and the
    /// request started it only if it began no earlier than the request.
    fn process_load_discovery(&mut self, discovery: LoadDiscoverySignal) {
        if self
            .load_spans
            .get(&discovery.package)
            .is_some_and(|span| span.start() >= discovery.requested_at)
        {
            self.credit_load_discovery(
                discovery.package,
                discovery.discovered_by,
                discovery.requested_at,
            );
        }
    }

    /// Credit `discovered`'s load to `from_pkg`'s load unless an earlier discoverer is
    /// already on record. A package does not discover itself.
    fn credit_load_discovery(
        &mut self,
        discovered: PackageLabel,
        from_pkg: PackageLabel,
        at: Instant,
    ) {
        if discovered == from_pkg {
            return;
        }
        let discovered = NodeKey::InterpreterResultsKey(InterpreterResultsKey(discovered));
        let from = NodeKey::InterpreterResultsKey(InterpreterResultsKey(from_pkg));
        match self.first_requester_for_load.entry(discovered) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert((from, at));
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if at < e.get().1 {
                    e.insert((from, at));
                }
            }
        }
    }

    /// Whether `discoverer`'s load finished before `discovered`'s began, or `None` if this
    /// command did not evaluate one of them. DICE signals a key before releasing its waiters,
    /// so the loads a requester depended on are signaled before the requester is.
    fn load_could_discover(
        &self,
        discoverer: PackageLabel,
        discovered: PackageLabel,
    ) -> Option<bool> {
        let discoverer = self.load_spans.get(&discoverer)?;
        let discovered = self.load_spans.get(&discovered)?;
        Some(discoverer.end() <= discovered.start())
    }

    fn process_top_level_target(&mut self, top_level: TopLevelTargetSignal) {
        self.backend.process_top_level_target(
            top_level.label,
            top_level
                .artifacts
                .iter()
                .map(|k| match k {
                    ResolvedArtifactGroupBuildSignalsKey::BuildKey(b) => {
                        NodeKey::BuildKey(b.clone())
                    }
                    ResolvedArtifactGroupBuildSignalsKey::EnsureTransitiveSetProjectionKey(e) => {
                        NodeKey::EnsureTransitiveSetProjectionKey(e.clone())
                    }
                })
                .collect::<Vec<_>>(),
        );
    }

    fn process_final_materialization(&mut self, materialization: FinalMaterializationSignal) {
        let dep = match &materialization.from_group {
            ArtifactGroup::TransitiveSetProjection(key) => {
                NodeKey::EnsureTransitiveSetProjectionKey(
                    EnsureTransitiveSetProjectionKey::ref_cast(&key.key).clone(),
                )
            }
            _ => NodeKey::BuildKey(BuildKey(materialization.artifact.key().dupe())),
        };

        self.backend.process_node(
            NodeKey::FinalMaterialization(materialization.artifact),
            NodeExtraData::None,
            materialization.duration,
            std::iter::once(dep),
            materialization.span_id.into_iter().collect(),
            materialization.waiting_data,
        );
    }

    fn process_test_execution(&mut self, signal: TestExecutionSignal) {
        let key = TestExecutionBuildSignalKey {
            target: signal.target.dupe(),
            suite: Arc::new(signal.suite.to_owned()),
            testcases: Arc::new(signal.testcases),
            variant: signal.variant.map(Arc::new),
        };

        let deps = signal
            .deps
            .into_iter()
            .map(|d| NodeKey::BuildKey(BuildKey(d)));

        let listing_key = self
            .test_listing_keys
            .get(&signal.suite)
            .map(|k| k.dupe())
            .into_iter();

        self.backend.process_node(
            NodeKey::TestExecution(key),
            NodeExtraData::None,
            signal.duration,
            deps.chain(listing_key),
            Default::default(),
            signal.waiting_data,
        );
    }

    fn process_test_listing(&mut self, signal: TestListingSignal) {
        let key = TestListingBuildSignalKey {
            target: signal.target,
            suite: Arc::new(signal.suite.to_owned()),
        };

        let node_key = NodeKey::TestListing(key);

        // Since the TestListing and TestExecution keys are only created here, use this hashmap to
        // create dependencies between them.
        self.test_listing_keys.insert(signal.suite, node_key.dupe());

        let deps = signal
            .deps
            .into_iter()
            .map(|d| NodeKey::BuildKey(BuildKey(d)));

        self.backend.process_node(
            node_key,
            NodeExtraData::None,
            signal.duration,
            deps,
            Default::default(),
            signal.waiting_data,
        );
    }

    fn process_install(&mut self, signal: InstallSignal) {
        let key = InstallBuildSignalKey {
            installer: signal.installer,
            targets: Arc::new(signal.targets),
        };

        let deps = signal
            .deps
            .into_iter()
            .map(|d| NodeKey::BuildKey(BuildKey(d)));

        self.backend.process_node(
            NodeKey::Install(key),
            NodeExtraData::None,
            signal.duration,
            deps,
            Default::default(),
            signal.waiting_data,
        );
    }

    /// Value-demand reads can follow an earlier activation. Keep their timing independent:
    /// the loaded key's metadata-only consumers did not wait for this read. Other page-ins
    /// are paired with their subsequent activation.
    fn process_page_in(&mut self, page_in: PageInSignal) {
        match page_in.phase {
            PageInPhase::ValueDemand => {
                self.backend.process_node(
                    NodeKey::PageIn(Arc::new(page_in.key)),
                    NodeExtraData::None,
                    page_in.duration,
                    std::iter::empty::<NodeKey>(),
                    Default::default(),
                    WaitingData::new(),
                );
            }
            PageInPhase::AfterRecompute => {
                self.pending_page_ins.insert(page_in.key.dupe(), page_in);
            }
        }
    }

    fn flush_unmatched_page_ins(&mut self) {
        for (_, page_in) in self.pending_page_ins.drain() {
            self.backend.process_node(
                NodeKey::PageIn(Arc::new(page_in.key)),
                NodeExtraData::None,
                page_in.duration,
                std::iter::empty::<NodeKey>(),
                Default::default(),
                WaitingData::new(),
            );
        }
    }
}

pub(crate) struct BuildInfo {
    /// Node, its data, and its potential for improvement
    critical_path: DetailedCriticalPath,
    /// Path where each node's predecessor is the dependency that finished last.
    /// Unlike critical path, waiting time is directly attributable to the immediate predecessor.
    slowest_path: DetailedCriticalPath,
    num_nodes: u64,
    num_edges: u64,
    /// Critical path for top level targets
    top_level_targets: Vec<(ConfiguredTargetLabel, Duration)>,
}

/// Entry in a detailed critical path, including metadata about timing and dependencies.
pub(crate) struct DetailedCriticalPathEntry {
    pub(crate) key: NodeKey,
    pub(crate) data: NodeData,
    /// The potential improvement if this node's duration was reduced to zero.
    pub(crate) potential_improvement: Option<Duration>,
    /// The time when all dependencies finished executing (if known).
    pub(crate) deps_finished_time: Option<Instant>,
}

pub(crate) struct DetailedCriticalPath {
    entries: Vec<DetailedCriticalPathEntry>,
}

impl DetailedCriticalPath {
    fn empty() -> DetailedCriticalPath {
        Self {
            entries: Vec::new(),
        }
    }

    fn new(entries: Vec<DetailedCriticalPathEntry>) -> Self {
        Self { entries }
    }

    fn create_proto_entries_for_early_timings(
        enhancer: &mut CriticalPathProtoEnhancer,
        early_command_timing: &EarlyCommandTiming,
    ) {
        let generic_entry = |kind: &str| -> buck2_data::critical_path_entry2::Entry {
            buck2_data::critical_path_entry2::GenericEntry {
                kind: kind.to_owned(),
            }
            .into()
        };

        let mut current_kind = "buckd_command_init";
        let mut current_start = early_command_timing.command_start;
        for (span_start, kind) in &early_command_timing.early_spans {
            let span_start = span_start.max(&current_start);

            enhancer.add_simple_entry(
                None,
                generic_entry(current_kind),
                TimeSpan::new_saturating(current_start, *span_start),
                true,
            );
            current_kind = kind;
            current_start = *span_start;
        }
        enhancer.add_simple_entry(
            None,
            generic_entry(current_kind),
            TimeSpan::new_saturating(current_start, early_command_timing.early_command_end),
            true,
        );
    }

    fn into_critical_path_proto(
        self,
        early_command_timing: &EarlyCommandTiming,
        critical_path_compute_start: Instant,
    ) -> Vec<buck2_data::CriticalPathEntry2> {
        let mut enhancer = CriticalPathProtoEnhancer::new(
            early_command_timing.command_start,
            1 + early_command_timing.early_spans.len() + self.entries.len() + 1,
        );

        Self::create_proto_entries_for_early_timings(&mut enhancer, early_command_timing);

        for entry in self.entries {
            enhancer.add_entry(entry);
        }

        enhancer.add_simple_entry(
            Some("unknown_final_work"),
            buck2_data::critical_path_entry2::ComputeCriticalPath {}.into(),
            TimeSpan::new_saturating(critical_path_compute_start, Instant::now()),
            true,
        );
        enhancer.into_entries()
    }
}

/// Struct to hold data about a build graph node for critical path analysis.
#[derive(Clone)]
struct NodeData {
    /// Node-type-specific extra data (action data, load result, or none).
    extra_data: NodeExtraData,
    duration: NodeDuration,
    /// Data about time spent waiting (not on critical path) during this node's execution.
    waiting_data: WaitingData,
    span_ids: SmallVec<[SpanId; 1]>,
}

/// Type-safe enum for extra data associated with different node types in the build graph.
#[derive(Clone)]
enum NodeExtraData {
    /// The data that corresponds to a `NodeKey::BuildKey` Evaluation.
    Action(ActionNodeData),
    Analysis(AnalysisNodeData),
    /// The node was not computed in this command: DICE validated its dependencies and reused the
    /// value from a previous command.
    Reused,
    /// No extra data (used for other node types or when data is not available).
    None,
}

/// Extra data specific to action nodes.
///
/// Contains action execution metadata including the registered action, execution kind,
/// rule type, digest, and invalidation information.
#[derive(Clone)]
struct ActionNodeData {
    action: Arc<RegisteredAction>,
    execution_kind: buck2_data::ActionExecutionKind,
    target_rule_type_name: Option<String>,
    action_digest: Option<String>,
    invalidation_info: Option<buck2_data::CommandInvalidationInfo>,
}

impl ActionNodeData {
    fn from_extra_data(data: ActionWithExtraData) -> Self {
        let ActionWithExtraData {
            action,
            extra_data:
                ActionExtraData {
                    execution_kind,
                    target_rule_type_name,
                    action_digest,
                    invalidation_info,
                    ..
                },
        } = data;
        Self {
            action,
            execution_kind,
            target_rule_type_name,
            action_digest,
            invalidation_info,
        }
    }
}

#[derive(Clone)]
struct AnalysisNodeData {
    target_rule_type_name: Option<String>,
    /// When analysis is split due to anon targets: None = whole, Some(1) = part 1, Some(2) = part 2
    part: Option<u32>,
}

impl AnalysisNodeData {
    fn from_extra_data(data: AnalysisWithExtraData) -> Self {
        Self {
            target_rule_type_name: data.target_rule_type_name,
            part: None,
        }
    }

    fn with_part(self, part: Option<u32>) -> Self {
        Self { part, ..self }
    }
}

size_assert::words_of_type!(NodeData, 20);

/// Finish key for the second part of a split analysis (regular target).
/// Used when analysis is split due to anon target dependencies.
#[derive(Debug, Clone)]
struct AnalysisFinishNodeKey {
    target: ConfiguredTargetLabel,
    target_rule_type_name: Option<String>,
}

impl PartialEq for AnalysisFinishNodeKey {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target
    }
}

impl Eq for AnalysisFinishNodeKey {}

impl Hash for AnalysisFinishNodeKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.target.hash(state);
    }
}

impl fmt::Display for AnalysisFinishNodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnalysisFinish({})", self.target)
    }
}

impl BuildSignalsNodeKeyImpl for AnalysisFinishNodeKey {
    fn critical_path_entry_proto(&self) -> Option<buck2_data::critical_path_entry2::Entry> {
        Some(
            buck2_data::critical_path_entry2::Analysis {
                target: Some(
                    buck2_data::critical_path_entry2::analysis::Target::StandardTarget(
                        self.target.as_proto(),
                    ),
                ),
                target_rule_type_name: self.target_rule_type_name.clone(),
                part: Some(2),
            }
            .into(),
        )
    }

    fn kind(&self) -> &'static str {
        "analysis_finish"
    }
}

/// Finish key for the second part of a split anon target analysis.
/// Used when anon target analysis is split due to inner anon target dependencies.
#[derive(Debug, Clone)]
struct AnonTargetFinishNodeKey {
    /// Display name for identity/hashing
    display_name: String,
    /// Proto for the critical path entry
    anon_target_proto: buck2_data::AnonTarget,
}

impl PartialEq for AnonTargetFinishNodeKey {
    fn eq(&self, other: &Self) -> bool {
        self.display_name == other.display_name
    }
}

impl Eq for AnonTargetFinishNodeKey {}

impl Hash for AnonTargetFinishNodeKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.display_name.hash(state);
    }
}

impl fmt::Display for AnonTargetFinishNodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnonTargetFinish({})", self.display_name)
    }
}

impl BuildSignalsNodeKeyImpl for AnonTargetFinishNodeKey {
    fn critical_path_entry_proto(&self) -> Option<buck2_data::critical_path_entry2::Entry> {
        Some(
            buck2_data::critical_path_entry2::AnonAnalysis {
                anon_target: Some(self.anon_target_proto.clone()),
                part: Some(2),
            }
            .into(),
        )
    }

    fn kind(&self) -> &'static str {
        "anon_target_finish"
    }
}

/// Data attached to a Part 2 evaluation to track which anon targets were discovered
/// by which analysis Part 1.
struct SplitDiscoveryData {
    /// The analysis Part 1 node that discovered the anon targets.
    discovering_analysis: NodeKey,
    /// Wall-clock end time of Part 1 (pre_promises). Used to determine which
    /// analysis first discovered a shared anon target — the one whose Part 1
    /// finished earliest is the true discoverer.
    discovering_analysis_end: Instant,
    /// The anon target node keys that were discovered.
    anon_targets: Vec<NodeKey>,
}

fn create_build_signals() -> (BuildSignalsInstaller, Box<dyn DeferredBuildSignals>) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

    let sender = Arc::new(BuildSignalSender {
        sender,
        pending_page_in_phases: BuckDashMap::default(),
        page_in_reachability: OnceLock::new(),
    });
    let installer = BuildSignalsInstaller {
        build_signals: sender.dupe() as _,
        activation_tracker: sender.dupe() as _,
    };

    let deferred = Box::new(DeferredBuildSignalsImpl { sender, receiver });

    (installer, deferred as _)
}

pub fn init_late_bindings() {
    CREATE_BUILD_SIGNALS.init(create_build_signals)
}

pub(crate) fn duration_to_proto_saturating(duration: Duration) -> prost_types::Duration {
    duration.try_into().unwrap_or(prost_types::Duration {
        seconds: i64::MAX,
        nanos: i32::MAX,
    })
}

#[cfg(test)]
mod tests {
    use buck2_core::configuration::data::ConfigurationData;

    use super::*;

    #[derive(Default)]
    struct RecordingBackend {
        deps: BuckMutMap<NodeKey, Vec<NodeKey>>,
        durations: BuckMutMap<NodeKey, NodeDuration>,
    }

    impl BuildListenerBackend for RecordingBackend {
        fn process_node(
            &mut self,
            key: NodeKey,
            _extra_data: NodeExtraData,
            duration: NodeDuration,
            dep_keys: impl IntoIterator<Item = NodeKey>,
            _span_ids: SmallVec<[SpanId; 1]>,
            _waiting_data: WaitingData,
        ) {
            self.durations.insert(key.dupe(), duration);
            assert!(
                self.deps
                    .insert(key, dep_keys.into_iter().collect())
                    .is_none(),
                "a key must be emitted only once"
            );
        }

        fn process_top_level_target(
            &mut self,
            _analysis: ConfiguredTargetLabel,
            _artifacts: impl IntoIterator<Item = NodeKey>,
        ) {
        }

        fn finish(
            self,
            _discovery_edges: BuckMutMap<NodeKey, NodeKey>,
        ) -> Result<BuildInfo, CriticalPathError> {
            unreachable!("topology tests do not finish the backend")
        }

        fn name() -> CriticalPathBackendName {
            CriticalPathBackendName::LongestPathGraph
        }
    }

    fn node_key(package: &str) -> NodeKey {
        NodeKey::PackageListingKey(PackageListingKey(PackageLabel::testing_parse(package)))
    }

    fn page_in(key: NodeKey, phase: PageInPhase) -> PageInSignal {
        PageInSignal {
            key,
            duration: NodeDuration {
                user: Duration::from_millis(1),
                total: TimeSpan::from_start_and_duration(Instant::now(), Duration::from_millis(1)),
                queue: None,
            },
            phase,
        }
    }

    fn evaluation(
        key: NodeKey,
        dep: NodeKey,
        page_in_key: NodeKey,
        phase: PageInPhase,
    ) -> Evaluation {
        Evaluation {
            key,
            duration: NodeDuration::zero(),
            dep_keys: vec![dep],
            spans: Default::default(),
            waiting_data: WaitingData::new(),
            extra_data: NodeExtraData::None,
            split_discovery: None,
            page_in: Some(PageInAssociation {
                key: Arc::new(page_in_key),
                phase,
            }),
        }
    }

    fn receiver() -> BuildSignalReceiver<RecordingBackend> {
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        BuildSignalReceiver::new(receiver, RecordingBackend::default())
    }

    fn load_key(package: &str) -> NodeKey {
        NodeKey::InterpreterResultsKey(InterpreterResultsKey(PackageLabel::testing_parse(package)))
    }

    fn analysis_key(target: &str) -> NodeKey {
        NodeKey::AnalysisKey(AnalysisKey(ConfiguredTargetLabel::testing_parse(
            target,
            ConfigurationData::testing_new(),
        )))
    }

    fn plain_evaluation(key: NodeKey, dep_keys: Vec<NodeKey>, end: Instant) -> Evaluation {
        Evaluation {
            key,
            duration: NodeDuration {
                user: Duration::ZERO,
                total: TimeSpan::new_saturating(end, end),
                queue: None,
            },
            dep_keys,
            spans: Default::default(),
            waiting_data: WaitingData::new(),
            extra_data: NodeExtraData::None,
            split_discovery: None,
            page_in: None,
        }
    }

    /// An evaluation without its own timing is stamped when its activation is reported,
    /// which is after the hydration it is modelled as preceding.
    #[test]
    fn page_in_after_recompute_never_precedes_its_evaluation_work() {
        let mut receiver = receiver();
        let key = node_key("cell//p");
        let hydrate = TimeSpan::from_start_and_duration(
            Instant::now() - Duration::from_millis(2),
            Duration::from_millis(1),
        );
        receiver.process_page_in(PageInSignal {
            key: key.dupe(),
            duration: NodeDuration {
                user: hydrate.duration(),
                total: hydrate,
                queue: None,
            },
            phase: PageInPhase::AfterRecompute,
        });
        receiver.process_evaluation(evaluation(
            key.dupe(),
            node_key("cell//dep"),
            key.dupe(),
            PageInPhase::AfterRecompute,
        ));

        let span = |key: &NodeKey| receiver.backend.durations[key].total;
        let work = span(&NodeKey::EvaluationWork(Arc::new(key.dupe())));
        let page_in = span(&NodeKey::PageIn(Arc::new(key.dupe())));
        let completion = span(&key);
        assert!(
            work.end() <= page_in.start(),
            "work {work:?} must end before the page-in {page_in:?} starts"
        );
        assert_eq!(page_in, hydrate);
        assert_eq!(completion.start(), page_in.end());
    }

    /// A package is discovered by the earliest-finishing computation about a target of
    /// another package that depends on its load or on one of its nodes; the discovery edge
    /// points at the load of that target's package. Dependencies within one package, and
    /// packages no tracked computation depended on, get no edge.
    #[test]
    fn load_discovery_edges_point_at_discovering_package_load() {
        let mut receiver = receiver();
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(1);

        receiver.process_evaluation(plain_evaluation(load_key("cell//a"), vec![], t0));
        receiver.process_evaluation(plain_evaluation(load_key("cell//b"), vec![], t0));
        receiver.process_evaluation(plain_evaluation(load_key("cell//c"), vec![], t0));
        receiver.process_evaluation(plain_evaluation(load_key("cell//d"), vec![], t0));
        // cell//c's target depends on cell//b's load first in time but is processed later.
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//a:x"),
            vec![load_key("cell//b"), load_key("cell//a")],
            later,
        ));
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//c:y"),
            vec![load_key("cell//b")],
            t0,
        ));
        // Depending on a node of cell//d, not its load, also discovers cell//d.
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//a:w"),
            vec![analysis_key("cell//d:leaf")],
            t0,
        ));
        // A computation with no target of its own (matching select keys) leaves its loads
        // to the packaged computation that depends on it.
        receiver.process_evaluation(plain_evaluation(load_key("cell//e"), vec![], t0));
        let nameless = NodeKey::Dyn(
            "Nameless",
            BuildSignalsNodeKey::new(Nameless("select keys")),
        );
        receiver.process_evaluation(plain_evaluation(
            nameless.dupe(),
            vec![load_key("cell//e")],
            t0,
        ));
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//a:v"),
            vec![nameless],
            t0,
        ));

        let edges: BuckMutMap<NodeKey, NodeKey> = receiver
            .first_requester_for_load
            .iter()
            .map(|(k, (v, _))| (k.dupe(), v.dupe()))
            .collect();
        assert_eq!(
            edges.get(&load_key("cell//b")),
            Some(&load_key("cell//c")),
            "the earliest-finishing requester's package should win"
        );
        assert_eq!(
            edges.get(&load_key("cell//a")),
            None,
            "a request from within the package itself is not a discovery"
        );
        assert_eq!(
            edges.get(&load_key("cell//c")),
            None,
            "nothing tracked depended on cell//c"
        );
        assert_eq!(
            edges.get(&load_key("cell//d")),
            Some(&load_key("cell//a")),
            "a dependency on another package's node discovers that package"
        );
        assert_eq!(
            edges.get(&load_key("cell//e")),
            Some(&load_key("cell//a")),
            "discoveries forward through a package-less computation"
        );
    }

    /// Only a package whose load finished before the discovered load began can have
    /// discovered it, so a package-level cycle yields one edge, not two. A load this command
    /// did not evaluate gets no edge.
    #[test]
    fn load_discovery_requires_the_discoverer_to_have_loaded_first() {
        let mut receiver = receiver();
        let t0 = Instant::now();
        let at = |secs| t0 + Duration::from_secs(secs);
        let load = |package: &str, start, end| Evaluation {
            duration: NodeDuration {
                user: Duration::ZERO,
                total: TimeSpan::new_saturating(start, end),
                queue: None,
            },
            ..plain_evaluation(load_key(package), vec![], end)
        };

        receiver.process_evaluation(load("cell//a", at(0), at(1)));
        receiver.process_evaluation(load("cell//b", at(2), at(3)));
        // cell//b:y finishes first, but cell//b loaded after cell//a.
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//b:y"),
            vec![load_key("cell//a")],
            at(4),
        ));
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//a:x"),
            vec![load_key("cell//b")],
            at(5),
        ));
        receiver.process_evaluation(load("cell//c", at(0), at(2)));
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//a:w"),
            vec![load_key("cell//c")],
            at(6),
        ));
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//a:v"),
            vec![load_key("cell//d")],
            at(6),
        ));

        let edge = |package: &str| {
            receiver
                .first_requester_for_load
                .get(&load_key(package))
                .map(|(from, _)| from.dupe())
        };
        assert_eq!(edge("cell//b"), Some(load_key("cell//a")));
        assert_eq!(
            edge("cell//a"),
            None,
            "cell//b loaded after cell//a began, so it cannot have discovered it"
        );
        assert_eq!(
            edge("cell//c"),
            None,
            "cell//c began loading before cell//a finished"
        );
        assert_eq!(
            edge("cell//d"),
            None,
            "this command did not evaluate cell//d's load"
        );
    }

    #[test]
    fn load_discovery_does_not_forward_through_chained_packageless_computations() {
        let mut receiver = receiver();
        let t0 = Instant::now();
        let nameless = |name| NodeKey::Dyn("Nameless", BuildSignalsNodeKey::new(Nameless(name)));
        let inner = nameless("inner");
        let outer = nameless("outer");

        receiver.process_evaluation(plain_evaluation(load_key("cell//a"), vec![], t0));
        receiver.process_evaluation(plain_evaluation(load_key("cell//e"), vec![], t0));
        receiver.process_evaluation(plain_evaluation(
            inner.dupe(),
            vec![load_key("cell//e")],
            t0,
        ));
        receiver.process_evaluation(plain_evaluation(outer.dupe(), vec![inner.dupe()], t0));
        receiver.process_evaluation(plain_evaluation(
            analysis_key("cell//a:v"),
            vec![outer.dupe()],
            t0,
        ));

        assert!(
            receiver.discoveries_awaiting_package.contains_key(&inner),
            "inner depends on cell//e's load directly, so it holds that discovery"
        );
        assert!(
            !receiver.discoveries_awaiting_package.contains_key(&outer),
            "outer only depends on another package-less computation, so it holds nothing"
        );
        assert_eq!(
            receiver.first_requester_for_load.get(&load_key("cell//e")),
            None,
            "cell//a:v reaches cell//e only through two package-less computations"
        );
    }

    /// A query traversal reports which node's expansion requested a package: the first
    /// report wins, a later one does not displace it, and a package never discovers itself.
    #[test]
    fn load_discovery_reports_credit_the_requesting_package() {
        let mut receiver = receiver();
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(1);
        let discovery = |package: &str, discovered_by: &str, at: Instant| LoadDiscoverySignal {
            package: PackageLabel::testing_parse(package),
            discovered_by: PackageLabel::testing_parse(discovered_by),
            requested_at: at,
        };

        receiver.process_evaluation(plain_evaluation(load_key("cell//a"), vec![], t0));
        receiver.process_evaluation(plain_evaluation(load_key("cell//b"), vec![], t0));
        receiver.process_load_discovery(discovery("cell//b", "cell//a", t0));
        receiver.process_load_discovery(discovery("cell//b", "cell//c", later));
        receiver.process_load_discovery(discovery("cell//a", "cell//a", t0));

        let edges: BuckMutMap<NodeKey, NodeKey> = receiver
            .first_requester_for_load
            .iter()
            .map(|(k, (v, _))| (k.dupe(), v.dupe()))
            .collect();
        assert_eq!(
            edges.get(&load_key("cell//b")),
            Some(&load_key("cell//a")),
            "the first reported requester should win"
        );
        assert_eq!(
            edges.get(&load_key("cell//a")),
            None,
            "a package does not discover itself"
        );
    }

    /// A traversal's request discovered a load only if the load began no earlier than the
    /// request, and only if this command evaluated it.
    #[test]
    fn load_discovery_reports_require_the_request_to_start_the_load() {
        let mut receiver = receiver();
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(1);
        let discovery = |package: &str, at: Instant| LoadDiscoverySignal {
            package: PackageLabel::testing_parse(package),
            discovered_by: PackageLabel::testing_parse("cell//a"),
            requested_at: at,
        };

        receiver.process_evaluation(plain_evaluation(load_key("cell//a"), vec![], t0));
        receiver.process_evaluation(plain_evaluation(load_key("cell//b"), vec![], t0));
        receiver.process_evaluation(plain_evaluation(load_key("cell//c"), vec![], later));
        receiver.process_load_discovery(discovery("cell//b", later));
        receiver.process_load_discovery(discovery("cell//c", t0));
        receiver.process_load_discovery(discovery("cell//d", t0));

        let edge = |package: &str| {
            receiver
                .first_requester_for_load
                .get(&load_key(package))
                .map(|(from, _)| from.dupe())
        };
        assert_eq!(
            edge("cell//b"),
            None,
            "cell//b began loading before the request"
        );
        assert_eq!(edge("cell//c"), Some(load_key("cell//a")));
        assert_eq!(
            edge("cell//d"),
            None,
            "this command did not evaluate cell//d's load"
        );
    }

    #[derive(Debug, PartialEq, Eq, Hash)]
    struct Nameless(&'static str);

    impl std::fmt::Display for Nameless {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl BuildSignalsNodeKeyImpl for Nameless {}

    #[test]
    fn value_demand_records_a_standalone_page_in() {
        let mut receiver = receiver();
        let key = node_key("cell//match");
        let page_in_key = NodeKey::PageIn(Arc::new(key.dupe()));

        receiver.process_page_in(page_in(key.dupe(), PageInPhase::ValueDemand));

        assert!(!receiver.backend.deps.contains_key(&key));
        assert!(receiver.backend.deps[&page_in_key].is_empty());
    }

    #[test]
    fn value_demand_preserves_an_earlier_activation() {
        let mut receiver = receiver();
        let key = node_key("cell//demand");
        let dep = node_key("cell//dep");
        let page_in_key = NodeKey::PageIn(Arc::new(key.dupe()));
        let mut activated =
            evaluation(key.dupe(), dep.dupe(), key.dupe(), PageInPhase::ValueDemand);
        activated.page_in = None;
        activated.extra_data = NodeExtraData::Reused;

        receiver.process_evaluation(activated);
        receiver.process_page_in(page_in(key.dupe(), PageInPhase::ValueDemand));

        assert_eq!(receiver.backend.deps[&key], [dep]);
        assert!(receiver.backend.deps[&page_in_key].is_empty());
        assert!(receiver.pending_page_ins.is_empty());
    }

    #[test]
    fn comparison_page_in_follows_recomputation() {
        let mut receiver = receiver();
        let paged_key = node_key("cell//recompute");
        let completion_key = node_key("cell//recompute_finish");
        let dep = node_key("cell//dep");
        let page_in_key = NodeKey::PageIn(Arc::new(paged_key.dupe()));
        let work_key = NodeKey::EvaluationWork(Arc::new(completion_key.dupe()));
        let phase = PageInPhase::AfterRecompute;

        receiver.process_page_in(page_in(paged_key.dupe(), phase));
        receiver.process_evaluation(evaluation(
            completion_key.dupe(),
            dep.dupe(),
            paged_key,
            phase,
        ));

        assert_eq!(receiver.backend.deps[&completion_key], [page_in_key.dupe()]);
        assert_eq!(receiver.backend.deps[&page_in_key], [work_key.dupe()]);
        assert_eq!(receiver.backend.deps[&work_key], [dep]);
    }
}
