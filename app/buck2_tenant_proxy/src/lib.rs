/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The tenant proxy of a shared daemon. Each tenant generation of a shared daemon gets a small
//! process of its own that owns the tenant's own daemon directory: it publishes the connection
//! files a daemon of any release publishes there and answers the daemon API for the shared
//! daemon, so that a client of any release reaches, kills and evicts the tenant the way it
//! reaches a daemon. The shared daemon launches and supervises these processes; the same binary
//! runs as one when started with `--tenant-proxy-file`.

#![cfg(target_os = "linux")]

mod emergency;
mod events;
mod launch;
mod process;
mod registry;
mod server;
mod slot;
mod supervisor;

pub use emergency::emergency_exit;
pub use launch::launch;
pub use process::run;
pub use registry::retire;
