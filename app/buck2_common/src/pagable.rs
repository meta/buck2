/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `pagable`'s concurrent helpers for code running under a command: the
//! calling thread's event dispatcher and soft-error policy travel onto the
//! blocking tasks, as they do for every other blocking spawn in the daemon.

use buck2_env::soft_error::capture_soft_error_context;
use buck2_env::soft_error::with_soft_error_context;
use buck2_events::dispatch::get_dispatcher_opt;
use buck2_events::dispatch::with_dispatcher_opt;

/// [`pagable::prepare::prepare_all`] with the caller's command context on
/// every task.
pub async fn prepare_all<I, T, F>(items: Vec<I>, prepare: F) -> buck2_error::Result<Vec<T>>
where
    I: Send + 'static,
    T: Send + 'static,
    F: Fn(I) -> T + Send + Sync + 'static,
{
    let dispatcher = get_dispatcher_opt();
    let soft_errors = capture_soft_error_context();
    pagable::prepare::prepare_all(items, move |item| {
        with_soft_error_context(soft_errors.clone(), || {
            with_dispatcher_opt(dispatcher.clone(), || prepare(item))
        })
    })
    .await
    .map_err(buck2_error::Error::from)
}
