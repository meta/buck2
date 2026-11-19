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
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::actions::calculation::BuildKey;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use derive_more::Display;
use dice::ActivationData;
use dice::ActivationTracker;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DynKey;
use dice::EqualityBehavior;
use dice::EvictionPolicy;
use dice::EvictionStrategy;
use dice::Key;
use dice::NoValueSerialize;
use dice::PageInPhase;
use dice::PageOutPolicy;
use dice::UserComputationData;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use pagable::storage::in_memory::InMemoryPagableStorage;
use starlark::pagable::DeferredFieldReadsEnabled;

#[derive(Default, Allocative)]
struct Tracker {
    values_computed: AtomicUsize,
    observers_computed: AtomicUsize,
    #[allocative(skip)]
    phases: Mutex<Vec<PageInPhase>>,
}

impl ActivationTracker for Tracker {
    fn key_activated(
        &self,
        _key: &DynKey,
        _deps: &mut dyn Iterator<Item = &DynKey>,
        _data: ActivationData,
    ) {
    }

    fn key_paged_in(&self, key: &DynKey, _start: Instant, _duration: Duration, phase: PageInPhase) {
        if key.downcast_ref::<OutputKey>().is_some() {
            self.phases.lock().unwrap().push(phase);
        }
    }
}

#[derive(Allocative, Clone, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct Input;

#[async_trait]
impl Key for Input {
    type Value = u64;

    async fn compute(&self, _ctx: &mut DiceComputations, _cancel: &CancellationContext) -> u64 {
        unreachable!("test input is injected")
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|a, b| a == b)
    }

    fn value_serialize() -> impl ValueSerialize<Value = u64> {
        NoValueSerialize::new()
    }
}

#[derive(Allocative, Clone, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct OutputKey(usize);

fn outputs(version: u64, count: usize) -> ActionOutputs {
    let owner = BaseDeferredKey::TargetLabel(ConfiguredTargetLabel::testing_parse(
        "cell//pkg:target",
        ConfigurationData::testing_new(),
    ));
    let config = CasDigestConfig::testing_default();
    ActionOutputs::new(
        (0..count)
            .map(|i| {
                let path = BuildArtifactPath::new(
                    owner.dupe(),
                    ForwardRelativePathBuf::unchecked_new(format!("out/{i}")),
                    BuckOutPathKind::Configuration,
                );
                let value = ArtifactValue::file(FileMetadata {
                    digest: TrackedFileDigest::from_content(
                        format!("{version}-{i}").as_bytes(),
                        config,
                    ),
                    is_executable: false,
                });
                (path, value)
            })
            .collect(),
    )
}

#[async_trait]
impl Key for OutputKey {
    // Exercise BuildKey's actual value, equality and storage representation,
    // while controlling invalidation independently of action-cache behavior.
    type Value = <BuildKey as Key>::Value;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancel: &CancellationContext,
    ) -> Self::Value {
        ctx.per_transaction_data()
            .data
            .get::<Arc<Tracker>>()
            .unwrap()
            .values_computed
            .fetch_add(1, Ordering::SeqCst);
        let version = *ctx.compute(&Input).await? % 2;
        Ok(outputs(version, self.0).into())
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        BuildKey::equality_behavior()
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        BuildKey::value_serialize()
    }
}

#[derive(Allocative, Clone, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct Observer(usize);

#[async_trait]
impl Key for Observer {
    type Value = bool;

    async fn compute(&self, ctx: &mut DiceComputations, _cancel: &CancellationContext) -> bool {
        ctx.per_transaction_data()
            .data
            .get::<Arc<Tracker>>()
            .unwrap()
            .observers_computed
            .fetch_add(1, Ordering::SeqCst);
        ctx.compute(&OutputKey(self.0)).await.unwrap().is_ok()
    }

    fn equality_behavior() -> EqualityBehavior<bool> {
        EqualityBehavior::Compare(|a, b| a == b)
    }

