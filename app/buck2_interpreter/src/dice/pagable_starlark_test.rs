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
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::CancellationContext;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceEvent;
use dice::DiceEventListener;
use dice::DiceKeyDyn;
use dice::DiceStorage;
use dice::EqualityBehavior;
use dice::EvictionPolicy;
use dice::InjectedKey;
use dice::Key;
use dice::NoValueSerialize;
use dice::PagableStorageBackend;
use dice::PagableValueSerialize;
use dice::PageOutPolicy;
use dice::UserComputationData;
use dice::ValueSerialize;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::pagable_typetag;
use starlark::pagable::StarlarkDeserialize;
use starlark::pagable::StarlarkDeserializeContext;
use starlark::pagable::StarlarkSerialize;
use starlark::pagable::StarlarkSerializeContext;
use starlark::starlark_simple_value;
use starlark::values::AllocFrozenValue;
use starlark::values::FrozenHeap;
use starlark::values::FrozenHeapName;
use starlark::values::NoSerialize;
use starlark::values::OwnedFrozen;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::starlark_value;
use tempfile::tempdir;
use tokio::sync::Notify;

struct PageInGate {
    reached: Notify,
    released: Mutex<bool>,
    released_cv: Condvar,
}

impl PageInGate {
    fn new() -> Self {
        Self {
            reached: Notify::new(),
            released: Mutex::new(false),
            released_cv: Condvar::new(),
        }
    }

    fn block_current_thread(&self) {
        self.reached.notify_one();
        let mut released = self.released.lock().expect("page-in gate poisoned");
        while !*released {
            released = self
                .released_cv
                .wait(released)
                .expect("page-in gate poisoned while waiting");
        }
    }

    fn release(&self) {
        *self.released.lock().expect("page-in gate poisoned") = true;
        self.released_cv.notify_all();
    }
}

fn page_in_gate_slot() -> &'static Mutex<Option<Arc<PageInGate>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<PageInGate>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

struct PageInGateGuard {
    gate: Arc<PageInGate>,
}

impl PageInGateGuard {
    fn install() -> Self {
        let gate = Arc::new(PageInGate::new());
        let previous = page_in_gate_slot()
            .lock()
            .expect("page-in gate slot poisoned")
            .replace(gate.clone());
        assert!(previous.is_none(), "page-in gate already installed");
        Self { gate }
    }

    async fn wait_until_reached(&self) {
        self.gate.reached.notified().await;
    }

    fn release(&self) {
        self.gate.release();
    }
}

impl Drop for PageInGateGuard {
    fn drop(&mut self) {
        self.gate.release();
        let mut slot = page_in_gate_slot()
            .lock()
            .expect("page-in gate slot poisoned");
        if slot
            .as_ref()
            .is_some_and(|gate| Arc::ptr_eq(gate, &self.gate))
        {
            *slot = None;
        }
    }
}

#[derive(Debug, Allocative)]
struct PageInGateMarker(bool);

impl StarlarkSerialize for PageInGateMarker {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> starlark::Result<()> {
        self.0.pagable_serialize(ctx.pagable())?;
        Ok(())
    }
}

impl<'fv> StarlarkDeserialize<'fv> for PageInGateMarker {
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> starlark::Result<Self> {
        let should_block = bool::pagable_deserialize(ctx.pagable())?;
        if should_block {
            let gate = page_in_gate_slot()
                .lock()
                .expect("page-in gate slot poisoned")
                .clone()
                .expect("old root hydrated without an installed page-in gate");
            gate.block_current_thread();
        }
        Ok(Self(should_block))
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
#[display("ScopeLeafData({}, {})", self.flag, self.count)]
struct ScopeLeafData {
    flag: bool,
    count: usize,
}

starlark_simple_value!(ScopeLeafData);

#[starlark_value(type = "ScopeLeafData")]
impl<'v> StarlarkValue<'v> for ScopeLeafData {}

#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("ScopeRootData({})", self.label)]
struct ScopeRootData<'v> {
    label: usize,
    gate: PageInGateMarker,
    target: Value<'v>,
}

impl<'fv> AllocFrozenValue<'fv> for ScopeRootData<'fv> {
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        heap.alloc_simple(self)
    }
}

