/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A tenant's own daemon directory under daemon sharing: the slot a client of any release
//! connects to. A daemon of the tenant's own owns it as before; under sharing, the proxy of the
//! tenant's generation in a shared daemon owns it and answers for the tenant.

use buck2_cli_proto::DaemonProcessInfo;
use buck2_common::daemon_compatibility::DaemonCompatibilityKey;
use buck2_common::daemon_dir::DaemonDir;
use buck2_common::invocation_paths::InvocationPaths;

use crate::daemon::client::connect::BootstrapBuckdClient;
use crate::daemon::client::connect::BuckdProcessInfo;
use crate::daemon::client::connect::establish_connection_existing;

/// What answers at a tenant's own daemon directory.
pub enum SlotOwner {
    /// No record, or the recorded process is gone.
    Vacant,
    /// A live process this client could not talk to; stopped the way any unreachable daemon is.
    Unreachable(DaemonProcessInfo),
    /// A daemon of the tenant's own, answering for itself.
    Legacy(BootstrapBuckdClient),
    /// The proxy of one generation of this tenant in a shared daemon.
    Proxy(TenantProxy),
}

pub struct TenantProxy {
    pub client: BootstrapBuckdClient,
    /// The namespace of the daemon the proxy represents, derived from the constraints it reports
    /// the way that daemon derives its own.
    pub key: DaemonCompatibilityKey,
    pub daemon_id: String,
    pub user_version: Option<String>,
    pub generation: u64,
    /// Whether that daemon records I/O traces, which its namespace key also encodes.
    pub trace_io_enabled: bool,
}

/// Connects to whatever owns the tenant's own daemon directory and asks it what it is. A proxy
/// reports its shared daemon's constraints and its tenant generation; a daemon of the tenant's own
/// reports no generation. Hold the directory's lifecycle lock while acting on the answer.
pub async fn probe_slot(paths: &InvocationPaths) -> buck2_error::Result<SlotOwner> {
    probe_slot_dir(&paths.tenant_paths().unshared_daemon_dir()?).await
}

/// [`probe_slot`] for a tenant directory the caller has already resolved.
pub async fn probe_slot_dir(directory: &DaemonDir) -> buck2_error::Result<SlotOwner> {
    let Some(record) = BuckdProcessInfo::load_if_exists(directory)? else {
        return Ok(SlotOwner::Vacant);
    };
    if !record.may_be_running()? {
        return Ok(SlotOwner::Vacant);
    }
    let info = record.info.clone();
    let client = match establish_connection_existing(directory).await {
        Ok(client) => client,
        Err(error) => {
            tracing::debug!(
                "Tenant directory owner `{}` did not answer: {error:#}",
                info.pid
            );
            return Ok(SlotOwner::Unreachable(info));
        }
    };
    let Some(generation) = client.tenant_generation() else {
        return Ok(SlotOwner::Legacy(client));
    };
    let proxied = {
        let constraints = client.daemon_constraints();
        match (&constraints.extra, &constraints.daemon_startup_config) {
            (Some(extra), Some(startup_config)) => Some((
                DaemonCompatibilityKey::new(
                    &constraints.version,
                    constraints.user_version.as_deref(),
                    startup_config,
                    extra.trace_io_enabled,
                ),
                constraints.daemon_id.clone(),
                constraints.user_version.clone(),
                extra.trace_io_enabled,
            )),
            _ => None,
        }
    };
    let Some((key, daemon_id, user_version, trace_io_enabled)) = proxied else {
        return Ok(SlotOwner::Legacy(client));
    };
    Ok(SlotOwner::Proxy(TenantProxy {
        client,
        key,
        daemon_id,
        user_version,
        generation,
        trace_io_enabled,
    }))
}
