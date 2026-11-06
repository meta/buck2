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

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use pagable::storage::handle::PagableStorageHandle;
use pagable::storage::in_memory::InMemoryPagableStorage;

use super::*;
use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializeContext;
use crate::values::layout::avalues::simple::AValueFrozen;

const TIMEOUT: Duration = Duration::from_secs(10);
const CHAIN_DEPTH: usize = 32;
static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
static CONTROLS: OnceLock<Mutex<HashMap<usize, Arc<Control>>>> = OnceLock::new();

#[derive(Clone, Copy)]
enum Outcome {
    Success,
    Error,
    #[cfg(panic = "unwind")]
    Panic,
}

struct Control {
    claimed: mpsc::Sender<usize>,
    start: [Mutex<mpsc::Receiver<()>>; 2],
    read_target: mpsc::Sender<usize>,
    finish: [Mutex<mpsc::Receiver<Outcome>>; 2],
    dropped: AtomicUsize,
}

#[derive(Debug, Display, Allocative, ProvidesStaticType, NoSerialize)]
#[display("CycleNode({})", self.node)]
struct CycleNode<'v> {
    fixture: usize,
    node: usize,
    target: Value<'v>,
    owned: String,
}

impl Drop for CycleNode<'_> {
    fn drop(&mut self) {
        if let Some(control) = CONTROLS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .get(&self.fixture)
        {
            control.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[starlark_value(type = "CycleReadinessNode", frozen_vtable)]
impl<'v> StarlarkValue<'v> for CycleNode<'v> {}

impl StarlarkSerialize for CycleNode<'_> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        self.fixture.starlark_serialize(ctx)?;
        self.node.starlark_serialize(ctx)?;
        self.target.starlark_serialize(ctx)?;
        self.owned.starlark_serialize(ctx)
    }
}

impl<'v> StarlarkDeserialize<'v> for CycleNode<'v> {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        let fixture = usize::starlark_deserialize(ctx)?;
        let node = usize::starlark_deserialize(ctx)?;
        let control = CONTROLS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .get(&fixture)
            .cloned();
        if let Some(control) = &control {
            control.claimed.send(node).unwrap();
            control.start[node]
                .lock()
                .unwrap()
                .recv_timeout(TIMEOUT)
                .expect("test starts both claimed constructors");
        }
        let target = Value::starlark_deserialize(ctx)?;
        let owned = String::starlark_deserialize(ctx)?;
        if let Some(control) = control {
            control.read_target.send(node).unwrap();
            let outcome = control.finish[node]
                .lock()
                .unwrap()
                .recv_timeout(TIMEOUT)
                .expect("test releases constructor");
            match outcome {
                Outcome::Success => {}
                Outcome::Error => {
                    return Err(anyhow::anyhow!("intentional cycle constructor failure").into());
                }
                #[cfg(panic = "unwind")]
                Outcome::Panic => panic!("intentional cycle constructor panic"),
            }
        }
        Ok(Self {
            fixture,
            node,
            target,
            owned,
        })
    }
}

#[derive(Debug, Allocative, ProvidesStaticType, StarlarkPagable)]
struct OwnedChild {
    #[starlark_pagable(pagable)]
    value: OwnedFrozen<Value<'static>>,
}

#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("OwnedCarrier")]
struct OwnedCarrier {
    child: Arc<OwnedChild>,
}

starlark_simple_value!(OwnedCarrier);

#[starlark_value(type = "CycleReadinessOwnedCarrier")]
impl<'v> StarlarkValue<'v> for OwnedCarrier {}

#[derive(Debug, Allocative, ProvidesStaticType)]
struct OwningBackEdge {
    value: Mutex<Option<OwnedFrozen<Value<'static>>>>,
}

