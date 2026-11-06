/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::prelude::*;

async fn build_output(buck: &Buck, target: &str) -> anyhow::Result<PathBuf> {
    let result = buck.build(&[target, "--show-output"]).await?;
    let outputs = result.target_to_build_output()?;
    let path = outputs
        .get(target)
        .with_context(|| format!("No output for `{target}`"))?;
    Ok(buck.cwd().join(path))
}

// Symlink materialization assertions are unreliable on Windows.
#[buck_test(skip_for_os = ["windows"])]
async fn test_assembled_dir_mixes_copies_and_symlinks(buck: &Buck) -> anyhow::Result<()> {
    let out = build_output(buck, "root//:mixed").await?;
    assert!(out.is_dir());

    // `assembled_dir.copy` entries are materialized as real files, with the
    // source's bytes, at their entry paths (including nested ones).
    for (name, data) in [
        ("bin/exe", "exe-bytes"),
        ("bin/exe.resources.json", "manifest-bytes"),
        ("src_copy.txt", "source-file-bytes\n"),
    ] {
        let entry = out.join(name);
        assert!(entry.is_file(), "{name}");
        assert!(!entry.is_symlink(), "{name}");
        assert_eq!(std::fs::read_to_string(&entry)?, data, "{name}");
    }

    // `assembled_dir.symlink` entries are materialized as symlinks that
    // resolve to the source artifact's bytes.
    for (name, data) in [
        ("deep/nested/link", "exe-bytes"),
        ("src_link.txt", "source-file-bytes\n"),
    ] {
        let entry = out.join(name);
        assert!(entry.is_symlink(), "{name}");
        assert_eq!(std::fs::read_to_string(&entry)?, data, "{name}");
    }

    // A symlinked directory artifact resolves as a directory.
    let res_dir = out.join("res");
    assert!(res_dir.is_symlink());
    assert_eq!(
        std::fs::read_to_string(res_dir.join("data.txt"))?,
        "resource-bytes"
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_assembled_dir_is_a_usable_input(buck: &Buck) -> anyhow::Result<()> {
    // A downstream action can read both copied and symlinked entries through
    // the assembled dir (i.e. the entries' sources are tracked as inputs).
    let out = build_output(buck, "root//:consumed").await?;
    assert_eq!(
        std::fs::read_to_string(out)?,
        "exe-bytes|source-file-bytes\n"
    );
    Ok(())
}

#[buck_test]
async fn test_assembled_dir_rejects_overlapping_paths(buck: &Buck) {
    buck.build(&["root//:overlap_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches("must be non-overlapping");
}

#[buck_test]
async fn test_assembled_dir_rejects_empty_path(buck: &Buck) {
    buck.build(&["root//:empty_path_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches("must not be empty");
}

#[buck_test]
async fn test_assembled_dir_rejects_untyped_entries(buck: &Buck) {
    // A bare artifact is not a valid entry: contents values must be built
    // with `assembled_dir.copy(...)` / `assembled_dir.symlink(...)`.
    buck.build(&["root//:untyped_entry_fail"])
        .await
        .expect_failure()
        .assert_stderr_matches("AssembledDirEntry");
}