#[starlark_value(type = "ScopeRootData", frozen_vtable)]
impl<'v> StarlarkValue<'v> for ScopeRootData<'v> {}

#[derive(Clone, Dupe, Allocative)]
struct ScopeHeapValue(#[allocative(skip)] Arc<Mutex<Option<OwnedFrozen<Value<'static>>>>>);

impl ScopeHeapValue {
    fn new(value: OwnedFrozen<Value<'static>>) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    fn clone_owned(&self) -> OwnedFrozen<Value<'static>> {
        self.0
            .lock()
            .expect("scope heap value poisoned")
            .as_ref()
            .expect("scope heap value already moved")
            .dupe()
    }

    fn take_owned(&self) -> OwnedFrozen<Value<'static>> {
        self.0
            .lock()
            .expect("scope heap value poisoned")
            .take()
            .expect("scope heap value already moved")
    }
}

struct ScopeHeapValueSerialize;

impl ValueSerialize for ScopeHeapValueSerialize {
    type Value = ScopeHeapValue;

    fn pagable_serialize_value(
        &self,
        value: &Self::Value,
        serializer: &mut dyn PagableSerializer,
    ) -> Option<pagable::Result<()>> {
        let value = value.0.lock().expect("scope heap value poisoned");
        Some(
            value
                .as_ref()
                .expect("a moved scope heap value must not be paged out again")
                .pagable_serialize(serializer),
        )
    }

    fn pagable_deserialize_value<'de, D: PagableDeserializer<'de> + ?Sized>(
        &self,
        deserializer: &mut D,
    ) -> pagable::Result<Self::Value> {
        Ok(ScopeHeapValue::new(
            OwnedFrozen::<Value<'static>>::pagable_deserialize(deserializer)?,
        ))
    }
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct ScopeGenerationInput(u8);

impl InjectedKey for ScopeGenerationInput {
    type Value = u32;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct ScopeHeapKey(u8);

#[async_trait]
impl Key for ScopeHeapKey {
    type Value = ScopeHeapValue;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        let generation = ctx
            .compute(&ScopeGenerationInput(self.0))
            .await
            .expect("injected heap generation should compute");
        ScopeHeapValue::new(OwnedFrozen::build(
            FrozenHeapName::user("dice_scope_dependency"),
            |heap| match generation {
                0 => heap.alloc_simple(ScopeLeafData {
                    flag: true,
                    count: 111,
                }),
                1 => heap.alloc_simple(ScopeLeafData {
                    flag: false,
                    count: 222,
                }),
                _ => unreachable!("unexpected heap generation {generation}"),
            },
        ))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        ScopeHeapValueSerialize
    }
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct ScopeRootKey(u8);

#[async_trait]
impl Key for ScopeRootKey {
    type Value = OwnedFrozen<Value<'static>>;

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        unreachable!("ScopeRootKey values are injected")
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

fn make_root(dependency: &OwnedFrozen<Value<'static>>, root_id: u8) -> OwnedFrozen<Value<'static>> {
    OwnedFrozen::build(
        FrozenHeapName::user(format!("dice_scope_root_{root_id}")),
        |heap| {
            let target = dependency.as_ref().add_to_frozen_heap(heap);
            heap.alloc_simple(ScopeRootData {
                label: if root_id == 0 { 10 } else { 20 },
                gate: PageInGateMarker(root_id == 0),
                target,
            })
        },
    )
}

fn root_data<'a>(value: &'a OwnedFrozen<Value<'static>>) -> &'a ScopeRootData<'a> {
    value
        .as_ref()
        .value()
        .downcast_ref::<ScopeRootData>()
        .expect("root should contain ScopeRootData")
}

/// The two stored roots have different exact dependency heaps but the same
/// logical heap ID:
///
/// ```text
/// first page-out:  B0 -> H0 (111), C0 owns H0
/// recompute:       C0 -> C1, where C1 owns H1 (222)
/// second page-out: A1 -> H1; C1 is not a page-out candidate again
/// page-in:         pause B0 before resolving H0, hydrate A1, then resume B0
/// ```
///
/// B0 and A1 are independent DICE roots. Hydrating A1 must not replace the
/// exact heap binding used by B0's in-progress deserialization scope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_in_scopes_starlark_heaps_by_dice_root() {
    page_in_scopes_starlark_heaps_by_dice_root_impl()
        .await
        .expect("cross-root page-in regression setup should succeed");
}

async fn page_in_scopes_starlark_heaps_by_dice_root_impl()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let tmp = tempdir()?;
    let mut storage = DiceStorage::open(tmp.path(), PagableStorageBackend::Sqlite)?;
    storage.set_page_out_policy(PageOutPolicy::OncePerKey);
    storage.set_eviction_policy(EvictionPolicy::OnIdle);
    let mut builder = Dice::builder();
    builder.set_pagable_storage(storage);
    let dice = builder.build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    let mut updater = dice.updater(root);
    updater.changed_to([(ScopeGenerationInput(0), 0)])?;
    let tx = updater.commit().await;
    let c0 = tx.compute(&ScopeHeapKey(0)).await?.clone_owned();
    let b0 = make_root(&c0, 0);
    assert_eq!(
        root_data(&b0)
            .target
            .downcast_ref::<ScopeLeafData>()
            .expect("B0 should point to H0")
            .count,
        111,
    );
    drop(c0);
    drop(tx);

    let mut updater = dice.updater(root);
    updater.changed_to([(ScopeRootKey(0), b0)])?;
    drop(updater.commit().await);

    dice.wait_for_idle().await;
    dice.page_out().await?;
    let status = dice.pagable_status().await;
    assert_eq!(status.resident_count, 0);
    assert_eq!(status.paged_out_count, 2, "B0 and C0 should be paged out");
    assert_eq!(status.candidate_count, 0);

    let mut updater = dice.updater(root);
    updater.changed_to([(ScopeGenerationInput(0), 1)])?;
    let tx = updater.commit().await;
    let c1 = tx.compute(&ScopeHeapKey(0)).await?.take_owned();
    let a1 = make_root(&c1, 1);
    assert_eq!(
        root_data(&a1)
            .target
            .downcast_ref::<ScopeLeafData>()
            .expect("A1 should point to H1")
            .count,
        222,
    );
    drop(c1);
    drop(tx);

    let mut updater = dice.updater(root);
    updater.changed_to([(ScopeRootKey(1), a1)])?;
    drop(updater.commit().await);

    let status = dice.pagable_status().await;
    assert_eq!(
        status.candidate_count, 1,
        "only the previously unseen A1 root should be a page-out candidate",
    );
    dice.wait_for_idle().await;
    dice.page_out().await?;
    let status = dice.pagable_status().await;
    assert_eq!(
        status.resident_count, 1,
        "recomputed C1 should remain resident",
    );
    assert_eq!(
        status.paged_out_count, 2,
        "B0 and A1 should be the two paged-out roots",
    );
    assert_eq!(status.candidate_count, 0);

    let gate = PageInGateGuard::install();
    let tx = dice.updater(root).commit().await;
    let old_tx = tx.dupe();
    let old_hydration = tokio::spawn(async move {
        old_tx
            .compute(&ScopeRootKey(0))
            .await
            .expect("B0 should hydrate")
            .dupe()
    });
    tokio::time::timeout(Duration::from_secs(10), gate.wait_until_reached())
        .await
        .expect("B0 hydration did not reach the page-in gate");

    let new_root = tx.compute(&ScopeRootKey(1)).await.map(Dupe::dupe);
    gate.release();
    let old_root = old_hydration.await?;
    let new_root = new_root?;

    let new_root = root_data(&new_root);
    assert_eq!(new_root.label, 20);
    assert!(!new_root.gate.0);
    assert_eq!(
        new_root
            .target
            .downcast_ref::<ScopeLeafData>()
            .expect("hydrated A1 should point to H1")
            .count,
        222,
    );

    let old_root = root_data(&old_root);
    assert_eq!(old_root.label, 10);
    assert!(old_root.gate.0);
    let old_target = old_root
        .target
        .downcast_ref::<ScopeLeafData>()
        .expect("hydrated B0 should point to ScopeLeafData");
    assert!(old_target.flag, "B0 must resolve its target through H0");
    assert_eq!(
        old_target.count, 111,
        "B0 must not resolve its target through A1's H1 state",
    );

    Ok(())
}

#[derive(Default, Allocative)]
struct CollisionEventRecorder {
    #[allocative(skip)]
    events: Mutex<Vec<DiceEvent>>,
}

impl CollisionEventRecorder {
    fn count_check_deps(&self, key_type: &'static str) -> usize {
        self.events
            .lock()
            .expect("collision event recorder poisoned")
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    DiceEvent::CheckDepsStarted {
                        key_type: actual,
                    } if *actual == key_type
                )
            })
            .count()
    }

    fn count_computes(&self, key_type: &'static str) -> usize {
        self.events
            .lock()
            .expect("collision event recorder poisoned")
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    DiceEvent::ComputeStarted {
                        key_type: actual,
                    } if *actual == key_type
                )
            })
            .count()
    }

    fn hydration_errors(&self, key_type: &'static str) -> Vec<String> {
        self.events
            .lock()
            .expect("collision event recorder poisoned")
            .iter()
            .filter_map(|event| match event {
                DiceEvent::HydrationFailed {
                    key_type: actual,
                    error,
                    ..
                } if *actual == key_type => Some(error.clone()),
                _ => None,
            })
            .collect()
    }

    fn clear(&self) {
        self.events
            .lock()
            .expect("collision event recorder poisoned")
            .clear();
    }
}

