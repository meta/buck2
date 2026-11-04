/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Cycle detection: what a detected cycle does to the tasks in it, how cycles are found among a
//! version's tasks, and the whole thing end to end.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use allocative::Allocative;
use assert_matches::assert_matches;
use async_trait::async_trait;
use derive_more::Display;
use dice_futures::cancellation::CancellationContext;
use dice_futures::spawner::TokioSpawner;
use dupe::Dupe;
use futures::FutureExt;
use futures::poll;
use pagable::Pagable;
use pagable::PagablePanic;
use pagable::pagable_typetag;
use tokio::sync::Notify;

use crate::BranchId;
use crate::DiceKeyDyn;
use crate::DiceProjectionDyn;
use crate::DiceTransaction;
use crate::EqualityBehavior;
use crate::api::computations::DiceComputations;
use crate::api::cycles::DetectCycles;
use crate::api::injected::InjectedKey;
use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::api::projection::DiceProjectionComputations;
use crate::api::projection::ProjectionKey;
use crate::core::graph::revision::Revision;
use crate::dice::Dice;
use crate::epoch::cache::SharedCache;
use crate::epoch::cycles::DetectedCycle;
use crate::epoch::cycles::Schedule;
use crate::epoch::cycles::find_cycles;
use crate::epoch::cycles::mark_cycles;
use crate::epoch::cycles::strongly_connected_components;
use crate::epoch::task::dice::DiceTask;
use crate::epoch::task::promise::DicePromise;
use crate::epoch::task::spawn_dice_task;
use crate::key::CowDiceKeyHashed;
use crate::key::DiceKey;
use crate::key::DiceKeyErased;
use crate::key::ParentKey;
use crate::key_index::DiceKeyIndex;
use crate::value::DiceKeyValue;
use crate::value::DiceValidValue;
use crate::value::MaybeResidentComputedValue;
use crate::value::MaybeValidDiceValue;

// A detected cycle, and what it does to the tasks in it.

/// A key that displays as `key<n>`, for cycles built by hand.
#[derive(Allocative, Clone, Debug, Display, Eq, PartialEq, Hash, Pagable)]
#[display("key{}", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct N(u32);

