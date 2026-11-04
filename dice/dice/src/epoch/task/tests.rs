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
use std::sync::Barrier as ThreadBarrier;
use std::task::Poll;
use std::thread;

use allocative::Allocative;
use assert_matches::assert_matches;
use async_trait::async_trait;
use derive_more::Display;
use dice_futures::cancellation::CancellationContext;
use dice_futures::spawner::TokioSpawner;
use dupe::Dupe;
use futures::FutureExt;
use futures::pin_mut;
use futures::poll;
use pagable::Pagable;
use pagable::pagable_typetag;
use tokio::runtime::Builder;
use tokio::sync::Barrier;
use tokio::sync::Mutex;
use tokio::sync::Notify;

use crate::DiceKeyDyn;
use crate::EqualityBehavior;
use crate::api::computations::DiceComputations;
use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::arc::Arc;
use crate::core::graph::revision::Revision;
use crate::epoch::task::dice::DiceTask;
use crate::epoch::task::dice::DiceTaskDependedOnByResult;
use crate::epoch::task::dice::spawn_prepared_task;
use crate::epoch::task::spawn_dice_task;
use crate::key::DiceKey;
use crate::key::ParentKey;
use crate::value::DiceKeyValue;
use crate::value::DiceValidValue;
use crate::value::MaybeResidentComputedValue;
use crate::value::MaybeValidDiceValue;
use crate::value::ResidentComputedValue;

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

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[tokio::test]
async fn simple_task() -> anyhow::Result<()> {
    let lock = Arc::new(Mutex::new(()));

    let lock_dupe = lock.dupe();
    let locked = lock_dupe.lock().await;

    let (task, _initial_promise) =
        spawn_dice_task(DiceKey { index: 10 }, &TokioSpawner, &(), |handle| {
            async move {
                // wait for the lock too
                let _lock = lock.lock().await;

                handle.finished(Ok(MaybeResidentComputedValue::new_resident(
                    MaybeValidDiceValue::valid(DiceValidValue::testing_new(
                        DiceKeyValue::<K>::new(2),
                    )),
                    InvalidationSources::NONE,
                    Revision::FIRST,
                )));
            }
            .boxed()
        });

    assert!(task.is_pending());

    let promise = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 1 }))
        .unwrap();
    pin_mut!(promise);

    assert_eq!(task.waiters_count(), 2);

    let polled = futures::poll!(&mut promise);
    assert!(
        !polled.is_ready(),
        "lock should be held so shouldn't be ready"
    );

    drop(locked);

    // since test runtime is one thread, dropping the lock shouldn't cause the task to complete yet
    let polled = futures::poll!(&mut promise);
    assert!(
        !polled.is_ready(),
        "lock should be held so shouldn't be ready"
    );

    // now await on the task, and see that we wake up and complete

    let v = promise.await?;
    assert!(
        v.testing_resident_value()
            .equality(&DiceValidValue::testing_new(DiceKeyValue::<K>::new(2)))
    );

    assert!(!task.is_pending());

    Ok(())
}

#[tokio::test]
async fn waiting_keys_are_the_dependents_that_polled_while_pending() {
    let (task, _initial_promise) =
        spawn_dice_task(DiceKey { index: 10 }, &TokioSpawner, &(), |handle| {
            async move {
                let _handle = handle;
                std::future::pending().await
            }
            .boxed()
        });
    let waiting = || {
        let mut keys = Vec::new();
        task.as_ref().for_each_waiting_key(|k| keys.push(k.index));
        keys.sort();
        keys
    };

    let mut p1 = Box::pin(
        task.depended_on_by(ParentKey::Some(DiceKey { index: 1 }))
            .unwrap(),
    );
    let mut p2 = Box::pin(
        task.depended_on_by(ParentKey::Some(DiceKey { index: 2 }))
            .unwrap(),
    );
    let _never_polled = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 3 }))
        .unwrap();
    let mut external = Box::pin(task.depended_on_by(ParentKey::None).unwrap());
    assert_eq!(waiting(), Vec::<u32>::new(), "nothing has polled yet");

    assert!(poll!(&mut p1).is_pending());
    assert!(poll!(&mut p2).is_pending());
    assert!(poll!(&mut external).is_pending());
    assert_eq!(waiting(), vec![1, 2]);

    drop(p1);
    assert_eq!(waiting(), vec![2]);
}

