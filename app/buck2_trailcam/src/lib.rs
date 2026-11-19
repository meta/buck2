/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! `buck2 log trailcam`: serves Trailcam, the web viewer for buck2 event logs,
//! from the buck2 binary on a local port.
//!
//! The viewer itself is built from `nest/apps/trailcam/core` and linked in as
//! a tarball of static files. This crate is the host it talks to: it answers
//! the two routes the bundle's local backend expects, `/api/invocation` with
//! what can be read from the log about the build, and `/api/event-log` with
//! the log itself.

mod bundle;
mod invocation;
mod server;

pub use server::ServeConfig;
pub use server::serve;
