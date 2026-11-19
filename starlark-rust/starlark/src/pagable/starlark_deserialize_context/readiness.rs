/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Readiness of cyclic construction references, separate from deadlock detection.
//! Only slots that consume an unfinished reference enter this graph. Its groups
//! are strongly connected components; completion propagates along their DAG,
//! without rescanning completed values or unrelated heaps.

use std::cell::Cell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use dupe::Dupe;

use super::HeapDeserializationState;
use super::HeapValueId;
use super::SlotReadiness;
use super::SlotState;
use super::StarlarkDeserScope;
use super::StarlarkDeserWaitGraph;
use super::conflicting_heap_binding;
use crate::pagable::heap_ref_id::HeapRefId;
use crate::values::layout::heap::sealed::FrozenHeapArc;
use crate::values::layout::heap::sealed::HeapAllocationOrigin;

thread_local! {
    static ACTIVE: Cell<Option<ActiveClaim>> = const { Cell::new(None) };
}

/// The synchronous construction stack survives hops through pagable Arc bodies.
#[derive(Clone, Copy)]
pub(super) struct ActiveClaim {
    graph: usize,
    value: HeapValueId,
    heap_id: HeapRefId,
}

pub(super) struct ActiveClaimGuard<'graph> {
    previous: Option<ActiveClaim>,
    graph: PhantomData<&'graph StarlarkDeserWaitGraph>,
    // Restoring a thread-local stack on another thread is invalid.
    thread_bound: PhantomData<*mut ()>,
}

impl ActiveClaim {
    pub(super) fn enter(
        graph: &StarlarkDeserWaitGraph,
        value: HeapValueId,
        heap_id: HeapRefId,
    ) -> ActiveClaimGuard<'_> {
        Self::replace(Some(Self {
            graph: graph as *const _ as usize,
            value,
            heap_id,
        }))
    }

    pub(super) fn root() -> ActiveClaimGuard<'static> {
        Self::replace(None)
    }

    fn replace<'graph>(active: Option<Self>) -> ActiveClaimGuard<'graph> {
        ActiveClaimGuard {
            previous: ACTIVE.with(|slot| slot.replace(active)),
            graph: PhantomData,
            thread_bound: PhantomData,
        }
    }

    pub(super) fn current(graph: &StarlarkDeserWaitGraph) -> Option<Self> {
        ACTIVE.with(|slot| {
            slot.get()
                .filter(|active| active.graph == graph as *const _ as usize)
        })
    }

    pub(super) fn depend_on(
        &self,
        scope: &StarlarkDeserScope,
        value: HeapValueId,
        heap: &FrozenHeapArc,
    ) -> crate::Result<()> {
        // The active constructor's call stack retains the heap. Only a pending
        // dependency needs an additional owner in the readiness graph.
        let parent_heap = scope
            .get_heap(&self.heap_id)
            .expect("an active constructor retains its registered heap");
        let bound_heap_ptr = parent_heap
            .downgrade()
            .expect("an active heap has an allocation")
            .heap_ptr();
        if bound_heap_ptr != self.value.heap_ptr {
            return Err(conflicting_heap_binding(
                self.heap_id,
                &parent_heap,
                bound_heap_ptr,
                Some(HeapAllocationOrigin::Deserialized),
                self.value.heap_ptr,
            )
            .into());
        }
        scope
            .wait_graph
            .readiness
            .lock()
            .expect("readiness lock poisoned")
            .depend_on(self.value, &parent_heap, value, heap)
    }
}

impl Drop for ActiveClaimGuard<'_> {
    fn drop(&mut self) {
        ACTIVE.with(|slot| slot.replace(self.previous.take()));
    }
}

struct PendingValue {
    // Retains the exact allocation named by the key until readiness or failure.
    heap: FrozenHeapArc,
    group: HeapValueId,
}

#[derive(Default)]
struct Group {
    members: Vec<HeapValueId>,
    unfinished_writers: usize,
    dependencies: HashSet<HeapValueId>,
    dependents: HashSet<HeapValueId>,
}

enum Edges {
    Dependencies,
    Dependents,
}

struct ReadyBatch {
    retired_heap_index: usize,
    slots: Vec<u32>,
}

