/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::hash::Hash;
use std::mem;
use std::sync::Arc;

use async_trait::async_trait;
use buck2_artifact::actions::key::ActionKey;
use buck2_build_api::actions::artifact::get_artifact_fs::GetArtifactFs;
use buck2_build_api::actions::calculation::ActionCalculation;
use buck2_build_api::actions::query::ActionInput;
use buck2_build_api::actions::query::ActionQueryNode;
use buck2_build_api::actions::query::ActionQueryNodeRef;
use buck2_build_api::actions::query::SetProjectionInputs;
use buck2_build_api::actions::query::iter_action_inputs;
use buck2_build_api::analysis::AnalysisResult;
use buck2_build_api::analysis::calculation::RuleAnalysisCalculation;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::artifact_groups::ResolvedArtifactGroup;
use buck2_build_api::artifact_groups::TransitiveSetProjectionKey;
use buck2_build_api::keep_going::KeepGoing;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckDashMap;
use buck2_hash::BuckMutMap;
use buck2_hash::BuckMutSet;
use buck2_node::target_calculation::ConfiguredTargetCalculation;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use dashmap::mapref::entry::Entry;
use dice::DiceComputations;
use dupe::Dupe;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use futures::future::Shared;
use itertools::Either;
use itertools::Itertools;
use tokio::sync::oneshot;

use crate::aquery::environment::AqueryDelegate;
use crate::cquery::environment::CqueryDelegate;
use crate::dice::DiceQueryData;
use crate::dice::DiceQueryDelegate;
use crate::uquery::environment::QueryLiterals;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ActionQueryError {
    #[error(
        "`aquery` currently only supports literal target patterns, not package or recursive (got `{0}`)"
    )]
    UnsupportedTargetPattern(String),
}

/// A simple concurrent map with a `get_or_compute()` function
struct NodeCache<K: Hash + Eq + PartialEq + Dupe, V: Dupe> {
    map: BuckDashMap<K, Shared<oneshot::Receiver<V>>>,
}

impl<K: Hash + Eq + PartialEq + Dupe, V: Dupe> NodeCache<K, V> {
    fn new() -> Self {
        Self {
            map: BuckDashMap::default(),
        }
    }

    /// Whether this entry's computation was cancelled, so no value will ever arrive.
    fn is_dead(fut: &Shared<oneshot::Receiver<V>>) -> bool {
        matches!(fut.clone().now_or_never(), Some(Err(_)))
    }

    /// Gets the value only if its computation has already finished; never awaits.
    fn try_get_completed(&self, key: &K) -> Option<V> {
        let fut = self.map.get(key).map(|entry| entry.value().clone())?;
        fut.now_or_never()?.ok()
    }

    /// Gets the value if it is already cached or currently being computed (in which case this
    /// awaits that computation). Returns `None` if the key is absent or if the in-flight
    /// computation was cancelled (cancelled entries are cleared so the value can be computed
    /// or seeded again).
    async fn try_get(&self, key: &K) -> Option<V> {
        let fut = self.map.get(key).map(|entry| entry.value().clone())?;
        match fut.await {
            Ok(v) => Some(v),
            Err(_) => {
                self.map.remove_if(key, |_, fut| Self::is_dead(fut));
                None
            }
        }
    }

    /// Caches an already-computed value. Does nothing if a computed or in-flight entry is
    /// already present, unless its computation was cancelled, in which case it is replaced.
    fn seed(&self, key: K, value: V) {
        let (tx, rx) = oneshot::channel();
        let _ignore = tx.send(value);
        match self.map.entry(key) {
            Entry::Occupied(mut occupied) => {
                if Self::is_dead(occupied.get()) {
                    let _replaced_dead_entry = occupied.insert(rx.shared());
                }
            }
            Entry::Vacant(vacant) => {
                vacant.insert(rx.shared());
            }
        }
    }

    fn remove(&self, key: &K) {
        self.map.remove(key);
    }

