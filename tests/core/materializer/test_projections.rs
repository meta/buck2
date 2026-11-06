/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A projection is a subpath of another target's directory artifact. These tests
//! consume projections from local actions, which forces their inputs onto disk, and
//! check that what those actions read is the current content: when one directory
//! is consumed at two granularities, when the projected subpath or the directory's
//! shape changes between builds, and across a daemon restart.

use std::fs::read_to_string;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_in_file;

/// Build `target` and return what its action read from its inputs.
async fn read_consumer(buck: &Buck, target: &str) -> anyhow::Result<String> {
    let result = buck.build(&[target]).await?;
    Ok(read_to_string(
        result.build_report()?.output_for_target(target)?,
    )?)
}

#[buck_test]
async fn test_two_projections_of_one_artifact(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["root//:split_a", "root//:split_b"]).await?;
    let report = result.build_report()?;

    assert_eq!(
        read_to_string(report.output_for_target("root//:split_a")?)?,
        "split-a"
    );
    assert_eq!(
        read_to_string(report.output_for_target("root//:split_b")?)?,
        "split-b"
    );
    Ok(())
}

#[buck_test]
async fn test_projection_and_whole_artifact_in_one_build(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["root//:mixed_projection", "root//:mixed_whole"])
        .await?;
    let report = result.build_report()?;

    assert_eq!(
        read_to_string(report.output_for_target("root//:mixed_projection")?)?,
        "mixed-a"
    );
    assert_eq!(
        read_to_string(report.output_for_target("root//:mixed_whole")?)?,
        "mixed-a|mixed-b"
    );
    Ok(())
}

#[buck_test]
async fn test_projected_subpath_changes_between_builds(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:moving_consumer";
    let targets_file = buck.cwd().join("TARGETS.fixture");

    assert_eq!(read_consumer(buck, target).await?, "moving-a-1");

    // Both the projected subpath and the whole directory's contents change, so
    // neither the old subpath's bytes nor the old bytes at the new subpath are
    // an acceptable answer.
    replace_in_file(
        r#"consume(name = "moving_consumer", dep = ":moving", projections = ["a"])"#,
        r#"consume(name = "moving_consumer", dep = ":moving", projections = ["b"])"#,
        &targets_file,
    );
    replace_in_file("moving-a-1", "moving-a-2", &targets_file);
    replace_in_file("moving-b-1", "moving-b-2", &targets_file);

    assert_eq!(read_consumer(buck, target).await?, "moving-b-2");
    Ok(())
}

#[buck_test]
async fn test_projection_consumed_after_daemon_restart(buck: &Buck) -> anyhow::Result<()> {
    // Declare the directory artifact without putting it on disk, so that the
    // work of materializing it falls to the restarted daemon.
    let result = buck
        .build(&["root//:restart", "--materializations=None"])
        .await?;
    let produced = result.build_report()?.output_for_target("root//:restart")?;
    assert!(!produced.exists());

    buck.kill().await?;

    assert_eq!(
        read_consumer(buck, "root//:restart_consumer").await?,
        "restart-a"
    );
    assert!(produced.exists());
    Ok(())
}

#[buck_test]
async fn test_whole_artifact_after_projection_at_newer_contents(buck: &Buck) -> anyhow::Result<()> {
    let targets_file = buck.cwd().join("TARGETS.fixture");
    let whole = "root//:interleaved_whole";
    let projection = "root//:interleaved_projection";

    assert_eq!(
        read_consumer(buck, whole).await?,
        "interleaved-a-1|interleaved-b-1"
    );

    replace_in_file("interleaved-a-1", "interleaved-a-2", &targets_file);
    replace_in_file("interleaved-b-1", "interleaved-b-2", &targets_file);

    // Only the projection is consumed at the new contents, so this build need
    // not bring `b` up to date on disk...
    assert_eq!(read_consumer(buck, projection).await?, "interleaved-a-2");
    // ...but the whole-directory consumer must not then see a current `a` next
    // to a stale `b`.
    assert_eq!(
        read_consumer(buck, whole).await?,
        "interleaved-a-2|interleaved-b-2"
    );
    Ok(())
}

#[buck_test]
async fn test_projected_subpath_did_not_exist_before(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:growing_consumer";
    let targets_file = buck.cwd().join("TARGETS.fixture");

    assert_eq!(read_consumer(buck, target).await?, "growing-a");

    replace_in_file(
        r#"contents = {"a": "growing-a"}"#,
        r#"contents = {"a": "growing-a", "b": "growing-b"}"#,
        &targets_file,
    );
    replace_in_file(
        r#"consume(name = "growing_consumer", dep = ":growing", projections = ["a"])"#,
        r#"consume(name = "growing_consumer", dep = ":growing", projections = ["b"])"#,
        &targets_file,
    );

    assert_eq!(read_consumer(buck, target).await?, "growing-b");
    Ok(())
}
