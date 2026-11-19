/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Demand recovery through the public DICE API, including cancellation and version isolation.

use std::sync::atomic::AtomicBool;

use dice::BranchId;
use dice::DiceProjectionComputations;
use dice::DiceProjectionDyn;
use dice::InjectedKey;
use dice::NoValueSerialize;
use dice::ProjectionKey;
use tokio::sync::Notify;
use tokio::sync::Semaphore;

use super::*;

struct RecoveryControl {
    computes: [AtomicUsize; 3],
    block: AtomicBool,
    transient: AtomicBool,
    started: Notify,
    release: Semaphore,
}

impl RecoveryControl {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            computes: std::array::from_fn(|_| AtomicUsize::new(0)),
            block: AtomicBool::new(false),
            transient: AtomicBool::new(false),
            started: Notify::new(),
            release: Semaphore::new(0),
        })
    }

    fn data(self: &Arc<Self>) -> UserComputationData {
        let mut data = UserComputationData::new();
        data.data.set(self.clone());
        data
    }

    fn count(&self, key: usize) -> usize {
        self.computes[key].load(Ordering::SeqCst)
    }
}

#[derive(Allocative, Clone, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct RecoveryInput(usize);

impl InjectedKey for RecoveryInput {
    type Value = u64;

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = u64> {
        NoValueSerialize::new()
    }
}

#[derive(Allocative, Clone, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct RecoveryKey(usize);

#[async_trait]
impl Key for RecoveryKey {
    type Value = u64;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellations: &CancellationContext,
    ) -> u64 {
        let control = ctx
            .per_transaction_data()
            .data
            .get::<Arc<RecoveryControl>>()
            .expect("test computation has recovery controls")
            .clone();
        control.computes[self.0].fetch_add(1, Ordering::SeqCst);
        let value = if self.0 == 0 {
            *ctx.compute(&RecoveryInput(0))
                .await
                .expect("input is injected")
        } else {
            *ctx.compute(&RecoveryKey(self.0 - 1))
                .await
                .expect("the dependency should recover")
                + 1
        };
        if self.0 == 0 && control.block.load(Ordering::SeqCst) {
            let critical = cancellations.enter_critical_section();
            control.started.notify_one();
            control
                .release
                .acquire()
                .await
                .expect("test gate remains open")
                .forget();
            critical.exit_critical_section().await;
        }
        if control.transient.load(Ordering::SeqCst) {
            0
        } else {
            value
        }
    }

    fn validity(value: &u64) -> bool {
        *value != 0
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = u64> {
        FailToHydrateSerialize::<false>
    }
}

async fn recovery_dice()
-> anyhow::Result<(tempfile::TempDir, Arc<Dice>, BranchId, Arc<RecoveryControl>)> {
    let tmp = tempdir()?;
    let mut builder = Dice::builder();
    builder.set_pagable_storage(test_storage(tmp.path())?);
    let dice = builder.build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let control = RecoveryControl::new();
    let mut updater = dice.updater_with_data(root, control.data());
    updater.changed_to([(RecoveryInput(0), 7), (RecoveryInput(1), 1)])?;
    drop(updater.commit().await);
    Ok((tmp, dice, root, control))
}

async fn paged_out(
    key: usize,
) -> anyhow::Result<(tempfile::TempDir, Arc<Dice>, BranchId, Arc<RecoveryControl>)> {
    let (tmp, dice, root, control) = recovery_dice().await?;
    let tx = dice.updater_with_data(root, control.data()).commit().await;
    assert_eq!(*tx.compute(&RecoveryKey(key)).await?, 7 + key as u64);
    drop(tx);
    dice.wait_for_idle().await;
    dice.page_out().await?;
    assert_eq!(dice.pagable_status().await.paged_out_count, key + 1);
    Ok((tmp, dice, root, control))
}

