/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! An artifact that is (or contains) a symlink carries the values its symlinks
//! point at alongside it, so that materializing the symlink also puts something at
//! the other end of it. `test_modify_dep_materialization` in `test_materializer.rs`
//! covers the single-daemon case; these tests cover the cases around it - a daemon
//! restart in the middle, targets that are source files rather than build
//! artifacts, and bare symlink artifacts rather than directories of symlinks.
//!
//! Every read through a symlink is done by a local action copying what it read to
//! its own output, so that the assertion is on what a consumer actually saw.

use buck2_e2e::prelude::*;

/// Build a `check` target and return what its action read.
async fn read_through(buck: &Buck, target: &str) -> anyhow::Result<String> {
    let result = buck.build(&[target]).await?;
    let output = result.build_report()?.output_for_target(target)?;
    Ok(std::fs::read_to_string(output)?.trim().to_owned())
}

fn write(buck: &Buck, name: &str, content: &str) -> std::io::Result<()> {
    std::fs::write(buck.cwd().join(name), format!("{content}\n"))
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_symlinked_dir_to_artifact_across_restart(buck: &Buck) -> anyhow::Result<()> {
    assert_eq!(
        read_through(buck, "root//:check_dir_to_artifact").await?,
        "TEXT"
    );

    // After the restart nothing the previous daemon held in memory is available,
    // so serving the new content has to work from the persisted state alone.
    buck.kill().await?;
    write(buck, "text", "TEXT2")?;
    assert_eq!(
        read_through(buck, "root//:check_dir_to_artifact").await?,
        "TEXT2"
    );

    buck.kill().await?;
    write(buck, "text", "TEXT3")?;
    assert_eq!(
        read_through(buck, "root//:check_dir_to_artifact").await?,
        "TEXT3"
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_restarted_daemon_materializes_symlink_and_its_target(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "root//:dir_to_artifact",
            "root//:remote_text",
            "--materializations=None",
        ])
        .await?;
    let report = result.build_report()?;
    let _link_dir = report.output_for_target("root//:dir_to_artifact")?;
    let target = report.output_for_target("root//:remote_text")?;
    assert!(!target.exists());

    buck.kill().await?;

    // The symlink's target is not on disk (nor, depending on the materializer,
    // the symlink), so the restarted daemon has to put it there before the
    // consumer can read through the link. What it put there is the consumer's
    // to see: a materializer may take it back once the consumer is done.
    assert_eq!(
        read_through(buck, "root//:check_dir_to_artifact").await?,
        "TEXT"
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_symlinked_dir_to_source_file(buck: &Buck) -> anyhow::Result<()> {
    // Nothing in buck-out covers a source file, so there is no artifact for the
    // materializer to chase; the symlink resolves because the source is already
    // where it points.
    assert_eq!(
        read_through(buck, "root//:check_dir_to_source").await?,
        "SOURCE"
    );

    write(buck, "source.txt", "SOURCE2")?;
    assert_eq!(
        read_through(buck, "root//:check_dir_to_source").await?,
        "SOURCE2"
    );

    buck.kill().await?;

    write(buck, "source.txt", "SOURCE3")?;
    assert_eq!(
        read_through(buck, "root//:check_dir_to_source").await?,
        "SOURCE3"
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_symlink_artifact_to_build_artifact(buck: &Buck) -> anyhow::Result<()> {
    assert_eq!(
        read_through(buck, "root//:check_file_to_artifact").await?,
        "TEXT"
    );

    write(buck, "text", "TEXT2")?;
    assert_eq!(
        read_through(buck, "root//:check_file_to_artifact").await?,
        "TEXT2"
    );

    buck.kill().await?;

    write(buck, "text", "TEXT3")?;
    assert_eq!(
        read_through(buck, "root//:check_file_to_artifact").await?,
        "TEXT3"
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_symlink_artifact_to_source_file(buck: &Buck) -> anyhow::Result<()> {
    assert_eq!(
        read_through(buck, "root//:check_file_to_source").await?,
        "SOURCE"
    );

    write(buck, "source.txt", "SOURCE2")?;
    assert_eq!(
        read_through(buck, "root//:check_file_to_source").await?,
        "SOURCE2"
    );

    buck.kill().await?;

    write(buck, "source.txt", "SOURCE3")?;
    assert_eq!(
        read_through(buck, "root//:check_file_to_source").await?,
        "SOURCE3"
    );
    Ok(())
}

/// The symlink's own value is a relative path leading out of it.
#[buck_test(skip_for_os = ["windows"])]
async fn test_symlink_artifact_target_escapes_the_artifact(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["root//:file_to_artifact", "root//:remote_text"])
        .await?;
    let report = result.build_report()?;
    let link = report.output_for_target("root//:file_to_artifact")?;
    let target = report.output_for_target("root//:remote_text")?;

    assert!(link.is_symlink());
    assert!(!std::fs::read_link(&link)?.is_absolute());
    assert_eq!(
        std::fs::canonicalize(&link)?,
        std::fs::canonicalize(&target)?
    );
    assert_eq!(std::fs::read_to_string(&link)?.trim(), "TEXT");
    Ok(())
}
