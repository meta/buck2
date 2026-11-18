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
use std::path::Path;

use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use serde_json::Value;

fn configure(buck: &Buck, kill_and_retry: bool) -> anyhow::Result<()> {
    let strategy = if kill_and_retry {
        "kill_and_retry"
    } else {
        "cgroup_freeze"
    };
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        format!("[buck2_resource_control]\npreferred_action_suspend_strategy = {strategy}\n"),
    )?;
    Ok(())
}

fn use_some_memory_args(buck: &Buck, temp: &Path) -> Vec<String> {
    let bin = buck
        .env_var("USE_SOME_MEMORY_BIN")
        .expect("`USE_SOME_MEMORY_BIN` is set");
    vec![
        "--show-full-simple-output".to_owned(),
        "-c".to_owned(),
        format!("use_some_memory.path={}", bin.to_string_lossy()),
        "-c".to_owned(),
        format!("start_marker_files.path={}", temp.display()),
        "--no-remote-cache".to_owned(),
        "--local-only".to_owned(),
    ]
}

/// The value of `key` in `value`, which must have one (`null` included).
#[track_caller]
fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("no `{key}` in {value}"))
}

async fn check_suspends(
    buck: &Buck,
    kill_and_retry: bool,
    temp: &Path,
    res: &BuckOutput,
) -> anyhow::Result<usize> {
    // First check the reported suspensions
    let actions = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    let mut reported_suspends: BTreeMap<String, Option<f64>> = BTreeMap::new();
    let mut num_suspended_actions = 0;
    for action in &actions {
        let category = action
            .pointer("/name/category")
            .unwrap_or_else(|| panic!("No `name.category` in {action}"));
        if category != "memory_allocating_actions" {
            continue;
        }
        let ident = action["name"]["identifier"].as_str().unwrap().to_owned();
        let command_meta =
            &action["commands"].as_array().unwrap().last().unwrap()["details"]["metadata"];
        if !field(command_meta, "suspend_duration").is_null() {
            num_suspended_actions += 1;
            if kill_and_retry {
                let count = field(command_meta, "suspend_count");
                assert!(!count.is_null());
                reported_suspends.insert(ident, Some(count.as_f64().unwrap()));
            } else {
                // Json representation of a duration is number of us
                let duration = field(command_meta, "suspend_duration").as_f64().unwrap() / 1000.0;
                reported_suspends.insert(ident, Some(duration));
            }
        } else {
            reported_suspends.insert(ident, None);
        }
    }

    if kill_and_retry {
        let mut total_detected_kills = 0.0;
        let mut expected_kills = 0.0;
        for (ident, count) in &reported_suspends {
            let detected_starts = std::fs::read_to_string(temp.join(ident))?.lines().count() as f64;
            match count {
                None => assert_eq!(detected_starts, 1.0),
                Some(count) => {
                    // We can't quite assert that the kills were observed by the action because sometimes
                    // we might kill the thing before it gets there. So add them up and assert that we're
                    // close enough
                    total_detected_kills += detected_starts - 1.0;
                    expected_kills += count;
                }
            }
        }
        assert!(total_detected_kills >= expected_kills - 2.0);
    } else {
        let paths = std::fs::read_to_string(res.stdout.trim())?;
        let paths: Vec<_> = paths.lines().map(|p| buck.cwd().join(p)).collect();
        assert_eq!(paths.len(), reported_suspends.len());

        // Then compare them to the detected suspensions
        for p in &paths {
            let contents = std::fs::read_to_string(p)?;
            let ident = p.file_name().unwrap().to_str().unwrap();
            let mut total_duration: Option<i64> = None;
            for line in contents.lines() {
                if let Some(this_duration) = line.strip_prefix("freeze_detected_ms ") {
                    let this_duration: i64 = this_duration.parse()?;
                    total_duration = Some(total_duration.unwrap_or(0) + this_duration);
                }
            }
            match total_duration {
                Some(total_duration) => assert!(
                    (reported_suspends[ident].unwrap() - total_duration as f64).abs() < 500.0
                ),
                None => assert!(reported_suspends[ident].unwrap_or(0.0) < 500.0),
            }
        }
    }

    Ok(num_suspended_actions)
}

// To not fail listing on Mac or Windows
#[buck_test]
async fn test_dummy() {}

async fn test_action_suspend(
    buck: &Buck,
    tmp_path: &Path,
    kill_and_retry: bool,
) -> anyhow::Result<()> {
    configure(buck, kill_and_retry)?;
    let args = use_some_memory_args(buck, tmp_path);
    let res = buck
        .build_without_report(
            &[
                &[":sleep_10"][..],
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
            ]
            .concat(),
        )
        .await?;

    check_suspends(buck, kill_and_retry, tmp_path, &res).await?;
    Ok(())
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    disable_daemon_cgroup = false,
    env = [("BUCK2_HARD_ERROR", "panic")],
)]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_action_suspend_True(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_action_suspend(buck, tmp_path, true).await
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    disable_daemon_cgroup = false,
    env = [("BUCK2_HARD_ERROR", "panic")],
)]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_action_suspend_False(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_action_suspend(buck, tmp_path, false).await
}

async fn test_action_suspend_stress_test(
    buck: &Buck,
    tmp_path: &Path,
    kill_and_retry: bool,
) -> anyhow::Result<()> {
    configure(buck, kill_and_retry)?;
    let args = use_some_memory_args(buck, tmp_path);
    buck.build(
        &[
            &[":very_fast_100"][..],
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        ]
        .concat(),
    )
    .await?;
    Ok(())
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    disable_daemon_cgroup = false,
    env = [("BUCK2_HARD_ERROR", "panic")],
)]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_action_suspend_stress_test_True(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_action_suspend_stress_test(buck, tmp_path, true).await
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    disable_daemon_cgroup = false,
    env = [("BUCK2_HARD_ERROR", "panic")],
)]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_action_suspend_stress_test_False(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_action_suspend_stress_test(buck, tmp_path, false).await
}

// Only kill_and_retry is exercised. Deterministic memory pressure requires swap to be disabled
// for actions (see `memory_swap_max_actions` in the data `.buckconfig`); otherwise the overshoot
// is paged out cheaply and pressure never crosses the suspension threshold. cgroup_freeze is
// incompatible with that: freezing an action does not release its memory, so with swap off the
// still-running action stays throttled above memory.high and never makes progress, deadlocking the
// build. There is no swap setting that both builds pressure (needs swap off) and lets a freeze
// relieve it (needs swap on), so this scenario cannot be tested deterministically for freeze.
async fn test_suspend_one_of_two(
    buck: &Buck,
    tmp_path: &Path,
    kill_and_retry: bool,
) -> anyhow::Result<()> {
    configure(buck, kill_and_retry)?;

    let args = use_some_memory_args(buck, tmp_path);
    let res = buck
        .build_without_report(
            &[
                &[":two_mutually_incompatible"][..],
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
            ]
            .concat(),
        )
        .await?;

    let num_suspends = check_suspends(buck, kill_and_retry, tmp_path, &res).await?;
    assert_eq!(num_suspends, 1);
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"], disable_daemon_cgroup = false)]
#[expect(non_snake_case, reason = "named as the Python harness named the case")]
async fn test_suspend_one_of_two_True(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    test_suspend_one_of_two(buck, tmp_path, true).await
}
