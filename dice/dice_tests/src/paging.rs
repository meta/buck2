/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! End-to-end tests for DICE paging (`page_out` / on-demand page-in) through the
//! public API.

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceEvent;
use dice::DiceEventListener;
use dice::DiceKeyDyn;
use dice::DiceStorage;
use dice::EqualityBehavior;
use dice::EvictionPolicy;
use dice::EvictionStrategy;
use dice::Key;
use dice::PagableStorageBackend;
use dice::PageOutPolicy;
use dice::UserComputationData;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use pagable::DeferredReadFailures;
use pagable::Pagable;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::ReadRefused;
use pagable::pagable_typetag;
use tempfile::tempdir;

mod recovery;

/// Storage paging each key out once and evicting at page-out, the semantics the tests here are
/// written against.
fn test_storage(path: &Path) -> anyhow::Result<DiceStorage> {
    let mut storage = DiceStorage::open(path, PagableStorageBackend::Sqlite)?;
    storage.set_page_out_policy(PageOutPolicy::OncePerKey);
    storage.set_eviction_policy(EvictionPolicy::OnIdle(EvictionStrategy::Default));
    Ok(storage)
}

/// A `ValueSerialize` that serializes successfully — so the node pages out — but
/// always fails to deserialize. This mimics a serialize/deserialize asymmetry
/// (e.g. a typetag mismatch) or on-disk corruption, exercising the worker's
/// page-in failure path. With `TRANSIENT` the failure is typed as one that never
/// reached the data, as a storage lock wait that ran out is.
struct FailToHydrateSerialize<const TRANSIENT: bool>;

impl<const TRANSIENT: bool> ValueSerialize for FailToHydrateSerialize<TRANSIENT> {
    type Value = u64;

    fn pagable_serialize_value(
        &self,
        v: &Self::Value,
        ser: &mut dyn PagableSerializer,
    ) -> Option<pagable::Result<()>> {
        Some(v.pagable_serialize(ser))
    }

    fn pagable_deserialize_value<'de, D: PagableDeserializer<'de> + ?Sized>(
        &self,
        _deser: &mut D,
    ) -> pagable::Result<Self::Value> {
        if TRANSIENT {
            Err(
                anyhow::Error::new(ReadRefused("simulated transient read failure"))
                    .context("while hydrating"),
            )
        } else {
            Err(anyhow::anyhow!("simulated hydrate failure"))
        }
    }
}

#[derive(Allocative, Clone, Copy, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[display("FailToHydrateKey({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct FailToHydrateKey(u32);

fn data_with_counter(counter: &Arc<AtomicUsize>) -> UserComputationData {
    let mut data = UserComputationData::new();
    data.data.set(counter.clone());
    data
}

#[async_trait]
impl Key for FailToHydrateKey {
    type Value = u64;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        if let Ok(counter) = ctx.per_transaction_data().data.get::<Arc<AtomicUsize>>() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        u64::from(self.0) * 100
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        FailToHydrateSerialize::<false>
    }
}

#[derive(Allocative, Clone, Copy, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[display("FailTransientlyToHydrateKey({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct FailTransientlyToHydrateKey(u32);

#[async_trait]
impl Key for FailTransientlyToHydrateKey {
    type Value = u64;

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        u64::from(self.0) * 100
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        FailToHydrateSerialize::<true>
    }
}

#[tokio::test]
async fn failed_hydrate_of_asserted_value_returns_error() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let storage = test_storage(tmp.path())?;
    let dice = {
        let mut builder = Dice::builder();
        builder.set_pagable_storage(storage);
        builder.build(DetectCycles::Disabled)
    };
    let root = dice.new_root().await;
    let counter = Arc::new(AtomicUsize::new(0));
    let mut updater = dice.updater_with_data(root, data_with_counter(&counter));
    updater.changed_to([(FailToHydrateKey(7), 700)])?;
    let tx = updater.commit().await;
    assert_eq!(*tx.compute(&FailToHydrateKey(7)).await?, 700);
    drop(tx);
    dice.wait_for_idle().await;
    dice.page_out().await?;

    let tx = dice
        .updater_with_data(root, data_with_counter(&counter))
        .commit()
        .await;
    let error = tokio::time::timeout(Duration::from_secs(10), tx.compute(&FailToHydrateKey(7)))
        .await
        .expect("failed page-in must finish without hanging")
        .expect_err("an unreadable value must fail the demand");
    assert!(error.to_string().contains("FailToHydrateKey(7)"));
    assert!(
        anyhow::Error::new(error)
            .chain()
            .any(|cause| cause.to_string() == "simulated hydrate failure")
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "an asserted value must not be replaced by a computation"
    );
    assert!(
        dice.pagable_storage_context()
            .expect("test has paging storage")
            .get_or_init(DeferredReadFailures::default)
            .snapshot()
            .is_empty(),
        "a failed demand must not request a deferred-read daemon restart"
    );
    Ok(())
}

