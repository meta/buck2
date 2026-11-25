/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A project as `buck2 init` makes it, with the bundled prelude and the demo toolchains, and
//! packages of C++, Go, Java, Kotlin, OCaml, Python, Rust and shell targets.
//!
//! Meta's build of the prelude names targets of Meta's repository in attributes of rules such as
//! `cxx_library`, `java_library` and `python_library`, and a new project has no such targets. So
//! the cases that query or analyze the targets run in the open-source build only.

use buck2_e2e::prelude::*;

const PACKAGES: &[&str] = &[
    "cpp", "go", "java", "kotlin", "ocaml", "python", "rust", "shell",
];

/// Runs `buck2 init` in the fixture, and gives the packages their build files. The fixture keeps
/// those under a name that buck2 does not read by default.
async fn init(buck: &Buck) -> anyhow::Result<()> {
    buck.init(&[]).await?;
    for package in PACKAGES {
        let dir = buck.cwd().join(package);
        tokio::fs::rename(dir.join("TARGETS.fixture"), dir.join("BUCK")).await?;
    }
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_targets_of_every_language_load(buck: &Buck) -> anyhow::Result<()> {
    init(buck).await?;
    let res = buck.targets(&["//..."]).await?;
    let prefixes = PACKAGES
        .iter()
        .map(|package| format!("root//{package}:"))
        .chain(["root//:hello_world".to_owned()]);
    for prefix in prefixes {
        assert!(
            res.stdout.lines().any(|target| target.starts_with(&prefix)),
            "No `{prefix}` target in:\n{}",
            res.stdout
        );
    }
    Ok(())
}

// The default of an attribute can be a `select`, and an unconfigured query follows every branch
// of it. So each target that a rule names there has to exist in a new project, also where the
// branch is never taken.
#[cfg(not(fbcode_build))]
#[buck_test(inplace = false)]
async fn test_unconfigured_queries_reach_every_dependency(buck: &Buck) -> anyhow::Result<()> {
    init(buck).await?;
    buck.uquery(&["deps(//...)"]).await?;
    buck.audit_visibility(&["//..."]).await?;
    Ok(())
}

#[cfg(not(fbcode_build))]
#[buck_test(inplace = false)]
async fn test_targets_of_every_language_configure_and_analyze(buck: &Buck) -> anyhow::Result<()> {
    init(buck).await?;
    buck.cquery(&["deps(//...)"]).await?;
    buck.audit(&["providers", "//..."]).await?;
    Ok(())
}

#[buck_test(inplace = false, skip_for_os = ["windows"])]
async fn test_targets_that_need_no_compiler_build_and_run(buck: &Buck) -> anyhow::Result<()> {
    init(buck).await?;
    buck.build(&["//:hello_world", "//shell/..."]).await?;
    let res = buck.run(&["//shell:hello"]).await?;
    assert_eq!(res.stdout.trim(), "Hello from a shell script");
    Ok(())
}
