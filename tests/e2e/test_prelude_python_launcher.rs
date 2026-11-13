/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Builds the inplace launcher of `python_binary` the way make_py_package.bzl builds it, as an
//! action of a buck2 build that runs `prelude//python/tools:make_py_package_inplace` over the
//! prelude's `__par__` runtime, and runs it with a real interpreter.

use std::path::Path;
use std::path::PathBuf;
use std::process::Output;

use buck2_e2e::prelude::*;

/// What `code` prints when the interpreter `python` runs it.
async fn python_prints(python: &str, code: &str) -> anyhow::Result<String> {
    let output = tokio::process::Command::new(python)
        .args(["-c", code])
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "`{python} -c {code:?}` failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// The real path of `python3` from `PATH`: the `#!` line of the launcher names it.
async fn interpreter() -> anyhow::Result<String> {
    python_prints(
        "python3",
        "import os, sys\nprint(os.path.realpath(sys.executable))",
    )
    .await
}

/// Whether the interpreter reads `PYTHONSAFEPATH`, which Python 3.11 added; an older one ignores
/// it.
async fn reads_safe_path(interpreter: &str) -> anyhow::Result<bool> {
    let flag = python_prints(
        interpreter,
        "import sys\nprint(hasattr(sys.flags, 'safe_path'))",
    )
    .await?;
    Ok(flag == "True")
}

async fn launcher(buck: &Buck, interpreter: &str) -> anyhow::Result<PathBuf> {
    let res = buck
        .build_without_report(&[
            "root//:app",
            "--show-full-simple-output",
            "-c",
            &format!("test.python={interpreter}"),
        ])
        .await?;
    Ok(PathBuf::from(res.stdout.trim()))
}

async fn run(launcher: &Path, env: &[(&str, &str)]) -> anyhow::Result<Output> {
    Ok(tokio::process::Command::new(launcher)
        .envs(env.iter().copied())
        .output()
        .await?)
}

// prelude/python/tools/run_inplace.py.in
#[buck_test(inplace = false, skip_for_os = ["windows"])]
async fn test_launcher_fails_under_pythonsafepath(buck: &Buck) -> anyhow::Result<()> {
    let interpreter = interpreter().await?;
    let launcher = launcher(buck, &interpreter).await?;
    let proc = run(&launcher, &[]).await?;
    let stdout = String::from_utf8_lossy(&proc.stdout);
    assert!(
        stdout.starts_with("app ran from "),
        "{stdout}\n{}",
        String::from_utf8_lossy(&proc.stderr)
    );

    // The startup code asserts that `-c` put the cwd first on `sys.path` before it drops
    // that entry; `PYTHONSAFEPATH` keeps the cwd off `sys.path`.
    let proc = run(&launcher, &[("PYTHONSAFEPATH", "1")]).await?;
    let stderr = String::from_utf8_lossy(&proc.stderr);
    if reads_safe_path(&interpreter).await? {
        assert!(!proc.status.success());
        assert!(stderr.contains("AssertionError"), "{stderr}");
    } else {
        assert!(proc.status.success(), "{stderr}");
    }
    Ok(())
}
