/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The event log beside a tenant's daemon directories, which records what became of its proxies.
//!
//! It exists for the one question a proxy's own logs cannot answer afterwards: why a shared
//! daemon went away. A proxy that forces the daemon's exit, or is retired, evicted or orphaned,
//! appends a line saying which generation of which daemon it served, what happened, why, and how
//! long after the eviction began; its stdout and stderr are truncated when the next owner takes
//! the directory, while this file outlives every proxy generation and the daemon itself.
//!
//! Only this crate writes it, and only the end-to-end tests and someone investigating an
//! incident read it. No client reads it, nothing of another release does, and nothing in the
//! daemon protocol depends on it, so its records can change shape freely.

/// Written beside the tenant's daemon directories, one JSON record per line.
pub(crate) const UNIDAEMON_EVENTS_FILE: &str = "unidaemon-events.jsonl";
