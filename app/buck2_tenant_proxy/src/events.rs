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
//! Only this crate writes it: the proxy about itself, and the shared daemon's supervisor about
//! the proxies it launches, loses and replaces. Only the end-to-end tests and someone
//! investigating an incident read it. No client reads it, nothing of another release does, and
//! nothing in the daemon protocol depends on it, so its records can change shape freely.

use std::fs::OpenOptions;
use std::io::Write;

use buck2_common::daemon_compatibility::TenantProxySpec;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::paths::file_name::FileName;

use crate::slot::proxy_dir;

/// Written beside the tenant's daemon directories, one JSON record per line.
pub(crate) const UNIDAEMON_EVENTS_FILE: &str = "unidaemon-events.jsonl";

/// Records, from the shared daemon, what became of the proxy `pid` of `spec`'s generation. Best
/// effort: the log is diagnostic, so a failure to write it is logged and does not stop whatever
/// the daemon is doing to the proxy.
pub(crate) fn record_proxy_event(spec: &TenantProxySpec, pid: i64, event: &str, reason: &str) {
    if let Err(error) = append_proxy_event(spec, pid, event, reason) {
        tracing::warn!(
            event,
            reason,
            "Could not record a tenant proxy event: {error:#}"
        );
    }
}

fn append_proxy_event(
    spec: &TenantProxySpec,
    pid: i64,
    event: &str,
    reason: &str,
) -> buck2_error::Result<()> {
    let path = proxy_dir(spec)?
        .path
        .parent()
        .internal_error("A tenant daemon directory has a parent")?
        .join(FileName::unchecked_new(UNIDAEMON_EVENTS_FILE));
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let row = serde_json::json!({"event": event, "reason": reason, "proxy_pid": pid, "daemon_id": spec.reference.daemon_id, "generation": spec.reference.generation, "project_root": spec.tenant.project_root, "isolation": spec.tenant.isolation});
    let mut line = serde_json::to_vec(&row)?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(())
}
