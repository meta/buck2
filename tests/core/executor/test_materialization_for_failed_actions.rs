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
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::json_get;
use buck2_e2e::utils::random_string;
use regex::Regex;
use serde_json::Value;

const HASH: &str = r"[0-9a-fA-F]{16}";

async fn check_materialize_inputs_for_failed_actions(buck: &Buck) -> anyhow::Result<()> {
    let log = buck.log(&["show"]).await?;

    let mut found_action_error = false;
    let mut found_materialize_failed_inputs_span = false;

    for line in log.stdout.trim().lines() {
        // Look for MaterializeFailedInputs ReStage event
        if line.contains("MaterializeFailedInputs") {
            found_materialize_failed_inputs_span = true;
        }

        // Inspect failed input
        let event: Value = serde_json::from_str(line)?;
        let materialized_inputs_for_failed = json_get(
            &event,
            &[
                "Event",
                "data",
                "Instant",
                "data",
                "ActionError",
                "last_command",
                "details",
                "command_kind",
                "command",
                "RemoteCommand",
                "materialized_inputs_for_failed",
            ],
        )
        .and_then(Value::as_array)
        .filter(|inputs| !inputs.is_empty());

        if let Some(materialized_inputs_for_failed) = materialized_inputs_for_failed {
            found_action_error = true;
            assert_eq!(materialized_inputs_for_failed.len(), 1);
            let input = materialized_inputs_for_failed[0]
                .as_str()
                .context("A non-string input path")?;
            let contents = std::fs::read_to_string(buck.cwd().join(input))?;
            assert_eq!(contents, "yay!");
        }
    }

    assert!(found_action_error, "Did not find relevant ActionError");
    assert!(
        found_materialize_failed_inputs_span,
        "Did not find relevant MaterializeFailedInputs span"
    );
    Ok(())
}

/// The output paths of the failed action that buck2 materialized, as the one action error of the
/// last command lists them.
async fn materialized_outputs_for_failed_actions(buck: &Buck) -> anyhow::Result<Vec<String>> {
    let materialized = filter_events(
        buck,
        &[
            "Event",
            "data",
            "Instant",
            "data",
            "ActionError",
            "last_command",
            "details",
            "command_kind",
            "command",
            "RemoteCommand",
            "materialized_outputs_for_failed_actions",
        ],
    )
    .await?;

    assert!(
        !materialized.is_empty(),
        "No materialization for failed actions!"
    );
    assert_eq!(materialized.len(), 1, "{materialized:?}");

    materialized[0]
        .as_array()
        .context("Materialized outputs that are not a list")?
        .iter()
        .map(|out| Ok(out.as_str().context("A non-string output path")?.to_owned()))
        .collect()
}

async fn check_materialized_outputs_for_failed_action(buck: &Buck) -> anyhow::Result<()> {
    let materialized = materialized_outputs_for_failed_actions(buck).await?;
    assert_eq!(materialized.len(), 2, "{materialized:?}");

    let out1 = &materialized[0];
    let contents = std::fs::read_to_string(buck.cwd().join(out1))?;
    assert_eq!(contents, "json");

    let out2 = &materialized[1];
    let contents = std::fs::read_to_string(buck.cwd().join(out2))?;
    assert_eq!(contents, "txt");

    let hash = Regex::new(HASH)?;
    assert!(hash.is_match(out1), "Expected hash in output path");
    assert!(hash.is_match(out2), "Expected hash in output path");
    Ok(())
}

async fn check_materialized_outputs_defined_by_run_action(buck: &Buck) -> anyhow::Result<()> {
    let materialized = materialized_outputs_for_failed_actions(buck).await?;
    assert_eq!(materialized.len(), 1, "{materialized:?}");

    let out = &materialized[0];
    let contents = std::fs::read_to_string(buck.cwd().join(out))?;
    assert_eq!(contents, "json");

    assert!(
        Regex::new(HASH)?.is_match(out),
        "Expected hash in output path"
    );
    Ok(())
}

#[buck_test(data_dir = "materialize_inputs_for_failed_actions")]
async fn test_materialize_inputs_for_failed_actions(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:action_fail",
        "--remote-only",
        "--no-remote-cache",
        "--materialize-failed-inputs",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await
    .expect_failure();
    check_materialize_inputs_for_failed_actions(buck).await
}

#[buck_test(data_dir = "materialize_inputs_for_failed_actions")]
async fn test_materialize_inputs_for_failed_actions_content_hash(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&[
        "//:action_fail",
        "--remote-only",
        "--no-remote-cache",
        "--materialize-failed-inputs",
        "-c",
        &format!("test.cache_buster={}", random_string()),
        "test.use_content_based_path=true",
    ])
    .await
    .expect_failure();
    check_materialize_inputs_for_failed_actions(buck).await
}

#[buck_test(
    skip_for_os = ["windows"],
    data_dir = "materialize_outputs_for_failed_actions"
)]
async fn test_materialize_outputs_for_failed_actions(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:action_fail",
        "--remote-only",
        "--materialize-failed-outputs",
    ])
    .await
    .expect_failure();
    check_materialized_outputs_for_failed_action(buck).await
}

#[buck_test(
    skip_for_os = ["windows"],
    data_dir = "materialize_outputs_for_failed_actions"
)]
async fn test_materialize_outputs_for_failed_actions_content_hash(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&[
        "//:action_fail",
        "--remote-only",
        "--materialize-failed-outputs",
        "-c",
        "test.use_content_based_path=true",
    ])
    .await
    .expect_failure();
    check_materialized_outputs_for_failed_action(buck).await
}

#[buck_test(data_dir = "materialize_outputs_for_failed_actions")]
async fn test_undeclared_outputs_to_materialize_will_fail(buck: &Buck) {
    buck.build(&["//:undeclared_output", "--remote-only", "--no-remote-cache"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "marked to be materialized on failure but is not declared as an output of the action",
        );
}

#[buck_test(
    skip_for_os = ["windows"],
    data_dir = "materialize_outputs_for_failed_actions"
)]
async fn test_materialize_outputs_defined_by_run_action(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:action_fail", "--remote-only", "--no-remote-cache"])
        .await
        .expect_failure();
    check_materialized_outputs_defined_by_run_action(buck).await
}

#[buck_test(
    skip_for_os = ["windows"],
    data_dir = "materialize_outputs_for_failed_actions"
)]
async fn test_materialize_outputs_defined_by_run_action_content_hash(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&[
        "//:action_fail",
        "--remote-only",
        "--no-remote-cache",
        "-c",
        "test.use_content_based_path=true",
    ])
    .await
    .expect_failure();
    check_materialized_outputs_defined_by_run_action(buck).await
}