impl DiceEventListener for CollisionEventRecorder {
    fn event(&self, event: DiceEvent) {
        self.events
            .lock()
            .expect("collision event recorder poisoned")
            .push(event);
    }
}

fn user_data_with_recorder(recorder: &Arc<CollisionEventRecorder>) -> UserComputationData {
    UserComputationData {
        tracker: recorder.clone(),
        ..Default::default()
    }
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct CollisionHeapKey;

#[async_trait]
impl Key for CollisionHeapKey {
    type Value = ScopeHeapValue;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        let _generation = ctx
            .compute(&ScopeGenerationInput(1))
            .await
            .expect("injected heap generation should compute");
        ScopeHeapValue::new(OwnedFrozen::build(
            FrozenHeapName::user("dice_collision_dependency"),
            |heap| {
                heap.alloc_simple(ScopeLeafData {
                    flag: true,
                    count: 333,
                })
            },
        ))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        ScopeHeapValueSerialize
    }
}

#[derive(Clone, Dupe, Allocative, Pagable)]
struct CollisionRootValue {
    direct: OwnedFrozen<Value<'static>>,
    enclosing: OwnedFrozen<Value<'static>>,
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct CollisionRootInput;

impl InjectedKey for CollisionRootInput {
    type Value = CollisionRootValue;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct CollisionRootKey;

#[async_trait]
impl Key for CollisionRootKey {
    type Value = CollisionRootValue;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.compute(&CollisionRootInput)
            .await
            .expect("collision root input should compute")
            .clone()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

/// Reproduces the production cache-substitution sequence without constructing
/// an inconsistent root up front:
///
/// ```text
/// page out C0 -> H0
/// change C's dependency, recompute C1 -> H1, then hydrate C0 for equality
///
/// construct R1:
/// R1.direct    -> H1
/// R1.enclosing -> P1 -> H1
///
/// page out R1
///
/// page in R1:
///   direct heap key    -> resident H1, binding X -> H1
///   enclosing heap key -> resident P1 -> H1
/// ```
///
/// `H0` and `H1` are distinct allocations with the same name and serialized
/// contents. Before page-out, both paths in `R1` point to the exact `H1`
/// allocation. Page-in must prefer that live resident allocation over the older
/// deserialized Arc cached for the same data key.
#[tokio::test]
async fn page_in_prefers_resident_heap_for_consistent_dice_root() {
    page_in_prefers_resident_heap_for_consistent_dice_root_impl()
        .await
        .expect("resident heap page-in should complete");
}

async fn page_in_prefers_resident_heap_for_consistent_dice_root_impl()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let tmp = tempdir()?;
    let mut storage = DiceStorage::open(tmp.path(), PagableStorageBackend::Sqlite)?;
    storage.set_page_out_policy(PageOutPolicy::OncePerKey);
    storage.set_eviction_policy(EvictionPolicy::OnIdle);
    let mut builder = Dice::builder();
    builder.set_pagable_storage(storage);
    let dice = builder.build(DetectCycles::Disabled);
    let branch = dice.new_root().await;
    let recorder = Arc::new(CollisionEventRecorder::default());

