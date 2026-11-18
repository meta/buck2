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

/// Test basic --json-report with only compatible targets
#[buck_test]
async fn test_ctargets_json_report_basic(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//a:target2",
            "--target-platforms=root//:p",
            "--json-report",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/basic.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/basic.stderr.golden",
    );
    Ok(())
}

/// Test --json-report with incompatible targets
#[buck_test]
async fn test_ctargets_json_report_with_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//a:macos_only",
            "//a:target2",
            "--target-platforms=root//:linux_platform",
            "--json-report",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/with_incompatible.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/with_incompatible.stderr.golden",
    );
    Ok(())
}

/// Test --json-report with transitively incompatible targets
#[buck_test]
async fn test_ctargets_json_report_with_transitive_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//c:depends_on_incompatible",
            "//a:target2",
            "--target-platforms=root//:linux_platform",
            "--keep-going",
            "--json-report",
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

/// Test --json-report with errors (should only appear in stderr, not JSON)
#[buck_test]
async fn test_ctargets_json_report_with_errors_and_keep_going(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//b:any",
            "//a:target2",
            "--target-platforms=root//:p",
            "--keep-going",
            "--json-report",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/with_errors.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/with_errors.stderr.golden",
    );
    Ok(())
}

/// Test --json-report with mix of compatible, incompatible, and errors
#[buck_test]
async fn test_ctargets_json_report_mixed(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//a:macos_only",
            "//b:any",
            "//a:target2",
            "//c:depends_on_incompatible",
            "--target-platforms=root//:linux_platform",
            "--keep-going",
            "--json-report",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/mixed.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/mixed.stderr.golden",
    );
    Ok(())
}

/// Test --json-report when all targets are incompatible
#[buck_test]
async fn test_ctargets_json_report_only_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:macos_only",
            "--target-platforms=root//:linux_platform",
            "--json-report",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/only_incompatible.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/only_incompatible.stderr.golden",
    );
    Ok(())
}

/// Test --json-report with attribute filtering
#[buck_test]
async fn test_ctargets_json_report_with_attributes(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "//a:target1",
            "//a:target2",
            "--target-platforms=root//:p",
            "--json-report",
            "--output-attribute",
            "^name$",
        ])
        .await?;

    golden_replace_cfg_hash(&result.stdout, "golden/with_attributes.stdout.golden");
    golden(
        &sanitize_stderr(&result.stderr),
        "golden/with_attributes.stderr.golden",
    );
    Ok(())
}
