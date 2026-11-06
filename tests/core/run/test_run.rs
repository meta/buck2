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

#[buck_test]
async fn test_run_executable(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.run(&["root//:print_hello"]).await?;
    assert_eq!(result.stdout.trim(), "hello");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_emit_shell(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.run(&["root//:print_hello", "--emit-shell"]).await?;

    let out = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&result.stdout)
        .output()
        .await?;
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8(out.stdout)?.trim(), "hello");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_run_non_executable_fails(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.run(&["root//:no_run_info"]).await.expect_failure();
    res.assert_stderr_matches(
        r"Target `[^`]+` is not a binary rule \(only binary rules can be `run`\)",
    );

    let record = res.invocation_record()?;
    let error = record.single_error();

    assert_eq!(error["category_key"], "RunCommandError::NonBinaryRule");
    assert_eq!(error["category"], "USER");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_run_exit_result(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.run(&["root//:print_hello"]).await?;
    let record = res.invocation_record()?;
    assert_eq!(record["exit_result_name"], "EXEC");
    Ok(())
}

async fn run_echo_args(buck: &Buck, args1: &[&str], args2: &[&str]) -> anyhow::Result<()> {
    let args = [&["root//:echo_args"], args1, args2].concat();
    let result = buck.run(&args).await?;
    assert_eq!(result.stdout.trim(), args2.join(" "));
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_passing_arguments(buck: &Buck) -> anyhow::Result<()> {
    run_echo_args(buck, &["--"], &["val", "--long", "-s", "spa  ces"]).await?;
    run_echo_args(buck, &["--"], &["val", "--", "test"]).await?;
    // Without --, the command still succeeds; the missing separator is recorded as a
    // deprecation, which is not shown on the console
    let result = buck.run(&["root//:echo_args", "val", "--long"]).await?;
    assert_eq!(result.stdout.trim(), "val --long");
    let log = buck.log(&["show"]).await?;
    assert!(
        log.stdout
            .contains("Soft Error: run_args_without_separator")
    );
    run_echo_args(buck, &[], &["val", "--", "x"]).await?; // Would work differently in Buck1 (no -- to user)
    buck.run(&["root//:echo_args", "--not-a-flag"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"unexpected argument '--not-a-flag'");
    Ok(())
}

#[buck_test]
async fn test_executable_fail_to_build(buck: &Buck) {
    buck.run(&["root//:build_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Failed to build");
}

#[buck_test(allow_soft_errors = true)]
async fn test_run_args_without_separator_is_recorded(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.run(&["root//:echo_args", "my_arg"]).await?;
    assert_eq!(result.stdout.trim(), "my_arg");
    let log = buck.log(&["show"]).await?;
    assert!(
        log.stdout
            .contains("Soft Error: run_args_without_separator")
    );
    Ok(())
}

#[buck_test]
async fn test_input(buck: &Buck) -> anyhow::Result<()> {
    buck.run(&["root//:check_input_test"]).stdin("test").await?;
    Ok(())
}

#[buck_test]
async fn test_change_cwd(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:print_cwd",
            &format!("--chdir={}", tmp_path.display()),
        ])
        .await?;
    assert_eq!(
        std::fs::canonicalize(tmp_path)?,
        std::fs::canonicalize(result.stdout.trim())?
    );
    Ok(())
}

#[buck_test]
async fn test_dont_change_cwd(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.run(&["root//:print_cwd"]).await?;
    assert_eq!(buck.cwd(), Path::new(result.stdout.trim()));
    Ok(())
}
