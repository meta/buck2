/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Cycles among the tasks of one version.
//!
//! A computation that, through its dependencies, waits on itself never finishes; neither does
//! anything waiting on it. DICE deals with that by failing the waits that make up the cycle: once
//! a cycle has been found, each task in it is marked with it (`DiceTaskRef::mark_in_cycle`), and
//! from then on a dependent that is itself a key of the cycle gets a cycle error from its
//! `DicePromise` on any of the cycle's tasks instead of a value. The computations involved see
//! that error from `DiceComputations::compute` and finish with whatever they make of it, and
//! their tasks complete like any other, so dependents from outside the cycle are served a value
//! as usual.
//!
//! Cycles are found by looking at the "waits on" graph of the pending tasks: `a` waits on `b` if
//! `a`'s computation has polled a `DicePromise` for `b` and found it pending. That graph is
//! discovered backwards, from each task to the keys registered as waiting on it
//! (`DiceTaskRef::for_each_waiting_key`), because that is the direction a task knows about.
//!
//! Not every cycle in it is reported, only the strongly connected components that no edge
//! leaves. A component with a way out may yet resolve itself: a key in it that is also waiting
//! on something outside may, once that comes back, stop waiting on the component, as it would
//! when racing its dependencies against each other. A component without a way out can only
//! be waiting on things that, in the end, wait on it.
//!
//! That is the extent of what can be told from here, though. A computation that races a
//! dependency against something DICE knows nothing about, a timer say, also looks like it is
//! stuck on the dependency, and so can be found in a cycle it would have gotten out of on its
//! own.
//!
//! Neither the scan nor the marking that follows it holds the tasks still. A task found in a
//! cycle may have finished, been cancelled, or been restarted as a new generation by the time
//! it is marked, and the scan itself may see one task's waits from before such a restart and
//! another's from after. Within one version, which is all a scan ever looks at, none of that
//! matters as long as computations are deterministic: a restarted computation requests what
//! its predecessor requested and ends up in the same cycle, and a cycle pieced together from
//! generations that never ran side by side is one the version would have formed anyway. A
//! computation that requests different keys on different runs, on the other hand, can be failed
//! for a cycle its rerun would not have been in. DICE otherwise holds up under such
//! computations, so this is a departure, and a deliberate one: ruling it out would mean pausing
//! the whole version to confirm a cycle before failing it, a lot of machinery on behalf of
//! computations that are broken in subtler ways already.
//!
//! The detection runs in a task of its own per version (`ensure_cycle_detector_running`),
//! started along with the version's first task and living as long as the version's cache. Every
//! so often it looks at whether any task of the version has finished since it last looked, and
//! only if none has does it scan the cache for cycles. The scan touches every pending task, so
//! how often it runs matters: the first look comes soon after a version stops making progress,
//! which is what a cycle in a small computation looks like, and the looks get rarer while the
//! version stays stalled without one, as a version waiting on a single slow computation does
//! (`Schedule`). A cycle that forms while other work is still finishing is found once that work
//! is done, or has run into the cycle itself.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::Weak;
use std::time::Duration;

use dice_error::cycles::CycleError;
use dice_error::cycles::RequestedKey;
use dupe::Dupe;
use futures::FutureExt;
use parking_lot::Mutex;

use crate::DynKey;
use crate::HashMap;
use crate::dice::Dice;
use crate::epoch::cache::SharedCache;
use crate::epoch::cache::SharedCacheLookup;
use crate::epoch::cache::WeakSharedCache;
use crate::epoch::evaluator::TransactionData;
use crate::key::DiceKey;
use crate::key::DiceKeyErased;
use crate::key::ParentKey;
use crate::key_index::DiceKeyIndex;

/// Starts the cycle detector for the version `eval` computes at, unless it is running already or
/// cycle detection is off.
///
/// Called for every task spawned in the version, which is also what provides the `Spawner` to
/// run the detector on; the first of them is the one that starts it.
pub(crate) fn ensure_cycle_detector_running(eval: &TransactionData) {
    let Some(interval) = eval.dice.cycle_check_interval else {
        return;
    };
    let cache = eval.version_state.cache();
    if !cache.claim_cycle_detector() {
        return;
    }
    let detector = run_detector(
        cache.downgrade(),
        Arc::downgrade(&eval.dice),
        interval,
        cache.completed_tasks(),
    );
    // Nothing waits for the detector; it ends on its own once the cache is gone.
    drop(
        eval.user_data.spawner.spawn(
            &eval.user_data,
            async move {
                detector.await;
                Box::new(()) as _
            }
            .boxed(),
        ),
    );
}

