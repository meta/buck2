/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use buck2_e2e::prelude::*;

fn is_buck_path(x: &str) -> bool {
    if cfg!(windows) {
        x.contains("\\.buck\\buckd\\") || x.contains("\\buck-out\\")
    } else {
        x.contains("/.buck/buckd/") || x.contains("/buck-out/")
    }
}

fn list_dir(path: &Path) -> std::io::Result<Vec<String>> {
    std::fs::read_dir(path)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect()
}

fn assert_all_paths_exist(paths: impl IntoIterator<Item = impl AsRef<Path>>) {
    for path in paths {
        let path = path.as_ref();
        assert!(path.exists(), "`{}` does not exist", path.display());
    }
}

fn assert_all_paths_do_not_exist(
    paths: impl IntoIterator<Item = impl AsRef<Path>>,
) -> anyhow::Result<()> {
    for path in paths {
        let path = path.as_ref();
        if path.join("buckd.lifecycle").exists() {
            // Clean keeps lifecycle file in daemon dir.
            assert_eq!(list_dir(path)?, ["buckd.lifecycle"]);
        } else if path.ends_with("buck-out/v2/log") {
            // Log dir should contain one entry, for the clean command itself.
            assert_eq!(list_dir(path)?.len(), 1);
        } else {
            assert!(!path.exists(), "`{}` exists", path.display());
        }
    }
    Ok(())
}

/// Kept as paths: on Windows the report mixes `\` and `/`, so only a component-wise
/// `Path::starts_with` matches them against the paths `buck2 clean` prints.
async fn trivial_build_outputs(buck: &Buck, args: &[&str]) -> anyhow::Result<Vec<PathBuf>> {
    let build_result = buck.build(args).await?;
    let build_report = build_result.build_report()?;
    build_report.outputs_for_target("root//:trivial_build", "DEFAULT")
}

#[buck_test]
async fn test_clean(buck: &Buck) -> anyhow::Result<()> {
    let build_report_outputs = trivial_build_outputs(buck, &["root//:trivial_build"]).await?;

    let clean_result = buck.clean(&[]).await?;
    let clean_paths: Vec<&str> = clean_result
        .stderr
        .split('\n')
        .filter(|line| !line.is_empty())
        .collect();

    for output in &build_report_outputs {
        assert!(
            clean_paths.iter().any(|p| output.starts_with(p)),
            "`{}` is not under any of {clean_paths:?}",
            output.display()
        );
    }

    assert_all_paths_do_not_exist(&build_report_outputs)
}

#[buck_test]
async fn test_clean_dry_run(buck: &Buck) -> anyhow::Result<()> {
    let build_report_outputs =
        trivial_build_outputs(buck, &["root//:trivial_build", "--show-output"]).await?;

    let dry_clean_result = buck.clean(&["--dry-run"]).await?;

    let dry_clean_paths: BTreeSet<String> = dry_clean_result
        .stderr
        .split('\n')
        .filter(|line| is_buck_path(line))
        .map(str::to_owned)
        .collect();
    assert_all_paths_exist(&dry_clean_paths);

    for output in &build_report_outputs {
        assert!(
            dry_clean_paths.iter().any(|p| output.starts_with(p)),
            "`{}` is not under any of {dry_clean_paths:?}",
            output.display()
        );
    }

    assert_all_paths_exist(&build_report_outputs);

    // Run clean without dry-run and make sure all files are removed now
    let clean_result = buck.clean(&[]).await?;
    let clean_paths: BTreeSet<String> = clean_result
        .stderr
        .split('\n')
        .filter(|line| is_buck_path(line))
        .map(str::to_owned)
        .collect();
    // dry_clean_paths and clean_paths should be the same
    for clean_path in &clean_paths {
        assert!(dry_clean_paths.contains(clean_path), "{clean_path}");
    }
    for dry_clean_path in &dry_clean_paths {
        assert!(clean_paths.contains(dry_clean_path), "{dry_clean_path}");
    }

    assert_all_paths_do_not_exist(&clean_paths)
}

#[buck_test]
async fn test_isolation_dir_reserved_prefix_rejected(buck: &mut Buck) {
    buck.set_isolation_prefix("._buck2_anything");
    let result = buck.build(&["root//:trivial_build"]).await;
    // The fixture teardown runs `buck clean`, which would itself trip the
    // reserved-name rejection with the prefix still set.
    buck.set_isolation_prefix("v2");
    result
        .expect_failure()
        .assert_stderr_matches("reserved for buck2");
}

/// Test that buck2 clean --background moves buck-out to trash and deletes it.
#[buck_test]
async fn test_clean_background(buck: &Buck) -> anyhow::Result<()> {
    let build_report_outputs = trivial_build_outputs(buck, &["root//:trivial_build"]).await?;

    // Run clean with --background flag
    let clean_result = buck.clean(&["--background"]).await?;

    // Check that the output contains the expected messages
    assert!(
        clean_result
            .stderr
            .contains("Buck-out moved to trash. Now cleaning up...")
    );
    assert!(
        clean_result
            .stderr
            .contains("Tip: Use Ctrl-Z to put this in the background")
    );
    assert!(
        clean_result
            .stderr
            .contains("You can run other buck2 commands while this completes.")
    );

    // Verify all build outputs are eventually deleted
    assert_all_paths_do_not_exist(&build_report_outputs)
}
