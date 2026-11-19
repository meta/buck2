/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;

use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::with_dispatcher_async;
use dice::DiceComputations;
use dupe::Dupe;

use crate::events::HasEvents;

pub struct Scope<'a, 'x, T>
where
    T: Send + 'static,
    'a: 'x,
{
    scope: &'x mut async_scoped::TokioScope<'a, T>,
    dispatcher: EventDispatcher,
}

impl<'a, 'x, T> Scope<'a, 'x, T>
where
    T: Send + 'static,
    'a: 'x,
{
    pub fn spawn_cancellable<F: Future<Output = T> + Send + 'a, Fu: FnOnce() -> T + Send + 'a>(
        &mut self,
        f: F,
        default: Fu,
    ) {
        self.scope
            .spawn_cancellable(with_dispatcher_async(self.dispatcher.dupe(), f), default)
    }
}

/// Wrap `async_scoped::TokioScope::scope_and_collect` propagating the event dispatcher.
///
/// # Safety
///
/// The caller must ensure that all futures spawned within the scope complete before
/// the scope returns, or that any data referenced by the futures has a lifetime that
/// extends beyond the scope. This is the same safety requirement as `async_scoped::TokioScope::scope_and_collect`.
pub async unsafe fn scope_and_collect_with_dispatcher<'d, 'a, T, R, F>(
    dispatcher: EventDispatcher,
    f: F,
) -> (
    R,
    Vec<<async_scoped::spawner::use_tokio::Tokio as async_scoped::spawner::Spawner<T>>::FutureOutput>,
)
    where
        T: Send + 'static,
        F: for<'x> FnOnce(&mut Scope<'a, 'x, T>) -> R,
{
    unsafe {
        async_scoped::TokioScope::scope_and_collect(|scope| {
            let mut scope = Scope { scope, dispatcher };
            f(&mut scope)
        })
        .await
    }
}

/// Wrap `async_scoped::TokioScope::scope_and_collect` propagating the event dispatcher.
///
/// # Safety
///
/// The caller must ensure that all futures spawned within the scope complete before
/// the scope returns, or that any data referenced by the futures has a lifetime that
/// extends beyond the scope. This is the same safety requirement as `async_scoped::TokioScope::scope_and_collect`.
pub async unsafe fn scope_and_collect_with_dice<'c, 'd, 'a, T, R, F>(
    ctx: &'c mut DiceComputations<'d>,
    f: F,
) -> (
    R,
    Vec<<async_scoped::spawner::use_tokio::Tokio as async_scoped::spawner::Spawner<T>>::FutureOutput>,
)
where
    T: Send + 'static,
    F: for<'x> FnOnce(&'c mut DiceComputations<'d>, &mut Scope<'a, 'x, T>) -> R,
{
    unsafe {
        let dispatcher = ctx.per_transaction_data().get_dispatcher().dupe();
        scope_and_collect_with_dispatcher(dispatcher, |scope| f(ctx, scope)).await
    }
}