/// Checks `cache` for cycles on the `Schedule` with the given base, while it and `dice` are
/// alive, skipping the checks after which tasks have completed; `completed` is their count as of
/// now. Both are held weakly so that the detector does not keep either around: the workers
/// running in the cache keep both alive for as long as there is anything to detect cycles among.
async fn run_detector(
    cache: WeakSharedCache,
    dice: Weak<Dice>,
    base_interval: Duration,
    mut completed: u64,
) {
    let mut schedule = Schedule::new(base_interval);
    loop {
        tokio::time::sleep(schedule.wait()).await;
        let (Some(cache), Some(dice)) = (cache.upgrade(), dice.upgrade()) else {
            return;
        };
        let now = cache.completed_tasks();
        if now != completed {
            completed = now;
            schedule.progressed();
            continue;
        }
        let cycles = find_cycles(&cache, &dice.key_index);
        schedule.scanned(!cycles.is_empty());
        mark_cycles(&cache, &cycles);
    }
}

/// When the detector next looks at a version: soon after it last made progress, and less and
/// less often while it stays stalled without a cycle.
pub(super) struct Schedule {
    base: Duration,
    next: Duration,
}

impl Schedule {
    /// How much longer than the base the waits get, at most.
    const BACKOFF_CAP: u32 = 16;

    pub(super) fn new(base: Duration) -> Self {
        Schedule { base, next: base }
    }

    /// How long to wait before looking again.
    pub(super) fn wait(&self) -> Duration {
        self.next
    }

    /// Tasks completed since the last look.
    pub(super) fn progressed(&mut self) {
        self.next = self.base;
    }

    /// The version was scanned, finding cycles or not. Finding none means it is waiting on
    /// something that is still working, which may take a while.
    pub(super) fn scanned(&mut self, found_cycles: bool) {
        self.next = if found_cycles {
            self.base
        } else {
            (self.next * 2).min(self.base * Self::BACKOFF_CAP)
        };
    }
}

/// The cycles among the pending tasks of `cache` that nothing is going to resolve, see the
/// module docs for what that means.
pub(crate) fn find_cycles(
    cache: &SharedCache,
    key_index: &DiceKeyIndex,
) -> Vec<Arc<DetectedCycle>> {
    let mut graph = WaitGraph::default();
    cache.for_each_pending_task(|task| {
        let awaited = graph.node(task.internal.key);
        task.for_each_waiting_key(|waiting| {
            let waiting = graph.node(waiting);
            graph.edges.push((waiting, awaited));
        });
    });

    let (components, component_of) = strongly_connected_components(graph.keys.len(), &graph.edges);
    let mut has_exit = vec![false; components.len()];
    let mut has_self_loop = vec![false; components.len()];
    for &(u, v) in &graph.edges {
        let (cu, cv) = (component_of[u as usize], component_of[v as usize]);
        if cu != cv {
            has_exit[cu as usize] = true;
        } else if u == v {
            has_self_loop[cu as usize] = true;
        }
    }

    components
        .iter()
        .enumerate()
        .filter(|&(c, nodes)| !has_exit[c] && (nodes.len() > 1 || has_self_loop[c]))
        .map(|(c, nodes)| {
            let c = c as u32;
            DetectedCycle::new(
                nodes.iter().map(|&n| {
                    let key = graph.keys[n as usize];
                    let erased = key_index.get(key).dupe();
                    let fails = matches!(erased, DiceKeyErased::Key(_));
                    (key, erased, fails)
                }),
                graph
                    .edges
                    .iter()
                    .filter(|&&(u, v)| {
                        component_of[u as usize] == c && component_of[v as usize] == c
                    })
                    .map(|&(u, v)| (graph.keys[u as usize], graph.keys[v as usize])),
            )
        })
        .collect()
}

/// Marks the tasks of `cycles` with them (`DiceTaskRef::mark_in_cycle`), which fails the waits
/// that make them up. The tasks may have moved on since the cycles were found; the module docs
/// say why that is fine.
pub(crate) fn mark_cycles(cache: &SharedCache, cycles: &[Arc<DetectedCycle>]) {
    for cycle in cycles {
        for key in cycle.keys() {
            match cache.get(key) {
                SharedCacheLookup::InProgress(task) => task.mark_in_cycle(cycle),
                // A task that finished in the meantime has nothing left to fail, unless the
                // waits are on the recomputation of its unreadable value.
                SharedCacheLookup::Finished(value) => {
                    if let Some(recovery) = value.recovery() {
                        recovery.task.as_ref().mark_in_cycle(cycle);
                    }
                }
                SharedCacheLookup::Vacant => {}
            }
        }
    }
}

