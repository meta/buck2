/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::prelude::*;

/// Gracefully restarts the EdenFS daemon the harness started in `eden_dir` (the mount survives via
/// takeover, but the daemon process changes).
async fn eden_restart(eden_dir: &Path) -> anyhow::Result<()> {
    let status = tokio::process::Command::new("eden")
        .arg("--config-dir")
        .arg(eden_dir.join("config"))
        .arg("--home-dir")
        .arg(eden_dir.join("home"))
        .arg("--etc-eden-dir")
        .arg(eden_dir.join("etc"))
        .args(["restart", "--graceful"])
        .status()
        .await?;
    anyhow::ensure!(
        status.success(),
        "`eden restart --graceful` failed: {status}"
    );
    Ok(())
}

// Graceful takeover is not supported on Windows.
// The test repo's .buckconfig opts into `buck2.detect_eden_restart`, which defaults to
// off while the fail-fast behavior is rolled out.
#[buck_test(setup_eden = true, skip_for_os = ["windows"])]
async fn test_eden_graceful_restart_fails_commands_fast(buck: &Buck) -> anyhow::Result<()> {
    // Warm the daemon; this captures the Eden daemon identity at I/O provider creation.
    buck.targets(&[":"]).await?;

    // Restart detection is wired through the Eden I/O provider, so make sure it is in use.
    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    assert_eq!(status["io_provider"], "eden", "{}", status["io_provider"]);

    let eden_dir = buck
        .cwd()
        .parent()
        .expect("the project is in a directory")
        .join("eden");
    eden_restart(&eden_dir).await?;

    // The mount survived the takeover, but the buck2 daemon's cached identity no longer
    // matches: the next command must fail fast instead of hanging on stale handles.
    buck.targets(&[":"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"EdenFS restarted \(pid \d+ -> \d+, start unixtime .+\)");
    Ok(())
}