#[tokio::test]
async fn not_ready_until_dropped() -> anyhow::Result<()> {
    let sent_finish = std::sync::Arc::new(Notify::new());
    let can_terminate = std::sync::Arc::new(Notify::new());

    let (task, _initial_promise) =
        spawn_dice_task(DiceKey { index: 500 }, &TokioSpawner, &(), |handle| {
            let sent_finish = sent_finish.dupe();
            let can_terminate = can_terminate.dupe();
            async move {
                // wait for the lock too
                handle.finished(Ok(MaybeResidentComputedValue::new_resident(
                    MaybeValidDiceValue::valid(DiceValidValue::testing_new(
                        DiceKeyValue::<K>::new(1),
                    )),
                    InvalidationSources::NONE,
                    Revision::FIRST,
                )));

                sent_finish.notify_one();

                can_terminate.notified().await;
            }
            .boxed()
        });

    let promise = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 1 }))
        .unwrap();
    pin_mut!(promise);

    assert_eq!(task.waiters_count(), 2);

    sent_finish.notified().await;

    assert!(!task.is_ready());
    assert!(!task.is_cancelled());
    assert!(task.is_pending());

    assert_matches!(poll!(&mut promise), Poll::Pending);

    can_terminate.notify_one();

    let v = promise.await?;

    assert!(task.is_ready());
    assert!(!task.is_cancelled());
    assert!(!task.is_pending());

    assert!(
        v.testing_resident_value()
            .equality(&DiceValidValue::testing_new(DiceKeyValue::<K>::new(1)))
    );

    // check that the references have been dropped
    assert_eq!(std::sync::Arc::strong_count(&can_terminate), 1);
    assert_eq!(std::sync::Arc::strong_count(&sent_finish), 1);

    Ok(())
}

#[tokio::test]
async fn never_ready_results_in_terminated() -> anyhow::Result<()> {
    let (task, initial_promise) =
        spawn_dice_task(DiceKey { index: 500 }, &TokioSpawner, &(), |handle| {
            async move {
                let _handle = handle;
                // never report ready
            }
            .boxed()
        });

    let promise = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 1 }))
        .unwrap();

    drop(initial_promise);

    assert_eq!(task.waiters_count(), 1);

    // The worker terminated without ever reporting a value. A dependent can no longer observe that
    // as a result (that would be a `WorkerCancelled`, and holding a `DicePromise` is supposed to
    // prevent worker cancellation), so we drop the promise and observe termination directly.
    drop(promise);
    task.as_ref().await_termination().await;

    assert!(!task.is_ready());
    assert!(task.is_cancelled());
    assert!(!task.is_pending());

    Ok(())
}

#[tokio::test]
async fn multiple_promises_all_completes() -> anyhow::Result<()> {
    let (task, initial_promise) =
        spawn_dice_task(DiceKey { index: 20 }, &TokioSpawner, &(), |handle| {
            async move {
                // wait for the lock too
                handle.finished(Ok(MaybeResidentComputedValue::new_resident(
                    MaybeValidDiceValue::valid(DiceValidValue::testing_new(
                        DiceKeyValue::<K>::new(2),
                    )),
                    InvalidationSources::NONE,
                    Revision::FIRST,
                )));
            }
            .boxed()
        });

    let promise1 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 1 }))
        .unwrap();
    let promise2 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 2 }))
        .unwrap();
    let promise3 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 3 }))
        .unwrap();
    let promise4 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 4 }))
        .unwrap();
    let promise5 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 5 }))
        .unwrap();

    drop(initial_promise);
    assert_eq!(task.waiters_count(), 5);

    let (v1, v2, v3, v4, v5) =
        futures::future::join5(promise1, promise2, promise3, promise4, promise5).await;
    let (v1, v2, v3, v4, v5) = (v1?, v2?, v3?, v4?, v5?);
    assert!(
        v1.testing_resident_value()
            .equality(&DiceValidValue::testing_new(DiceKeyValue::<K>::new(2)))
    );
    assert!(
        v2.testing_resident_value()
            .equality(&DiceValidValue::testing_new(DiceKeyValue::<K>::new(2)))
    );
    assert!(
        v3.testing_resident_value()
            .equality(&DiceValidValue::testing_new(DiceKeyValue::<K>::new(2)))
    );
    assert!(
        v4.testing_resident_value()
            .equality(&DiceValidValue::testing_new(DiceKeyValue::<K>::new(2)))
    );
    assert!(
        v5.testing_resident_value()
            .equality(&DiceValidValue::testing_new(DiceKeyValue::<K>::new(2)))
    );

    Ok(())
}