    /// Gets the value or computes it with the provided function. The function is called while holding
    /// a lock on the map and so should not do much work. The future returned by that function isn't
    /// polled until later so it's fine for it to do more work.
    async fn get_or_compute<Fut: Future<Output = V> + Send, F: FnOnce(K) -> Fut>(
        self: Arc<Self>,
        key: K,
        f: F,
    ) -> V {
        if let Some(v) = self.try_get(&key).await {
            return v;
        }

        loop {
            // Loop until we get a successful result from polling the channel
            // This is because only the original task that inserted the channel is responsible for
            // completing the channel, and since tasks may be cancelled (which would result in a
            // drop), the channel can return 'Err', indicating that no one will now complete the
            // task. In this case, all threads race to insert a task themselves, and the rest
            // will now await the new channel.
            let entry = self.map.entry(key.dupe());
            match entry {
                Entry::Occupied(occ) => {
                    let fut = occ.get().clone();
                    drop(occ);
                    if let Ok(v) = fut.await {
                        return v;
                    }
                    self.map.remove_if(&key, |_, fut| Self::is_dead(fut));
                }
                Entry::Vacant(vacant) => {
                    let (tx, rx) = oneshot::channel();
                    let key = vacant.key().dupe();
                    vacant.insert(rx.shared());

                    let v = f(key).await;
                    let _ignore = tx.send(v.dupe());

                    return v;
                }
            }
        }
    }
}

/// The maximum supported tset nesting depth in aquery.
///
/// `SetProjectionInputs` owns its children recursively through immutable `ArcIntern`s, so
/// dropping a chain of nodes recurses one stack frame set per nesting level and there is no
/// way to hook the drop to make it iterative. On the daemon's 2 MiB worker stacks that
/// cascade was observed to SIGSEGV somewhere between depth 8000 and 12000; this limit keeps
/// roughly 3x margin. Node construction itself is iterative and imposes no depth limit.
const MAX_TSET_NESTING_DEPTH: u32 = 4096;

/// A built tset projection node together with the nesting depth of its subtree, so that
/// depth accumulated across separately-computed (cached) subgraphs still counts toward
/// `MAX_TSET_NESTING_DEPTH`.
#[derive(Clone, Dupe)]
struct TsetNode {
    node: SetProjectionInputs,
    depth: u32,
}

/// Cache for ActionQueryNode and things needed to construct it. This is required because
/// QueryTarget::deps() requires that deps are synchronously available and so we need to
/// be able to iterate the tset structure synchronously.
#[derive(Clone, Dupe)]
struct DiceAqueryNodesCache {
    action_nodes: Arc<NodeCache<ActionKey, buck2_error::Result<ActionQueryNode>>>,
    tset_nodes: Arc<NodeCache<TransitiveSetProjectionKey, buck2_error::Result<TsetNode>>>,
    /// Completed shallow resolutions of tset nodes, inserted as concurrent
    /// `compute_tset_node` walks finish them so that walks over overlapping subgraphs
    /// reuse each other's finished work. Never awaited — see the discovery loop for why
    /// walks must not wait on one another. Entries are dropped once the built node is
    /// seeded into `tset_nodes`.
    tset_discovery:
        Arc<NodeCache<TransitiveSetProjectionKey, buck2_error::Result<Arc<DiscoveredTset>>>>,
}

impl DiceAqueryNodesCache {
    fn new() -> Self {
        Self {
            action_nodes: Arc::new(
                NodeCache::<ActionKey, buck2_error::Result<ActionQueryNode>>::new(),
            ),
            tset_nodes: Arc::new(NodeCache::<
                TransitiveSetProjectionKey,
                buck2_error::Result<TsetNode>,
            >::new()),
            tset_discovery: Arc::new(NodeCache::<
                TransitiveSetProjectionKey,
                buck2_error::Result<Arc<DiscoveredTset>>,
            >::new()),
        }
    }
}

pub(crate) struct DiceAqueryDelegate<'c, 'd> {
    base_delegate: DiceQueryDelegate<'c, 'd>,
    query_data: Arc<AqueryData>,
}

pub(crate) struct AqueryData {
    artifact_fs: ArtifactFs,
    delegate_query_data: Arc<DiceQueryData>,
    nodes_cache: DiceAqueryNodesCache,
}

// SAFETY: `_assert_send_sync` requires every field to be `Send + Sync`. Spelled out
// so that auto-trait proofs for aquery futures stop here; see `dice::DiceComputations`.
unsafe impl Send for AqueryData {}
unsafe impl Sync for AqueryData {}

fn _assert_send_sync(
    AqueryData {
        artifact_fs,
        delegate_query_data,
        nodes_cache,
    }: &AqueryData,
) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(artifact_fs);
    assert(delegate_query_data);
    assert(nodes_cache);
}