#[derive(Default)]
pub(super) struct ReadinessGraph {
    values: HashMap<HeapValueId, PendingValue>,
    groups: HashMap<HeapValueId, Group>,
}

impl ReadinessGraph {
    pub(super) fn pending_writers(&self, value: HeapValueId) -> Vec<HeapValueId> {
        let Some(value) = self.values.get(&value) else {
            return Vec::new();
        };
        self.reachable(value.group, Edges::Dependencies)
            .into_iter()
            .flat_map(|id| self.groups[&id].members.iter().copied())
            .filter(|id| {
                let state = self.values[id].heap.deser_state().expect("restored heap");
                matches!(
                    state.metadata.get().expect("claimed metadata").init_states
                        [id.value_index as usize]
                        .load(Ordering::Acquire),
                    SlotState::InProgress(_)
                )
            })
            .collect()
    }

    fn enroll(&mut self, value: HeapValueId, heap: &FrozenHeapArc) -> crate::Result<bool> {
        if self.values.contains_key(&value) {
            return Ok(true);
        }
        let state = heap
            .deser_state()
            .expect("pending value has deserialization state");
        let slot_state = state
            .publication()
            .track_readiness(value.value_index as usize);
        match slot_state {
            SlotState::Ready(_) => return Ok(false),
            SlotState::Failed => {
                return Err(state
                    .partial_deserialization_error(value.value_index)
                    .into());
            }
            SlotState::InProgress(_) | SlotState::Constructed(_) => {}
            SlotState::NotStarted => unreachable!("only claimed references can be dependencies"),
        }
        self.groups.insert(
            value,
            Group {
                members: vec![value],
                unfinished_writers: 1,
                ..Group::default()
            },
        );
        self.values.insert(
            value,
            PendingValue {
                heap: heap.dupe(),
                group: value,
            },
        );
        Ok(true)
    }

    fn depend_on(
        &mut self,
        parent: HeapValueId,
        parent_heap: &FrozenHeapArc,
        value: HeapValueId,
        heap: &FrozenHeapArc,
    ) -> crate::Result<()> {
        if !self.enroll(value, heap)? {
            return Ok(());
        }
        if !self.enroll(parent, parent_heap)? {
            unreachable!("the current constructor cannot have completed");
        }
        let from = self.values[&parent].group;
        let to = self.values[&value].group;
        if from == to
            || !self
                .groups
                .get_mut(&from)
                .expect("enrolled parent")
                .dependencies
                .insert(to)
        {
            return Ok(());
        }
        self.groups
            .get_mut(&to)
            .expect("enrolled dependency")
            .dependents
            .insert(from);
        // A path cannot return to a group with no incoming edges. This also
        // avoids rescanning a pending cycle for each new ancestor joining it.
        if self.groups[&from].dependents.is_empty() {
            return Ok(());
        }
        let forward = self.reachable(to, Edges::Dependencies);
        if forward.contains(&from) {
            let backward = self.reachable(from, Edges::Dependents);
            let cycle = forward.intersection(&backward).copied().collect();
            self.merge(from, cycle);
        }
        Ok(())
    }

    fn reachable(&self, start: HeapValueId, edges: Edges) -> HashSet<HeapValueId> {
        let mut seen = HashSet::new();
        let mut pending = vec![start];
        while let Some(id) = pending.pop() {
            if seen.insert(id) {
                let group = &self.groups[&id];
                pending.extend(match edges {
                    Edges::Dependencies => &group.dependencies,
                    Edges::Dependents => &group.dependents,
                });
            }
        }
        seen
    }

