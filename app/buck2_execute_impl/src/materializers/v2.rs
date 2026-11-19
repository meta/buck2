/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The materializer implementation that tracks disk state only: its state is what is on disk,
//! its synchronization is path leases, and content it does not hold locally comes from the CAS.

pub(crate) mod clean_stale;
pub mod core;
pub(crate) mod entry;
pub(crate) mod index;
pub(crate) mod task;
pub(crate) mod tree;

#[cfg(test)]
mod tests;
