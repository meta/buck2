/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Builds in a project as `buck2 init` makes it, with the bundled prelude and the demo toolchains.
//! Those take the compilers from `PATH`: clang and lld for C++, rustc for Rust, and Python to run
//! the prelude's own tools.
//!
//! Meta's build of the prelude names targets of Meta's repository in attributes of the C++ rules,
//! and Meta's test hosts have no compilers on `PATH`. So this test target is defined in open
//! source only.

use buck2_e2e::prelude::*;

/// Runs `buck2 init` in the fixture, and gives the packages their build files. The fixture keeps
/// those under a name that buck2 does not read by default.
async fn init(buck: &Buck, packages: &[&str]) -> anyhow::Result<()> {
    buck.init(&[]).await?;
    for package in packages {
        let dir = buck.cwd().join(package);
        tokio::fs::rename(dir.join("TARGETS.fixture"), dir.join("BUCK")).await?;
    }
    Ok(())
}

#[buck_test(inplace = false, skip_for_os = ["darwin", "windows"])]
async fn test_cxx_binary_builds_and_runs(buck: &Buck) -> anyhow::Result<()> {
    init(buck, &["cpp"]).await?;
    let res = buck.run(&["//cpp:main"]).await?;
    assert_eq!(res.stdout.trim(), "Hello from C++");
    Ok(())
}

#[buck_test(inplace = false, skip_for_os = ["darwin", "windows"])]
async fn test_rust_binary_with_proc_macro_and_build_script_builds_and_runs(
    buck: &Buck,
) -> anyhow::Result<()> {
    init(buck, &["rust"]).await?;
    let res = buck.run(&["//rust:main"]).await?;
    assert_eq!(res.stdout.trim(), "Hello from Rust, 42");
    Ok(())
}