    fn value_serialize() -> impl ValueSerialize<Value = bool> {
        NoValueSerialize::new()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_equality_after_eviction_does_not_restore_output_map() -> anyhow::Result<()> {
    check_output_equality(true, false, 1000).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_equality_with_eager_field_reads() -> anyhow::Result<()> {
    check_output_equality(false, false, 1000).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_demand_propagates_deferred_read_failure() -> anyhow::Result<()> {
    check_output_equality(true, true, 1000).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn small_output_demand_needs_no_deferred_read() -> anyhow::Result<()> {
    check_output_equality(true, true, 8).await
}

async fn check_output_equality(
    defer_reads: bool,
    drop_storage_before_demand: bool,
    count: usize,
) -> anyhow::Result<()> {
    let backing = InMemoryPagableStorage::new();
    let mut storage = dice::DiceStorage::new(backing.handle());
    if defer_reads {
        storage
            .storage_context()
            .get_or_init(|| DeferredFieldReadsEnabled);
    }
    storage.set_page_out_policy(PageOutPolicy::OncePerValue);
    storage.set_eviction_policy(EvictionPolicy::OnIdle(EvictionStrategy::Default));
    let mut builder = Dice::builder();
    builder.set_pagable_storage(storage);
    let dice = builder.build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let tracker = Arc::new(Tracker::default());
    let data = || {
        let mut data = UserComputationData::new();
        data.data.set(tracker.dupe());
        data.activation_tracker = Some(tracker.dupe());
        data
    };
    let mut updater = dice.updater_with_data(root, data());
    updater.changed_to([(Input, 1)])?;
    let tx = updater.commit().await;
    assert!(*tx.compute(&Observer(count)).await?);
    drop(tx);
    dice.wait_for_idle().await;
    dice.page_out().await?;
    assert_eq!(dice.pagable_node_counts().await.paged_out, 1);

    let before = dice.storage_io_metrics().unwrap();
    let mut updater = dice.updater_with_data(root, data());
    updater.changed_to([(Input, 3)])?;
    let tx = updater.commit().await;
    assert!(*tx.compute(&Observer(count)).await?);
    assert_eq!(tracker.values_computed.load(Ordering::SeqCst), 2);
    assert_eq!(tracker.observers_computed.load(Ordering::SeqCst), 1);
    assert_eq!(
        *tracker.phases.lock().unwrap(),
        [PageInPhase::AfterRecompute]
    );
    let after = dice.storage_io_metrics().unwrap();
    let deferred = defer_reads && count > 8;
    if deferred {
        assert_eq!(
            after.data_keys_in - before.data_keys_in,
            1,
            "equality should read only its fingerprint row: before={before:?} after={after:?}"
        );
        assert_eq!(after.bytes_in - before.bytes_in, 33);
    } else {
        assert!(after.data_keys_in - before.data_keys_in > count as u64);
    }
    let cached = tx.compute(&OutputKey(count)).await?.as_ref().unwrap();
    if drop_storage_before_demand {
        drop(backing);
        if !deferred {
            assert!(
                cached
                    .outputs()
                    .unwrap()
                    .iter()
                    .eq(outputs(1, count).iter())
            );
            return Ok(());
        }
        for _ in 0..2 {
            let error = cached
                .outputs()
                .expect_err("missing payload must remain an error");
            assert!(format!("{error:#}").contains("Failed to restore action outputs"));
        }
        return Ok(());
    }
    assert_eq!(cached.outputs().unwrap(), &outputs(1, count));
    assert!(
        cached
            .outputs()
            .unwrap()
            .iter()
            .eq(outputs(1, count).iter())
    );
    assert_eq!(cached.outputs().unwrap().iter().count(), count);
    let loaded = dice.storage_io_metrics().unwrap().data_keys_in;
    if deferred {
        assert!(loaded > after.data_keys_in + count as u64);
    }
    assert_eq!(cached.outputs().unwrap().iter().count(), count);
    assert_eq!(dice.storage_io_metrics().unwrap().data_keys_in, loaded);
    drop(tx);

    let mut updater = dice.updater_with_data(root, data());
    updater.changed_to([(Input, 2)])?;
    let tx = updater.commit().await;
    assert!(*tx.compute(&Observer(count)).await?);
    assert_eq!(tracker.values_computed.load(Ordering::SeqCst), 3);
    assert_eq!(tracker.observers_computed.load(Ordering::SeqCst), 2);
    Ok(())
}
