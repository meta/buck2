/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::OnceLock;

use allocative::Allocative;
use dupe::Dupe;

use crate::arc::Arc;
use crate::introspection::DiceTaskState;
use crate::key::DiceKey;
use crate::value::MaybeResidentComputedValue;

#[derive(Allocative)]
#[allocative(skip)]
pub(crate) struct ProjectionTask {
    pub(crate) key: DiceKey,
    /// Set once the projection's `compute` has finished running, before the completing thread
    /// does its core state roundtrip.
    ///
    /// This exists for `wait_computed`, which must be callable from the core state thread itself:
    /// waiting on `value` there would deadlock, because setting `value` requires a response from
    /// the core state thread.
    computed: OnceLock<()>,
    /// Set once the computation has finished entirely, to `None` if it panicked instead.
    ///
    /// This includes the roundtrip to the core state - the thread performing the compute blocks
    /// on the core state's response before completing the task with it.
    value: OnceLock<Option<MaybeResidentComputedValue>>,
}

/// The handle given to the thread responsible for performing and completing the computation.
///
/// This type should be treated as linear - the caller absolutely must take the handle and
/// `complete` it. Only a panic in the computation drops one uncompleted, and that poisons the
/// task: everyone waiting on it panics too, rather than waiting forever.
pub(crate) struct ProjectionTaskCompletionHandle(Option<Arc<ProjectionTask>>);

impl ProjectionTask {
    #[cfg(test)]
    pub(crate) fn prepare_testing(
        key: DiceKey,
    ) -> (Arc<ProjectionTask>, ProjectionTaskCompletionHandle) {
        let mut t = None;
        let h = Self::prepare::<()>(key, |task| {
            t = Some(task.dupe());
            Ok(Box::leak(Box::new(task)).as_ref())
        })
        .unwrap();
        (t.unwrap(), h)
    }

    /// Prepare a new projection task
    ///
    /// This would not need the callback API, but it's kept for consistency with the non-projection
    /// analogue.
    pub(crate) fn prepare<'d, E>(
        key: DiceKey,
        alloc: impl FnOnce(Arc<ProjectionTask>) -> Result<&'d ProjectionTask, E>,
    ) -> Result<ProjectionTaskCompletionHandle, E> {
        let t = Arc::new(Self {
            key,
            computed: OnceLock::new(),
            value: OnceLock::new(),
        });
        // We don't actually need the return value but we keep it for consistency with the
        // non-projection API
        _ = alloc(t.dupe())?;
        Ok(ProjectionTaskCompletionHandle(Some(t)))
    }

    /// Read the finished value if it's available. Panics if the computation panicked.
    pub(crate) fn try_read(&self) -> Option<&'_ MaybeResidentComputedValue> {
        self.value.get().map(Self::unpoisoned)
    }

    fn unpoisoned(value: &Option<MaybeResidentComputedValue>) -> &MaybeResidentComputedValue {
        value
            .as_ref()
            .expect("the projection's computation panicked")
    }

    fn insert_computed(
        this: Arc<Self>,
        result: MaybeResidentComputedValue,
    ) -> MaybeResidentComputedValue {
        let _ignored = this.computed.set(());
        assert!(
            this.value.set(Some(result.dupe())).is_ok(),
            "a projection task is completed once"
        );
        result
    }

    fn poison(&self) {
        let _ignored = self.computed.set(());
        let _ignored = self.value.set(None);
    }

    /// Waits for the finished value. Panics if the computation panicked.
    pub(crate) fn wait_sync(&self) -> MaybeResidentComputedValue {
        Self::unpoisoned(self.value.wait()).dupe()
    }

    /// Waits until the projection's `compute` has finished running, but not necessarily its core
    /// state roundtrip.
    ///
    /// Unlike `wait_sync`, this is safe to call from the core state thread.
    pub(crate) fn wait_computed(&self) {
        self.computed.wait();
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.value.get().is_none()
    }

    pub(crate) fn introspect_state(&self) -> DiceTaskState {
        if self.is_pending() {
            DiceTaskState::InProgress
        } else {
            DiceTaskState::Ready
        }
    }
}