impl StarlarkSerialize for OwningBackEdge {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        // Use an independent wire owner to reach page-in's owning boundary:
        // production Arc page-out cannot emit the native heap -> Arc -> heap cycle.
        let wire_heap = OwnedFrozenHeap::new();
        wire_heap.with(|heap| {
            heap.alloc("synthetic owning back edge");
        });
        wire_heap
            .seal(FrozenHeapName::user("owning_back_edge_wire_owner"))
            .pagable_serialize(ctx.pagable())
            .map_err(crate::Error::new_other)?;
        let value = self
            .value
            .lock()
            .unwrap()
            .as_ref()
            .expect("back edge is installed after the fixture heap is sealed")
            .clone();
        value.as_ref().value().starlark_serialize(ctx)
    }
}

impl<'v> StarlarkDeserialize<'v> for OwningBackEdge {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'v>,
    ) -> crate::Result<Self> {
        Ok(Self {
            value: Mutex::new(Some(
                OwnedFrozen::pagable_deserialize(ctx.pagable()).map_err(crate::Error::new_other)?,
            )),
        })
    }
}

#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("SelfOwningCarrier")]
struct SelfOwningCarrier {
    child: Arc<OwningBackEdge>,
}

starlark_simple_value!(SelfOwningCarrier);

#[starlark_value(type = "CycleReadinessSelfOwningCarrier")]
impl<'v> StarlarkValue<'v> for SelfOwningCarrier {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Root {
    CycleA,
    CycleB,
    Dependent,
    Diamond,
    Healthy,
    OwningCarrier,
    CachedOwningCarrier,
    AncestorChain,
    CrossHeapDependent,
    OuterHealthy,
}

struct Fixture {
    id: usize,
    backing: InMemoryPagableStorage,
    handle: PagableStorageHandle,
    keys: [pagable::DataKey; 10],
}

impl Fixture {
    fn new() -> Self {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let heap = ErasingHeap::new();
        let values = heap.with(|heap| {
            let (a, _) = heap.reserve_with_extra::<AValueFrozen<CycleNode>>(0);
            // SAFETY: the reservation is filled below before any value is read,
            // and every edge remains in this same retained frozen heap.
            let a_ptr = unsafe { a.forward_ptr().unpack_frozen_value() };
            let b = heap.alloc_simple(CycleNode {
                fixture: id,
                node: 1,
                target: a_ptr,
                owned: "b".to_owned(),
            });
            let a = a.fill(CycleNode {
                fixture: id,
                node: 0,
                target: b,
                owned: "a".to_owned(),
            });
            let dependent = heap.alloc_simple(RefData {
                label: 2,
                target: b,
            });
            let diamond = heap.alloc(AllocTuple([dependent, b]));
            let healthy = heap.alloc_simple(SimpleData {
                flag: true,
                count: 17,
            });
            let chain = (0..CHAIN_DEPTH).fold(b, |target, label| {
                heap.alloc_simple(RefData { label, target })
            });
            [a, b, dependent, diamond, healthy, chain].map(erase)
        });
        let owner = heap.into_ref_named(FrozenHeapName::user(format!("cycle_readiness_{id}")));
        let backing = InMemoryPagableStorage::new();
        let [a, b, dependent, diamond, healthy, chain] = values.map(|value| {
            // SAFETY: every fixture value lives in `owner`'s heap.
            let root = unsafe { OwnedFrozen::from_erased(owner.clone(), value) };
            ser_owned_frozen_value_into_storage(&backing, &root).unwrap()
        });
        // Two slots share an Arc whose owning child must not be cached before
        // readiness. A cache hit in the second slot bypasses child deserialization.
        let outer = ErasingHeap::new();
        // SAFETY: B was allocated in `owner`'s arena.
        let child = Arc::new(OwnedChild {
            value: unsafe { OwnedFrozen::from_erased(owner.clone(), values[1]) },
        });
        let p = outer.alloc_simple(OwnedCarrier {
            child: child.clone(),
        });
        let q = outer.alloc_simple(OwnedCarrier { child });
        outer.add_reference(owner.as_ref());
        let cross_heap = outer.alloc_ref_data(8, values[1]);
        let outer_healthy = outer.alloc_simple(SimpleData {
            flag: true,
            count: 19,
        });
        let outer_owner =
            outer.into_ref_named(FrozenHeapName::user(format!("cycle_owned_carriers_{id}")));
        let [owning, cached_owning, cross_heap, outer_healthy] = [p, q, cross_heap, outer_healthy]
            .map(|value| {
                // SAFETY: these values were allocated in `outer_owner`'s arena.
                let root = unsafe { OwnedFrozen::from_erased(outer_owner.clone(), value) };
                ser_owned_frozen_value_into_storage(&backing, &root).unwrap()
            });
        let keys = [
            a,
            b,
            dependent,
            diamond,
            healthy,
            owning,
            cached_owning,
            chain,
            cross_heap,
            outer_healthy,
        ];
        let storage = backing.handle();
        storage.flush().unwrap();
        drop(owner);
        drop(outer_owner);
        storage.arc_cache().clear();
        let handle = PagableStorageHandle::new(storage);
        Self {
            id,
            backing,
            handle,
            keys,
        }
    }

