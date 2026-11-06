/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::InvocationRecord;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;
use serde_json::json;

// `buck2_data::UploadResult`
const UPLOAD_RESULT_ACTION_UPLOAD_NOT_ALLOWED: i64 = 6;
const UPLOAD_RESULT_LOCAL_IN_PLACE_OF_REMOTE: i64 = 19;

/// The executor of each action that ran, by the action's identity.
fn executors(what_ran: &[Value]) -> Value {
    Value::Object(
        what_ran
            .iter()
            .map(|line| {
                (
                    line["identity"]
                        .as_str()
                        .expect("an action with an identity")
                        .to_owned(),
                    line["reproducer"]["executor"].clone(),
                )
            })
            .collect(),
    )
}

/// The commands of the first action execution in the last command's event log that ran any.
async fn first_commands(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    filter_events(
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
    .await?
    .into_iter()
    .find_map(|commands| match commands {
        Value::Array(commands) if !commands.is_empty() => Some(commands),
        _ => None,
    })
    .context("No action execution with commands in the event log")
}

#[buck_test]
async fn test_hybrid_executor_threshold(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//executor_threshold_tests/...",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    let out = read_what_ran(buck, &[]).await?;

    let expected = json!({
        "root//executor_threshold_tests:big (<unspecified>) (head)": "Local",
        "root//executor_threshold_tests:cp_big (<unspecified>) (cp)": "Local",
        "root//executor_threshold_tests:small (<unspecified>) (head)": "Local",
        "root//executor_threshold_tests:cp_small (<unspecified>) (cp)": "Re",
    });
    assert_eq!(executors(&out), expected);
    Ok(())
}

async fn hybrid_executor_fallbacks(buck: &Buck, low_pass_filter: &str) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let low_pass_filter = format!("test.experimental_low_pass_filter={low_pass_filter}");
    let opts = ["-c", &cache_buster, "-c", &low_pass_filter];

    // Those work as they are allowed to fallback:
    buck.build(
        &[
            &[
                "root//executor_fallback_tests:local_only",
                "root//executor_fallback_tests:local_only_full_hybrid",
                "root//executor_fallback_tests:remote_only_prefer_local",
            ][..],
            &opts,
        ]
        .concat(),
    )
    .await?;

    // This one doesn't:
    buck.build(
        &[
            &["root//executor_fallback_tests:local_only_no_fallback"][..],
            &opts,
        ]
        .concat(),
    )
    .await
    .expect_failure();
    Ok(())
}

#[buck_test]
async fn test_hybrid_executor_fallbacks_true(buck: &Buck) -> anyhow::Result<()> {
    hybrid_executor_fallbacks(buck, "true").await
}

#[buck_test]
async fn test_hybrid_executor_fallbacks_false(buck: &Buck) -> anyhow::Result<()> {
    hybrid_executor_fallbacks(buck, "false").await
}