#[tokio::test]
async fn failed_demand_repairs_the_graph_without_requesting_restart() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(0).await?;
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut data = control.data();
    data.tracker = Arc::new(CapturingListener {
        hydration_failures: captured.clone(),
    });
    let tx = dice.updater_with_data(root, data).commit().await;
    let value = tx.compute(&RecoveryKey(0)).await?;
    assert_eq!(*value, 7);
    assert_eq!(control.count(0), 2);
    let overlapping = dice.updater_with_data(root, control.data()).commit().await;
    let same = overlapping.compute(&RecoveryKey(0)).await?;
    assert!(
        std::ptr::eq(value, same),
        "overlapping transactions share the repair"
    );
    assert_eq!(control.count(0), 2);
    drop((tx, overlapping));
    dice.wait_for_idle().await;

    let tx = dice.updater_with_data(root, control.data()).commit().await;
    assert_eq!(*tx.compute(&RecoveryKey(0)).await?, 7);
    assert_eq!(
        control.count(0),
        2,
        "a new cache must use the repaired graph"
    );
    assert_eq!(captured.lock().unwrap().len(), 1);
    assert!(
        dice.pagable_storage_context()
            .unwrap()
            .get_or_init(DeferredReadFailures::default)
            .snapshot()
            .is_empty(),
        "a recovered demand must not poison the daemon"
    );
    Ok(())
}

#[tokio::test]
async fn another_waiter_keeps_shared_recovery_running() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(0).await?;
    let first_tx = dice.updater_with_data(root, control.data()).commit().await;
    let second_tx = dice.updater_with_data(root, control.data()).commit().await;
    control.block.store(true, Ordering::SeqCst);
    let mut first = Box::pin(first_tx.compute(&RecoveryKey(0)));
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut first => panic!("recovery must wait for the gate: {result:?}"),
            _ = control.started.notified() => {}
        }
    })
    .await?;
    let mut second = Box::pin(second_tx.compute(&RecoveryKey(0)));
    assert!(futures::poll!(&mut second).is_pending());
    drop(first);
    control.block.store(false, Ordering::SeqCst);
    control.release.add_permits(1);
    assert_eq!(
        *tokio::time::timeout(Duration::from_secs(10), second).await??,
        7
    );
    assert_eq!(
        control.count(0),
        2,
        "dropping one waiter must not cancel shared work"
    );
    Ok(())
}

#[tokio::test]
async fn cancelled_recovery_can_restart_in_the_same_transaction() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(0).await?;
    let tx = dice.updater_with_data(root, control.data()).commit().await;
    control.block.store(true, Ordering::SeqCst);
    let mut abandoned = Box::pin(tx.compute(&RecoveryKey(0)));
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut abandoned => panic!("recovery must wait for the gate: {result:?}"),
            _ = control.started.notified() => {}
        }
    })
    .await?;
    drop(abandoned);
    let mut retry = Box::pin(tx.compute(&RecoveryKey(0)));
    assert!(futures::poll!(&mut retry).is_pending());
    assert_eq!(
        control.count(0),
        2,
        "the next generation must wait for the critical section"
    );
    control.block.store(false, Ordering::SeqCst);
    control.release.add_permits(1);
    assert_eq!(
        *tokio::time::timeout(Duration::from_secs(10), retry).await??,
        7
    );
    assert_eq!(
        control.count(0),
        3,
        "the cancelled generation is replaced once"
    );
    Ok(())
}

#[tokio::test]
async fn recovery_is_drained_after_the_transaction_is_dropped() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(0).await?;
    let tx = dice.updater_with_data(root, control.data()).commit().await;
    control.block.store(true, Ordering::SeqCst);
    let request = tokio::spawn(async move { tx.compute(&RecoveryKey(0)).await.copied() });
    tokio::time::timeout(Duration::from_secs(10), control.started.notified()).await?;
    request.abort();
    assert!(
        request
            .await
            .expect_err("the caller was aborted")
            .is_cancelled()
    );
    assert!(
        !dice.is_idle().await,
        "recovery is still inside its critical section"
    );
    let mut idle = Box::pin(dice.wait_for_idle());
    assert!(futures::poll!(&mut idle).is_pending());
    control.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), idle).await?;
    assert!(dice.is_idle().await);
    assert_eq!(control.count(0), 2);
    Ok(())
}

#[tokio::test]
async fn nested_read_failures_recompute_each_dependency_once() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(1).await?;
    let tx = dice.updater_with_data(root, control.data()).commit().await;
    assert_eq!(*tx.compute(&RecoveryKey(1)).await?, 8);
    assert_eq!(*tx.compute(&RecoveryKey(0)).await?, 7);
    assert_eq!((control.count(0), control.count(1)), (2, 2));
    assert_eq!(dice.pagable_status().await.paged_out_count, 0);
    Ok(())
}

