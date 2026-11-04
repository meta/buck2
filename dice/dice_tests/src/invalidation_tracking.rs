/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::InvalidationSourcePriority;
use dice::InvalidationSources;
use dice::Key;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

#[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct NormalInjected(u32);

#[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct HighInjected(u32);

#[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct NormalChanged(u32);

#[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct HighChanged(u32);

impl InjectedKey for NormalInjected {
    type Value = u32;
    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }
    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }
}

impl InjectedKey for HighInjected {
    type Value = u32;
    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }
    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }

    fn invalidation_source_priority() -> InvalidationSourcePriority {
        InvalidationSourcePriority::High
    }
}

#[async_trait]
impl Key for NormalChanged {
    type Value = u32;
    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        *ctx.compute(&NormalInjected(self.0)).await.unwrap()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }
}

#[async_trait]
impl Key for HighChanged {
    type Value = u32;
    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        *ctx.compute(&HighInjected(self.0)).await.unwrap()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }

    fn invalidation_source_priority() -> InvalidationSourcePriority {
        InvalidationSourcePriority::High
    }
}

#[derive(Allocative, Debug, Clone, Dupe)]
struct CapturedInvalidationSources(InvalidationSources);

/// Asserts the seqs of the most recent normal- and high-priority sources.
#[track_caller]
fn assert_invalidations(
    normal: Option<usize>,
    high: Option<usize>,
    actual: &CapturedInvalidationSources,
) {
    let seq = |v: Option<dice::VersionNumber>| v.map(|v| v.testing_value());
    assert_eq!(seq(actual.0.normal_priority()), normal, "normal priority");
    assert_eq!(seq(actual.0.high_priority()), high, "high priority");
}

#[test]
fn test_compute_tracks_invalidations() -> anyhow::Result<()> {
    // Detecting that a dice compute panicked is actually kinda tricky, in normal flow
    // that is a hard error but in tests it instead just looks to dice like the node is cancelled.
    // We detect it by configuring the runtime to shutdown and panic itself if any task panics, but
    // that only works right now with the current_thread runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .unhandled_panic(tokio::runtime::UnhandledPanic::ShutdownRuntime)
        .build()
        .unwrap();

    // Spawn the root task
    rt.block_on(async {
        let dice = {
            let builder = Dice::builder();
            builder.build(DetectCycles::Enabled)
        };
        let root = dice.new_root().await;

        // We want to set up the invalidation sources such that the FooInjected(n) are all invalidated at `2 * n + 2` (i.e. NormalInjected(4)
        // is invalidated at 10) and the FooChanged(n) forms are at `2 * n + 3`

        for i in 0..10 {
            let mut updater = dice.updater(root);
            updater.changed_to([(NormalInjected(i), i)])?;
            updater.changed_to([(HighInjected(i), i)])?;
            updater.commit().await;

            let mut updater = dice.updater(root);
            updater.changed([NormalChanged(i)])?;
            updater.changed([HighChanged(i)])?;
            updater.commit().await;
        }

        #[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
        #[pagable_typetag(DiceKeyDyn)]
        struct Top(u32);

        #[async_trait]
        impl Key for Top {
            type Value = CapturedInvalidationSources;
            fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
                dice::NoValueSerialize::<Self::Value>::new()
            }
            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellations: &CancellationContext,
            ) -> Self::Value {
                match self.0 {
                    0 => {}
                    1 => {
                        ctx.compute(&HighInjected(0)).await.unwrap();
                        ctx.compute(&NormalInjected(1)).await.unwrap();
                    }
                    2 => {
                        ctx.compute(&HighChanged(1)).await.unwrap();
                        ctx.compute(&NormalChanged(0)).await.unwrap();
                    }
                    3 => {
                        ctx.compute(&HighChanged(0)).await.unwrap();
                        ctx.compute2(
                            async |ctx| ctx.compute(&NormalInjected(1)).await.unwrap(),
                            async |ctx| ctx.compute(&NormalChanged(1)).await.unwrap(),
                        )
                        .await;
                    }
                    n if n < 8 => {
                        ctx.compute(&Top(n - 1)).await.unwrap();
                    }
                    k => {
                        panic!("don't request such a big key {k}")
                    }
                }

                CapturedInvalidationSources(ctx.get_invalidation_sources())
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|_x, _y| false)
            }
        }

        let ctx = dice.updater(root).commit().await;

        // Top(0) reads nothing.
        assert_invalidations(None, None, ctx.compute(&Top(0)).await?);
        // NormalInjected(1) was injected at 4, HighInjected(0) at 2.
        assert_invalidations(Some(4), Some(2), ctx.compute(&Top(1)).await?);
        // HighChanged(1) was dirtied at 5, which outranks NormalChanged(0) at 3 for both.
        assert_invalidations(Some(5), Some(5), ctx.compute(&Top(2)).await?);
        // NormalChanged(1) at 5 is the most recent source, HighChanged(0) at 3 the most recent
        // high-priority one.
        assert_invalidations(Some(5), Some(3), ctx.compute(&Top(3)).await?);
        // Sources propagate unchanged through a chain of dependents.
        assert_invalidations(Some(5), Some(3), ctx.compute(&Top(7)).await?);

        Ok(())
    })
}

