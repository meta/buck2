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

/// The `-c nested.buck2_path=...` value that points the nested invocation at the buck2 under test.
fn nested_buck2_path(buck: &Buck) -> String {
    let executable = buck
        .env_var("TEST_EXECUTABLE")
        .expect("the test fixture sets `TEST_EXECUTABLE`");
    format!("nested.buck2_path={}", executable.to_string_lossy())
}

#[buck_test(allow_soft_errors = true)]
async fn test_same_state(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:nested_normal", "-c", &nested_buck2_path(buck)])
        .env("SANDCASTLE_ID", "")
        .await?;
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_different_state_error(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // FIXME(JakobDegen): Nested invocations seem to have buggy behavior around writing the event
    // logs, so `log show` and friends don't work without this
    let log = tmp_path.join("logfile.json-lines");
    let log = log.to_str().unwrap();
    buck.build(&[
        "-c",
        "some.config=Val",
        "root//:nested_normal",
        "--event-log",
        log,
        "-c",
        &nested_buck2_path(buck),
    ])
    .env("SANDCASTLE_ID", "")
    .await
    .expect_failure()
    .assert_stderr_matches("Failed to build 'root//:nested_normal");
    let res = buck
        .log(&["what-ran", "--failed", "--show-std-err", log])
        .await?;
    assert!(
        res.stdout
            .contains("Recursive invocation of Buck, with a different state")
    );
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_different_user_version_and_state(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let log = tmp_path.join("logfile.json-lines");
    let log = log.to_str().unwrap();
    buck.build(&[
        "-c",
        "some.config=Val",
        "root//:nested_normal",
        "--event-log",
        log,
        "-c",
        &nested_buck2_path(buck),
    ])
    // Set a `SANDCASTLE_ID`; this affects the daemon constraints
    .env("SANDCASTLE_ID", "12345")
    .await
    .expect_failure()
    .assert_stderr_matches("Failed to build 'root//:nested_normal");
    let res = buck
        .log(&["what-ran", "--failed", "--show-std-err", log])
        .await?;
    assert!(
        res.stdout
            .contains("Recursive invocation of Buck, with a different state")
    );
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_trace_io_mismatch(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let log = tmp_path.join("logfile.json-lines");
    let log = log.to_str().unwrap();
    buck.build(&[
        "root//:nested_trace",
        "--event-log",
        log,
        "-c",
        &nested_buck2_path(buck),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Failed to build 'root//:nested_trace");
    let res = buck
        .log(&["what-ran", "--failed", "--show-std-err", log])
        .await?;
    assert!(
        res.stdout
            .contains("daemon constraint mismatch during nested invocation: Trace IO mismatch")
    );
    Ok(())
}
