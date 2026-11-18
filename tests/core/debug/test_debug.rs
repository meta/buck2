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

#[buck_test(allow_daemon_panics = true)]
async fn test_debug_crash(buck: &Buck) -> anyhow::Result<()> {
    // If the first operation immediately does a panic then we fail to connect.
    // While that's not great, having some panics is better than none, so test once after we spawn.
    buck.build(&[]).await?;
    let result = buck.debug(&["crash", "panic"]).await.expect_failure();
    assert!(result.stderr.contains("explicitly requested panic"));
    // Our crash output should include a stack trace.
    assert!(result.stderr.contains("stack backtrace:"));
    Ok(())
}

#[buck_test]
async fn test_debug_exe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.debug(&["exe"]).await?;
    let path = result.stdout.trim();
    assert!(Path::new(path).exists());
    Ok(())
}

#[buck_test]
async fn test_debug_allocative(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Start the server.
    buck.uquery(&["root//:"]).await?;

    let file_path = tmp_path.join("profile");

    let output = buck
        .debug(&["allocative", "--output", &file_path.to_string_lossy()])
        .await?;
    assert!(file_path.join("flame.src").exists());
    assert!(file_path.join("flame.svg").exists());
    assert!(output.stderr.contains("Allocative profile written to"));

    buck.debug(&["allocative"]).await?;
    assert!(buck.cwd().join("allocative-out").join("flame.src").exists());
    assert!(buck.cwd().join("allocative-out").join("flame.svg").exists());
    Ok(())
}

#[buck_test]
async fn test_debug_filestatus(buck: &Buck) -> anyhow::Result<()> {
    // Start the server.
    buck.uquery(&["root//:"]).await?;
    // FIXME(JakobDegen): `.` is an error
    let output = buck.debug(&["file-status", "TARGETS.fixture"]).await?;
    assert!(output.stderr.contains("No mismatches detected"));
    Ok(())
}

#[buck_test]
async fn test_debug_flush_pgo_profile(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[]).await?;
    let result = buck.debug(&["flush-pgo-profile"]).await?;
    assert!(result.stderr.contains("was not flushed"));
    Ok(())
}

#[buck_test(skip_for_os = ["windows", "darwin"])]
async fn test_thread_dump(buck: &Buck) -> anyhow::Result<()> {
    // Make sure we don't start a daemon if there isn't one
    buck.debug(&["thread-dump"])
        .await
        .expect_failure()
        .assert_stderr_matches("No running buck daemon");
    // Start the daemon
    buck.uquery(&["root//:"]).await?;
    let output = buck.debug(&["thread-dump"]).await?;
    assert!(output.stdout.contains("frame #0"));
    Ok(())
}