/// Resolves inputs one level deep: direct artifacts to their producing actions, tset
/// projections to their keys (without building the projections' nodes).
async fn convert_inputs_shallow<
    'a,
    Iter: IntoIterator<Item = &'a ArtifactGroup, IntoIter: ExactSizeIterator>,
>(
    ctx: &mut DiceComputations<'_>,
    inputs: Iter,
) -> buck2_error::Result<(Vec<ActionQueryNodeRef>, Vec<TransitiveSetProjectionKey>)> {
    let resolved_artifacts: Vec<_> =
        KeepGoing::try_compute_join_all(ctx, inputs, async |ctx, input| {
            input.resolved_artifact(ctx).await
        })
        .await?;

    Ok(Itertools::partition_map(
        resolved_artifacts
            .into_iter()
            .filter_map(|resolved_artifact| match resolved_artifact {
                ResolvedArtifactGroup::Artifact(a) => a
                    .action_key()
                    .map(|a| Either::Left(ActionQueryNodeRef::Action(a.dupe()))),
                ResolvedArtifactGroup::TransitiveSetProjection(key) => {
                    Some(Either::Right(key.dupe()))
                }
            }),
        |v| v,
    ))
}

/// Converts artifact inputs into aquery's ActionInput. This is mostly a matter of resolving the indirect
/// `TransitiveSetProjectionKey` to our direct shadow tset graph node `SetProjectionInputs`.
// TODO(jtbraun/cjhopman): Resolving a `TransitiveSetKey` to its tset value takes an async
// analysis lookup (cached by DICE, and the built nodes by `DiceAqueryNodesCache`).
// `ArtifactGroup` cannot hold the tset value itself to make this synchronous: an owned frozen
// ref there would pin the producing analysis's heap in every consuming action's inputs,
// preventing analysis results from being paged out of memory.
async fn convert_inputs<
    'a,
    Iter: IntoIterator<Item = &'a ArtifactGroup, IntoIter: ExactSizeIterator>,
>(
    ctx: &mut DiceComputations<'_>,
    node_cache: DiceAqueryNodesCache,
    inputs: Iter,
) -> buck2_error::Result<Vec<ActionInput>> {
    let (artifacts, projections) = convert_inputs_shallow(ctx, inputs).await?;

    let mut deps = artifacts
        .into_iter()
        .map(ActionInput::ActionKey)
        .collect::<Vec<_>>();
    let projection_deps = ctx
        .try_compute_join(projections, async |ctx, key| {
            get_tset_node(node_cache.dupe(), ctx, key).await
        })
        .await?;

    for node in projection_deps {
        deps.push(ActionInput::IndirectInputs(node));
    }
    Ok(deps)
}

/// A tset projection node's shallowly-resolved inputs, recorded by the discovery phase of
/// `compute_tset_node`.
struct DiscoveredTset {
    direct: Vec<ActionQueryNodeRef>,
    children: Vec<TransitiveSetProjectionKey>,
}

enum TsetDiscovery {
    /// The node was already cached (or being computed elsewhere); no need to descend into it.
    Cached(TransitiveSetProjectionKey, TsetNode),
    New(TransitiveSetProjectionKey, Arc<DiscoveredTset>),
}