/// Captures `DiceEvent::HydrationFailed` as `(key type, transient)` so a test can
/// assert a hydration failure is reported out-of-band (rather than silently
/// swallowed) and classified.
#[derive(Allocative)]
struct CapturingListener {
    #[allocative(skip)]
    hydration_failures: Arc<Mutex<Vec<(String, bool)>>>,
}

impl DiceEventListener for CapturingListener {
    fn event(&self, ev: DiceEvent) {
        if let DiceEvent::HydrationFailed {
            key_type,
            transient,
            ..
        } = ev
        {
            self.hydration_failures
                .lock()
                .unwrap()
                .push((key_type.to_owned(), transient));
        }
    }
}

/// Pages `key` out, computes it again so its page-in fails, and returns the
/// hydration failures reported.
async fn hydration_failures_of<K: Key<Value = u64>>(key: K) -> anyhow::Result<Vec<(String, bool)>> {
    let tmp = tempdir()?;
    let storage = test_storage(tmp.path())?;
    let dice = {
        let mut builder = Dice::builder();
        builder.set_pagable_storage(storage);
        builder.build(DetectCycles::Disabled)
    };
    let root = dice.new_root().await;

    let tx = dice.updater(root).commit().await;
    let _: u64 = *tx.compute(&key).await?;
    drop(tx);

    dice.wait_for_idle().await;
    dice.page_out().await?;

    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut data = UserComputationData::new();
    data.tracker = Arc::new(CapturingListener {
        hydration_failures: captured.clone(),
    });
    let tx = dice.updater_with_data(root, data).commit().await;
    let _ignored = tokio::time::timeout(Duration::from_secs(10), tx.compute(&key))
        .await
        .expect("compute must not hang when a paged-out value fails to hydrate");

    let failures = captured.lock().unwrap().clone();
    Ok(failures)
}

/// A failed page-in reports a `HydrationFailed` event (which buck2 maps to a
/// `soft_error`), so the failure is visible in telemetry rather than lost.
#[tokio::test]
async fn failed_hydrate_reports_a_hydration_failed_event() -> anyhow::Result<()> {
    let failures = hydration_failures_of(FailToHydrateKey(7)).await?;
    assert_eq!(
        failures.len(),
        1,
        "exactly one hydration failure should be reported, got {failures:?}"
    );
    assert!(
        failures[0].0.contains("FailToHydrateKey"),
        "reported key type should identify the failing key, got {:?}",
        failures[0]
    );
    assert!(!failures[0].1, "a failure of the data is not transient");
    Ok(())
}

/// A page-in that failed before reaching the data, as a storage lock wait that
/// ran out does, is reported as transient, however the error was wrapped.
#[tokio::test]
async fn transient_hydrate_failure_is_reported_as_such() -> anyhow::Result<()> {
    let failures = hydration_failures_of(FailTransientlyToHydrateKey(7)).await?;
    assert_eq!(failures.len(), 1, "got {failures:?}");
    assert!(failures[0].1, "got {failures:?}");
    Ok(())
}

#[tokio::test]
async fn concurrent_failed_page_ins_of_asserted_values_share_the_error() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let mut builder = Dice::builder();
    builder.set_pagable_storage(test_storage(tmp.path())?);
    let dice = builder.build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let counter = Arc::new(AtomicUsize::new(0));
    let mut updater = dice.updater_with_data(root, data_with_counter(&counter));
    updater.changed_to([(FailToHydrateKey(7), 700)])?;
    let tx = updater.commit().await;
    assert_eq!(*tx.compute(&FailToHydrateKey(7)).await?, 700);
    drop(tx);
    dice.wait_for_idle().await;
    dice.page_out().await?;

    let tx = dice
        .updater_with_data(root, data_with_counter(&counter))
        .commit()
        .await;
    let results: [_; 2] = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            tx.compute(&FailToHydrateKey(7)),
            tx.compute(&FailToHydrateKey(7))
        )
    })
    .await
    .expect("both failed demands must finish")
    .into();
    for result in results {
        let error = anyhow::Error::new(result.expect_err("the value cannot be read"));
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string() == "simulated hydrate failure")
        );
    }
    assert!(tx.compute(&FailToHydrateKey(7)).await.is_err());
    drop(tx);
    tokio::time::timeout(Duration::from_secs(10), dice.wait_for_idle()).await?;

    let tx = dice
        .updater_with_data(root, data_with_counter(&counter))
        .commit()
        .await;
    assert!(tx.compute(&FailToHydrateKey(7)).await.is_err());
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "failed demands must leave the value paged out"
    );
    Ok(())
}
