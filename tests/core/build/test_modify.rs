/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;

async fn modify_genrule(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:writer"]).await?;
    let output = result.build_report()?.output_for_target("root//:writer")?;
    assert_eq!(std::fs::read_to_string(output)?, "HELLO\n");

    // Change "HELLO" in TARGETS to "GOODBYE"
    let targets = buck.cwd().join("TARGETS.fixture");
    let content = std::fs::read_to_string(&targets)?;
    std::fs::write(&targets, content.replace("HELLO", "GOODBYE"))?;

    let result = buck.build(&["//:writer"]).await?;
    let output = result.build_report()?.output_for_target("root//:writer")?;
    assert_eq!(std::fs::read_to_string(output)?, "GOODBYE\n");
    Ok(())
}

#[buck_test(data_dir = "modify")]
async fn test_modify_genrule(buck: &Buck) -> anyhow::Result<()> {
    modify_genrule(buck).await
}

#[buck_test(data_dir = "modify")]
async fn test_modify_src(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:mysrcrule"]).await?;
    let output = result
        .build_report()?
        .output_for_target("root//:mysrcrule")?;
    assert_eq!(std::fs::read_to_string(output)?, "HELLO\n");

    std::fs::write(buck.cwd().join("src.txt"), "GOODBYE\n")?;
    let result = buck.build(&["//:mysrcrule"]).await?;
    let output = result
        .build_report()?
        .output_for_target("root//:mysrcrule")?;
    assert_eq!(std::fs::read_to_string(output)?, "GOODBYE\n");
    Ok(())
}

#[buck_test(data_dir = "modify")]
async fn test_modify_genrule_notify(buck: &Buck) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"\n[buck2]\nfile_watcher = notify")?;
    buck.kill().await?; // Ensure the config gets picked up
    modify_genrule(buck).await
}

#[buck_test(data_dir = "modify")]
async fn test_modify_directory(buck: &Buck) -> anyhow::Result<()> {
    // Test for the bug reported in T99593442
    std::fs::create_dir(buck.cwd().join("a_dir"))?;
    std::fs::write(buck.cwd().join("a_dir").join("test.txt"), "test")?;
    buck.build(&["//:writer"]).await?;
    // Remove a directory, and change a file, so the file gets spotted,
    // and we'd better note that the directory no longer exists
    std::fs::remove_file(buck.cwd().join("a_dir").join("test.txt"))?;
    std::fs::remove_dir(buck.cwd().join("a_dir"))?;
    buck.build(&["//:writer"]).await?;
    Ok(())
}

#[buck_test(data_dir = "modify_file_during_build")]
async fn test_modify_file_during_build(buck: &Buck) -> anyhow::Result<()> {
    // We need to write some random stuff to the file first so that Buck will
    // have to attempt to upload it to RE (which will fail because by that time
    // we will have overwritten it with other content).
    std::fs::write(buck.cwd().join("text"), random_string())?;

    buck.build(&["//:check"])
        .await
        .expect_failure()
        .assert_stderr_matches("modified files while the build was in progress");
    Ok(())
}

#[buck_test(data_dir = "modify_file_during_build")]
async fn test_file_notify(buck: &Buck) -> anyhow::Result<()> {
    // We need to write some random stuff to the file first so that Buck will
    // have to attempt to upload it to RE (which will fail because by that time
    // we will have overwritten it with other content).
    std::fs::write(buck.cwd().join("text"), random_string())?;

    buck.build(&["//:check"])
        .await
        .expect_failure()
        .assert_stderr_matches("modified files while the build was in progress");
    Ok(())
}
