/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Read;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use regex::Regex;
use serde_json::Value;

const FOO_UNMODIFIED: &[&str] = &["4", "5", "6", "7"];
const BAR_UNMODIFIED: &[&str] = &["1", "2", "3", "4", "5"];

/// The `buck.target_hash` of the first target `targets --json` printed.
fn grab(x: &BuckOutput) -> anyhow::Result<String> {
    let targets: Value = serde_json::from_str(&x.stdout)?;
    targets
        .get(0)
        .and_then(|t| t.get("buck.target_hash"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("No `buck.target_hash` in {targets}"))
}

async fn configured_target_hashing(
    buck: &Buck,
    rule: &str,
    unmodified: &[&str],
    src_changed: &str,
) -> anyhow::Result<()> {
    let target = format!("fbcode//buck2/tests/targets/target_hashing:rule{rule}");
    let result = buck
        .targets(&[
            &target,
            "--show-target-hash",
            "--json",
            "--target-hash-file-mode",
            "PATHS_ONLY",
            "--target-hash-recursive=true",
        ])
        .await?;

    // Modify a target
    let modified_result = buck
        .targets(&[
            &target,
            "--show-target-hash",
            "--json",
            "--target-hash-file-mode",
            "PATHS_ONLY",
            "--target-hash-recursive=true",
            "--target-hash-modified-paths",
            &format!("buck2/tests/targets/target_hashing/{src_changed}"),
        ])
        .await?;
    let output = grab(&result)?;
    let modified_output = grab(&modified_result)?;

    // Hash should change iff the target is the modified target or depends on the modified target
    if unmodified.contains(&rule) {
        assert_eq!(output, modified_output);
    } else {
        assert_ne!(output, modified_output);
    }
    Ok(())
}

macro_rules! configured_target_hashing_tests {
    ($($name:ident: $rule:literal, $unmodified:expr, $src_changed:literal;)*) => {
        $(
            // TODO: Make this test isolated, i.e. inplace=False.
            #[buck_test(inplace = true)]
            async fn $name(buck: &Buck) -> anyhow::Result<()> {
                configured_target_hashing(buck, $rule, $unmodified, $src_changed).await
            }
        )*
    };
}

configured_target_hashing_tests! {
    test_configured_target_hashing_1_unmodified0_foo_txt: "1", FOO_UNMODIFIED, "foo.txt";
    test_configured_target_hashing_2_unmodified1_foo_txt: "2", FOO_UNMODIFIED, "foo.txt";
    test_configured_target_hashing_3_unmodified2_foo_txt: "3", FOO_UNMODIFIED, "foo.txt";
    test_configured_target_hashing_4_unmodified3_foo_txt: "4", FOO_UNMODIFIED, "foo.txt";
    test_configured_target_hashing_5_unmodified4_foo_txt: "5", FOO_UNMODIFIED, "foo.txt";
    test_configured_target_hashing_6_unmodified5_foo_txt: "6", FOO_UNMODIFIED, "foo.txt";
    test_configured_target_hashing_7_unmodified6_foo_txt: "7", FOO_UNMODIFIED, "foo.txt";
    test_configured_target_hashing_1_unmodified7_bar_txt: "1", BAR_UNMODIFIED, "bar.txt";
    test_configured_target_hashing_2_unmodified8_bar_txt: "2", BAR_UNMODIFIED, "bar.txt";
    test_configured_target_hashing_3_unmodified9_bar_txt: "3", BAR_UNMODIFIED, "bar.txt";
    test_configured_target_hashing_4_unmodified10_bar_txt: "4", BAR_UNMODIFIED, "bar.txt";
    test_configured_target_hashing_5_unmodified11_bar_txt: "5", BAR_UNMODIFIED, "bar.txt";
    test_configured_target_hashing_6_unmodified12_bar_txt: "6", BAR_UNMODIFIED, "bar.txt";
    test_configured_target_hashing_7_unmodified13_bar_txt: "7", BAR_UNMODIFIED, "bar.txt";
}

#[buck_test(inplace = true)]
async fn test_configured_ignores_unconfigured(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/target_hashing:rule8";
    let pre_unconfigured = buck
        .targets(&[target, "--show-unconfigured-target-hash", "--json"])
        .await?;
    let pre_configured = buck
        .targets(&[target, "--show-target-hash", "--json"])
        .await?;

    let config = "-ctesting.hashing=1";
    let post_unconfigured = buck
        .targets(&[target, config, "--show-unconfigured-target-hash", "--json"])
        .await?;
    let post_configured = buck
        .targets(&[target, config, "--show-target-hash", "--json"])
        .await?;

    // Hashes differ configured vs unconfigured
    assert_ne!(grab(&pre_unconfigured)?, grab(&pre_configured)?);
    // We spot the unconfigured change
    assert_ne!(grab(&pre_unconfigured)?, grab(&post_unconfigured)?);
    // But the configured remains consistent
    assert_eq!(grab(&pre_configured)?, grab(&post_configured)?);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_non_recursive_target_hash(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/target_hashing:rule9";
    let pre_recursive = buck
        .targets(&[
            target,
            "--show-target-hash",
            "--json",
            "--target-hash-recursive=true",
        ])
        .await?;
    let pre_direct = buck
        .targets(&[
            target,
            "--show-target-hash",
            "--json",
            "--target-hash-recursive=false",
        ])
        .await?;

    let config = "-ctesting.hashing=1";
    let post_recursive = buck
        .targets(&[
            target,
            config,
            "--show-target-hash",
            "--json",
            "--target-hash-recursive=true",
        ])
        .await?;
    let post_direct = buck
        .targets(&[
            target,
            config,
            "--show-target-hash",
            "--json",
            "--target-hash-recursive=false",
        ])
        .await?;

    // Hashes changed for recursive
    assert_ne!(grab(&pre_recursive)?, grab(&post_recursive)?);
    // But not for non-recursive
    assert_eq!(grab(&pre_direct)?, grab(&post_direct)?);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_show_inputs(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/target_hashing:rule1";
    let result = buck.targets(&[target, "--json"]).await?;
    let targets: Value = serde_json::from_str(&result.stdout)?;
    let deps = targets[0]["buck.deps"]
        .as_array()
        .with_context(|| format!("No `buck.deps` list in {targets}"))?;
    assert!(deps.contains(&Value::from(
        "fbcode//buck2/tests/targets/target_hashing:rule5"
    )));
    assert_eq!(
        targets[0]["buck.inputs"],
        serde_json::json!(["fbcode//buck2/tests/targets/target_hashing/foo.txt"])
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_streaming_uncached(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // This test aims to check the kind of things TD might do - the streaming plus other related features
    let file = tmp_path.join("targets.jsonl");
    buck.targets(&[
        "fbcode//buck2:buck2",
        "--json-lines",
        "--streaming",
        "--imports",
        "--output-attribute",
        "^buck\\.|name",
        "--no-cache",
        "--show-unconfigured-target-hash",
        &format!("--output={}", file.display()),
    ])
    .await?;
    let target_hash = Regex::new("^[0-9a-f]+$")?;
    let mut found = 0;
    for x in std::fs::read_to_string(&file)?.lines() {
        let x: Value = serde_json::from_str(x)?;
        if x.get("buck.package").and_then(Value::as_str) == Some("fbcode//buck2") {
            if x.get("name").and_then(Value::as_str) == Some("buck2") {
                let hash = x["buck.target_hash"]
                    .as_str()
                    .with_context(|| format!("No `buck.target_hash` in {x}"))?;
                assert!(target_hash.is_match(hash), "{hash}");
                found += 1;
            } else if let Some(imports) = x.get("buck.imports") {
                let imports = imports
                    .as_array()
                    .with_context(|| format!("`buck.imports` is not a list in {x}"))?;
                assert!(imports.contains(&Value::from("prelude//prelude.bzl")));
                found += 1;
            }
        }
    }
    assert_eq!(found, 2);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_compression(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let name = tmp_path.display();
    buck.targets(&["fbcode//buck2:", &format!("--output={name}/out.txt")])
        .await?;
    buck.targets(&[
        "fbcode//buck2:",
        &format!("--output={name}/out.txt.gz"),
        "--compression=gzip",
    ])
    .await?;
    buck.targets(&[
        "fbcode//buck2:",
        &format!("--output={name}/out.txt.zst"),
        "--compression=zstd",
    ])
    .await?;
    let out_uncompressed = std::fs::read(tmp_path.join("out.txt"))?;
    let mut out_gzip = Vec::new();
    flate2::read::MultiGzDecoder::new(std::fs::File::open(tmp_path.join("out.txt.gz"))?)
        .read_to_end(&mut out_gzip)?;
    let status = tokio::process::Command::new("zstd")
        .arg("-d")
        .arg(tmp_path.join("out.txt.zst"))
        .arg("-o")
        .arg(tmp_path.join("out.txt.unzst"))
        .status()
        .await?;
    anyhow::ensure!(status.success(), "zstd failed: {status}");
    let out_zstd = std::fs::read(tmp_path.join("out.txt.unzst"))?;
    assert_eq!(out_uncompressed, out_gzip);
    assert_eq!(out_uncompressed, out_zstd);
    Ok(())
}