impl ProjectionTaskCompletionHandle {
    /// Reports that the projection's `compute` has finished running.
    ///
    /// Must be called before the completing thread blocks on the core state.
    pub(crate) fn compute_finished(&self) {
        let _ignored = self.0.as_ref().unwrap().computed.set(());
    }

    pub(crate) fn complete(
        mut self,
        result: MaybeResidentComputedValue,
    ) -> MaybeResidentComputedValue {
        ProjectionTask::insert_computed(self.0.take().unwrap(), result)
    }
}

impl Drop for ProjectionTaskCompletionHandle {
    fn drop(&mut self) {
        if let Some(t) = self.0.take() {
            t.poison();
            debug_assert!(
                std::thread::panicking(),
                "a projection completion handle must be completed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use async_trait::async_trait;
    use derive_more::Display;
    use dice_futures::cancellation::CancellationContext;
    use dupe::Dupe;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use super::ProjectionTask;
    use crate::DiceKeyDyn;
    use crate::api::computations::DiceComputations;
    use crate::api::invalidation_tracking::InvalidationSources;
    use crate::api::key::Key;
    use crate::api::key::NoValueSerialize;
    use crate::api::key::ValueSerialize;
    use crate::core::graph::revision::Revision;
    use crate::key::DiceKey;
    use crate::value::DiceKeyValue;
    use crate::value::DiceValidValue;
    use crate::value::MaybeResidentComputedValue;
    use crate::value::MaybeValidDiceValue;

    #[derive(Allocative, Clone, Dupe, Debug, Display, Eq, PartialEq, Hash, Pagable)]
    #[pagable_typetag(DiceKeyDyn)]
    struct K;

    #[async_trait]
    impl Key for K {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            unimplemented!("test")
        }

        fn equality_behavior() -> crate::EqualityBehavior<Self::Value> {
            crate::EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    fn computed(val: usize) -> MaybeResidentComputedValue {
        MaybeResidentComputedValue::new_resident(
            MaybeValidDiceValue::valid(DiceValidValue::testing_new(DiceKeyValue::<K>::new(val))),
            InvalidationSources::NONE,
            Revision::FIRST,
        )
    }

    fn is_val(v: &MaybeResidentComputedValue, val: usize) -> bool {
        v.testing_resident_value()
            .equality(&DiceValidValue::testing_new(DiceKeyValue::<K>::new(val)))
    }

    /// The winner of the insertion computes the value once; that value is handed back to it and to
    /// everyone else waiting on the same projection.
    #[tokio::test]
    async fn insert_computed_delivers_value_to_winner_and_waiters() -> anyhow::Result<()> {
        let (task, handle) = ProjectionTask::prepare_testing(DiceKey { index: 100 });

        assert!(
            task.try_read().is_none(),
            "the value is unavailable before completion"
        );

        // The compute-finished flag becomes visible before the value does.
        handle.compute_finished();
        task.wait_computed();
        assert!(
            task.try_read().is_none(),
            "compute_finished does not make the value available"
        );

        // Callers that ask for the value before it has been computed block until it lands.
        let waiters: Vec<_> = (0..3)
            .map(|_| {
                let task = task.dupe();
                tokio::task::spawn_blocking(move || task.wait_sync())
            })
            .collect();
        // Give the waiters a chance to reach their blocking point before the value is inserted.
        tokio::task::yield_now().await;

        let returned = handle.complete(computed(2));
        assert!(
            is_val(&returned, 2),
            "insert_computed returns the computed value"
        );
        assert!(
            is_val(task.try_read().unwrap(), 2),
            "the value is available immediately after completion"
        );

        for waiter in waiters {
            assert!(
                is_val(&waiter.await?, 2),
                "every waiter observes the computed value"
            );
        }

        Ok(())
    }
}