/// The one action execution of building `target`.
async fn execution(buck: &Buck, target: &str) -> anyhow::Result<Value> {
    buck.build(&[
        target,
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    let executions = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    assert_eq!(executions.len(), 1, "{executions:?}");
    Ok(executions[0].clone())
}

async fn cache_uploads(buck: &Buck) -> anyhow::Result<usize> {
    Ok(
        filter_events(buck, &["Event", "data", "SpanEnd", "data", "CacheUpload"])
            .await?
            .len(),
    )
}

#[buck_test]
async fn test_hybrid_executor_fallback_is_not_published(buck: &Buck) -> anyhow::Result<()> {
    // Remote execution fails the command and the fallback runs it locally. The platform
    // allows cache uploads, but a result from a run the configuration did not decide on is
    // not published.
    let fallback = execution(
        buck,
        "root//executor_fallback_tests:local_only_with_uploads",
    )
    .await?;
    assert_eq!(
        fallback["cache_upload_result"],
        UPLOAD_RESULT_LOCAL_IN_PLACE_OF_REMOTE
    );
    assert_eq!(cache_uploads(buck).await?, 0);

    // An action that may not publish at all reports that, whatever run produced its result.
    let not_allowed = execution(
        buck,
        "root//executor_fallback_tests:local_only_with_uploads_not_allowed",
    )
    .await?;
    assert_eq!(
        not_allowed["cache_upload_result"],
        UPLOAD_RESULT_ACTION_UPLOAD_NOT_ALLOWED
    );
    assert_eq!(cache_uploads(buck).await?, 0);

    // The same command run locally by its own preference is published (or its upload refused
    // for want of permission where the test runs, which is still an upload attempted).
    let preferred = execution(
        buck,
        "root//executor_fallback_tests:prefer_local_with_uploads",
    )
    .await?;
    assert_ne!(
        *preferred
            .get("cache_upload_result")
            .context("No `cache_upload_result` for the preferred-local action")?,
        UPLOAD_RESULT_LOCAL_IN_PLACE_OF_REMOTE
    );
    assert_eq!(cache_uploads(buck).await?, 1);
    Ok(())
}

#[buck_test]
async fn test_hybrid_executor_fallback_preferred_error(buck: &Buck) {
    let cache_buster = format!("test.cache_buster={}", random_string());

    buck.build(&[
        "root//executor_fallback_tests:fails_both",
        "-c",
        &cache_buster,
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Failed on local");

    buck.build(&[
        "root//executor_fallback_tests:fails_both_prefer_local",
        "-c",
        &cache_buster,
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Failed on local");
}

async fn hybrid_executor_cancels_local_execution(buck: &Buck, target: &str) -> anyhow::Result<()> {
    buck.build(&[
        &format!("root//executor_race_tests:{target}"),
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;

    let commands = first_commands(buck).await?;
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0]["status"], json!({"Cancelled": {}}));
    assert_eq!(commands[1]["status"], json!({"Success": {}}));
    Ok(())
}

#[buck_test]
async fn test_hybrid_executor_cancels_local_execution_slower_locally(
    buck: &Buck,
) -> anyhow::Result<()> {
    hybrid_executor_cancels_local_execution(buck, "slower_locally").await
}

#[buck_test]
async fn test_hybrid_executor_cancels_local_execution_slower_locally_force_full_hybrid(
    buck: &Buck,
) -> anyhow::Result<()> {
    hybrid_executor_cancels_local_execution(buck, "slower_locally_force_full_hybrid").await
}

#[buck_test]
async fn test_hybrid_executor_logging(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//executor_fallback_tests:local_only",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;

    let commands = first_commands(buck).await?;
    assert_eq!(commands.len(), 2);
    assert!(
        commands[0]
            .pointer("/details/signed_exit_code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0),
        "{}",
        commands[0]
    );
    assert_eq!(commands[0]["status"], json!({"Failure": {}}));
    assert_eq!(commands[1]["details"]["signed_exit_code"], 0);
    assert_eq!(commands[1]["status"], json!({"Success": {}}));
    Ok(())
}

async fn hybrid_executor_prefer_local(buck: &Buck, low_pass_filter: &str) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let low_pass_filter = format!("test.experimental_low_pass_filter={low_pass_filter}");
    let opts = ["-c", &cache_buster, "-c", &low_pass_filter];

    // heavyweight_works_only_locally will only succeed if it runs locally, but
    // its weight would normally prevent that from happening. It has
    // prefer_local, so it only works if that results in local execution being
    // attempted.
    //
    // slower_and_works_only_locally will only work locally but it'll fail
    // faster on RE. This means it must not be attempted at al on RE.
    buck.build(
        &[
            &[
                "root//executor_race_tests:heavyweight_works_only_locally",
                "root//executor_race_tests:slower_and_works_only_locally",
            ][..],
            &opts,
        ]
        .concat(),
    )
    .await?;

    // Same as above, but with prefer-local on the build command line instead of the command.
    buck.build(
        &[
            &[
                "root//executor_race_tests:heavyweight_works_only_locally_local_not_preferred",
                "root//executor_race_tests:slower_and_works_only_locally_local_not_preferred",
                "--prefer-local",
            ][..],
            &opts,
        ]
        .concat(),
    )
    .await?;
    Ok(())
}

#[buck_test]
async fn test_hybrid_executor_prefer_local_true(buck: &Buck) -> anyhow::Result<()> {
    hybrid_executor_prefer_local(buck, "true").await
}

#[buck_test]
async fn test_hybrid_executor_prefer_local_false(buck: &Buck) -> anyhow::Result<()> {
    hybrid_executor_prefer_local(buck, "false").await
}

#[buck_test]
async fn test_hybrid_executor_prefer_remote_local_fallback(buck: &Buck) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    // Local only command that fails with --remote-only, passes with --prefer-remote
    buck.build(&[
        "root//executor_fallback_tests:local_only_full_hybrid",
        "--remote-only",
        "-c",
        &cache_buster,
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Failed to build .*local_only_full_hybrid");

    buck.build(&[
        "root//executor_fallback_tests:local_only_full_hybrid",
        "--prefer-remote",
        "-c",
        &cache_buster,
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_hybrid_executor_prefer_remote(buck: &Buck) -> anyhow::Result<()> {
    // Build execution is sequential and remote first with --prefer-remote
    // using an action that succeeds slowly on RE and fails fast locally
    // that would fail if run concurrently
    buck.build(&[
        "root//executor_race_tests:slower_remotely",
        "--prefer-remote",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_executor_preference_priority(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//executor_preference_tests:",
        "--prefer-remote",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_executor_preference_with_remote_args(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//executor_preference_prefer_remote_arg_tests:",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_executor_preference_with_remote_args_and_cli_override(buck: &Buck) {
    buck.build(&[
        "root//executor_preference_prefer_remote_arg_tests:",
        // `--prefer-local` takes priority over any `ctx.actions.run()`
        "--prefer-local",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await
    .expect_failure();
}

#[buck_test]
async fn test_prefer_local(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//executor_fallback_tests:local_only_no_fallback",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await
    .expect_failure();

    buck.build(&[
        "root//executor_fallback_tests:local_only_no_fallback",
        "--prefer-local",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_local_only(buck: &Buck) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let args = [
        "root//executor_fallback_tests:local_only_no_fallback",
        "-c",
        &cache_buster,
    ];

    buck.build(&args).await.expect_failure();

    buck.build(&[&args[..], &["--local-only"]].concat()).await?;
    Ok(())
}

#[buck_test]
async fn test_remote_only(buck: &Buck) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let args = [
        "root//executor_fallback_tests:remote_only_no_fallback",
        "root//executor_fallback_tests:remote_only_full_hybrid",
        "-c",
        &cache_buster,
    ];

    buck.build(&args).await.expect_failure();

    buck.build(&[&args[..], &["--remote-only"]].concat())
        .await?;
    Ok(())
}

#[buck_test]
async fn test_remote_only_rejects_oversized_action(buck: &Buck) -> anyhow::Result<()> {
    // `cp_big` copies an input above the remote input size limit of this fixture, so it cannot
    // run on RE, and `--remote-only` does not allow it to run locally.
    let cache_buster = format!("test.cache_buster={}", random_string());
    let opts = ["-c", &cache_buster];
    // `big` is local only, which `--remote-only` rejects, so build it on its own first.
    buck.build(&[&["root//executor_threshold_tests:big"][..], &opts].concat())
        .await?;

    buck.build(
        &[
            &["root//executor_threshold_tests:cp_big", "--remote-only"][..],
            &opts,
        ]
        .concat(),
    )
    .await
    .expect_failure()
    .assert_stderr_matches("above the remote execution limit");
    // Without the restriction the action falls back to local execution.
    buck.build(&[&["root//executor_threshold_tests:cp_big"][..], &opts].concat())
        .await?;
    let out = read_what_ran(buck, &[]).await?;
    assert_eq!(
        executors(&out),
        json!({
            "root//executor_threshold_tests:cp_big (<unspecified>) (cp)": "Local",
        })
    );
    Ok(())
}

#[buck_test]
async fn test_build_fails_with_mutually_exclusive_executors(buck: &Buck) {
    buck.build(&[
        "--local-only",
        "--remote-only",
        "root//executor_threshold_tests/...",
    ])
    .await
    .expect_failure();
}

#[buck_test(env = [("BUCK_OFFLINE_BUILD", "1")])]
async fn test_build_offline(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//executor_threshold_tests/..."]).await?;
    let out = read_what_ran(buck, &[]).await?;

    let expected = json!({
        "root//executor_threshold_tests:big (<unspecified>) (head)": "Local",
        "root//executor_threshold_tests:cp_big (<unspecified>) (cp)": "Local",
        "root//executor_threshold_tests:small (<unspecified>) (head)": "Local",
        "root//executor_threshold_tests:cp_small (<unspecified>) (cp)": "Local",
    });
    assert_eq!(executors(&out), expected);
    Ok(())
}

/// Builds `target` of `executor_race_tests` with `opts`, in a new daemon with `env`.
async fn build_for_record(
    buck: &Buck,
    target: &str,
    opts: &[&str],
    env: &[(&str, &str)],
) -> anyhow::Result<InvocationRecord> {
    // kill to update env
    buck.kill().await?;
    let target = format!("root//executor_race_tests:{target}");
    let cache_buster = format!("test.cache_buster={}", random_string());
    let args = [&[target.as_str(), "-c", &cache_buster][..], opts].concat();
    let res = env
        .iter()
        .fold(buck.build(&args), |command, (key, value)| {
            command.env(*key, *value)
        })
        .await?;
    res.invocation_record()
}

async fn scheduling_mode(buck: &Buck) -> anyhow::Result<Value> {
    let actions = read_what_ran(buck, &[]).await?;
    Ok(actions[0]["scheduling_mode"].clone())
}

#[buck_test(write_invocation_record = true)]
async fn test_hybrid_executor_remote_queuing_fallback(buck: &Buck) -> anyhow::Result<()> {
    let record = build_for_record(
        buck,
        "slower_remotely_and_works_on_both_full_hybrid",
        &[],
        &[],
    )
    .await?;
    assert_eq!(record["run_local_count"], 1);
    assert_eq!(record["run_remote_count"], 0);
    assert_eq!(record["run_fallback_count"], 0);
    assert_eq!(scheduling_mode(buck).await?, "FullHybrid");

    let record = build_for_record(
        buck,
        "slower_remotely_and_works_on_both_fallback_only",
        &[],
        &[("BUCK2_TEST_RE_QUEUE_ESTIMATE_S", "0")],
    )
    .await?;
    assert_eq!(record["run_local_count"], 0);
    assert_eq!(record["run_remote_count"], 1);
    assert_eq!(record["run_fallback_count"], 0);
    assert_eq!(scheduling_mode(buck).await?, "Fallback");

    let record = build_for_record(
        buck,
        "slower_remotely_and_works_on_both_fallback_only",
        &[
            "-c",
            "build.remote_execution_fallback_on_estimated_queue_time_exceeds_s=10",
        ],
        &[("BUCK2_TEST_RE_QUEUE_ESTIMATE_S", "100")],
    )
    .await?;
    assert_eq!(record["run_local_count"], 1);
    assert_eq!(record["run_remote_count"], 0);
    assert_eq!(record["run_fallback_count"], 1);
    assert_eq!(scheduling_mode(buck).await?, "FallbackReQueueEstimate");
    Ok(())
}