/// Computes the `SetProjectionInputs` node for `root`, along with every uncached tset
/// projection node reachable from it.
///
/// Tset graphs nest arbitrarily deep, so this must not recurse per tset level: a recursive
/// walk polls one set of nested future frames per level on a fixed-size worker thread stack
/// and overflows around depth ~1500 (builds don't have this problem because each projection
/// there is its own spawned DICE computation). Instead, reachable projection nodes are
/// discovered with a breadth-first worklist and then assembled children-before-parents with
/// an explicit postorder stack.
async fn compute_tset_node(
    node_cache: DiceAqueryNodesCache,
    ctx: &mut DiceComputations<'_>,
    root: TransitiveSetProjectionKey,
) -> buck2_error::Result<TsetNode> {
    let mut discovered: BuckMutMap<TransitiveSetProjectionKey, Arc<DiscoveredTset>> =
        BuckMutMap::default();
    let mut built: BuckMutMap<TransitiveSetProjectionKey, TsetNode> = BuckMutMap::default();

    let mut seen: BuckMutSet<TransitiveSetProjectionKey> = BuckMutSet::default();
    seen.insert(root.dupe());
    let mut frontier = vec![root.dupe()];
    // The caller (`get_or_compute`) has already registered an in-flight cache entry for
    // `root`, so probing the cache for it would await our own computation.
    let mut probe_cache = false;

    while !frontier.is_empty() {
        let wave = ctx
            .try_compute_join(mem::take(&mut frontier), async |ctx, key| {
                if probe_cache && let Some(v) = node_cache.tset_nodes.try_get(&key).await {
                    return buck2_error::Ok(TsetDiscovery::Cached(key, v?));
                }
                // Reuse a resolution another walk has already *completed*; one that is
                // still in flight elsewhere is redone here instead. Waiting for it would
                // make every overlapping walk block at each shared node until the first
                // walk to reach that node finishes it, so walks could never run ahead of
                // each other and all of them would advance one node per wake-up.
                if let Some(info) = node_cache.tset_discovery.try_get_completed(&key) {
                    return Ok(TsetDiscovery::New(key, info?));
                }
                let set = key.key.lookup(ctx).await?;
                let sub_inputs = set.by_ref(|s| s.get_projection_sub_inputs(key.projection))?;
                let (direct, children) = convert_inputs_shallow(ctx, sub_inputs.iter()).await?;
                let info = Arc::new(DiscoveredTset { direct, children });
                node_cache.tset_discovery.seed(key.dupe(), Ok(info.dupe()));
                Ok(TsetDiscovery::New(key, info))
            })
            .await?;
        probe_cache = true;

        for discovery in wave {
            match discovery {
                TsetDiscovery::Cached(key, node) => {
                    built.insert(key, node);
                }
                TsetDiscovery::New(key, info) => {
                    for child in &info.children {
                        if seen.insert(child.dupe()) {
                            frontier.push(child.dupe());
                        }
                    }
                    discovered.insert(key, info);
                }
            }
        }
    }

    enum Visit {
        Enter(TransitiveSetProjectionKey),
        Exit(TransitiveSetProjectionKey),
    }

    let mut entered: BuckMutSet<TransitiveSetProjectionKey> = BuckMutSet::default();
    let mut stack = vec![Visit::Enter(root.dupe())];
    while let Some(visit) = stack.pop() {
        match visit {
            Visit::Enter(key) => {
                if built.contains_key(&key) || !entered.insert(key.dupe()) {
                    continue;
                }
                // A concurrent walk may have finished this node since discovery; reusing
                // it skips its whole subtree here.
                if key != root
                    && let Some(Ok(node)) = node_cache.tset_nodes.try_get_completed(&key)
                {
                    built.insert(key, node);
                    continue;
                }
                let info = discovered
                    .get(&key)
                    .internal_error("every non-cached reachable node was discovered")?;
                stack.push(Visit::Exit(key));
                for child in &info.children {
                    if !built.contains_key(child) {
                        stack.push(Visit::Enter(child.dupe()));
                    }
                }
            }
            Visit::Exit(key) => {
                let info = discovered
                    .remove(&key)
                    .internal_error("exited nodes are discovered and not yet built")?;
                let mut depth = 0;
                let children = info
                    .children
                    .iter()
                    .map(|child| {
                        let child = built
                            .get(child)
                            .internal_error("children are built before their parents")?;
                        depth = depth.max(child.depth);
                        Ok(child.node.dupe())
                    })
                    .collect::<buck2_error::Result<Vec<_>>>()?;
                let depth = depth + 1;
                if depth > MAX_TSET_NESTING_DEPTH {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Transitive set nesting depth exceeds the maximum of {} supported by aquery",
                        MAX_TSET_NESTING_DEPTH
                    ));
                }
                let node = TsetNode {
                    node: SetProjectionInputs::new(key.dupe(), info.direct.clone(), children),
                    depth,
                };
                if key != root {
                    node_cache.tset_nodes.seed(key.dupe(), Ok(node.dupe()));
                }
                node_cache.tset_discovery.remove(&key);
                built.insert(key, node);
            }
        }
    }

    built
        .remove(&root)
        .internal_error("the root node is built last")
}

async fn get_tset_node(
    node_cache: DiceAqueryNodesCache,
    ctx: &mut DiceComputations<'_>,
    key: TransitiveSetProjectionKey,
) -> buck2_error::Result<SetProjectionInputs> {
    let copied_node_cache = node_cache.dupe();
    let node = node_cache
        .tset_nodes
        .dupe()
        .get_or_compute(key, move |key| {
            compute_tset_node(copied_node_cache, ctx, key)
        })
        .await?;
    Ok(node.node)
}