#[tokio::test]
async fn dropping_all_waiters_cancels_task() {
    let barrier = Arc::new(Barrier::new(2));

    let (task, initial_promise) = spawn_dice_task(DiceKey { index: 600 }, &TokioSpawner, &(), {
        let barrier = barrier.dupe();
        |handle| {
            async move {
                let _handle = handle;
                // wait for the lock too
                barrier.wait().await;
                futures::future::pending().await
            }
            .boxed()
        }
    });

    barrier.wait().await;

    let await_termination = task.as_ref().await_termination();
    futures::pin_mut!(await_termination);

    assert!(!task.is_ready());
    assert!(!task.is_cancelled());
    assert!(task.is_pending());
    assert_matches!(futures::poll!(&mut await_termination), Poll::Pending);

    let promise1 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 0 }))
        .unwrap();

    drop(initial_promise);

    assert!(task.is_pending());
    assert_matches!(futures::poll!(&mut await_termination), Poll::Pending);

    let promise2 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 0 }))
        .unwrap();

    assert!(task.is_pending());
    assert_matches!(futures::poll!(&mut await_termination), Poll::Pending);

    drop(promise1);

    assert!(task.is_pending());
    assert_matches!(futures::poll!(&mut await_termination), Poll::Pending);

    let promise3 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 0 }))
        .unwrap();

    drop(promise2);
    drop(promise3);

    await_termination.await;
    assert!(!task.is_ready());
    assert!(task.is_cancelled());
    assert!(!task.is_pending());

    // After cancellation, depended_on_by enters the restart path (the new
    // design restarts cancelled-then-redepended tasks rather than returning
    // a cancelled result). NeedsRestart confirms the task was cancelled.
    match task.depended_on_by(ParentKey::None) {
        DiceTaskDependedOnByResult::Pending(_) | DiceTaskDependedOnByResult::Finished(_) => {
            panic!("should be cancelled")
        }
        DiceTaskDependedOnByResult::NeedsRestart(_, _) => {}
    }
}

#[tokio::test]
async fn task_that_already_cancelled_returns_cancelled() {
    let (task, initial_promise) = spawn_dice_task(DiceKey { index: 777 }, &TokioSpawner, &(), {
        |_handle| async move { futures::future::pending().await }.boxed()
    });

    // Cancel the worker by dropping its only dependent.
    drop(initial_promise);
    task.as_ref().await_termination().await;

    // After cancellation, depended_on_by enters the restart path (the new
    // design restarts cancelled-then-redepended tasks rather than returning
    // a cancelled result). NeedsRestart confirms the task was cancelled.
    match task.depended_on_by(ParentKey::None) {
        DiceTaskDependedOnByResult::Pending(_) | DiceTaskDependedOnByResult::Finished(_) => {
            panic!("should be cancelled")
        }
        DiceTaskDependedOnByResult::NeedsRestart(_, _) => {}
    }
}

#[tokio::test]
async fn dropping_termination_observer_does_not_cancel_task() {
    let barrier = Arc::new(Barrier::new(2));

    let (task, _initial_promise) = spawn_dice_task(DiceKey { index: 888 }, &TokioSpawner, &(), {
        let barrier = barrier.dupe();
        |handle| {
            async move {
                let _handle = handle;
                barrier.wait().await;
                futures::future::pending().await
            }
            .boxed()
        }
    });

    barrier.wait().await;

    // Register a dependent so the task stays alive
    let promise = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 0 }))
        .unwrap();

    assert!(task.is_pending());

    // Create and drop a termination observer — this should NOT cancel the task
    let observer = task.as_ref().await_termination();
    drop(observer);

    // Task should still be pending and not cancelled
    assert!(task.is_pending());
    assert!(!task.is_cancelled());

    // Should still be able to register new dependents
    let promise2 = task
        .depended_on_by(ParentKey::Some(DiceKey { index: 1 }))
        .unwrap();
    assert!(task.is_pending());

    drop(promise);
    drop(promise2);
}

