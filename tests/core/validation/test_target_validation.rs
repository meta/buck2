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

const MATE_FAILED: &str = "
Validation for `prelude//:mate \\(<unspecified>\\)` failed:

Here I am describing the failure reason

Full validation result is located at";

#[buck_test]
async fn test_validation_affects_build_command(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":plate"])
        .await
        .expect_failure()
        .assert_stderr_matches(MATE_FAILED);
    buck.build(&[":date"]).await?;
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_validation_affects_run_command(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.run(&[":plate"]).await.expect_failure();
    res.assert_stderr_matches(MATE_FAILED);

    let record = res.invocation_record()?;
    assert_eq!(record["errors"].as_array().map(Vec::len), Some(1));

    buck.run(&[":date"]).await?;
    Ok(())
}

#[buck_test(
    write_invocation_record = true,
    env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")],
)]
async fn test_validation_affects_test_command(buck: &mut Buck) -> anyhow::Result<()> {
    // No test executor.
    buck.set_env("BUCK2_TPX", "");
    let res = buck.test(&[":plate"]).await.expect_failure();
    res.assert_stderr_matches(MATE_FAILED);

    let record = res.invocation_record()?;
    assert_eq!(record["errors"].as_array().map(Vec::len), Some(1));

    buck.test(&[":date"]).await?;
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_validation_affects_install_command(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.install(&[":plate"]).await.expect_failure();
    res.assert_stderr_matches("Validation for `prelude//:mate \\(<unspecified>\\)` failed");

    let record = res.invocation_record()?;
    assert_eq!(record["errors"].as_array().map(Vec::len), Some(1));

    // It's too complicated to set up installer properly.
    // We intentionally fail on the installer side, but interpret
    // an attempt to run it as a successful verification.
    let res = buck.install(&[":date"]).await.expect_failure();
    res.assert_stderr_matches("Installer: Incoming connection accepted, now closing it");

    let record = res.invocation_record()?;
    assert_eq!(record["errors"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[buck_test]
async fn test_optional_validation(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":optional_passing"]).await?;

    // Optional validations are not run by default.
    buck.build(&[":optional_failing"]).await?;

    // Expect a failure when run with --enable-optional-validations.
    buck.build(&[
        ":optional_failing",
        "--enable-optional-validations",
        "whistle",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Validation for `.+` failed");
    Ok(())
}

#[buck_test]
async fn test_malformed_validation_result_names_target_and_file(buck: &Buck) {
    // The parse error of a malformed result file names the target that owns the validation
    // and the result file, like a failing validation does.
    let res = buck.build(&[":consumer"]).await.expect_failure();
    res.assert_stderr_matches("JSON content doesn't match schema");
    assert!(res.stderr.contains("prelude//:broken"));
    assert!(res.stderr.contains("flute.json"));
}

/// Valid JSON without a `version` is a schema mismatch; valid JSON written for another
/// version is an incompatible version. Neither is 'invalid JSON'.
#[buck_test]
async fn test_version_and_schema_problems_are_reported_as_documented(buck: &Buck) {
    let res = buck.build(&[":no_version"]).await.expect_failure();
    res.assert_stderr_matches("JSON content doesn't match schema");
    assert!(
        !res.stderr.contains("should contain valid JSON"),
        "{}",
        res.stderr
    );
    let res = buck.build(&[":other_version"]).await.expect_failure();
    res.assert_stderr_matches("Incompatible version of validation result `2`, expected `1`");
    assert!(
        !res.stderr.contains("should contain valid JSON"),
        "{}",
        res.stderr
    );
}
