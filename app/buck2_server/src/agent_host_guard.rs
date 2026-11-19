/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_common::invocation_paths::DEFAULT_ISOLATION_DIR;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_core::buck2_env;
use buck2_core::fs::project::ProjectRoot;
use buck2_events::metadata::hostname;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;

use crate::daemon::state::DaemonStateData;

/// Substring that marks an originating cgroup as a coding-agent cgroup.
const CGROUP_AGENT_MARKER: &str = "3pai";

/// Comma-separated hostname globs matched against the local hostname.
/// Unset/empty disables the feature. Example:
/// ```ini
/// [buck2]
/// agent_hostname_fail_v2_glob = bad-host-*.example.com,other-*
/// agent_hostname_fail_v2_context = See S123456.
/// ```
const FAIL_GLOB_KEY: BuckconfigKeyRef = BuckconfigKeyRef {
    section: "buck2",
    property: "agent_hostname_fail_v2_glob",
};
/// Optional extra context appended to the rejection message, e.g. a SEV link.
/// Unset/empty just omits it.
const FAIL_CONTEXT_KEY: BuckconfigKeyRef = BuckconfigKeyRef {
    section: "buck2",
    property: "agent_hostname_fail_v2_context",
};

#[derive(Debug, buck2_error::Error)]
enum AgentHostGuardError {
    #[error("{0}")]
    #[buck2(tag = Environment)]
    Denied(String),
    #[error("Invalid hostname glob `{glob}`: {error}")]
    #[buck2(tag = Input)]
    InvalidGlob { glob: String, error: String },
    #[error("Invalid hostname glob set: {error}")]
    #[buck2(tag = Input)]
    InvalidGlobSet { error: String },
}

/// Build the user-facing rejection message. The optional `context` (e.g. a SEV
/// link) is appended after a blank line when present.
fn denial_message(
    hostname: &str,
    project_root: &AbsNormPath,
    isolation: &str,
    context: Option<&str>,
) -> String {
    // The default isolation dir needs no flag, but a non-default one must be named
    // explicitly or the suggested command targets the wrong daemon and never unblocks.
    let run_command = if isolation == DEFAULT_ISOLATION_DIR {
        "buck2 kill && buck2 server".to_owned()
    } else {
        format!(
            "buck2 --isolation-dir {isolation} kill && buck2 --isolation-dir {isolation} server"
        )
    };

    let mut message = format!(
        "Running buck2 with buck2 daemon spawned from a coding agent's sandbox on certain hosts can make builds extremely slow. This daemon is in that situation on host `{hostname}`, so its builds are blocked for now. We are working on a long-term fix to remove buck2 daemon from sandbox.

To mitigate in the meantime, a person, not a coding agent, needs to run the following. Please open a terminal on this host, go to:

    {project_root}

and run:

    {run_command}

This restarts the buck2 daemon outside the sandbox, after which all buck2 builds (including ones triggered by a coding agent) will work again."
    );

    if let Some(context) = context {
        message.push_str("\n\n");
        message.push_str(context);
    }

    message
}

/// Reject the build if it originates from a 3pai cgroup and runs on a denylisted host.
pub(crate) fn check_agent_host_guard(
    config: &LegacyBuckConfig,
    daemon: &DaemonStateData,
    project_root: &ProjectRoot,
    isolation: &str,
) -> buck2_error::Result<()> {
    let glob = config.get(FAIL_GLOB_KEY).unwrap_or("").trim();
    if glob.is_empty() {
        // Feature disabled.
        return Ok(());
    }

    let context = config
        .get(FAIL_CONTEXT_KEY)
        .map(str::trim)
        .filter(|c| !c.is_empty());

    // The cgroup spawner is disabled in integration tests, so `daemon_originating_cgroup`
    // is always `None` there. This test-only override lets tests exercise the gate.
    let cgroup_override = buck2_env!(
        "BUCK2_TEST_DAEMON_ORIGINATING_CGROUP",
        applicability = testing,
    )?;
    let cgroup = cgroup_override.or(daemon.daemon_originating_cgroup.as_deref());

    if cgroup.filter(|c| c.contains(CGROUP_AGENT_MARKER)).is_none() {
        // Not an agent-originated daemon.
        return Ok(());
    }

    let Some(hostname) = hostname() else {
        // Can't determine the hostname; don't block.
        return Ok(());
    };

    if build_hostname_globset(glob)?.is_match(&hostname) {
        return Err(AgentHostGuardError::Denied(denial_message(
            &hostname,
            project_root.root(),
            isolation,
            context,
        ))
        .into());
    }

    Ok(())
}

/// Build a `GlobSet` from a comma-separated list of hostname glob patterns.
fn build_hostname_globset(spec: &str) -> buck2_error::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let glob = Glob::new(pattern).map_err(|e| AgentHostGuardError::InvalidGlob {
            glob: pattern.to_owned(),
            error: e.to_string(),
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|e| {
        AgentHostGuardError::InvalidGlobSet {
            error: e.to_string(),
        }
        .into()
    })
}
