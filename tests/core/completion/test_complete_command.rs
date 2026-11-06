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

async fn complete_test(
    buck: &Buck,
    input: &str,
    expected: &[&str],
    cwd: &str,
) -> anyhow::Result<()> {
    let res = buck.complete(&["--target", input]).rel_cwd(cwd).await?;
    assert_eq!(res.stdout.lines().collect::<Vec<_>>(), expected);
    Ok(())
}

async fn flagfile_complete_test(
    buck: &Buck,
    input: &str,
    expected: &[&str],
    cwd: &str,
) -> anyhow::Result<()> {
    // Pass the partial as a single `--flagfile=<value>` token (the form the shell
    // wrappers use). Passing it as two tokens would let buck2's own argfile
    // expansion try to read a partial `@...` value as a real flagfile.
    let res = buck
        .complete(&[&format!("--flagfile={input}")])
        .rel_cwd(cwd)
        .await?;
    assert_eq!(res.stdout.lines().collect::<Vec<_>>(), expected);
    Ok(())
}

#[buck_test]
async fn test_target_provides_targets_for_path_ending_with_a_colon(
    buck: &Buck,
) -> anyhow::Result<()> {
    complete_test(
        buck,
        "baredir0/buckdir0b:",
        &[
            "baredir0/buckdir0b:target1",
            "baredir0/buckdir0b:target2",
            "baredir0/buckdir0b:target3",
        ],
        "",
    )
    .await
}

#[buck_test]
async fn test_provides_targets_in_nested_cell(buck: &Buck) -> anyhow::Result<()> {
    complete_test(
        buck,
        "buck2:",
        &["buck2:buck2", "buck2:symlinked_buck2_and_tpx"],
        "cell1",
    )
    .await
}

#[buck_test]
async fn test_completes_a_partial_target(buck: &Buck) -> anyhow::Result<()> {
    complete_test(buck, "buck2:bu", &["buck2:buck2"], "cell1").await
}

#[buck_test]
async fn test_completes_targets_for_fully_qualified_cell(buck: &Buck) -> anyhow::Result<()> {
    complete_test(buck, "cell1//:", &["cell1//:target1"], "").await
}

#[buck_test]
async fn test_completes_other_cell_from_subdirectory(buck: &Buck) -> anyhow::Result<()> {
    complete_test(
        buck,
        "cell1//buck2:",
        &["cell1//buck2:buck2", "cell1//buck2:symlinked_buck2_and_tpx"],
        "baredir0",
    )
    .await
}

#[buck_test]
async fn test_expands_cell_to_canonical_with_colon(buck: &Buck) -> anyhow::Result<()> {
    complete_test(
        buck,
        "cell1/buck2:",
        &["cell1//buck2:buck2", "cell1//buck2:symlinked_buck2_and_tpx"],
        "",
    )
    .await
}

#[buck_test]
async fn test_expands_cell_to_canonical_with_partial_target(buck: &Buck) -> anyhow::Result<()> {
    complete_test(buck, "cell1/buck2:bu", &["cell1//buck2:buck2"], "").await
}

#[buck_test]
async fn test_expands_target_for_bare_colon(buck: &Buck) -> anyhow::Result<()> {
    complete_test(
        buck,
        ":",
        &[":buck2", ":symlinked_buck2_and_tpx"],
        "cell1/buck2",
    )
    .await
}

#[buck_test]
async fn test_target_completion_with_aliased_cells(buck: &Buck) -> anyhow::Result<()> {
    complete_test(
        buck,
        "cell1_alias//buck2:",
        &[
            "cell1_alias//buck2:buck2",
            "cell1_alias//buck2:symlinked_buck2_and_tpx",
        ],
        "cell1/buck2/fake_prelude",
    )
    .await
}

#[buck_test]
async fn test_flagfile_completes_a_directory_fragment(buck: &Buck) -> anyhow::Result<()> {
    flagfile_complete_test(buck, "baredir0/bare", &["baredir0/baredir0a/"], "").await
}

#[buck_test]
async fn test_flagfile_completes_plain_files_as_flagfiles(buck: &Buck) -> anyhow::Result<()> {
    flagfile_complete_test(
        buck,
        "baredir0/baredir0a/unus",
        &["baredir0/baredir0a/unused"],
        "",
    )
    .await
}

#[buck_test]
async fn test_flagfile_preserves_at_prefix_and_completes_a_fragment(
    buck: &Buck,
) -> anyhow::Result<()> {
    flagfile_complete_test(
        buck,
        "@baredir0/baredir0a/unus",
        &["@baredir0/baredir0a/unused"],
        "",
    )
    .await
}
