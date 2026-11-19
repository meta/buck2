/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Blocking per-item work for many items at once, such as resolving paged-out
//! values, run on the current Tokio runtime's blocking pool so every caller in
//! the process shares one bound on threads.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::task::JoinError;

/// Most items per blocking task. Bounds how much a task amortizes its spawn
/// over, so a long tail still spreads across tasks.
const MAX_BATCH: usize = 64;

/// Blocking tasks in flight at once.
const CONCURRENCY: usize = 16;

/// Runs `prepare` over `items` on blocking threads and returns its results in
/// item order.
///
/// Items are batched so one task spawn covers many of them, with batches sized
/// to keep all the concurrent tasks busy even for a short list; a lone item
/// runs inline. Per-item failures belong in `T`; the call itself fails only
/// when a task did not join. Returns only once every task has finished, so
/// `prepare` may use data the caller keeps alive for the call. The current
/// runtime must be the multi-thread one.
pub async fn prepare_all<I, T, F>(items: Vec<I>, prepare: F) -> Result<Vec<T>, JoinError>
where
    I: Send + 'static,
    T: Send + 'static,
    F: Fn(I) -> T + Send + Sync + 'static,
{
    if items.len() <= 1 {
        return Ok(items.into_iter().map(prepare).collect());
    }
    let batch_size = items.len().div_ceil(CONCURRENCY).clamp(1, MAX_BATCH);
    let mut pending: VecDeque<(usize, Vec<I>)> = VecDeque::new();
    let mut items = items.into_iter().peekable();
    while items.peek().is_some() {
        let batch = items.by_ref().take(batch_size).collect();
        pending.push_back((pending.len(), batch));
    }
    let batch_count = pending.len();
    let pending = Arc::new(Mutex::new(pending));
    let results: Arc<Mutex<Vec<Option<Vec<T>>>>> =
        Arc::new(Mutex::new((0..batch_count).map(|_| None).collect()));
    let prepare = Arc::new(prepare);
    let runtime = tokio::runtime::Handle::current();
    // Each task takes the next batch until none are left, so a slow batch
    // holds up only its own task.
    let tasks: Vec<_> = (0..CONCURRENCY.min(batch_count))
        .map(|_| {
            let pending = pending.clone();
            let results = results.clone();
            let prepare = prepare.clone();
            runtime.spawn_blocking(move || {
                loop {
                    let next = pending.lock().pop_front();
                    let Some((index, batch)) = next else {
                        return;
                    };
                    let done: Vec<T> = batch.into_iter().map(|item| prepare(item)).collect();
                    results.lock()[index] = Some(done);
                }
            })
        })
        .collect();
    let mut failure = None;
    for task in tasks {
        if let Err(join) = task.await {
            failure.get_or_insert(join);
        }
    }
    if let Some(failure) = failure {
        return Err(failure);
    }
    let results = Arc::try_unwrap(results)
        .ok()
        .expect("every task has finished")
        .into_inner();
    Ok(results
        .into_iter()
        .flat_map(|batch| batch.expect("every batch ran"))
        .collect())
}

// `#[tokio::test]` starts tokio's IO driver: epoll, kqueue or IOCP by OS, and Miri models only
// epoll.
#[cfg(all(test, any(not(miri), target_os = "linux")))]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn results_keep_item_order_across_batches() {
        // Enough items for many batches at the maximum size, with earlier
        // batches finishing last.
        let items: Vec<usize> = (0..(MAX_BATCH * CONCURRENCY * 2 + 1)).collect();
        let out = prepare_all(items.clone(), |i| {
            std::thread::sleep(Duration::from_millis(if i < MAX_BATCH { 20 } else { 1 }));
            i * 2
        })
        .await
        .unwrap();
        assert_eq!(out, items.iter().map(|i| i * 2).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn single_item_runs_inline() {
        let out = prepare_all(vec![7], |i| i + 1).await.unwrap();
        assert_eq!(out, vec![8]);
    }
}