#[async_trait]
impl Key for N {
    type Value = u32;

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        unimplemented!("test")
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

fn key(i: u32) -> DiceKey {
    DiceKey { index: i }
}

/// A cycle of the given keys, each with whether its waits fail, with the given waits among them.
fn cycle_of(members: &[(u32, bool)], edges: &[(u32, u32)]) -> Arc<DetectedCycle> {
    DetectedCycle::new(
        members
            .iter()
            .map(|&(i, fails)| (key(i), DiceKeyErased::key(N(i)), fails)),
        edges.iter().map(|&(a, b)| (key(a), key(b))),
    )
}

fn error_message(cycle: &DetectedCycle, parent: u32, awaited: u32) -> Option<String> {
    cycle
        .error_for(ParentKey::Some(key(parent)), key(awaited))
        .map(|e| e.to_string())
}

#[test]
fn errors_describe_a_shortest_cycle_through_the_failing_wait() {
    // 1 -> 2 -> 3 -> 1, plus a shortcut 2 -> 1.
    let cycle = cycle_of(
        &[(1, true), (2, true), (3, true)],
        &[(1, 2), (2, 3), (3, 1), (2, 1)],
    );
    assert_eq!(
        error_message(&cycle, 1, 2).unwrap(),
        "Dependency cycle detected (`->` means \"depends on\"):\n  key1 ->\n  key2 ->\n  key1"
    );
    assert_eq!(
        error_message(&cycle, 3, 1).unwrap(),
        "Dependency cycle detected (`->` means \"depends on\"):\n  key3 ->\n  key1 ->\n  key2 ->\n  key3"
    );
}

#[test]
fn a_key_waiting_on_itself_is_a_cycle() {
    let cycle = cycle_of(&[(7, true)], &[(7, 7)]);
    assert_eq!(
        error_message(&cycle, 7, 7).unwrap(),
        "Dependency cycle detected (`->` means \"depends on\"):\n  key7 ->\n  key7"
    );
}

#[test]
fn only_failing_members_get_errors() {
    let cycle = cycle_of(&[(1, true), (2, false)], &[(1, 2), (2, 1)]);
    assert!(error_message(&cycle, 1, 2).is_some());
    assert!(
        error_message(&cycle, 2, 1).is_none(),
        "member whose waits don't fail"
    );
    assert!(error_message(&cycle, 3, 1).is_none(), "not a member");
    assert!(
        cycle.error_for(ParentKey::None, key(1)).is_none(),
        "not a computation"
    );
}

/// A task for key 10 that finishes with the value 2 once `release` is notified.
fn spawn_releasable_task(release: &Arc<Notify>) -> (DiceTask, DicePromise<'static>) {
    let release = release.dupe();
    spawn_dice_task(key(10), &TokioSpawner, &(), move |handle| {
        async move {
            release.notified().await;
            handle.finished(Ok(MaybeResidentComputedValue::new_resident(
                MaybeValidDiceValue::valid(DiceValidValue::testing_new(DiceKeyValue::<N>::new(2))),
                InvalidationSources::NONE,
                Revision::FIRST,
            )));
        }
        .boxed()
    })
}

fn is_value(v: &MaybeResidentComputedValue, expected: u32) -> bool {
    v.testing_resident_value()
        .equality(&DiceValidValue::testing_new(DiceKeyValue::<N>::new(
            expected,
        )))
}

#[tokio::test]
async fn a_cycle_fails_the_waits_of_its_keys_and_serves_the_others() -> anyhow::Result<()> {
    let release = Arc::new(Notify::new());
    let (task, _initial_promise) = spawn_releasable_task(&release);
    let cycle = cycle_of(&[(1, true), (10, true)], &[(1, 10), (10, 1)]);

    let mut member = Box::pin(task.depended_on_by(ParentKey::Some(key(1))).unwrap());
    let mut outsider = Box::pin(task.depended_on_by(ParentKey::Some(key(2))).unwrap());
    assert!(poll!(&mut member).is_pending());
    assert!(poll!(&mut outsider).is_pending());

    task.as_ref().mark_in_cycle(&cycle);

    assert_matches!(poll!(&mut member), Poll::Ready(Err(e)) => {
        assert_eq!(
            e.to_string(),
            "Dependency cycle detected (`->` means \"depends on\"):\n  key1 ->\n  key10 ->\n  key1"
        );
    });
    // Asking again comes to the same.
    assert_matches!(
        task.depended_on_by(ParentKey::Some(key(1))).unwrap().await,
        Err(_)
    );
    assert!(poll!(&mut outsider).is_pending());

    // The task finishes as usual, and that is what the outsider gets.
    release.notify_one();
    assert!(is_value(outsider.await?, 2));

    Ok(())
}

#[tokio::test]
async fn a_waiting_key_of_a_cycle_gets_the_error_even_if_the_value_arrives_first()
-> anyhow::Result<()> {
    let release = Arc::new(Notify::new());
    let (task, _initial_promise) = spawn_releasable_task(&release);

    let mut member = Box::pin(task.depended_on_by(ParentKey::Some(key(1))).unwrap());
    assert!(poll!(&mut member).is_pending());

    task.as_ref()
        .mark_in_cycle(&cycle_of(&[(1, true), (10, true)], &[(1, 10), (10, 1)]));
    // Let the task finish before the member gets to look again.
    release.notify_one();
    task.depended_on_by(ParentKey::None).unwrap().await?;

    assert_matches!(poll!(&mut member), Poll::Ready(Err(_)));
    // A key of the cycle that only comes asking now gets the value like anyone else.
    assert!(is_value(
        task.depended_on_by(ParentKey::Some(key(1)))
            .unwrap()
            .await?,
        2
    ));

    Ok(())
}

// Finding the cycles among a version's tasks.

#[test]
fn components_are_strongly_connected() {
    // 0 <-> 1 -> 2 <-> 3, 4 on its own, 5 -> 5.
    let edges = [(0, 1), (1, 0), (1, 2), (2, 3), (3, 2), (5, 5)];
    let (components, component_of) = strongly_connected_components(6, &edges);
    for (c, nodes) in components.iter().enumerate() {
        for &n in nodes {
            assert_eq!(component_of[n as usize], c as u32);
        }
    }
    let mut components: Vec<Vec<u32>> = components
        .into_iter()
        .map(|mut c| {
            c.sort();
            c
        })
        .collect();
    components.sort();
    assert_eq!(components, vec![vec![0, 1], vec![2, 3], vec![4], vec![5]]);
}

#[test]
fn long_chains_do_not_overflow_the_stack() {
    let n = 1_000_000u32;
    let mut edges: Vec<(u32, u32)> = (0..n - 1).map(|i| (i, i + 1)).collect();
    edges.push((n - 1, 0));
    let (components, _) = strongly_connected_components(n as usize, &edges);
    assert_eq!(components.len(), 1);
}

/// A task for `key` that runs until the returned dependent is dropped.
fn running_task(cache: &SharedCache, key: DiceKey) -> (DiceTask, DicePromise<'static>) {
    let (task, promise) = spawn_dice_task(key, &TokioSpawner, &(), |handle| {
        async move {
            let _handle = handle;
            std::future::pending().await
        }
        .boxed()
    });
    cache.testing_insert_task(key, task.dupe());
    (task, promise)
}

/// Makes `waiting` wait on `task`: the wait counts once its promise has been polled.
async fn wait(task: &DiceTask, waiting: DiceKey) -> Pin<Box<DicePromise<'_>>> {
    let mut promise = Box::pin(task.depended_on_by(ParentKey::Some(waiting)).unwrap());
    assert!(poll!(promise.as_mut()).is_pending());
    promise
}