/// The "waits on" graph: an edge `(a, b)` for `a` waiting on `b`, over nodes numbered in the
/// order in which their keys came up.
#[derive(Default)]
struct WaitGraph {
    keys: Vec<DiceKey>,
    nodes: HashMap<DiceKey, u32>,
    edges: Vec<(u32, u32)>,
}

impl WaitGraph {
    fn node(&mut self, key: DiceKey) -> u32 {
        *self.nodes.entry(key).or_insert_with(|| {
            self.keys.push(key);
            (self.keys.len() - 1) as u32
        })
    }
}

/// Tarjan's algorithm: the strongly connected components of the graph on nodes `0..n` with
/// `edges`, each as the nodes in it, and the component of each node.
///
/// Iterative rather than recursive since the graph may contain long chains, and the stacks of the
/// threads this runs on are small.
pub(super) fn strongly_connected_components(
    n: usize,
    edges: &[(u32, u32)],
) -> (Vec<Vec<u32>>, Vec<u32>) {
    // Adjacency in CSR form.
    let mut offsets = vec![0u32; n + 1];
    for &(u, _) in edges {
        offsets[u as usize + 1] += 1;
    }
    for i in 0..n {
        offsets[i + 1] += offsets[i];
    }
    let mut adjacent = vec![0u32; edges.len()];
    let mut fill = offsets.clone();
    for &(u, v) in edges {
        adjacent[fill[u as usize] as usize] = v;
        fill[u as usize] += 1;
    }

    const UNVISITED: u32 = u32::MAX;
    struct State {
        index: Vec<u32>,
        lowlink: Vec<u32>,
        on_stack: Vec<bool>,
        stack: Vec<u32>,
        next_index: u32,
        /// The DFS stack: a node and the position of the next edge of it to follow.
        dfs: Vec<(u32, u32)>,
    }
    impl State {
        fn visit(&mut self, v: u32, first_edge: u32) {
            self.index[v as usize] = self.next_index;
            self.lowlink[v as usize] = self.next_index;
            self.next_index += 1;
            self.stack.push(v);
            self.on_stack[v as usize] = true;
            self.dfs.push((v, first_edge));
        }
    }
    let mut st = State {
        index: vec![UNVISITED; n],
        lowlink: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next_index: 0,
        dfs: Vec::new(),
    };
    let mut component_of = vec![UNVISITED; n];
    let mut components = Vec::new();

    for root in 0..n as u32 {
        if st.index[root as usize] != UNVISITED {
            continue;
        }
        st.visit(root, offsets[root as usize]);
        while let Some(&(v, next_edge)) = st.dfs.last() {
            if next_edge < offsets[v as usize + 1] {
                st.dfs.last_mut().expect("just peeked").1 += 1;
                let w = adjacent[next_edge as usize];
                if st.index[w as usize] == UNVISITED {
                    st.visit(w, offsets[w as usize]);
                } else if st.on_stack[w as usize] {
                    st.lowlink[v as usize] = st.lowlink[v as usize].min(st.index[w as usize]);
                }
                continue;
            }
            st.dfs.pop();
            if let Some(&(parent, _)) = st.dfs.last() {
                st.lowlink[parent as usize] =
                    st.lowlink[parent as usize].min(st.lowlink[v as usize]);
            }
            if st.lowlink[v as usize] == st.index[v as usize] {
                let c = components.len() as u32;
                let mut component = Vec::new();
                loop {
                    let w = st.stack.pop().expect("v is on the stack");
                    st.on_stack[w as usize] = false;
                    component_of[w as usize] = c;
                    component.push(w);
                    if w == v {
                        break;
                    }
                }
                components.push(component);
            }
        }
    }
    (components, component_of)
}

/// A dependency cycle found among the pending tasks of a version: a set of keys each of which
/// waits, directly or through the others, on itself.
pub(crate) struct DetectedCycle {
    /// Sorted by key.
    members: Box<[Member]>,
    /// Who waits on whom within the cycle, as indices into `members`.
    edges: Box<[(u32, u32)]>,
}

struct Member {
    key: DiceKey,
    erased: DiceKeyErased,
    /// Whether this member's waits on the other members fail.
    ///
    /// They don't for a projection key: its task only ever waits for its base, on behalf of a
    /// dep check that could do nothing with an error (see the projection arm of
    /// `TransactionData::evaluate`), so it keeps waiting and projects the base's eventual value
    /// instead. That leaves every cycle with a failing wait all the same, since a base is never
    /// a projection key.
    fails: bool,
}

