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

/// The target hash of the first target of a `--json` output.
fn first_target_hash(output: &Value) -> &Value {
    output[0]
        .get("buck.target_hash")
        .expect("a `buck.target_hash`")
}

async fn assert_hashes(
    buck: &Buck,
    target: &str,
    modified_path: &str,
    same_hash: bool,
) -> anyhow::Result<()> {
    let result = buck
        .targets(&[
            target,
            "--show-unconfigured-target-hash",
            "--json",
            "--target-hash-file-mode",
            "PATHS_ONLY",
            "--target-hash-recursive=true",
        ])
        .await?;

    let modified_result = buck
        .targets(&[
            target,
            "--show-unconfigured-target-hash",
            "--json",
            "--target-hash-file-mode",
            "PATHS_ONLY",
            "--target-hash-recursive=true",
            "--target-hash-modified-paths",
            modified_path,
        ])
        .await?;
    let output: Value = serde_json::from_str(&result.stdout)?;
    let modified_output: Value = serde_json::from_str(&modified_result.stdout)?;

    // Hash should change if modified path belongs to target or to any of its dependencies
    if same_hash {
        assert_eq!(
            first_target_hash(&output),
            first_target_hash(&modified_output)
        );
    } else {
        assert_ne!(
            first_target_hash(&output),
            first_target_hash(&modified_output)
        );
    }
    Ok(())
}

async fn test_hash_function_preserves_128_bit_length(
    buck: &Buck,
    hash_function: &str,
    recursive: &str,
) -> anyhow::Result<()> {
    let result = buck
        .targets(&[
            ":foo_dep",
            "--show-unconfigured-target-hash",
            "--json",
            &format!("--target-hash-function={hash_function}"),
            &format!("--target-hash-recursive={recursive}"),
        ])
        .await?;
    let output: Vec<Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(output.len(), 1, "{output:?}");
    assert_eq!(output[0]["buck.target_hash"].as_str().unwrap().len(), 32);
    Ok(())
}

#[buck_test]
async fn test_hash_function_preserves_128_bit_length_false_fast(buck: &Buck) -> anyhow::Result<()> {
    test_hash_function_preserves_128_bit_length(buck, "fast", "false").await
}

#[buck_test]
async fn test_hash_function_preserves_128_bit_length_false_strong(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_hash_function_preserves_128_bit_length(buck, "strong", "false").await
}

#[buck_test]
async fn test_hash_function_preserves_128_bit_length_true_fast(buck: &Buck) -> anyhow::Result<()> {
    test_hash_function_preserves_128_bit_length(buck, "fast", "true").await
}

#[buck_test]
async fn test_hash_function_preserves_128_bit_length_true_strong(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_hash_function_preserves_128_bit_length(buck, "strong", "true").await
}

#[buck_test]
async fn test_unconfigured_target_hashing(buck: &Buck) -> anyhow::Result<()> {
    assert_hashes(buck, ":foo", "foo.txt", false).await?;
    assert_hashes(buck, ":foo", "bar.txt", true).await?;
    assert_hashes(buck, ":foo_dep", "foo.txt", false).await?;
    assert_hashes(buck, ":foo_dep", "bar.txt", true).await?;
    assert_hashes(buck, ":none", "bar.txt", true).await?;
    Ok(())
}

#[buck_test]
async fn test_cfg_modifiers_change_target_hash(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&[
            ":foo",
            "--show-unconfigured-target-hash",
            "--target-hash-recursive=false",
            "--json",
        ])
        .await?;

    std::fs::write(buck.cwd().join("PACKAGE"), "set_modifiers(['aaabbbccc'])")?;

    let modified_result = buck
        .targets(&[
            ":foo",
            "--show-unconfigured-target-hash",
            "--target-hash-recursive=false",
            "--json",
        ])
        .await?;
    let output: Value = serde_json::from_str(&result.stdout)?;
    let modified_output: Value = serde_json::from_str(&modified_result.stdout)?;

    // modifiers should change target hash
    assert_ne!(
        first_target_hash(&output),
        first_target_hash(&modified_output)
    );
    Ok(())
}

#[buck_test]
async fn test_visibility_intersection_change_target_hash(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&[
            ":public_lib",
            "--show-unconfigured-target-hash",
            "--target-hash-recursive=false",
            "--json",
        ])
        .await?;

    // `enforce_visibility_intersection()` restricts visibility at the PACKAGE
    // level without touching the target's `visibility` attribute (here
    // `PUBLIC`), so without hashing the intersection this change would be
    // invisible to the target hash and thus to target determination.
    // Regression for T279420508.
    std::fs::write(
        buck.cwd().join("PACKAGE"),
        "package(visibility = [\"//foo/...\"])\nenforce_visibility_intersection()\n",
    )?;

    let modified_result = buck
        .targets(&[
            ":public_lib",
            "--show-unconfigured-target-hash",
            "--target-hash-recursive=false",
            "--json",
        ])
        .await?;
    let output: Value = serde_json::from_str(&result.stdout)?;
    let modified_output: Value = serde_json::from_str(&modified_result.stdout)?;

    assert_ne!(
        first_target_hash(&output),
        first_target_hash(&modified_output)
    );
    Ok(())
}

#[buck_test]
async fn test_parent_cfg_modifiers_change_target_hash(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&[
            "foo:bar",
            "--show-unconfigured-target-hash",
            "--target-hash-recursive=false",
            "--json",
        ])
        .await?;

    std::fs::write(buck.cwd().join("PACKAGE"), "set_modifiers(['aaabbbccc'])")?;

    let modified_result = buck
        .targets(&[
            "foo:bar",
            "--show-unconfigured-target-hash",
            "--target-hash-recursive=false",
            "--json",
        ])
        .await?;
    let output: Value = serde_json::from_str(&result.stdout)?;
    let modified_output: Value = serde_json::from_str(&modified_result.stdout)?;

    // parent set_modifiers value should change target hash
    // note that we merge parent modifiers and current package modifiers
    assert_ne!(
        first_target_hash(&output),
        first_target_hash(&modified_output)
    );
    Ok(())
}
