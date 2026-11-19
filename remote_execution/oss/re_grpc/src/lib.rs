/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod client;
mod digest;
mod error;
mod grpc;
mod metadata;
mod pool;
mod request;
mod response;
mod stats;

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

pub use client::*;
pub use error::*;
pub use grpc::*;
pub use metadata::*;
pub use request::*;
pub use response::*;

/// The global version of the network stats full of atomics
#[derive(Default, Debug)]
struct NetworkStatisticsGlobal {
    uploaded: AtomicI64,
    downloaded: AtomicI64,
}

static NETWORK_STATS: OnceLock<Arc<NetworkStatisticsGlobal>> = OnceLock::new();

fn get_network_stats_global() -> Arc<NetworkStatisticsGlobal> {
    Arc::clone(NETWORK_STATS.get_or_init(|| Arc::new(NetworkStatisticsGlobal::default())))
}

pub fn get_network_stats() -> anyhow::Result<NetworkStatisticsResponse> {
    let g = get_network_stats_global();
    Ok(NetworkStatisticsResponse {
        uploaded: g.uploaded.load(Ordering::Relaxed),
        downloaded: g.downloaded.load(Ordering::Relaxed),
        ..Default::default()
    })
}
