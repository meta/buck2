/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Materializing an artifact must not leave content from a previous layout
//! underneath it. These tests move a target's artifact boundaries between two
//! builds - a directory artifact becomes files underneath it, or the reverse - and
//! check that the second build's outputs are correct on disk and that the first
//! build's layout does not survive underneath them.
//!
//! Both directions also run with a daemon restart between the two builds, so that
//! the second build has only persisted state to work from.

use std::fs::read_to_string;
use std::path::PathBuf;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_in_file;

/// Switch `//:{name}` to the `{name}_2` layout defined in `defs.bzl`.
fn use_second_layout(buck: &Buck, name: &str) {
    replace_in_file(
        &format!(r#"variant = "{name}_1""#),
        &format!(r#"variant = "{name}_2""#),
        &buck.cwd().join("TARGETS.fixture"),
    );
}

async fn build_outputs(buck: &Buck, target: &str) -> anyhow::Result<Vec<PathBuf>> {
    let result = buck.build(&[target]).await?;
    let mut outputs = result
        .build_report()?
        .outputs_for_target(target, "DEFAULT")?;
    outputs.sort();
    Ok(outputs)
}

async fn dir_to_files(buck: &Buck, restart: bool) -> anyhow::Result<()> {
    let target = "root//:dir_to_files";

    let [out] = <[PathBuf; 1]>::try_from(build_outputs(buck, target).await?)
        .map_err(|outputs| anyhow::anyhow!("expected one output: {outputs:?}"))?;
    assert_eq!(read_to_string(out.join("a"))?, "A1");
    assert_eq!(read_to_string(out.join("sub").join("keep"))?, "K1");
    assert_eq!(read_to_string(out.join("sub").join("stale"))?, "S1");

    use_second_layout(buck, "dir_to_files");
    if restart {
        buck.kill().await?;
    }

    // The new artifacts land inside the path the directory artifact used to
    // occupy; without this the checks below would be vacuous.
    assert_eq!(
        build_outputs(buck, target).await?,
        [out.join("a"), out.join("sub")]
    );
    assert_eq!(read_to_string(out.join("a"))?, "A2");
    assert_eq!(read_to_string(out.join("sub").join("keep"))?, "K2");
    assert!(!out.join("sub").join("stale").exists());
    Ok(())
}

async fn files_to_dir(buck: &Buck, restart: bool) -> anyhow::Result<()> {
    let target = "root//:files_to_dir";

    let [a, b] = <[PathBuf; 2]>::try_from(build_outputs(buck, target).await?)
        .map_err(|outputs| anyhow::anyhow!("expected two outputs: {outputs:?}"))?;
    let out = a.parent().expect("an output has a parent").to_owned();
    assert_eq!([&a, &b], [&out.join("a"), &out.join("b")]);
    assert_eq!(read_to_string(&a)?, "A1");
    assert_eq!(read_to_string(&b)?, "B1");

    use_second_layout(buck, "files_to_dir");
    if restart {
        buck.kill().await?;
    }

    assert_eq!(
        build_outputs(buck, target).await?,
        std::slice::from_ref(&out)
    );
    assert_eq!(read_to_string(out.join("a"))?, "A2");
    assert_eq!(read_to_string(out.join("c"))?, "C2");
    // `b` is not part of the directory artifact, so it must not be visible to
    // anything reading the directory.
    assert!(!out.join("b").exists());
    Ok(())
}

#[buck_test]
async fn test_directory_artifact_becomes_files_inside_it(buck: &Buck) -> anyhow::Result<()> {
    dir_to_files(buck, false).await
}

#[buck_test]
async fn test_directory_artifact_becomes_files_inside_it_across_restart(
    buck: &Buck,
) -> anyhow::Result<()> {
    dir_to_files(buck, true).await
}

#[buck_test]
async fn test_files_become_a_directory_artifact_around_them(buck: &Buck) -> anyhow::Result<()> {
    files_to_dir(buck, false).await
}

#[buck_test]
async fn test_files_become_a_directory_artifact_around_them_across_restart(
    buck: &Buck,
) -> anyhow::Result<()> {
    files_to_dir(buck, true).await
}

#[buck_test]
async fn test_directory_artifact_contents_shrink(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:shrinking_dir";

    let [out] = <[PathBuf; 1]>::try_from(build_outputs(buck, target).await?)
        .map_err(|outputs| anyhow::anyhow!("expected one output: {outputs:?}"))?;
    assert_eq!(read_to_string(out.join("keep.txt"))?, "KEEP1");
    assert_eq!(read_to_string(out.join("stale.txt"))?, "STALE");

    use_second_layout(buck, "shrinking_dir");

    assert_eq!(
        build_outputs(buck, target).await?,
        std::slice::from_ref(&out)
    );
    assert_eq!(read_to_string(out.join("keep.txt"))?, "KEEP2");
    assert!(!out.join("stale.txt").exists());
    Ok(())
}

#[buck_test]
async fn test_local_consumer_does_not_see_removed_entry(buck: &Buck) -> anyhow::Result<()> {
    // The tests above look at the disk state a final materialization leaves
    // behind. This one looks at what a local action consuming the directory
    // artifact as an input can see in it.
    let target = "root//:listed_dir_listing";

    let result = buck.build(&[target]).await?;
    let listing = result.build_report()?.output_for_target(target)?;
    assert_eq!(read_to_string(listing)?, "keep,stale");

    use_second_layout(buck, "listed_dir");

    let result = buck.build(&[target]).await?;
    assert_eq!(
        read_to_string(result.build_report()?.output_for_target(target)?)?,
        "keep"
    );
    Ok(())
}
