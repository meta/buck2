/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;

/// The `srcs` of the one target printed with `--output-attribute srcs`, as project paths.
fn srcs(res: &BuckOutput) -> anyhow::Result<Vec<String>> {
    let targets: serde_json::Value = serde_json::from_str(&res.stdout)?;
    Ok(targets[0]["srcs"]
        .as_array()
        .expect("a list of sources")
        .iter()
        .map(|s| {
            let s = s.as_str().expect("a source label");
            s.split_once("//")
                .expect("a source label")
                .1
                .trim_start_matches('/')
                .to_owned()
        })
        .collect())
}

// `.buckconfig` ignores `ignored` and every `*.gen` directory, and un-ignores parts of them
// with `!` patterns. Reading an ignored path is a soft error, which fails any test not marked
// `allow_soft_errors`: a build that passes read nothing ignored.

#[buck_test]
async fn test_unignored_directory(buck: &Buck) -> anyhow::Result<()> {
    // `!kept.gen` has no glob characters, so it covers the directory and everything in it.
    let res = buck
        .targets(&["//kept.gen:t", "--output-attribute", "srcs"])
        .await?;
    assert_eq!(srcs(&res)?, ["kept.gen/TARGETS.fixture", "kept.gen/sub/f"]);
    Ok(())
}

#[buck_test]
async fn test_unignored_file(buck: &Buck) -> anyhow::Result<()> {
    // `!shown/*.gen` and `!shown/*.gen/keep` un-ignore one file. The rest of the directory
    // stays ignored, in globs as in explicit reads.
    buck.build(&["//:keep"]).await?;
    let res = buck.build(&["//:other"]).await.expect_failure();
    assert!(
        res.stderr
            .contains("`shown/x.gen/other` matches the `project.ignore` pattern `**/*.gen/**`"),
        "{}",
        res.stderr
    );
    let res = buck
        .targets(&["//:globbed", "--output-attribute", "srcs"])
        .await?;
    assert_eq!(srcs(&res)?, ["shown/x.gen/keep"]);
    Ok(())
}

#[buck_test]
async fn test_unignored_file_changes_are_seen(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build_without_report(&["//:keep", "--out", "-"])
        .await?;
    assert_eq!(res.stdout.trim(), "KEEP");
    std::fs::write(
        buck.cwd().join("shown").join("x.gen").join("keep"),
        "CHANGED",
    )?;
    let res = buck
        .build_without_report(&["//:keep", "--out", "-"])
        .await?;
    assert_eq!(res.stdout.trim(), "CHANGED");
    Ok(())
}

#[buck_test]
async fn test_no_unignoring_under_an_ignored_directory(buck: &Buck) {
    // `!ignored/inside` does nothing while `ignored` stays ignored.
    let res = buck.targets(&["//ignored/inside:"]).await.expect_failure();
    assert!(
        res.stderr
            .contains("dir `root//ignored` does not exist (project.ignore contains `ignored`)"),
        "{}",
        res.stderr
    );
}