    fn merge(&mut self, id: HeapValueId, cycle: HashSet<HeapValueId>) {
        let mut merged = Group::default();
        for old in &cycle {
            let group = self.groups.remove(old).expect("cycle group exists");
            merged.members.extend(group.members);
            merged.unfinished_writers += group.unfinished_writers;
            merged.dependencies.extend(
                group
                    .dependencies
                    .into_iter()
                    .filter(|dep| !cycle.contains(dep)),
            );
            merged.dependents.extend(
                group
                    .dependents
                    .into_iter()
                    .filter(|dep| !cycle.contains(dep)),
            );
        }
        for member in &merged.members {
            self.values
                .get_mut(member)
                .expect("cycle member exists")
                .group = id;
        }
        for dependency in &merged.dependencies {
            let edges = &mut self
                .groups
                .get_mut(dependency)
                .expect("external dependency exists")
                .dependents;
            edges.retain(|old| !cycle.contains(old));
            edges.insert(id);
        }
        for dependent in &merged.dependents {
            let edges = &mut self
                .groups
                .get_mut(dependent)
                .expect("external dependent exists")
                .dependencies;
            edges.retain(|old| !cycle.contains(old));
            edges.insert(id);
        }
        self.groups.insert(id, merged);
    }

    pub(super) fn complete(
        &mut self,
        value: HeapValueId,
        state: &HeapDeserializationState,
        retired: &mut Vec<FrozenHeapArc>,
    ) -> crate::Result<SlotReadiness> {
        let index = value.value_index as usize;
        let ptr = match state.metadata.get().expect("claimed metadata").init_states[index]
            .load(Ordering::Acquire)
        {
            SlotState::InProgress(ptr) => ptr,
            SlotState::Failed => {
                return Err(state
                    .partial_deserialization_error(value.value_index)
                    .into());
            }
            _ => unreachable!("a tracked claim completes exactly once"),
        };
        let group_id = self.values[&value].group;
        let group = self
            .groups
            .get_mut(&group_id)
            .expect("pending group exists");
        group.unfinished_writers -= 1;
        if group.unfinished_writers != 0 || !group.dependencies.is_empty() {
            // Other constructors may need this pointer to make progress even
            // though owning readers must keep waiting for readiness.
            state.publication().constructed(index, ptr);
            return Ok(SlotReadiness::Pending(ptr));
        }

        let mut ready = HashMap::new();
        let mut pending = vec![group_id];
        while let Some(id) = pending.pop() {
            // Several dependencies can finish in the same propagation pass.
            let Some(group) = self.groups.get(&id) else {
                continue;
            };
            if group.unfinished_writers != 0 || !group.dependencies.is_empty() {
                continue;
            }
            let group = self.groups.remove(&id).expect("ready group exists");
            for member in group.members {
                let value = self.values.remove(&member).expect("ready member exists");
                let batch = ready.entry(member.heap_ptr).or_insert_with(|| ReadyBatch {
                    retired_heap_index: retired.len(),
                    slots: Vec::new(),
                });
                batch.slots.push(member.value_index);
                retired.push(value.heap);
            }
            for dependent in group.dependents {
                self.groups
                    .get_mut(&dependent)
                    .expect("dependent exists")
                    .dependencies
                    .remove(&id);
                pending.push(dependent);
            }
        }
        // `retired` retains every removed heap until the caller releases the
        // readiness lock. Only one heap's waiter lock is held at a time.
        // All collected payloads and their dependencies are initialized, so
        // publishing heaps separately cannot expose an unfinished edge.
        for batch in ready.into_values() {
            let mut publication = retired[batch.retired_heap_index]
                .deser_state()
                .expect("restored heap")
                .publication();
            for index in batch.slots {
                publication.ready(index as usize);
            }
        }
        Ok(SlotReadiness::Ready(ptr))
    }

    /// Failure flows to dependents, never to otherwise healthy dependencies.
    pub(super) fn fail(
        &mut self,
        value: HeapValueId,
        cause: Arc<str>,
        retired: &mut Vec<FrozenHeapArc>,
    ) {
        let Some(value) = self.values.get(&value) else {
            return;
        };
        let failed = self.reachable(value.group, Edges::Dependents);
        for id in &failed {
            let group = self.groups.remove(id).expect("failed group exists");
            for member in group.members {
                let value = self.values.remove(&member).expect("failed member exists");
                value
                    .heap
                    .deser_state()
                    .expect("restored heap")
                    .fail_slot(member.value_index as usize, cause.dupe());
                retired.push(value.heap);
            }
            for dependency in group.dependencies {
                if !failed.contains(&dependency) {
                    self.groups
                        .get_mut(&dependency)
                        .expect("healthy dependency exists")
                        .dependents
                        .remove(id);
                }
            }
        }
    }
}