fn compute_action_node<'c>(
    node_cache: DiceAqueryNodesCache,
    ctx: &'c mut DiceComputations<'_>,
    key: ActionKey,
    fs: ArtifactFs,
) -> BoxFuture<'c, buck2_error::Result<ActionQueryNode>> {
    async move {
        let action = ActionCalculation::get_action(ctx, &key).await?;
        let deps = convert_inputs(ctx, node_cache, action.inputs()?.iter()).await?;
        Ok(ActionQueryNode::new_action(action, deps, fs))
    }
    .boxed()
}

async fn get_action_node(
    node_cache: DiceAqueryNodesCache,
    ctx: &mut DiceComputations<'_>,
    key: ActionKey,
    fs: ArtifactFs,
) -> buck2_error::Result<ActionQueryNode> {
    let copied_node_cache = node_cache.dupe();
    node_cache
        .action_nodes
        .get_or_compute(key, move |key| {
            compute_action_node(copied_node_cache, ctx, key, fs)
        })
        .await
}

impl<'c, 'd> DiceAqueryDelegate<'c, 'd> {
    pub(crate) async fn new(
        base_delegate: DiceQueryDelegate<'c, 'd>,
    ) -> buck2_error::Result<DiceAqueryDelegate<'c, 'd>> {
        let artifact_fs = base_delegate.ctx().get_artifact_fs().await?.dupe();
        let query_data = Arc::new(AqueryData {
            artifact_fs,
            delegate_query_data: base_delegate.query_data().dupe(),
            nodes_cache: DiceAqueryNodesCache::new(),
        });
        Ok(DiceAqueryDelegate {
            base_delegate,
            query_data,
        })
    }

    pub(crate) fn query_data(&self) -> &Arc<AqueryData> {
        &self.query_data
    }

    pub(crate) async fn get_action_node(
        &self,
        key: &ActionKey,
    ) -> buck2_error::Result<ActionQueryNode> {
        get_action_node(
            self.query_data.nodes_cache.dupe(),
            &mut self.base_delegate.ctx(),
            key.dupe(),
            self.query_data.artifact_fs.dupe(),
        )
        .await
    }
}

#[async_trait]
impl AqueryDelegate for DiceAqueryDelegate<'_, '_> {
    fn cquery_delegate(&self) -> &dyn CqueryDelegate {
        &self.base_delegate
    }

    fn ctx(&self) -> DiceComputations<'_> {
        self.base_delegate.ctx()
    }

    async fn get_node(&self, key: &ActionKey) -> buck2_error::Result<ActionQueryNode> {
        self.get_action_node(key).await
    }

    async fn get_tset_node(
        &self,
        key: &TransitiveSetProjectionKey,
    ) -> buck2_error::Result<SetProjectionInputs> {
        get_tset_node(
            self.query_data.nodes_cache.dupe(),
            &mut self.base_delegate.ctx(),
            key.dupe(),
        )
        .await
    }

    async fn expand_artifacts(
        &self,
        artifacts: &[ArtifactGroup],
    ) -> buck2_error::Result<Vec<ActionQueryNode>> {
        let inputs = convert_inputs(
            &mut self.base_delegate.ctx(),
            self.query_data.nodes_cache.dupe(),
            artifacts,
        )
        .await?;

        let refs = iter_action_inputs(&inputs)
            .map(|i| i.require_action())
            .collect::<Result<Vec<_>, _>>()?;

        buck2_util::future::try_join_all(refs.iter().map(|n| self.get_node(n))).await
    }

    async fn get_target_set_from_analysis(
        &self,
        configured_label: &ConfiguredProvidersLabel,
        analysis: AnalysisResult,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        get_target_set_from_analysis_inner(
            self.query_data().as_ref(),
            configured_label,
            analysis,
            &mut self.ctx(),
        )
        .await
    }
}