impl DetectedCycle {
    /// `members` gives each key of the cycle with whether its waits fail; `edges` the waits among
    /// them, as `(waiting key, awaited key)`.
    pub(crate) fn new(
        members: impl IntoIterator<Item = (DiceKey, DiceKeyErased, bool)>,
        edges: impl IntoIterator<Item = (DiceKey, DiceKey)>,
    ) -> Arc<Self> {
        let mut members: Box<[Member]> = members
            .into_iter()
            .map(|(key, erased, fails)| Member { key, erased, fails })
            .collect();
        members.sort_by_key(|m| m.key);
        let index_of = |k: DiceKey| -> u32 {
            members
                .binary_search_by_key(&k, |m| m.key)
                .expect("edges connect members") as u32
        };
        let edges = edges
            .into_iter()
            .map(|(from, to)| (index_of(from), index_of(to)))
            .collect();
        Arc::new(DetectedCycle { members, edges })
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = DiceKey> + '_ {
        self.members.iter().map(|m| m.key)
    }

    /// The error with which `parent`'s wait on `awaited`, a key of this cycle, fails, or `None`
    /// if that wait is to go on: `parent` is not a key of the cycle, or one whose waits don't
    /// fail.
    pub(crate) fn error_for(&self, parent: ParentKey, awaited: DiceKey) -> Option<CycleError> {
        let ParentKey::Some(parent) = parent else {
            return None;
        };
        let parent = self.index_of(parent)?;
        if !self.members[parent].fails {
            return None;
        }
        let awaited = self
            .index_of(awaited)
            .expect("a task is only marked with cycles it is a key of");
        Some(CycleError {
            keys: self
                .shortest_cycle(parent, awaited)
                .into_iter()
                .map(|i| {
                    Arc::new(DynKey {
                        erased: self.members[i].erased.dupe(),
                    }) as Arc<dyn RequestedKey>
                })
                .collect(),
        })
    }

    fn index_of(&self, k: DiceKey) -> Option<usize> {
        self.members.binary_search_by_key(&k, |m| m.key).ok()
    }

    /// The members along a shortest cycle that starts with `from` waiting on `to`; the last one
    /// waits on `from` again. Every pair of members has one, the cycle being strongly connected.
    fn shortest_cycle(&self, from: usize, to: usize) -> Vec<usize> {
        if from == to {
            return vec![from];
        }
        // A BFS from `to` back to `from`, remembering how each member was reached.
        let mut reached_from = vec![None; self.members.len()];
        let mut queue = VecDeque::from([to]);
        reached_from[to] = Some(to);
        while let Some(cur) = queue.pop_front() {
            if cur == from {
                break;
            }
            for &(u, v) in self.edges.iter() {
                let (u, v) = (u as usize, v as usize);
                if u == cur && reached_from[v].is_none() {
                    reached_from[v] = Some(cur);
                    queue.push_back(v);
                }
            }
        }
        assert!(
            reached_from[from].is_some(),
            "a cycle's members are strongly connected"
        );
        let mut path = vec![from];
        let mut cur = from;
        while cur != to {
            cur = reached_from[cur].expect("walking back along the BFS");
            path.push(cur);
        }
        path.reverse();
        // `path` now runs `to ... from`; the cycle starts with `from` waiting on `to`.
        path.rotate_right(1);
        path
    }
}

/// The cycles a task has been found to be a key of, see `DiceTaskRef::mark_in_cycle`.
///
/// Usually none, and normally at most one; more only for a key whose compute swallows the cycle
/// error, goes on waiting, and ends up in another cycle.
#[derive(Default)]
pub(crate) struct TaskCycles(OnceLock<Box<Mutex<Vec<Arc<DetectedCycle>>>>>);

impl TaskCycles {
    pub(crate) fn add(&self, cycle: &Arc<DetectedCycle>) {
        self.0
            .get_or_init(Default::default)
            .lock()
            .push(cycle.dupe());
    }

    /// The error with which `waiting`'s wait on `awaited`, the task's key, fails, if one of the
    /// cycles fails it; see `DetectedCycle::error_for`.
    pub(crate) fn error_for(&self, waiting: ParentKey, awaited: DiceKey) -> Option<CycleError> {
        self.0
            .get()?
            .lock()
            .iter()
            .find_map(|cycle| cycle.error_for(waiting, awaited))
    }
}
