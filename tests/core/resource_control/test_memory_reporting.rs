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

// `memory_peak` is derived from the action cgroup's `memory.current`, which includes page
// cache. Faulting in the ~14MB `use_some_memory` binary is charged to that cgroup whenever its
// pages are not already resident, and the same holds for the interpreter and shared libraries.
// The allocation has to dominate that variable overhead for the bounds below to hold.
const ALLOCATE_MB: u64 = 100;
const MEMORY_PEAK_MIN: u64 = 100_000_000;
const MEMORY_PEAK_MAX: u64 = 130_000_000;

fn use_some_memory_args(buck: &Buck) -> Vec<String> {
    let bin = buck
        .env_var("USE_SOME_MEMORY_BIN")
        .expect("`USE_SOME_MEMORY_BIN` is set");
    vec![
        "root//:use_some_memory".to_owned(),
        "--no-remote-cache".to_owned(),
        "--local-only".to_owned(),
        "-c".to_owned(),
        format!("test.cache_buster={}", random_string()),
        "-c".to_owned(),
        format!("use_some_memory.path={}", bin.to_string_lossy()),
        "-c".to_owned(),
        format!("use_some_memory.allocate_mb={ALLOCATE_MB}"),
    ]
}

async fn get_matching_details(buck: &Buck) -> anyhow::Result<Value> {
    let events = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    for action in &events {
        if action["name"]["category"] == "use_some_memory" {
            return Ok(action["commands"].as_array().unwrap().last().unwrap()["details"].clone());
        }
    }

    panic!("did not find the expected target");
}

#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_memory_reporting_disabled(buck: &Buck) -> anyhow::Result<()> {
    let args = use_some_memory_args(buck);
    buck.build(&args.iter().map(String::as_str).collect::<Vec<_>>())
        .env(
            "BUCK2_TEST_RESOURCE_CONTROL_CONFIG",
            r#"{"status":"Off","init":"Systemd","memory_max":null,"memory_high":null,"memory_max_per_action":null,"memory_high_per_action":null,"memory_high_actions":null,"memory_max_actions":null,"enable_suspension":false,"preferred_action_suspend_strategy":"KillAndRetry"}"#,
        )
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
            let execution_stats = &c["details"]["metadata"]["execution_stats"];
            assert!(execution_stats.is_object(), "no execution stats: {c}");
            assert!(
                execution_stats
                    .get("memory_peak")
                    .is_some_and(Value::is_null)
            );
        }
    }
    Ok(())
}

#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_memory_reporting(buck: &Buck) -> anyhow::Result<()> {
    let args = use_some_memory_args(buck);
    buck.build(&args.iter().map(String::as_str).collect::<Vec<_>>())
        .await?;

    let details = get_matching_details(buck).await?;
    assert!(
        details["command_kind"]["command"]
            .get("OmittedLocalCommand")
            .is_some()
    );

    let memory_peak = details["metadata"]["execution_stats"]["memory_peak"]
        .as_u64()
        .unwrap();
    assert!(memory_peak > MEMORY_PEAK_MIN);
    assert!(memory_peak < MEMORY_PEAK_MAX);
    Ok(())
}

#[buck_test(skip_for_os = ["windows", "darwin"], disable_daemon_cgroup = false)]
async fn test_memory_reporting_in_test(buck: &Buck) -> anyhow::Result<()> {
    let args = use_some_memory_args(buck);
    buck.test(&args.iter().map(String::as_str).collect::<Vec<_>>())
        .await?;

    let events = filter_events(buck, &["Event", "data", "SpanEnd", "data", "TestRun"]).await?;
    assert_eq!(events.len(), 1);
    let details = &events[0]["command_report"]["details"];

    let memory_peak = details["metadata"]["execution_stats"]["memory_peak"]
        .as_u64()
        .unwrap();
    assert!(memory_peak > MEMORY_PEAK_MIN);
    assert!(memory_peak < MEMORY_PEAK_MAX);
    Ok(())
}

#[buck_test]
async fn test_nop(_buck: &Buck) {
    // Pytest gets upset if we have no windows or mac tests in this file
}
