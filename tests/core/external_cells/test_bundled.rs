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
use std::path::PathBuf;

use buck2_e2e::prelude::*;

fn read_trimmed(path: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(path)?.trim().to_owned())
}

#[buck_test]
async fn test_buckconfig_works_in_external_cells(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["config", "--cell", "test_bundled_cell", "user_section.key"])
        .await?;
    assert!(result.stdout.contains("key = value"), "{}", result.stdout);
    Ok(())
}

#[buck_test]
async fn test_uquery(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.uquery(&["deps(other//:other_alias)"]).await?;
    assert_eq!(
        result.stdout.split_whitespace().collect::<Vec<_>>(),
        [
            "test_bundled_cell//dir:test_hidden",
            "test_bundled_cell//dir:test",
            "other//:other_alias",
        ]
    );
    let result = buck
        .uquery(&["deps(test_bundled_cell//dir:test)"])
        .rel_cwd("other")
        .await?;
    assert_eq!(
        result.stdout.split_whitespace().collect::<Vec<_>>(),
        [
            "test_bundled_cell//dir:test_hidden",
            "test_bundled_cell//dir:test",
        ]
    );
    Ok(())
}

#[buck_test]
async fn test_build_local(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build_without_report(&[
            "--show-full-simple-output",
            "--local-only",
            "other//:other_alias",
        ])
        .await?;
    let p = PathBuf::from(result.stdout.trim());
    assert_eq!(
        read_trimmed(&p)?,
        ["value", "6", "foobar", "foobar2"].join("\n")
    );
    Ok(())
}

#[buck_test]
async fn test_build_remote(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build_without_report(&[
            "--show-full-simple-output",
            "--remote-only",
            "other//:other_alias",
        ])
        .await?;
    let p = PathBuf::from(result.stdout.trim());
    assert_eq!(
        read_trimmed(&p)?,
        ["value", "6", "foobar", "foobar2"].join("\n")
    );
    Ok(())
}

#[buck_test]
async fn test_materialize_source_directly(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build_without_report(&[
            "--show-full-simple-output",
            "test_bundled_cell//dir:exported",
        ])
        .await?;
    let p = PathBuf::from(result.stdout.trim());
    let s = p.to_string_lossy();
    assert!(
        s.contains(&format!(
            "external_cells{}bundled",
            std::path::MAIN_SEPARATOR
        )),
        "{s}"
    );
    assert!(s.ends_with("src.txt"), "{s}");
    assert_eq!(read_trimmed(&p)?, "foobar");
    Ok(())
}

#[buck_test]
async fn test_expand_external_cell(buck: &Buck) -> anyhow::Result<()> {
    buck.expand_external_cell(&["test_bundled_cell"]).await?;
    assert!(
        buck.cwd()
            .join("test_bundled_cell")
            .join(".buckconfig")
            .exists()
    );

    // Remove the external cell declaration
    std::fs::rename(
        buck.cwd().join(".buckconfig_no_external"),
        buck.cwd().join(".buckconfig"),
    )?;
    std::fs::write(
        buck.cwd()
            .join("test_bundled_cell")
            .join("dir")
            .join("src.txt"),
        "foobar3\n",
    )?;

    let result = buck
        .build_without_report(&["--show-full-simple-output", "other//:other_alias"])
        .await?;
    let p = PathBuf::from(result.stdout.trim());
    assert_eq!(
        read_trimmed(&p)?,
        ["value", "6", "foobar3", "foobar2"].join("\n")
    );
    Ok(())
}
