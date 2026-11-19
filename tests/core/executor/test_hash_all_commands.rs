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
use buck2_e2e::utils::expect_exec_count;

async fn hash_all_commands(buck: &Buck, local_only: &str) -> anyhow::Result<()> {
    // Expecting a rebuild since the command wasn't hashed previously.
    buck.build(&[
        "//:test",
        "-c",
        "test.seed=123",
        "-c",
        &format!("test.local_only={local_only}"),
    ])
    .await?;
    expect_exec_count(buck, 1).await?;
    // Disable remote cache lookup so we actually utilize the local depfile cache for this test
    // No longer expecting a rebuild.
    let res = buck
        .build(&[
            "//:test",
            "-c",
            "test.seed=456",
            "-c",
            &format!("test.local_only={local_only}"),
            "--no-remote-cache",
        ])
        .await?;
    expect_exec_count(buck, 0).await?;

    // Check that we're matching on just the directory here
    assert!(
        res.stderr
            .contains("Command line and directory have not changed")
    );
    Ok(())
}

#[buck_test(env = [("BUCK_LOG", "buck2_action_impl::actions::impls::run::dep_files=trace")])]
async fn test_hash_all_commands_true(buck: &Buck) -> anyhow::Result<()> {
    hash_all_commands(buck, "true").await
}

#[buck_test(env = [("BUCK_LOG", "buck2_action_impl::actions::impls::run::dep_files=trace")])]
async fn test_hash_all_commands_false(buck: &Buck) -> anyhow::Result<()> {
    hash_all_commands(buck, "false").await
}

#[buck_test]
async fn test_hash_all_commands_key_change(buck: &Buck) -> anyhow::Result<()> {
    // Expecting a rebuild since the command wasn't hashed previously.
    buck.build(&[
        "//:test",
        "-c",
        "test.param=123",
        "-c",
        "test.category=cat1",
    ])
    .await?;
    expect_exec_count(buck, 1).await?;

    // Again expecting a rebuild as the action doesn't match and also it's not
    // even the same action.
    buck.build(&[
        "//:test",
        "-c",
        "test.param=456",
        "-c",
        "test.category=cat2",
    ])
    .await?;
    expect_exec_count(buck, 1).await?;

    // Again expecting a rebuild as the action once again doesn't match, but
    // while is mismatches with the 2nd action (which had a differet key),
    // it does match the 1st action (which had the same key). Since the 2nd
    // action clobbered the output, however, this *must* re-run.
    buck.build(&[
        "//:test",
        "-c",
        "test.param=123",
        "-c",
        "test.category=cat1",
    ])
    .await?;
    expect_exec_count(buck, 1).await?;
    Ok(())
}

#[buck_test]
async fn test_hash_all_commands_key_change_deps(buck: &Buck) -> anyhow::Result<()> {
    // Expecting a rebuild since the command wasn't hashed previously.
    buck.build(&[
        "//:symlink_test",
        "-c",
        "test.param=123",
        "-c",
        "test.category=cat1",
    ])
    .await?;
    expect_exec_count(buck, 1).await?;

    // Again expecting a rebuild because the cache key is different anyway.
    buck.build(&[
        "//:symlink_test",
        "-c",
        "test.param=456",
        "-c",
        "test.category=cat2",
    ])
    .await?;
    expect_exec_count(buck, 1).await?;

    // Not actually expecting a rebuild this time, because the symlink
    // output is unchanged.
    let res = buck
        .build(&[
            "//:symlink_test",
            "-c",
            "test.param=123",
            "-c",
            "test.category=cat1",
        ])
        .await?;
    expect_exec_count(buck, 0).await?;

    // But we should have the right output here
    let build_report = res.build_report()?;
    let output = build_report.output_for_target("//:symlink_test")?;
    assert_eq!(std::fs::read_to_string(output)?.trim_end(), "123");
    Ok(())
}