#[tokio::test]
async fn recovery_uses_the_requesting_transactions_version() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(0).await?;
    let old = dice.updater_with_data(root, control.data()).commit().await;
    control.block.store(true, Ordering::SeqCst);
    let mut recovering = Box::pin(old.compute(&RecoveryKey(0)));
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut recovering => panic!("old-version recovery must reach the gate: {result:?}"),
            _ = control.started.notified() => {}
        }
    }).await?;
    control.block.store(false, Ordering::SeqCst);
    let mut updater = dice.updater_with_data(root, control.data());
    updater.changed_to([(RecoveryInput(0), 11)])?;
    let new = updater.commit().await;
    assert_ne!(old.version(), new.version());
    assert_eq!(*new.compute(&RecoveryKey(0)).await?, 11);
    control.release.add_permits(1);
    assert_eq!(
        *tokio::time::timeout(Duration::from_secs(10), recovering).await??,
        7
    );
    assert_eq!(*new.compute(&RecoveryKey(0)).await?, 11);
    assert_eq!(control.count(0), 3);
    Ok(())
}

#[tokio::test]
async fn unchanged_value_recovers_independently_in_a_new_version() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(0).await?;
    let old = dice.updater_with_data(root, control.data()).commit().await;
    control.block.store(true, Ordering::SeqCst);
    let mut recovering = Box::pin(old.compute(&RecoveryKey(0)));
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::select! {
            result = &mut recovering => panic!("old recovery must wait at the gate: {result:?}"),
            _ = control.started.notified() => {}
        }
    })
    .await?;

    // Advance the version without changing this key or its dependency. Both versions
    // reuse the same paged-out graph value, but must not share its recovery task.
    control.block.store(false, Ordering::SeqCst);
    let mut updater = dice.updater_with_data(root, control.data());
    updater.changed_to([(RecoveryInput(1), 2)])?;
    let new = updater.commit().await;
    assert_ne!(old.version(), new.version());
    assert_eq!(
        *tokio::time::timeout(Duration::from_secs(10), new.compute(&RecoveryKey(0))).await??,
        7
    );
    assert!(futures::poll!(&mut recovering).is_pending());
    assert_eq!(control.count(0), 3, "each cache started its own recovery");

    control.release.add_permits(1);
    assert_eq!(
        *tokio::time::timeout(Duration::from_secs(10), recovering).await??,
        7
    );
    assert_eq!(*new.compute(&RecoveryKey(0)).await?, 7);
    assert_eq!(control.count(0), 3);
    Ok(())
}

/// Drop the last transaction while its cancelled recovery is still in a critical section.
/// A replacement cache must recover independently, even if its version number is unchanged.
async fn recover_while_old_cache_drains(advance_version: bool) -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(0).await?;
    let old = dice.updater_with_data(root, control.data()).commit().await;
    let old_version = old.version();
    control.block.store(true, Ordering::SeqCst);
    let request = tokio::spawn(async move { old.compute(&RecoveryKey(0)).await.copied() });
    tokio::time::timeout(Duration::from_secs(10), control.started.notified()).await?;
    request.abort();
    assert!(
        request
            .await
            .expect_err("the caller was aborted")
            .is_cancelled()
    );
    assert!(!dice.is_idle().await, "the old recovery is still draining");

    control.block.store(false, Ordering::SeqCst);
    let mut updater = dice.updater_with_data(root, control.data());
    if advance_version {
        updater.changed_to([(RecoveryInput(1), 2)])?;
    }
    let new = updater.commit().await;
    assert_eq!(old_version == new.version(), !advance_version);
    assert_eq!(
        *tokio::time::timeout(Duration::from_secs(10), new.compute(&RecoveryKey(0))).await??,
        7
    );
    assert_eq!(
        control.count(0),
        3,
        "the new cache started its own recovery"
    );
    drop(new);
    assert!(!dice.is_idle().await, "the old recovery was not released");

    control.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), dice.wait_for_idle()).await?;
    assert!(dice.is_idle().await);
    assert_eq!(control.count(0), 3);
    Ok(())
}

#[tokio::test]
async fn new_version_does_not_wait_for_cancelled_recovery() -> anyhow::Result<()> {
    recover_while_old_cache_drains(true).await
}

#[tokio::test]
async fn new_cache_at_the_same_version_does_not_wait_for_cancelled_recovery() -> anyhow::Result<()>
{
    recover_while_old_cache_drains(false).await
}

