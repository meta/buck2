/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;
use regex::NoExpand;
use regex::Regex;

// Cgroup of an agent-originated daemon; `BUCK2_TEST_DAEMON_ORIGINATING_CGROUP`
// overrides the real cgroup, which is unavailable since tests disable the cgroup spawner.
const AGENT_CGROUP: &str = "/user.slice/3pai_sandbox.slice/buck2.scope";
const NON_AGENT_CGROUP: &str = "/user.slice/user-1000.slice/buck2.scope";

fn write_buckconfig_local(buck: &Buck, contents: &str) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join(".buckconfig.local"), contents)?;
    Ok(())
}

fn sanitize_denial(stderr: &str, project_root: &str) -> String {
    // Replace the project root path before `sanitize_stderr`, which would
    // otherwise rewrite the hash inside the path and break a literal match.
    let s = stderr.replace(project_root, "<PROJECT_ROOT>");
    // The hostname is host-specific; normalize it so the golden is portable.
    let s = Regex::new(r"host `[^`]+`")
        .unwrap()
        .replace_all(&s, NoExpand("host `<HOSTNAME>`"));
    sanitize_stderr(&s)
}

#[buck_test]
async fn test_agent_host_guard_disabled(buck: &Buck) -> anyhow::Result<()> {
    // No glob configured -> feature off, build succeeds even from an agent cgroup.
    buck.build(&[":pass"])
        .env("BUCK2_TEST_DAEMON_ORIGINATING_CGROUP", AGENT_CGROUP)
        .await?;
    Ok(())
}

#[buck_test]
async fn test_agent_host_guard_denied(buck: &Buck) -> anyhow::Result<()> {
    write_buckconfig_local(buck, "[buck2]\nagent_hostname_fail_v2_glob=*\n")?;
    let result = buck
        .build(&[":pass"])
        .env("BUCK2_TEST_DAEMON_ORIGINATING_CGROUP", AGENT_CGROUP)
        .await
        .expect_failure();
    golden(
        &sanitize_denial(&result.stderr, &buck.cwd().to_string_lossy()),
        "golden/denied.golden.stderr",
    );
    Ok(())
}

#[buck_test]
async fn test_agent_host_guard_denied_with_context(buck: &Buck) -> anyhow::Result<()> {
    write_buckconfig_local(
        buck,
        "[buck2]\nagent_hostname_fail_v2_glob=*\nagent_hostname_fail_v2_context=See S123456\n",
    )?;
    let result = buck
        .build(&[":pass"])
        .env("BUCK2_TEST_DAEMON_ORIGINATING_CGROUP", AGENT_CGROUP)
        .await
        .expect_failure();
    golden(
        &sanitize_denial(&result.stderr, &buck.cwd().to_string_lossy()),
        "golden/denied_with_context.golden.stderr",
    );
    Ok(())
}

#[buck_test]
async fn test_agent_host_guard_denied_custom_isolation_dir(buck: &mut Buck) -> anyhow::Result<()> {
    // The remediation command must target the daemon's isolation dir. Run under a
    // non-default isolation dir and check the rejection message accounts for it.
    buck.set_isolation_prefix("custom_iso");
    write_buckconfig_local(buck, "[buck2]\nagent_hostname_fail_v2_glob=*\n")?;
    let result = buck
        .build(&[":pass"])
        .env("BUCK2_TEST_DAEMON_ORIGINATING_CGROUP", AGENT_CGROUP)
        .await
        .expect_failure();
    golden(
        &sanitize_denial(&result.stderr, &buck.cwd().to_string_lossy()),
        "golden/denied_custom_isolation_dir.golden.stderr",
    );
    Ok(())
}

#[buck_test]
async fn test_agent_host_guard_non_agent_cgroup(buck: &Buck) -> anyhow::Result<()> {
    // Hostname matches but the daemon is not agent-originated -> build succeeds.
    write_buckconfig_local(buck, "[buck2]\nagent_hostname_fail_v2_glob=*\n")?;
    buck.build(&[":pass"])
        .env("BUCK2_TEST_DAEMON_ORIGINATING_CGROUP", NON_AGENT_CGROUP)
        .await?;
    Ok(())
}

#[buck_test]
async fn test_agent_host_guard_hostname_no_match(buck: &Buck) -> anyhow::Result<()> {
    // Agent cgroup but the hostname glob does not match -> build succeeds.
    write_buckconfig_local(
        buck,
        "[buck2]\nagent_hostname_fail_v2_glob=definitely-not-this-host-*\n",
    )?;
    buck.build(&[":pass"])
        .env("BUCK2_TEST_DAEMON_ORIGINATING_CGROUP", AGENT_CGROUP)
        .await?;
    Ok(())
}