async fn get_target_set_from_analysis_inner(
    query_data: &AqueryData,
    configured_label: &ConfiguredProvidersLabel,
    analysis: AnalysisResult,
    dice: &mut DiceComputations<'_>,
) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
    let mut result = TargetSet::new();

    let providers = analysis.lookup_inner(configured_label)?;
    let outputs = providers
        .provider_collection()
        .default_info()?
        .default_outputs()?;

    for output in outputs {
        if let Some(action_key) = output.artifact().action_key() {
            result.insert(
                get_action_node(
                    query_data.nodes_cache.dupe(),
                    dice,
                    action_key.dupe(),
                    query_data.artifact_fs.dupe(),
                )
                .await?,
            );
        }
    }

    result.insert(ActionQueryNode::new_analysis(
        configured_label.dupe(),
        analysis,
    ));

    Ok(result)
}

#[async_trait]
impl QueryLiterals<ActionQueryNode> for AqueryData {
    async fn eval_literals(
        &self,
        literals: &[&str],
        dice: &mut DiceComputations<'_>,
    ) -> buck2_error::Result<TargetSet<ActionQueryNode>> {
        // For literal evaluation, we resolve the providers pattern to the analysis result, pull out
        // the default outputs and look up the corresponding actions.
        // TODO(cjhopman): This is a common pattern and we should probably pull it out to a common
        // place, maybe with streaming results like we have for `target pattern -> target node`
        let mut result = TargetSet::new();
        for literal in literals {
            let label = self
                .delegate_query_data
                .literal_parser()
                .parse_providers_pattern(literal)?;
            match label {
                ParsedPattern::Target(package, target_name, providers) => {
                    let label = providers.into_providers_label(package, target_name.as_ref());
                    let configured_label = dice
                        .get_configured_provider_label(
                            &label,
                            self.delegate_query_data.global_cfg_options(),
                        )
                        .await?;

                    match dice
                        .get_analysis_result(configured_label.target())
                        .await
                        .ok()?
                    {
                        MaybeCompatible::Incompatible(_) => {
                            // ignored
                        }
                        MaybeCompatible::Compatible(analysis) => {
                            let target_set = get_target_set_from_analysis_inner(
                                self,
                                &configured_label,
                                analysis.dupe(),
                                dice,
                            )
                            .await?;
                            result.extend(&target_set);
                        }
                    }
                }
                ParsedPattern::Package(_) | ParsedPattern::Recursive(_) => {
                    return Err(
                        ActionQueryError::UnsupportedTargetPattern((*literal).to_owned()).into(),
                    );
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::future::ready;
    use std::sync::Arc;
    use std::task::Poll;

    use assert_matches::assert_matches;
    use dupe::Dupe;
    use futures::pin_mut;
    use futures::poll;

    use crate::dice::aquery::NodeCache;

    #[tokio::test]
    async fn test_node_cache() {
        let cache = Arc::new(NodeCache::new());

        let fut1 = cache.dupe().get_or_compute(1, |k| async move {
            tokio::task::yield_now().await;

            k
        });

        let fut2 = cache
            .dupe()
            .get_or_compute(1, |_k| async move { panic!("shouldn't run") });

        pin_mut!(fut1);
        pin_mut!(fut2);

        assert_matches!(poll!(&mut fut1), Poll::Pending);
        assert_matches!(poll!(&mut fut2), Poll::Pending);
        assert_matches!(poll!(&mut fut2), Poll::Pending);

        assert_matches!(poll!(&mut fut1), Poll::Ready(1));
        assert_matches!(poll!(&mut fut2), Poll::Ready(1));
    }

    #[tokio::test]
    async fn test_node_cache_takes_over_cancelled_computation() {
        let cache = Arc::new(NodeCache::new());

        {
            let fut = cache.dupe().get_or_compute(1, |_k| pending());
            pin_mut!(fut);
            assert_matches!(poll!(&mut fut), Poll::Pending);
        }

        assert_eq!(
            cache.dupe().get_or_compute(1, |k| ready(k + 1)).await,
            2,
            "a later caller should compute the value after the owner was cancelled"
        );
    }

    #[tokio::test]
    async fn test_node_cache_seed() {
        let cache = Arc::new(NodeCache::new());

        cache.seed(1, 10);
        cache.seed(1, 11);
        assert_eq!(
            cache.try_get(&1).await,
            Some(10),
            "a live entry should not be replaced"
        );

        {
            let fut = cache.dupe().get_or_compute(2, |_k| pending());
            pin_mut!(fut);
            assert_matches!(poll!(&mut fut), Poll::Pending);
        }

        cache.seed(2, 12);
        assert_eq!(
            cache.try_get(&2).await,
            Some(12),
            "a cancelled entry should be replaced"
        );
    }
}