    let mut updater = dice.updater(branch);
    updater.changed_to([(ScopeGenerationInput(1), 0)])?;
    let tx = updater.commit().await;
    let c0 = tx.compute(&CollisionHeapKey).await?.clone_owned();
    assert_eq!(
        c0.as_ref()
            .value()
            .downcast_ref::<ScopeLeafData>()
            .expect("C0 should contain ScopeLeafData")
            .count,
        333,
    );
    drop(c0);
    drop(tx);

    dice.wait_for_idle().await;
    dice.page_out().await?;
    assert_eq!(
        dice.pagable_status().await.paged_out_count,
        1,
        "C0 should be paged out before its dependency changes",
    );

    let mut updater = dice.updater_with_data(branch, user_data_with_recorder(&recorder));
    updater.changed_to([(ScopeGenerationInput(1), 1)])?;
    let tx = updater.commit().await;
    let c1 = tx.compute(&CollisionHeapKey).await?.clone_owned();
    let collision_key_type = <CollisionHeapKey as Key>::key_type_name();
    assert_eq!(recorder.count_check_deps(collision_key_type), 1);
    assert_eq!(recorder.count_computes(collision_key_type), 1);
    assert_eq!(
        dice.page_in_metrics()
            .get(collision_key_type)
            .map_or(0, |metrics| metrics.count),
        1,
        "CheckDepsPagedOut should hydrate C0 after recomputing C1",
    );