/// The keys of `cycle`, sorted.
fn members(cycle: &DetectedCycle) -> Vec<u32> {
    cycle.keys().map(|k| k.index).collect()
}

fn sorted(mut keys: Vec<u32>) -> Vec<u32> {
    keys.sort();
    keys
}

#[tokio::test]
async fn finds_the_cycles_nothing_can_resolve() {
    let key_index = DiceKeyIndex::default();
    let cache = SharedCache::new();
    let keys: Vec<DiceKey> = (0..5).map(|i| key_index.index_key(N(i))).collect();
    let tasks: Vec<_> = keys.iter().map(|&k| running_task(&cache, k)).collect();
    let task = |i: usize| &tasks[i].0;

    // 1 and 2 wait on each other, but 2 also waits on 3, which is running on its own:
    // whatever 3 ends up as may let 2 go, so this is not a cycle yet. 0 waiting on 1 from
    // the outside changes nothing either way.
    let _w12 = wait(task(2), keys[1]).await;
    let _w21 = wait(task(1), keys[2]).await;
    let w23 = wait(task(3), keys[2]).await;
    let _w01 = wait(task(1), keys[0]).await;
    assert!(find_cycles(&cache, &key_index).is_empty());

    drop(w23);
    let cycles = find_cycles(&cache, &key_index);
    assert_eq!(cycles.len(), 1);
    assert_eq!(
        members(&cycles[0]),
        sorted(vec![keys[1].index, keys[2].index])
    );

    // 4 waits on itself.
    let _w44 = wait(task(4), keys[4]).await;
    let cycles = find_cycles(&cache, &key_index);
    let mut found: Vec<Vec<u32>> = cycles.iter().map(|c| members(c)).collect();
    found.sort();
    let mut expected = vec![
        sorted(vec![keys[1].index, keys[2].index]),
        vec![keys[4].index],
    ];
    expected.sort();
    assert_eq!(found, expected);
}

#[tokio::test]
async fn marking_fails_the_waits_of_the_cycle() {
    let key_index = DiceKeyIndex::default();
    let cache = SharedCache::new();
    let keys: Vec<DiceKey> = (0..3).map(|i| key_index.index_key(N(i))).collect();
    let tasks: Vec<_> = keys.iter().map(|&k| running_task(&cache, k)).collect();
    let task = |i: usize| &tasks[i].0;

    let mut w12 = wait(task(2), keys[1]).await;
    let mut w21 = wait(task(1), keys[2]).await;
    let mut w01 = wait(task(1), keys[0]).await;

    let cycles = find_cycles(&cache, &key_index);
    mark_cycles(&cache, &cycles);

    assert_matches!(poll!(w12.as_mut()), Poll::Ready(Err(_)));
    assert_matches!(poll!(w21.as_mut()), Poll::Ready(Err(_)));
    assert!(poll!(w01.as_mut()).is_pending(), "not a key of the cycle");
}

/// A projection of `N`, for cycles through a projection key's task.
#[derive(Allocative, Debug, Display, Clone, Dupe, Eq, PartialEq, Hash, Pagable)]
#[pagable_typetag(DiceProjectionDyn)]
struct P;

impl ProjectionKey for P {
    type DeriveFromKey = N;
    type Value = ();

