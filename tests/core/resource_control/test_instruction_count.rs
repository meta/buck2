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
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::random_string;
use serde_json::Value;

fn helper_bin_flags(buck: &Buck) -> [String; 2] {
    let bin = buck
        .env_var("THREE_BILLION_INSTRUCTIONS_BIN")
        .expect("`THREE_BILLION_INSTRUCTIONS_BIN` is set");
    [
        "-c".to_owned(),
        format!("three_billion_instructions.path={}", bin.to_string_lossy()),
    ]
}

async fn get_matching_details(buck: &Buck) -> anyhow::Result<Value> {
    let events = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    for action in &events {
        if action["name"]["category"] == "three_billion_instructions" {
            return Ok(action["commands"].as_array().unwrap().last().unwrap()["details"].clone());
        }
    }

    panic!("did not find the expected target");
}

#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_instruction_count_disabled(buck: &Buck) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let helper_bin_flags = helper_bin_flags(buck);
    buck.build(&[
        "root//:three_billion_instructions",
        "-c",
        "buck2.miniperf2=false",
        "--no-remote-cache",
        "--local-only",
        "-c",
        &cache_buster,
        &helper_bin_flags[0],
        &helper_bin_flags[1],
    ])
    .await?;

    let events = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "ActionExecution",
            "commands",
        ],
    )
    .await?;
    for commands in &events {
        for c in commands.as_array().unwrap() {
            assert!(
                c["details"]["metadata"]
                    .as_object()
                    .unwrap()
                    .get("execution_stats")
                    .is_none_or(Value::is_null)
            );
        }
    }
    Ok(())
}

#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_instruction_count_enabled(buck: &Buck) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let helper_bin_flags = helper_bin_flags(buck);
    buck.build(&[
        "root//:three_billion_instructions",
        "-c",
        "buck2.miniperf2=true",
        "--no-remote-cache",
        "--local-only",
        "-c",
        &cache_buster,
        &helper_bin_flags[0],
        &helper_bin_flags[1],
    ])
    .await?;

    let details = get_matching_details(buck).await?;
    assert!(
        details["command_kind"]["command"]
            .get("OmittedLocalCommand")
            .is_some()
    );

    // Check that we are within 20%
    let instruction_count = details["metadata"]["execution_stats"]["cpu_instructions_user"]
        .as_u64()
        .unwrap();
    // FIXME(JakobDegen): Are we really expecting downward variation? Why? Leave a comment
    assert!(instruction_count > 2700000000);
    assert!(instruction_count < 3300000000);
    Ok(())
}

#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_instruction_count_remote(buck: &Buck) -> anyhow::Result<()> {
    let helper_bin_flags = helper_bin_flags(buck);
    buck.build(&[
        "root//:three_billion_instructions",
        "--no-remote-cache",
        "--write-to-cache-anyway",
        "--prefer-remote",
        &helper_bin_flags[0],
        &helper_bin_flags[1],
    ])
    .await?;

    let details = get_matching_details(buck).await?;
    assert_eq!(
        details["command_kind"]["command"]["RemoteCommand"]["cache_hit"],
        false
    );

    // Check that we are within 10%
    let instruction_count = details["metadata"]["execution_stats"]["cpu_instructions_user"]
        .as_u64()
        .unwrap();
    assert!(instruction_count > 2850000000);
    assert!(instruction_count < 3150000000);

    // Check we also get it on a cache hit.

    buck.kill().await?;
    buck.build(&[
        "root//:three_billion_instructions",
        "--prefer-remote",
        &helper_bin_flags[0],
        &helper_bin_flags[1],
    ])
    .await?;

    let details = get_matching_details(buck).await?;
    assert_eq!(
        details["command_kind"]["command"]["RemoteCommand"]["cache_hit"],
        true
    );

    // Check that we are within 10%
    let instruction_count = details["metadata"]["execution_stats"]["cpu_instructions_user"]
        .as_u64()
        .unwrap();
    assert!(instruction_count > 2850000000);
    assert!(instruction_count < 3150000000);
    Ok(())
}

#[buck_test]
async fn test_instruction_count_nop(_buck: &Buck) {
    // Pytest gets upset if we have no windows or mac tests in this file
}
