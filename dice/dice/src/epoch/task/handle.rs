/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Handle to the DiceTask as seen by the thread responsible for completing the task

use dice_futures::cancellation::CancellationContext;

use crate::epoch::task::dice::DiceTaskCompletionHandle;
use crate::epoch::worker::WorkerCancelled;
use crate::epoch::worker::WorkerResult;
use crate::value::MaybeResidentComputedValue;

/// The handle to the 'DiceTask' owned by the spawned thread that is responsible for completing
/// the task.
pub(crate) struct DiceTaskHandle<'a, T = MaybeResidentComputedValue> {
    pub(super) cancellations: &'a CancellationContext,
    completion_handle: Option<DiceTaskCompletionHandle<T>>,
    // holds the result while `DiceTaskHandle` is not dropped, then upon drop, stores it into
    // `DiceTaskInternal` so that the result is always reported consistently at the very end of the task.
    result: Option<WorkerResult<T>>,
}

impl<'a, T> DiceTaskHandle<'a, T> {
    pub(super) fn new(
        completion_handle: DiceTaskCompletionHandle<T>,
        cancellations: &'a CancellationContext,
    ) -> Self {
        Self {
            cancellations,
            completion_handle: Some(completion_handle),
            result: None,
        }
    }

    pub(crate) fn cancellation_ctx(&self) -> &'a CancellationContext {
        self.cancellations
    }

    pub(crate) fn finished(&mut self, value: WorkerResult<T>) {
        self.result = Some(value);
    }
}

impl<T> Drop for DiceTaskHandle<'_, T> {
    fn drop(&mut self) {
        let completion_handle = self.completion_handle.take().unwrap();
        match self.result.take() {
            Some(Ok(v)) => {
                completion_handle.completed(v);
            }
            Some(Err(token)) => {
                completion_handle.cancelled(token);
            }
            // A panic in the key's computation drops the handle without a result. A cancelled task
            // never gets a value, so its waiters would wait forever. Poison the task instead, so
            // that the waiters panic too.
            None if std::thread::panicking() => {
                completion_handle.poisoned();
            }
            None => {
                completion_handle.cancelled(WorkerCancelled);
            }
        }
    }
}