    fn compute(
        &self,
        _derive_from: &<<Self as ProjectionKey>::DeriveFromKey as Key>::Value,
        _ctx: &DiceProjectionComputations,
    ) -> Self::Value {
        unimplemented!("test")
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_, _| true)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

/// A projection key's task only exists for dep checks, and only ever waits for its base (see
/// the projection arm of `TransactionData::evaluate`). It is a node of the wait graph like any
/// other, but the detector does not fail its wait: that wait is served the base's value instead,
/// once failing the regular keys' waits has broken the cycle.
#[tokio::test]
async fn a_projection_key_in_a_cycle_keeps_waiting_for_its_base() {
    let key_index = DiceKeyIndex::default();
    let cache = SharedCache::new();
    let q = key_index.index_key(N(0));
    let base = key_index.index_key(N(1));
    let proj = key_index.index(CowDiceKeyHashed::proj_ref(base, &P));
    let (q_task, _q_promise) = running_task(&cache, q);
    let (base_task, _base_promise) = running_task(&cache, base);
    let (proj_task, _proj_promise) = running_task(&cache, proj);

    // q is checking a dep on the projection, whose task waits for the base, whose compute
    // requested q.
    let mut q_on_proj = wait(&proj_task, q).await;
    let mut proj_on_base = wait(&base_task, proj).await;
    let mut base_on_q = wait(&q_task, base).await;

    let cycles = find_cycles(&cache, &key_index);
    assert_eq!(cycles.len(), 1);
    assert_eq!(
        members(&cycles[0]),
        sorted(vec![q.index, base.index, proj.index])
    );
    mark_cycles(&cache, &cycles);

    assert_matches!(poll!(q_on_proj.as_mut()), Poll::Ready(Err(_)));
    assert_matches!(poll!(base_on_q.as_mut()), Poll::Ready(Err(_)));
    assert!(poll!(proj_on_base.as_mut()).is_pending());
}

// End to end: computations that wait on each other get cycle errors, and only once nothing else
// can get them going again.

/// The dependency graph the `Node`s of a test compute along, kept in the global data.
#[derive(Default)]
struct Graph {
    deps: HashMap<&'static str, Vec<&'static str>>,
    /// Deps a node has in addition once `Switch` is on; a node listed here reads `Switch`.
    switched_deps: HashMap<&'static str, Vec<&'static str>>,
    /// Nodes that use `Projected` of the given node rather than the node itself.
    projected_deps: HashMap<&'static str, &'static str>,
    /// Nodes that wait for the notification before finishing.
    blocked: HashMap<&'static str, Arc<Notify>>,
}

impl Graph {
    fn with_deps(deps: &[(&'static str, &[&'static str])]) -> Self {
        Graph {
            deps: deps.iter().map(|(k, v)| (*k, v.to_vec())).collect(),
            ..Default::default()
        }
    }
}

#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("switch")]
#[pagable_typetag(DiceKeyDyn)]
struct Switch;

#[async_trait]
impl InjectedKey for Switch {
    type Value = bool;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

/// A node of the graph, which computes its deps in parallel and whose value is the first error
/// among them, if any. Errors are transient, as they are in buck2.
#[derive(
    Clone,
    Dupe,
    Debug,
    Display,
    Eq,
    Hash,
    PartialEq,
    Allocative,
    PagablePanic
)]
#[display("{}", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct Node(&'static str);

#[async_trait]
impl Key for Node {
    type Value = Result<(), String>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        let graph = ctx.global_data().get::<Graph>().unwrap();
        let mut deps = graph.deps.get(self.0).cloned().unwrap_or_default();
        if let Some(switched) = graph.switched_deps.get(self.0)
            && *ctx.compute(&Switch).await.unwrap()
        {
            deps.extend(switched);
        }
        let results = ctx
            .compute_join(deps, async |ctx, dep| {
                let result = match graph.projected_deps.get(dep) {
                    None => ctx.compute(&Node(dep)).await.cloned(),
                    Some(base) => match ctx.compute_opaque(&Node(base)).await {
                        Ok(base) => ctx.projection(&base, &Projected).map(|v| (*v).clone()),
                        Err(e) => Err(e),
                    },
                };
                result.map_err(|e| e.to_string())?
            })
            .await;
        for result in results {
            result?;
        }
        if let Some(notify) = graph.blocked.get(self.0) {
            notify.notified().await;
        }
        Ok(())
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn validity(x: &Self::Value) -> bool {
        x.is_ok()
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("projected")]
#[pagable_typetag(DiceProjectionDyn)]
struct Projected;

impl ProjectionKey for Projected {
    type DeriveFromKey = Node;
    type Value = Arc<Result<(), String>>;