#[tokio::test]
async fn resident_task_shares_result_after_restart() -> anyhow::Result<()> {
    let task = DiceTask::<ResidentComputedValue>::new_idle(DiceKey { index: 900 });
    let DiceTaskDependedOnByResult::NeedsRestart(prepared, previous) =
        task.depended_on_by(ParentKey::None)
    else {
        panic!("the first dependent must start the idle task");
    };
    assert!(
        previous.await_termination().await.is_none(),
        "generation zero has no worker or result"
    );
    let initial = spawn_prepared_task(prepared, &TokioSpawner, &(), |_handle| {
        async { futures::future::pending().await }.boxed()
    });
    drop(initial);
    task.as_ref().await_termination().await;

    let DiceTaskDependedOnByResult::NeedsRestart(prepared, previous) =
        task.depended_on_by(ParentKey::None)
    else {
        panic!("dropping the only dependent must allow a restart");
    };
    assert!(previous.await_termination().await.is_none());

    let expected = DiceValidValue::testing_new(DiceKeyValue::<K>::new(2));
    let value =
        ResidentComputedValue::new(expected.dupe(), InvalidationSources::NONE, Revision::FIRST);
    let first = spawn_prepared_task(prepared, &TokioSpawner, &(), |handle| {
        async move { handle.finished(Ok(value)) }.boxed()
    });
    let DiceTaskDependedOnByResult::Pending(second) = task.depended_on_by(ParentKey::None) else {
        panic!("the restarted worker has not run yet");
    };

    let first: &ResidentComputedValue = first.await?;
    let second: &ResidentComputedValue = second.await?;
    assert!(
        std::ptr::eq(first, second),
        "dependents share the stored result"
    );
    let value = first.as_ref();
    assert!(value.value().instance_equal(&expected));
    assert_eq!(value.revision(), Some(Revision::FIRST));

    let DiceTaskDependedOnByResult::Finished(finished) = task.depended_on_by(ParentKey::None)
    else {
        panic!("a completed resident task must remain ready");
    };
    assert!(std::ptr::eq(first, finished));
    Ok(())
}

#[test]
fn concurrent_dependents_start_one_idle_task() {
    let task = DiceTask::<usize>::new_idle(DiceKey { index: 901 });
    assert!(!task.is_pending(), "an idle task has no outstanding worker");
    assert_eq!(task.waiters_count(), 0, "no dependent has registered yet");
    assert_eq!(
        task.as_ref().await_termination().now_or_never(),
        Some(None),
        "generation zero is already terminated without a result"
    );

    let barrier = ThreadBarrier::new(8);
    let dependents = thread::scope(|scope| {
        let threads = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    task.depended_on_by(ParentKey::None)
                })
            })
            .collect::<Vec<_>>();
        threads
            .into_iter()
            .map(|thread| thread.join().expect("dependent thread should not panic"))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        task.waiters_count(),
        8,
        "every thread registered a dependent"
    );

    Builder::new_current_thread()
        .build()
        .expect("test runtime should build")
        .block_on(async {
            let mut starts = 0;
            let promises = dependents
                .into_iter()
                .map(|dependent| match dependent {
                    DiceTaskDependedOnByResult::NeedsRestart(prepared, previous) => {
                        starts += 1;
                        spawn_prepared_task(prepared, &TokioSpawner, &(), |handle| {
                            async move {
                                assert!(
                                    previous.await_termination().await.is_none(),
                                    "the first worker has no previous result to reuse"
                                );
                                handle.finished(Ok(42));
                            }
                            .boxed()
                        })
                    }
                    DiceTaskDependedOnByResult::Pending(promise) => promise,
                    DiceTaskDependedOnByResult::Finished(_) => panic!("no worker has started yet"),
                })
                .collect::<Vec<_>>();
            assert_eq!(starts, 1, "exactly one dependent starts the idle task");
            for promise in promises {
                assert_eq!(
                    *promise.await.unwrap(),
                    42,
                    "all dependents share the worker result"
                );
            }
        });
    assert!(task.is_ready(), "the worker result remains available");
    assert!(!task.is_pending(), "the only worker has terminated");
}