    let h1 = c1.owner().to_owned();
    let p1 = make_root(&c1, 1);
    assert!(
        p1.owner().refs().any(|dependency| dependency == h1.owner()),
        "P1 should retain the exact H1 allocation",
    );
    let root = CollisionRootValue {
        direct: c1,
        enclosing: p1,
    };
    assert_eq!(root.direct.owner(), h1.owner());
    assert!(
        root.enclosing
            .owner()
            .refs()
            .any(|dependency| dependency == h1.owner()),
        "both pre-page-out paths should reach the exact H1 allocation",
    );
    drop(tx);

    let mut updater = dice.updater(branch);
    updater.changed_to([(CollisionRootInput, root)])?;
    let tx = updater.commit().await;
    let resident_root = tx.compute(&CollisionRootKey).await?;
    assert_eq!(resident_root.direct.owner(), h1.owner());
    assert!(
        resident_root
            .enclosing
            .owner()
            .refs()
            .any(|dependency| dependency == h1.owner()),
    );
    drop(tx);

    dice.wait_for_idle().await;
    dice.page_out().await?;
    recorder.clear();

    let tx = dice
        .updater_with_data(branch, user_data_with_recorder(&recorder))
        .commit()
        .await;
    let restored = tx.compute(&CollisionRootKey).await?;
    let root_key_type = <CollisionRootKey as Key>::key_type_name();
    let errors = recorder.hydration_errors(root_key_type);
    assert!(
        errors.is_empty(),
        "R1 should hydrate without conflicting heap bindings: {errors:#?}",
    );
    assert_eq!(
        recorder.count_computes(root_key_type),
        0,
        "R1 should hydrate instead of recomputing",
    );
    assert_eq!(restored.direct.owner(), h1.owner());
    assert!(
        restored
            .enclosing
            .owner()
            .refs()
            .any(|dependency| dependency == h1.owner()),
        "the recomputed root should still consistently reference H1",
    );

    Ok(())
}