#[tokio::test]
async fn a_transient_recovery_is_shared_but_not_written_as_valid() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = paged_out(0).await?;
    control.transient.store(true, Ordering::SeqCst);
    let tx = dice.updater_with_data(root, control.data()).commit().await;
    assert_eq!(*tx.compute(&RecoveryKey(0)).await?, 0);
    assert_eq!(*tx.compute(&RecoveryKey(0)).await?, 0);
    assert_eq!(control.count(0), 2);
    drop(tx);
    dice.wait_for_idle().await;

    control.transient.store(false, Ordering::SeqCst);
    let tx = dice.updater_with_data(root, control.data()).commit().await;
    assert_eq!(*tx.compute(&RecoveryKey(0)).await?, 7);
    assert_eq!(
        control.count(0),
        3,
        "a transient result must not repair the core graph"
    );
    Ok(())
}

/// Changes to input 1 invalidate parents, but retain this dependency's value and revision
/// while the input stays odd. Validation must then check the other dependency too.
#[derive(Allocative, Clone, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct ValidationTrigger;

#[async_trait]
impl Key for ValidationTrigger {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _cancel: &CancellationContext) -> u64 {
        ctx.compute(&RecoveryInput(1)).await.expect("trigger input") % 2
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = u64> {
        NoValueSerialize::new()
    }
}

#[derive(Allocative, Clone, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceProjectionDyn)]
struct RecoveryProjection;

impl ProjectionKey for RecoveryProjection {
    type DeriveFromKey = RecoveryKey;
    type Value = u64;

    fn compute(&self, base: &u64, _ctx: &DiceProjectionComputations) -> u64 {
        base * 10
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = u64> {
        NoValueSerialize::new()
    }
}

#[derive(Allocative, Clone, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
enum RecoveryParent {
    Direct,
    Projected,
}

#[async_trait]
impl Key for RecoveryParent {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _cancel: &CancellationContext) -> u64 {
        ctx.per_transaction_data()
            .data
            .get::<Arc<RecoveryControl>>()
            .expect("recovery controls")
            .computes[2]
            .fetch_add(1, Ordering::SeqCst);
        ctx.compute(&ValidationTrigger)
            .await
            .expect("validation trigger");
        match self {
            Self::Direct => *ctx
                .compute(&RecoveryKey(0))
                .await
                .expect("recoverable dependency"),
            Self::Projected => {
                let base = ctx
                    .compute_opaque(&RecoveryKey(0))
                    .await
                    .expect("recoverable base");
                ctx.projection(&base, &RecoveryProjection)
                    .expect("projection")
            }
        }
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = u64> {
        NoValueSerialize::new()
    }
}

async fn check_recovery_metadata(parent: RecoveryParent, original: u64) -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = recovery_dice().await?;
    let tx = dice.updater_with_data(root, control.data()).commit().await;
    assert_eq!(*tx.compute(&parent).await?, original);
    drop(tx);
    dice.wait_for_idle().await;
    dice.page_out().await?;
    assert_eq!(dice.pagable_status().await.paged_out_count, 1);

    let mut updater = dice.updater_with_data(root, control.data());
    updater.changed_to([(RecoveryInput(1), 3)])?;
    let tx = updater.commit().await;
    control.transient.store(true, Ordering::SeqCst);
    assert_eq!(*tx.compute(&RecoveryKey(0)).await?, 0);
    assert_eq!(
        *tx.compute(&parent).await?,
        0,
        "dependency validation must observe recovery's transient result, not the old revision"
    );
    assert_eq!(control.count(0), 2);
    assert_eq!(control.count(2), 2, "the stale parent must recompute");
    Ok(())
}

#[tokio::test]
async fn recovery_metadata_reaches_later_dependency_checks() -> anyhow::Result<()> {
    check_recovery_metadata(RecoveryParent::Direct, 7).await
}

#[tokio::test]
async fn recovery_metadata_reaches_later_projection_dependency_checks() -> anyhow::Result<()> {
    check_recovery_metadata(RecoveryParent::Projected, 70).await
}

#[tokio::test]
async fn recovery_preserves_a_force_dirtied_keys_untracked_input_revision() -> anyhow::Result<()> {
    let (_tmp, dice, root, control) = recovery_dice().await?;
    let mut updater = dice.updater_with_data(root, control.data());
    updater.changed([RecoveryKey(0)])?;
    let tx = updater.commit().await;
    assert_eq!(*tx.compute(&RecoveryKey(0)).await?, 7);
    drop(tx);
    dice.wait_for_idle().await;
    dice.page_out().await?;
    let tx = dice.updater_with_data(root, control.data()).commit().await;
    assert_eq!(*tx.compute(&RecoveryKey(0)).await?, 7);
    assert_eq!(control.count(0), 2);
    Ok(())
}