    fn read(&self, root: Root) -> crate::Result<OwnedFrozen<Value<'static>>> {
        deser_owned_frozen_from_storage(&self.backing, &self.handle, &self.keys[root as usize])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        CONTROLS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .remove(&self.id);
    }
}

fn assert_cycle(root: &OwnedFrozen<Value<'static>>, node: usize) {
    let value = root.as_ref().value();
    let first = value.downcast_ref::<CycleNode>().unwrap();
    assert_eq!(first.node, node);
    let second = first.target.downcast_ref::<CycleNode>().unwrap();
    assert_eq!(second.node, 1 - node);
    assert!(
        second.target.ptr_eq(value),
        "restoration preserves the back edge"
    );
}

#[test]
fn same_thread_cycle_is_ready_on_return() {
    let fixture = Fixture::new();
    let root = fixture.read(Root::CycleA).unwrap();
    assert!(
        root.owner().heap_arc().deser_state().is_some(),
        "must restore a cold heap"
    );
    assert_cycle(&root, 0);
}

#[test]
fn same_thread_owning_back_edge_is_rejected_at_page_in() {
    let heap = ErasingHeap::new();
    let child = Arc::new(OwningBackEdge {
        value: Mutex::new(None),
    });
    let value = heap.alloc_simple(SelfOwningCarrier {
        child: child.clone(),
    });
    let healthy = heap.alloc_simple(SimpleData {
        flag: true,
        count: 23,
    });
    let owner = heap.into_ref_named(FrozenHeapName::user("same_thread_owning_back_edge"));
    let native_heap = owner.heap_arc().downgrade().unwrap();
    // SAFETY: both values were allocated in `owner`'s heap, which each root retains.
    let root = unsafe { OwnedFrozen::from_erased(owner.clone(), value) };
    // SAFETY: `healthy` was allocated in `owner`'s heap.
    let healthy_root = unsafe { OwnedFrozen::from_erased(owner, healthy) };
    *child.value.lock().unwrap() = Some(root.clone());

    let backing = InMemoryPagableStorage::new();
    let storage = backing.handle();
    let serialized = (|| -> crate::Result<_> {
        let key = ser_owned_frozen_value_into_storage(&backing, &root)?;
        let healthy_key = ser_owned_frozen_value_into_storage(&backing, &healthy_root)?;
        storage.flush().map_err(crate::Error::new_other)?;
        Ok((key, healthy_key))
    })();
    // Break the native ownership cycle only after all deferred Arc bodies are
    // serialized, and before checking the serialization result.
    child.value.lock().unwrap().take();
    drop((root, healthy_root, child));
    storage.arc_cache().clear();
    let (key, healthy_key) = serialized.expect("the synthetic owning back edge serializes");
    assert!(
        native_heap.upgrade().is_none(),
        "the source heap is released"
    );

    let handle = PagableStorageHandle::new(storage.clone());
    let healthy = deser_owned_frozen_from_storage(&backing, &handle, &healthy_key).unwrap();
    assert!(healthy.owner().heap_arc().deser_state().is_some());
    let data = storage.fetch_data_blocking(&key).unwrap();
    let worker_handle = handle.clone();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut de = worker_handle.root_deserializer(key, &data);
        let result = OwnedFrozen::<Value>::pagable_deserialize(&mut de)
            .map(|_| ())
            .map_err(|error| format!("{error:#}"));
        tx.send(result).unwrap();
    });
    let result = rx
        .recv_timeout(TIMEOUT)
        .expect("same-thread owning page-in must fail rather than deadlock");
    worker.join().unwrap();
    let expected = "an owning deserialization result depends on its unfinished constructor";
    let error = result.expect_err("an unfinished owning reference must not escape");
    assert!(error.contains(expected), "{error}");
    let retry = deser_owned_frozen_from_storage(&backing, &handle, &key).unwrap_err();
    assert!(format!("{retry:#}").contains(expected), "{retry:#}");
    assert_eq!(
        healthy
            .as_ref()
            .value()
            .downcast_ref::<SimpleData>()
            .unwrap()
            .count,
        23,
        "the rejected cycle does not poison its independent sibling"
    );
}