    fn compute(
        &self,
        derive_from: &<<Self as ProjectionKey>::DeriveFromKey as Key>::Value,
        _ctx: &DiceProjectionComputations,
    ) -> Self::Value {
        Arc::new(derive_from.clone())
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn validity(x: &Self::Value) -> bool {
        x.is_ok()
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

/// A `Dice` computing `graph`, and the branch its transactions run on.
async fn dice(graph: Graph) -> (Arc<Dice>, BranchId) {
    let mut builder = Dice::builder();
    builder.set(graph);
    let dice = builder.build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    (dice, root)
}

async fn transaction(dice: &Arc<Dice>, root: BranchId, switch: bool) -> DiceTransaction {
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Switch, switch)]).unwrap();
    updater.commit().await
}

fn cycle_message(keys: &[&str]) -> String {
    let mut s = "Dependency cycle detected (`->` means \"depends on\"):".to_owned();
    for k in keys {
        s.push_str(&format!("\n  {k} ->"));
    }
    s.push_str(&format!("\n  {}", keys[0]));
    s
}

/// Asserts that `value` reports the cycle `keys`, starting from any of them.
#[track_caller]
fn assert_cycle(value: &Result<(), String>, keys: &[&str]) {
    let rotations: Vec<String> = (0..keys.len())
        .map(|i| {
            let mut rotated = keys.to_vec();
            rotated.rotate_left(i);
            cycle_message(&rotated)
        })
        .collect();
    match value {
        Err(e) if rotations.contains(e) => {}
        other => panic!("expected the cycle {keys:?}, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn keys_waiting_on_each_other_are_failed_with_the_cycle() {
    let (dice, root) = dice(Graph::with_deps(&[("a", &["b"]), ("b", &["a"])])).await;
    let ctx = transaction(&dice, root, false).await;

    let (a, b) = futures::join!(ctx.compute(&Node("a")), ctx.compute(&Node("b")));
    // Each sees the cycle from its own side.
    assert_eq!(a.unwrap(), &Err(cycle_message(&["a", "b"])));
    assert_eq!(b.unwrap(), &Err(cycle_message(&["b", "a"])));
}

#[test]
fn the_schedule_backs_off_while_stalled_and_resets_on_progress() {
    let second = Duration::from_secs(1);
    let mut schedule = Schedule::new(second);
    assert_eq!(schedule.wait(), second);
    let mut waits = Vec::new();
    for _ in 0..6 {
        schedule.scanned(false);
        waits.push(schedule.wait().as_secs());
    }
    assert_eq!(waits, vec![2, 4, 8, 16, 16, 16]);
    schedule.progressed();
    assert_eq!(schedule.wait(), second);
    schedule.scanned(false);
    schedule.scanned(true);
    assert_eq!(
        schedule.wait(),
        second,
        "a found cycle gets things moving again"
    );
}

#[tokio::test(start_paused = true)]
async fn a_key_waiting_on_itself_is_failed() {
    let (dice, root) = dice(Graph::with_deps(&[("a", &["a"])])).await;
    let ctx = transaction(&dice, root, false).await;

    let a = ctx.compute(&Node("a")).await.unwrap();
    assert_eq!(a, &Err(cycle_message(&["a"])));
}

#[tokio::test(start_paused = true)]
async fn a_longer_cycle_is_reported_in_dependency_order() {
    let (dice, root) = dice(Graph::with_deps(&[
        ("a", &["b"]),
        ("b", &["c"]),
        ("c", &["a"]),
    ]))
    .await;
    let ctx = transaction(&dice, root, false).await;

    let a = ctx.compute(&Node("a")).await.unwrap();
    assert_eq!(a, &Err(cycle_message(&["a", "b", "c"])));
}

#[tokio::test(start_paused = true)]
async fn keys_outside_the_cycle_get_what_the_cycle_computes() {
    let (dice, root) = dice(Graph::with_deps(&[
        ("top", &["a"]),
        ("a", &["b"]),
        ("b", &["a"]),
    ]))
    .await;
    let ctx = transaction(&dice, root, false).await;

    // `top` is not failed for a cycle; it gets `a`'s value, which carries `a`'s error.
    let top = ctx.compute(&Node("top")).await.unwrap();
    assert_eq!(top, &Err(cycle_message(&["a", "b"])));
}

#[tokio::test(start_paused = true)]
async fn a_cycle_with_a_way_out_waits_for_the_way_out() {
    let release = Arc::new(Notify::new());
    let mut graph = Graph::with_deps(&[("a", &["b"]), ("b", &["a", "c"])]);
    graph.blocked.insert("c", release.dupe());
    let (dice, root) = dice(graph).await;
    let ctx = transaction(&dice, root, false).await;

    let a = tokio::spawn({
        let ctx = ctx.dupe();
        async move { ctx.compute(&Node("a")).await.unwrap().clone() }
    });
    // `b` waits on `c` too, which is working: whatever it comes back with may let `b` go, so
    // this is not a cycle yet, however long it takes.
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(!a.is_finished());

    // Once `c` is done, `a` and `b` are only waiting on each other.
    release.notify_one();
    assert_eq!(a.await.unwrap(), Err(cycle_message(&["a", "b"])));
}

#[tokio::test(start_paused = true)]
async fn a_cycle_that_forms_during_a_dependency_check_is_found() {
    let mut graph = Graph::with_deps(&[("a", &["b"])]);
    graph.switched_deps.insert("b", vec!["a"]);
    let (dice, root) = dice(graph).await;

    // At first, `b` only reads the switch.
    {
        let ctx = transaction(&dice, root, false).await;
        assert_eq!(ctx.compute(&Node("a")).await.unwrap(), &Ok(()));
    }

    // Flipping the switch makes `b` depend on `a`. `a` finds out by checking its dependency on
    // `b`, which recomputes and asks for `a`. The failed check makes `a` recompute and ask for
    // `b` again, which gets it either the cycle error or, if `b` finished with its own cycle
    // error first, that; either way `a` reports the cycle.
    let ctx = transaction(&dice, root, true).await;
    assert_cycle(ctx.compute(&Node("a")).await.unwrap(), &["a", "b"]);
}

/// The dep check of a key that read a projection waits for the projection's base itself (see
/// `check_dependency`), so a base that comes to depend on the key makes a cycle of the two.
#[tokio::test(start_paused = true)]
async fn a_cycle_through_a_projection_is_found() {
    let mut graph = Graph::with_deps(&[("a", &["b"])]);
    graph.projected_deps.insert("b", "base");
    graph.switched_deps.insert("base", vec!["a"]);
    let (dice, root) = dice(graph).await;

    // At first, `a` reads `base` through the projection, and `base` only reads the switch.
    {
        let ctx = transaction(&dice, root, false).await;
        assert_eq!(ctx.compute(&Node("a")).await.unwrap(), &Ok(()));
    }

    // Flipping the switch makes `base` depend on `a`. `a` checks its dependency on the
    // projection, for which it waits for `base`, which recomputes and asks for `a`. `a`'s failed
    // check makes it recompute and ask for `base` directly, which gets it either its own cycle
    // error, or `base`'s if that finished first.
    let ctx = transaction(&dice, root, true).await;
    assert_cycle(ctx.compute(&Node("a")).await.unwrap(), &["a", "base"]);
}

#[tokio::test(start_paused = true)]
async fn nothing_is_reported_without_a_cycle() {
    let release = Arc::new(Notify::new());
    let mut graph = Graph::with_deps(&[("a", &["b"]), ("b", &["c"])]);
    graph.blocked.insert("c", release.dupe());
    let (dice, root) = dice(graph).await;
    let ctx = transaction(&dice, root, false).await;

    let a = tokio::spawn({
        let ctx = ctx.dupe();
        async move { ctx.compute(&Node("a")).await.unwrap().clone() }
    });
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(!a.is_finished());
    release.notify_one();
    assert_eq!(a.await.unwrap(), Ok(()));
}

#[tokio::test(start_paused = true)]
async fn detection_can_be_disabled() {
    let mut builder = Dice::builder();
    builder.set(Graph::with_deps(&[("a", &["a"])]));
    let dice = builder.build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let ctx = transaction(&dice, root, false).await;

    let a = tokio::spawn({
        let ctx = ctx.dupe();
        async move { ctx.compute(&Node("a")).await.unwrap().clone() }
    });
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(!a.is_finished());
    a.abort();
}
