/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_lint_fails(buck: &Buck) {
    buck.starlark(&["lint", "bad_warning.bzl"])
        .await
        .expect_failure()
        .assert_stderr_matches("Found 3 lints");
}

#[buck_test]
async fn test_exclude(buck: &Buck) -> anyhow::Result<()> {
    for d in ["dir/keep", "dir/skip"] {
        std::fs::create_dir_all(buck.cwd().join(d))?;
        std::fs::copy(
            buck.cwd().join("bad_warning.bzl"),
            buck.cwd().join(d).join("bad_warning.bzl"),
        )?;
    }
    buck.starlark(&["lint", "dir"])
        .await
        .expect_failure()
        .assert_stderr_matches("Found 6 lints");
    buck.starlark(&["lint", "dir", "-X", "dir/skip"])
        .await
        .expect_failure()
        .assert_stderr_matches("Found 3 lints");
    buck.starlark(&[
        "lint",
        "dir",
        "-X",
        "dir/skip",
        "-X",
        "dir/keep/bad_warning.bzl",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_typecheck_fails(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["typecheck", "good.bzl"]).await?;
    buck.starlark(&["typecheck", "bad.bzl"])
        .await
        .expect_failure()
        .assert_stderr_matches("Detected 2 errors");
    Ok(())
}

#[buck_test]
async fn test_explicit_path_goes_through_the_path_check(buck: &Buck) -> anyhow::Result<()> {
    buck.starlark(&["lint", "nope.bzl"])
        .await
        .expect_failure()
        .assert_stderr_matches("File `nope.bzl` does not exist");
    symlink_dir(".", &buck.cwd().join("linkdir"))?;
    buck.starlark(&["lint", "linkdir/good.bzl"])
        .await
        .expect_failure()
        .assert_stderr_matches("is behind the symlink `linkdir`, which buck2 does not follow");
    Ok(())
}

#[buck_test]
async fn test_typecheck_reports_unresolved_append(buck: &Buck) {
    // A typo'd `y.append(1)` inside a def is a scope error, not a daemon panic.
    let failure = buck
        .starlark(&["typecheck", "unresolved_append.bzl"])
        .await
        .expect_failure();
    failure.assert_stderr_matches("Detected 1 errors");
    assert!(
        failure.stdout.contains("Variable `y` not found"),
        "{}",
        failure.stdout
    );
}

#[buck_test]
async fn test_typecheck_reports_duplicate_argument(buck: &Buck) -> anyhow::Result<()> {
    // A parameter bound both positionally and by name is a typing error naming it, as at
    // runtime, not an internal error; a union of callables where another alternative accepts
    // the call is accepted.
    let failure = buck
        .starlark(&["typecheck", "duplicate_arg.bzl"])
        .await
        .expect_failure();
    failure.assert_stderr_matches("Detected 1 errors");
    assert!(
        failure
            .stdout
            .contains("Argument `a` occurs more than once"),
        "{}",
        failure.stdout
    );
    assert!(
        !failure.stdout.contains("Internal error"),
        "{}",
        failure.stdout
    );
    buck.starlark(&["typecheck", "duplicate_arg_union.bzl"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_typecheck_accepts_int_float_membership(buck: &Buck) -> anyhow::Result<()> {
    // `in` and dict indexing widen int and float like `==` does.
    buck.starlark(&["typecheck", "numeric_membership.bzl"])
        .await?;
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(target: &str, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &str, link: &std::path::Path) -> std::io::Result<()> {
    // As Python's `os.symlink`, which makes a file symlink unless told otherwise.
    std::os::windows::fs::symlink_file(target, link)
}
