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
use std::path::PathBuf;
use std::time::Duration;

use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;

async fn start_health_check_server(
    buck: &Buck,
    tmp_path: &Path,
    request_setup: &str,
) -> anyhow::Result<(PathBuf, PathBuf, BuckProcess)> {
    let server_output = tmp_path.join("server_output");
    let health_check_state_file = tmp_path.join("health_check_state");
    std::fs::write(&server_output, "")?;
    std::fs::write(&health_check_state_file, "")?;

    // Start the server before the build begins to ensure that the server is ready to accept requests.
    // This is necessary since the run_request_hang_server.sh script may need to build the server target.
    let mut process = buck
        .run(&[
            "fbcode//buck2/tests/e2e/health_check:health_check_server_bin",
            "--",
            "--isolation-dir", // Avoids interference with the build of test target
            "health_check_server",
            "--state-info-file",
            &health_check_state_file.to_string_lossy(),
            request_setup,
        ])
        .env("HEALTH_CHECK_SERVER_STATS_OUTPUT", &server_output)
        .spawn();

    // Loop until the tcp_port is written which implies that the health check server has started
    loop {
        if !std::fs::read_to_string(&health_check_state_file)?
            .trim()
            .is_empty()
        {
            break;
        }
        if let Some(status) = process.try_wait() {
            let output = match process.wait().await {
                Ok(output) => output,
                Err(error) => error.into_output(),
            };
            anyhow::bail!(
                "The health check server exited ({status}) before it started:\n{}",
                output.stderr
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok((health_check_state_file, server_output, process))
}

#[buck_test(inplace = true)]
async fn test_health_check_with_request_hang(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let (health_check_state_file, server_output, server_process) =
        start_health_check_server(buck, tmp_path, "--with-request-hang").await?;

    buck.build(&["fbcode//buck2/tests/targets/rules/rust/hello_world:welcome"])
        // Set the CLI_PATH to `echo` making it effectively a no-op since we want buck to use the test server and not spawn a new one.
        .env("BUCK2_HEALTH_CHECK_CLI_PATH", "echo")
        .env(
            "BUCK2_HEALTH_CHECK_STATE_INFO_PATH",
            &health_check_state_file,
        )
        .await?;
    let output = std::fs::read_to_string(&server_output)?;
    let count = |line: &str| output.split_inclusive('\n').filter(|l| *l == line).count();
    // Only the first call to update is logged. The server is stuck processing that request.
    assert_eq!(count("UpdateContext Requested\n"), 1);
    assert_eq!(count("RunChecks Requested\n"), 0);
    assert_eq!(count("UpdateContext Completed\n"), 0);
    assert_eq!(count("RunChecks Completed\n"), 0);

    // Killed, it exits unsuccessfully.
    let _killed = server_process.kill().await;
    Ok(())
}