fn cross_thread_cycle(outcome: Outcome) {
    let fixture = Fixture::new();
    let healthy = fixture.read(Root::Healthy).unwrap();
    let outer_healthy = fixture.read(Root::OuterHealthy).unwrap();
    let state = healthy
        .owner()
        .heap_arc()
        .deser_state()
        .expect("must restore a cold heap");
    let (claimed_tx, claimed_rx) = mpsc::channel();
    let (read_tx, read_rx) = mpsc::channel();
    let (start_a, start_a_rx) = mpsc::channel();
    let (start_b, start_b_rx) = mpsc::channel();
    let (finish_a, finish_a_rx) = mpsc::channel();
    let (finish_b, finish_b_rx) = mpsc::channel();
    let control = Arc::new(Control {
        claimed: claimed_tx,
        start: [Mutex::new(start_a_rx), Mutex::new(start_b_rx)],
        read_target: read_tx,
        finish: [Mutex::new(finish_a_rx), Mutex::new(finish_b_rx)],
        dropped: AtomicUsize::new(0),
    });
    CONTROLS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .insert(fixture.id, control.clone());
    let spawn_reader = |root: Root, wait_started: Option<mpsc::Sender<()>>| {
        let handle = fixture.handle.clone();
        let heap_owner = if root == Root::CrossHeapDependent {
            // The raw dependent waits on its own heap's readiness; the owning
            // carrier instead waits on the child before publishing its Arc.
            outer_healthy.clone()
        } else {
            healthy.clone()
        };
        let key = fixture.keys[root as usize];
        let data = fixture.backing.handle().fetch_data_blocking(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            if let Some(started) = wait_started {
                heap_owner
                    .owner()
                    .heap_arc()
                    .deser_state()
                    .unwrap()
                    .notify_on_wait_for_test(Some(std::thread::current().id()), started);
            }
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut de = handle.root_deserializer(key, &data);
                OwnedFrozen::<Value>::pagable_deserialize(&mut de)
            }))
            .map_err(|_| "constructor panicked".to_owned())
            .and_then(|result| result.map_err(|e| format!("{e:#}")));
            tx.send(result).unwrap();
        });
        (thread, rx)
    };
    let (thread_a, result_a) = spawn_reader(Root::CycleA, None);
    let (thread_b, result_b) = spawn_reader(Root::CycleB, None);
    let mut claimed = [
        claimed_rx.recv_timeout(TIMEOUT).unwrap(),
        claimed_rx.recv_timeout(TIMEOUT).unwrap(),
    ];
    claimed.sort_unstable();
    assert_eq!(claimed, [0, 1]);

    let (waiting_tx, waiting_rx) = mpsc::channel();
    state.notify_on_wait_for_test(Some(thread_a.thread().id()), waiting_tx);
    start_a.send(()).unwrap();
    waiting_rx
        .recv_timeout(TIMEOUT)
        .expect("A waits on B before B reads A");
    start_b.send(()).unwrap();
    assert_eq!(
        read_rx.recv_timeout(TIMEOUT).unwrap(),
        1,
        "B breaks the cycle"
    );
    let (waiting_tx, waiting_rx) = mpsc::channel();
    state.notify_on_wait_for_test(Some(thread_b.thread().id()), waiting_tx);
    finish_b.send(Outcome::Success).unwrap();
    waiting_rx
        .recv_timeout(TIMEOUT)
        .expect("B's owning result waits for readiness");
    assert_eq!(
        read_rx.recv_timeout(TIMEOUT).unwrap(),
        0,
        "A can consume B's constructed pointer"
    );
    assert!(
        matches!(result_b.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "B must not escape before A completes"
    );

    let spawn_waiting_reader = |root, reason: &str| {
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (thread, result) = spawn_reader(root, Some(waiting_tx));
        waiting_rx.recv_timeout(TIMEOUT).expect(reason);
        assert!(
            matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "{root:?} must not escape before the cycle completes"
        );
        (thread, result)
    };
    let (thread_c, result_c) =
        spawn_waiting_reader(Root::Dependent, "a dependent outside the cycle also waits");
    let (thread_d, result_d) =
        spawn_waiting_reader(Root::Diamond, "a diamond dependent waits on both paths");
    let (chain_thread, chain_result) = spawn_waiting_reader(
        Root::AncestorChain,
        "a growing chain of ancestors waits for the pending cycle",
    );
    let (cross_heap_thread, cross_heap_result) = spawn_waiting_reader(
        Root::CrossHeapDependent,
        "a dependent in another heap waits for the cycle",
    );
    let (owning_thread, owning_result) = spawn_waiting_reader(
        Root::OwningCarrier,
        "nested owning results wait on the child heap before entering the shared Arc cache",
    );
    let values: Vec<_> = healthy.owner().heap_arc().iter_values().collect();
    assert_eq!(
        values.len(),
        1,
        "heap enumeration hides constructed-but-unready slots"
    );
    assert_eq!(values[0].downcast_ref::<SimpleData>().unwrap().count, 17);
    let allocated_slots: usize = healthy
        .owner()
        .heap_arc()
        .build_chunk_index()
        .iter()
        .map(|chunk| chunk.payload_offsets.len())
        .sum();
    assert_eq!(
        allocated_slots,
        CHAIN_DEPTH + 5,
        "the physical index includes A's unwritten slot, B, both dependents, the healthy value and the chain"
    );
    assert_eq!(
        fixture
            .read(Root::Healthy)
            .unwrap()
            .as_ref()
            .value()
            .downcast_ref::<SimpleData>()
            .unwrap()
            .count,
        17,
        "an unrelated root does not wait for the cycle"
    );

    finish_a.send(outcome).unwrap();
    let a = result_a.recv_timeout(TIMEOUT).expect("A completes");
    let b = result_b
        .recv_timeout(TIMEOUT)
        .expect("B's readiness waiter wakes");
    let c = result_c
        .recv_timeout(TIMEOUT)
        .expect("dependent readiness waiter wakes");
    let d = result_d
        .recv_timeout(TIMEOUT)
        .expect("diamond readiness waiter wakes");
    let chain = chain_result
        .recv_timeout(TIMEOUT)
        .expect("ancestor chain readiness waiter wakes");
    let cross_heap = cross_heap_result
        .recv_timeout(TIMEOUT)
        .expect("publication also wakes the other heap's waiter");
    thread_a.join().unwrap();
    thread_b.join().unwrap();
    thread_c.join().unwrap();
    thread_d.join().unwrap();
    chain_thread.join().unwrap();
    cross_heap_thread.join().unwrap();
    {
        let result = owning_result
            .recv_timeout(TIMEOUT)
            .expect("nested owning waiter wakes");
        owning_thread.join().unwrap();
        if matches!(outcome, Outcome::Success) {
            let carrier = result.unwrap();
            let cached = fixture.read(Root::CachedOwningCarrier).unwrap();
            assert!(
                Arc::ptr_eq(
                    &carrier
                        .as_ref()
                        .value()
                        .downcast_ref::<OwnedCarrier>()
                        .unwrap()
                        .child,
                    &cached
                        .as_ref()
                        .value()
                        .downcast_ref::<OwnedCarrier>()
                        .unwrap()
                        .child,
                ),
                "the later reader exercises the shared Arc cache"
            );
            assert_cycle(
                &carrier
                    .as_ref()
                    .value()
                    .downcast_ref::<OwnedCarrier>()
                    .unwrap()
                    .child
                    .value,
                1,
            );
        } else {
            assert!(
                result.is_err(),
                "an Arc cache hit cannot expose a failed cycle"
            );
            assert!(
                fixture.read(Root::CachedOwningCarrier).is_err(),
                "retrying the Arc initializer must preserve the failure"
            );
        }
    }
    match outcome {
        Outcome::Success => {
            assert_cycle(&a.unwrap(), 0);
            assert_cycle(&b.unwrap(), 1);
            let chain = chain.unwrap();
            let mut value = chain.as_ref().value();
            for label in (0..CHAIN_DEPTH).rev() {
                let link = value.downcast_ref::<RefData>().unwrap();
                assert_eq!(link.label, label);
                value = link.target;
            }
            assert_eq!(value.downcast_ref::<CycleNode>().unwrap().node, 1);
            let cross_heap = cross_heap.unwrap();
            let dependent = cross_heap
                .as_ref()
                .value()
                .downcast_ref::<RefData>()
                .unwrap();
            assert_eq!(dependent.label, 8);
            assert_eq!(
                dependent.target.downcast_ref::<CycleNode>().unwrap().node,
                1
            );
            assert_eq!(d.unwrap().as_ref().value().length().unwrap(), 2);
            assert_eq!(
                c.unwrap()
                    .as_ref()
                    .value()
                    .downcast_ref::<RefData>()
                    .unwrap()
                    .label,
                2
            );
        }
        _ => {
            assert!(a.is_err());
            let expected = if matches!(outcome, Outcome::Error) {
                "intentional cycle constructor failure"
            } else {
                "value deserialization did not complete"
            };
            assert!(b.unwrap_err().contains(expected));
            assert!(c.unwrap_err().contains(expected));
            assert!(d.unwrap_err().contains(expected));
            assert!(chain.unwrap_err().contains(expected));
            assert!(cross_heap.unwrap_err().contains(expected));
            assert!(
                format!("{:#}", fixture.read(Root::CycleB).unwrap_err()).contains(expected),
                "later readers see the same terminal failure"
            );
        }
    }
    assert!(
        fixture.read(Root::Healthy).is_ok(),
        "failure must not poison an independent sibling"
    );
    drop(values);
    drop(healthy);
    drop(outer_healthy);
    fixture.backing.handle().arc_cache().clear();
    assert_eq!(
        control.dropped.load(Ordering::Relaxed),
        if matches!(outcome, Outcome::Success) {
            2
        } else {
            1
        },
        "readiness tracking releases heaps; a written payload is dropped even if its dependency failed"
    );
}

#[test]
fn cross_thread_cycle_waits_for_transitive_readiness() {
    cross_thread_cycle(Outcome::Success);
}

#[test]
fn cycle_failure_reaches_completed_dependents() {
    cross_thread_cycle(Outcome::Error);
}

#[cfg(panic = "unwind")]
#[test]
fn cycle_unwind_reaches_completed_dependents() {
    cross_thread_cycle(Outcome::Panic);
}
