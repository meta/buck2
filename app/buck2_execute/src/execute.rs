/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod action_digest;
pub mod action_digest_and_blobs;
pub mod blobs;
pub mod blocking;
pub mod cache_uploader;
pub mod claim;
pub mod clean_output_paths;
pub mod command_executor;
pub mod dep_file_digest;
pub mod environment_inheritance;
pub mod inputs_directory;
pub mod kind;
pub mod low_pass_filter;
pub mod manager;
pub mod output;
pub mod paths_with_digest;
pub mod prepared;
pub mod readonly_outputs;
pub mod request;
pub mod result;
pub mod target;
pub mod testing_dry_run;
pub mod write_action_cache;

use std::future::Future;

use buck2_events::dispatch::span;
use buck2_events::dispatch::span_async_simple;

pub fn executor_stage_async<F: Future>(
    stage: impl Into<buck2_data::executor_stage_start::Stage>,
    f: F,
) -> impl Future<Output = <F as Future>::Output> {
    // We avoid using `async fn` or `async move` here to avoid doubling the
    // future size. See https://github.com/rust-lang/rust/issues/62958
    let event = buck2_data::ExecutorStageStart {
        stage: Some(stage.into()),
    };
    span_async_simple(event, f, buck2_data::ExecutorStageEnd::default())
}

pub fn executor_stage<F, R>(stage: impl Into<buck2_data::executor_stage_start::Stage>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let event = buck2_data::ExecutorStageStart {
        stage: Some(stage.into()),
    };
    span(event, || {
        let r = f();
        (r, buck2_data::ExecutorStageEnd::default())
    })
}
