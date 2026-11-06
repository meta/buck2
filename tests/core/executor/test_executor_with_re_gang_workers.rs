/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

/// Test that a target with two gang workers builds successfully on RE.
#[buck_test]
async fn test_target_with_two_gang_workers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            ":target_with_two_gang_workers",
            "-c",
            "build.execution_platforms=root//platforms:platforms",
            "-c",
            &format!("test.cache_buster={}", random_string()),
            "--show-full-output",
        ])
        .await?;

    // Read and validate the content of /run/re_worker/gang_info (copied to output)
    let output_dict = result.target_to_build_output()?;
    for output_path in output_dict.values() {
        let content = std::fs::read_to_string(output_path)?;
        // Parse as GetGangInfoResponse JSON
        let gang_info: Value = serde_json::from_str(&content)?;
        eprintln!("gang_info: {gang_info}");
        // Assert that we got 2 workers back
        let workers = gang_info
            .get("workers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert_eq!(workers, 2, "Expected 2 gang workers, got {workers}");
    }

    // Make sure it actually ran on RE
    let out = read_what_ran(buck, &[]).await?;
    let executors: BTreeMap<String, Value> = out
        .iter()
        .map(|line| {
            (
                line["identity"].as_str().unwrap().to_owned(),
                line["reproducer"]["executor"].clone(),
            )
        })
        .collect();
    let expected = BTreeMap::from([(
        "root//:target_with_two_gang_workers (<unspecified>) (cp)".to_owned(),
        Value::from("Re"),
    )]);
    assert_eq!(executors, expected);
    Ok(())
}