#[test]
fn test_compute_tracks_invalidations_over_versions() -> anyhow::Result<()> {
    // Detecting that a dice compute panicked is actually kinda tricky, in normal flow
    // that is a hard error but in tests it instead just looks to dice like the node is cancelled.
    // We detect it by configuring the runtime to shutdown and panic itself if any task panics, but
    // that only works right now with the current_thread runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .unhandled_panic(tokio::runtime::UnhandledPanic::ShutdownRuntime)
        .build()
        .unwrap();

    // Spawn the root task
    rt.block_on(async {
        let dice = {
            let builder = Dice::builder();
            builder.build(DetectCycles::Enabled)
        };
        let root = dice.new_root().await;

        #[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
        #[pagable_typetag(DiceKeyDyn)]
        struct Top(u32);

        #[async_trait]
        impl Key for Top {
            type Value = CapturedInvalidationSources;
            fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
                dice::NoValueSerialize::<Self::Value>::new()
            }
            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellations: &CancellationContext,
            ) -> Self::Value {
                ctx.compute(&HighChanged(self.0)).await.unwrap();

                CapturedInvalidationSources(ctx.get_invalidation_sources())
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|_x, _y| false)
            }
        }

        let mut updater = dice.updater(root);
        updater.changed_to([(HighInjected(0), 0)])?;
        updater.commit().await;

        let ctx = dice.updater(root).commit().await;
        assert_invalidations(Some(2), Some(2), ctx.compute(&Top(0)).await?);

        let mut updater = dice.updater(root);
        updater.changed_to([(HighInjected(0), 1)])?;
        updater.commit().await;

        let ctx = dice.updater(root).commit().await;
        assert_invalidations(Some(3), Some(3), ctx.compute(&Top(0)).await?);

        let mut updater = dice.updater(root);
        updater.changed([HighChanged(0)])?;
        updater.commit().await;

        let ctx = dice.updater(root).commit().await;
        // The dirty of HighChanged(0) at 4 is now the most recent source; the injected key's
        // change at 3 is still in force but older.
        assert_invalidations(Some(4), Some(4), ctx.compute(&Top(0)).await?);

        Ok(())
    })
}

/// Sources describe the derivation of the value being computed, not the reason for its
/// recomputation: a change to an early dep that stops the compute from reading a later one
/// removes that dep's source from the result, even at a higher priority.
#[test]
fn sources_describe_the_new_derivation() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .unhandled_panic(tokio::runtime::UnhandledPanic::ShutdownRuntime)
        .build()
        .unwrap();

    rt.block_on(async {
        let dice = {
            let builder = Dice::builder();
            builder.build(DetectCycles::Enabled)
        };
        let root = dice.new_root().await;

        #[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
        #[pagable_typetag(DiceKeyDyn)]
        struct Switch;

        #[async_trait]
        impl Key for Switch {
            type Value = CapturedInvalidationSources;
            fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
                dice::NoValueSerialize::<Self::Value>::new()
            }
            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellations: &CancellationContext,
            ) -> Self::Value {
                if *ctx.compute(&NormalInjected(0)).await.unwrap() == 0 {
                    ctx.compute(&HighChanged(0)).await.unwrap();
                }
                CapturedInvalidationSources(ctx.get_invalidation_sources())
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|_x, _y| false)
            }
        }

        let mut updater = dice.updater(root);
        updater.changed_to([(NormalInjected(0), 0)])?;
        updater.changed_to([(HighInjected(0), 0)])?;
        updater.commit().await;
        let mut updater = dice.updater(root);
        updater.changed([HighChanged(0)])?;
        updater.commit().await;

        // At 3 the switch reads both: the dirty of HighChanged(0) at 3 is the most recent
        // source at either priority.
        let ctx = dice.updater(root).commit().await;
        assert_invalidations(Some(3), Some(3), ctx.compute(&Switch).await?);

        // The change at 4 makes the switch skip HighChanged(0). The recompute depends on no
        // high-priority source any more, though one caused the previous value to be read and is
        // still in force.
        let mut updater = dice.updater(root);
        updater.changed_to([(NormalInjected(0), 1)])?;
        updater.commit().await;
        let ctx = dice.updater(root).commit().await;
        assert_invalidations(Some(4), None, ctx.compute(&Switch).await?);

        Ok(())
    })
}
