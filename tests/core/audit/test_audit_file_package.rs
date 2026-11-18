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
use serde_json::Value;
use serde_json::json;

/// Test basic file-package mapping
#[buck_test]
async fn test_audit_file_package_simple(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["file-package", "TARGETS.fixture"]).await?;
    assert!(result.stdout.contains(": root//"));
    Ok(())
}

/// Test file-package mapping with JSON output
#[buck_test]
async fn test_audit_file_package_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["file-package", "TARGETS.fixture", "--json"])
        .await?;

    let data: Value = serde_json::from_str(&result.stdout)?;
    let expected = json!({"TARGETS.fixture": {"package": "root//"}});
    assert_eq!(data, expected, "Expected {expected}, got {data}");
    Ok(())
}

/// Test file-package mapping for a file in 'newcell'
#[buck_test]
async fn test_audit_file_package_newcell_json(buck: &Buck) -> anyhow::Result<()> {
    // Assume 'newcell/TARGETS.fixture' exists in the test workspace
    let result = buck
        .audit(&["file-package", "newcell/TARGETS.fixture", "--json"])
        .await?;

    let data: Value = serde_json::from_str(&result.stdout)?;
    let expected = json!({"newcell/TARGETS.fixture": {"package": "newcell//"}});
    assert_eq!(data, expected, "Expected {expected}, got {data}");
    Ok(())
}

/// Test file-package mapping with multiple paths, including a file in 'newcell'
#[buck_test]
async fn test_audit_file_package_multiple_paths_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "file-package",
            "TARGETS.fixture",
            "subdir/testfile",
            "newcell/TARGETS.fixture",
            "--json",
        ])
        .await?;

    let data: Value = serde_json::from_str(&result.stdout)?;
    let expected = json!({
        "TARGETS.fixture": {"package": "root//"},
        "subdir/testfile": {"package": "root//subdir"},
        "newcell/TARGETS.fixture": {"package": "newcell//"},
    });
    assert_eq!(data, expected, "Expected {expected}, got {data}");
    Ok(())
}

/// Test file-package mapping with a mix of valid and invalid paths
#[buck_test]
async fn test_audit_file_package_with_errors_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "file-package",
            "TARGETS.fixture",
            "nonexistent/file.txt",
            "newcell/TARGETS.fixture",
            "--json",
        ])
        .await?;

    let data: Value = serde_json::from_str(&result.stdout)?;
    let expected = json!({
        "TARGETS.fixture": {"package": "root//"},
        "newcell/TARGETS.fixture": {"package": "newcell//"},
        "nonexistent/file.txt": {"error": "Directory `nonexistent` does not exist"},
    });
    assert_eq!(data, expected, "Expected {expected}, got {data}");
    Ok(())
}

/// Test file-package mapping with a mix of valid and invalid paths (plain text)
#[buck_test]
async fn test_audit_file_package_with_errors_plain(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "file-package",
            "TARGETS.fixture",
            "nonexistent/file.txt",
            "newcell/TARGETS.fixture",
        ])
        .await?;

    // Verify successful paths are in the output with correct format
    assert!(result.stdout.contains("TARGETS.fixture: root//"));
    assert!(result.stdout.contains("newcell/TARGETS.fixture: newcell//"));

    // Verify error path shows error message
    assert!(result.stdout.contains("nonexistent/file.txt: Error:"));
    Ok(())
}

/// Test file-package mapping with an absolute path
#[buck_test]
async fn test_audit_file_package_absolute_path(buck: &Buck) -> anyhow::Result<()> {
    let abs_path = buck
        .cwd()
        .join("TARGETS.fixture")
        .to_string_lossy()
        .into_owned();
    let result = buck.audit(&["file-package", &abs_path, "--json"]).await?;

    let data: Value = serde_json::from_str(&result.stdout)?;
    let expected = json!({abs_path.as_str(): {"package": "root//"}});
    assert_eq!(data, expected, "Expected {expected}, got {data}");
    Ok(())
}
