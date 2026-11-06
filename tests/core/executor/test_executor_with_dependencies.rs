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

#[buck_test]
async fn test_executor_with_dependencies(buck: &Buck) -> anyhow::Result<()> {
    // Smoke test: run on RE and correctly pass the `remote_execution_dependencies` parameter specified in the platform
    // The RE external dependency (https://fburl.com/wiki/e55nloow) is purposefully wrong as the smc_tier is non existent,
    // We just want to check that the parameter is passed correctly to RE
    buck.build(&[
        ":target_without_dependencies",
        "-c",
        "build.execution_platforms=root//platforms:platforms_with_dependencies",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await
    .expect_failure()
    // Full error message looks like this: P1217423393
    .assert_stderr_matches(
        r#"facebook::remote_execution::scheduler::TaskCancelledException: Error acquiring dependency DependencyAcquisitionRequest \{ dependency: TDependency \{ smc_tier: "buck2_smoke_test_tier", id: "dep_a""#,
    );
    Ok(())
}

#[buck_test]
async fn test_good_target_with_dependencies(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            ":good_target_with_dependencies",
            "-c",
            "build.execution_platforms=root//platforms:platforms_without_dependencies",
            "-c",
            &format!("test.cache_buster={}", random_string()),
            "--show-full-output",
        ])
        .await?;
    let output_dict = result.target_to_build_output()?;

    for output in output_dict.values() {
        let deps: Value = serde_json::from_str(&std::fs::read_to_string(output)?)?;
        let deps = deps.as_array().unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0]["smc_tier"], "noop");
        assert_eq!(deps[0]["id"], "foo");
        // reservation_id is a random string which is 20 characters long
        assert_eq!(deps[0]["reservation_id"].as_str().unwrap().len(), 20);
    }

    // Make sure it actually did run on RE.
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
        "root//:good_target_with_dependencies (<unspecified>) (cp)".to_owned(),
        Value::from("Re"),
    )]);
    assert_eq!(executors, expected);
    Ok(())
}

#[buck_test]
async fn test_bad_target_with_dependencies(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        ":bad_target_with_dependencies",
        "-c",
        "build.execution_platforms=root//platforms:platforms_without_dependencies",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "error: too many fields set for RE dependency: `enable_interpolation, extra_field, id, smc_tier`",
    );
    Ok(())
}
