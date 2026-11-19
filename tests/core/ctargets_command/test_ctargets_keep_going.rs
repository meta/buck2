/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::golden_replace_cfg_hash;
use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;

/// Test that package parse errors appear in JSON output with --keep-going
#[buck_test]
async fn test_ctargets_keep_going_parse_error_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//b:any",
            "//a:target2",
            "--target-platforms=root//:p",
            "--keep-going",
            "--json",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/parse_error_json.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/parse_error_json.stderr.golden",
    );
    Ok(())
}

/// Test that parse errors go to stderr in plain text mode
#[buck_test]
async fn test_ctargets_keep_going_parse_error_plain_text(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//b:any",
            "//a:target2",
            "--target-platforms=root//:p",
            "--keep-going",
        ])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        "golden/parse_error_plain_text.stdout.golden",
    );
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/parse_error_plain_text.stderr.golden",
    );
    Ok(())
}

/// Test that missing packages are handled with --keep-going
#[buck_test]
async fn test_ctargets_keep_going_missing_package(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//nonexistent_package:target",
            "//a:target2",
            "--target-platforms=root//:p",
            "--keep-going",
            "--json",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/missing_package.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/missing_package.stderr.golden",
    );
    Ok(())
}

/// Test that incompatible targets and errors work together correctly
#[buck_test]
async fn test_ctargets_keep_going_with_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//a:macos_only",
            "//b:any",
            "--target-platforms=root//:linux_platform",
            "--keep-going",
            "--json",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/with_incompatible.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/with_incompatible.stderr.golden",
    );
    Ok(())
}

/// Test edge case with single failing target
#[buck_test]
async fn test_ctargets_keep_going_single_error(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//b:does_not_matter",
            "--target-platforms=root//:p",
            "--keep-going",
            "--json",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/single_error.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/single_error.stderr.golden",
    );
    Ok(())
}

/// Test errors from multiple different packages
#[buck_test]
async fn test_ctargets_keep_going_multiple_packages_with_errors(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//b:any",
            "//d:exists",
            "//nonexistent_package:any",
            "--target-platforms=root//:p",
            "--keep-going",
            "--json",
        ])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        "golden/multiple_packages_with_errors.stdout.golden",
    );
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/multiple_packages_with_errors.stderr.golden",
    );
    Ok(())
}

/// Test transitive incompatibility
#[buck_test]
async fn test_ctargets_keep_going_transitive_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//c:depends_on_incompatible",
            "//a:target2",
            "--target-platforms=root//:linux_platform",
            "--keep-going",
            "--json",
        ])
        .await?;

    golden_replace_cfg_hash(
        &result.stdout,
        "golden/transitive_incompatible.stdout.golden",
    );
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/transitive_incompatible.stderr.golden",
    );
    Ok(())
}

/// Test modifier conflict: pattern modifiers + global modifiers
#[buck_test]
async fn test_ctargets_keep_going_modifier_conflict(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//a:target2?root//:linux",
            "--modifier",
            "root//:macos",
            "--keep-going",
            "--json",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/modifier_conflict.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/modifier_conflict.stderr.golden",
    );
    Ok(())
}
