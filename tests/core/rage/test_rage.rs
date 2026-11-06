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

// No windows since mocking pastry command didn't work D41623200
#[buck_test(skip_for_os = ["windows", "darwin"])]
async fn test_rage(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Build a trivial action
    buck.build(&["//:simple"]).await?;

    for command in ["pastry", "hg"] {
        mock_command(&tmp_path.join(command))?;
    }

    // We want to find our executable first
    let path = std::env::join_paths(std::iter::once(tmp_path.to_owned()).chain(
        std::env::split_paths(buck.env_var("PATH").unwrap_or_default()),
    ))?;
    // Run rage aginst the most recent invocation.
    buck.rage(&[]).stdin("0").env("PATH", path).await?;
    Ok(())
}

/// A command that prints its arguments.
#[cfg(unix)]
fn mock_command(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, "#! /bin/sh\necho \"$@\"\n")?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o777))
}

#[cfg(not(unix))]
fn mock_command(_path: &Path) -> std::io::Result<()> {
    unreachable!("test_rage does not run on Windows")
}

#[buck_test(skip_for_os = ["darwin"])]
async fn test_rage_no_paste(buck: &Buck) -> anyhow::Result<()> {
    // Build a trivial action
    buck.build(&["//:simple"]).await?;
    // Run rage aginst the most recent invocation.
    buck.rage(&["--no-paste", "--invocation-offset", "0"])
        .await?;
    Ok(())
}

#[buck_test(skip_for_os = ["darwin"])]
async fn test_rage_no_logs(buck: &Buck) -> anyhow::Result<()> {
    // Rage doesn't crash even with no invocation logs
    buck.rage(&["--no-paste"]).await?;
    Ok(())
}

// The one test that runs on mac, where the others are skipped
#[buck_test]
async fn test_nop(_buck: &Buck) {}
