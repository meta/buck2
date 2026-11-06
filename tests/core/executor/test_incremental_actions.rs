/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::random_string;

fn seed() -> String {
    format!("test.seed={}", random_string())
}

/// The `test.use_content_based_path` config, set to `True` or `False`.
fn content_based_path(use_content_based_path: bool) -> String {
    let value = if use_content_based_path {
        "True"
    } else {
        "False"
    };
    format!("test.use_content_based_path={value}")
}

// Incremental actions use the output of previous actions, mimic this behavior by
// appending a string - Note that this is not how incremental actions behave in practice
async fn basic_incremental_action_local_only_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo bar");
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo bar bar");
    Ok(())
}

async fn incremental_action_from_remote_action_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--remote-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

async fn incremental_action_with_non_incremental_remote_action_inbetween_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--remote-only",
            "-c",
            &seed(),
            "-c",
            "test.use_incremental=false",
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

async fn incremental_action_with_non_incremental_local_action_inbetween_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            "test.use_incremental=false",
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

async fn basic_incremental_action_cached_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    let result = buck
        .run(&["root//:basic_incremental_action", "--remote-only"])
        .await?;
    assert_eq!(result.stdout, "foo");

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    // This is the correct/expected behavior because it means that the cached output was used and the action was
    // not re-executed because re-execution would have resulted in the output to be "foo bar". See below
    assert_eq!(result.stdout, "foo");

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

async fn basic_incremental_action_after_cache_hit_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    // Populate the remote cache
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--remote-only",
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    buck.clean(&[]).await?;

    // Run again, and make sure we got an action cache hit
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    let execution_kinds = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "ActionExecution",
            "execution_kind",
        ],
    )
    .await?;
    const ACTION_EXECUTION_KIND_ACTION_CACHE: i64 = 3;
    assert_eq!(
        *execution_kinds
            .last()
            .expect("the build has an action execution"),
        ACTION_EXECUTION_KIND_ACTION_CACHE
    );

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;

    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

async fn incremental_action_interleave_platforms_helper(
    buck: &Buck,
    platform: &str,
    use_content_based_path: bool,
) -> anyhow::Result<BuckOutput> {
    Ok(buck
        .run(&[
            "root//:basic_incremental_action",
            "--target-platforms",
            platform,
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?)
}

// We shouldn't lose the state from killing the daemon in between invocations
async fn incremental_action_persist_between_daemon_restart_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    buck.kill().await?;

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

// If we haven't materialized the outputs, then we won't run incrementally on the first run
// after a daemon restart
async fn unmaterialized_incremental_action_not_persist_between_daemon_restart_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    buck.build(&[
        "root//:basic_incremental_action",
        "--remote-only",
        "-c",
        &seed(),
        "-c",
        &content_based_path(use_content_based_path),
        "--materializations",
        "none",
    ])
    .await?;

    buck.kill().await?;

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");
    Ok(())
}

// Clean wipes buck-out, which should reset everything so incremental actions should start anew
async fn incremental_action_clean_resets_state_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    buck.clean(&[]).await?;

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");
    Ok(())
}

// In practice, there will be multiple actions with multiple outputs running. This test
// mimics that behavior a bit to ensure the states don't step over each other.
async fn incremental_action_multi_outputs_with_daemon_restart_helper(
    buck: &Buck,
    use_content_based_path: bool,
) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo");

    buck.kill().await?;

    let result = buck
        .run(&[
            "root//:incremental_action_with_multiple_outputs",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "ab");

    buck.kill().await?;

    let result = buck
        .run(&[
            "root//:basic_incremental_action",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "foo bar");

    buck.kill().await?;

    let result = buck
        .run(&[
            "root//:incremental_action_with_multiple_outputs",
            "--local-only",
            "-c",
            &seed(),
            "-c",
            &content_based_path(use_content_based_path),
        ])
        .await?;
    assert_eq!(result.stdout, "aabb");
    Ok(())
}

#[buck_test]
async fn test_basic_incremental_action_local_only(buck: &Buck) -> anyhow::Result<()> {
    basic_incremental_action_local_only_helper(buck, false).await
}

#[buck_test]
async fn test_basic_incremental_action_local_only_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    basic_incremental_action_local_only_helper(buck, true).await
}

#[buck_test]
async fn test_incremental_action_from_remote_action(buck: &Buck) -> anyhow::Result<()> {
    incremental_action_from_remote_action_helper(buck, false).await
}

#[buck_test]
async fn test_incremental_action_from_remote_action_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_from_remote_action_helper(buck, true).await
}

#[buck_test]
async fn test_incremental_action_with_non_incremental_remote_action_inbetween(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_with_non_incremental_remote_action_inbetween_helper(buck, false).await
}

#[buck_test]
async fn test_incremental_action_with_non_incremental_remote_action_inbetween_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_with_non_incremental_remote_action_inbetween_helper(buck, true).await
}

#[buck_test]
async fn test_incremental_action_with_non_incremental_local_action_inbetween(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_with_non_incremental_local_action_inbetween_helper(buck, false).await
}

#[buck_test]
async fn test_incremental_action_with_non_incremental_local_action_inbetween_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_with_non_incremental_local_action_inbetween_helper(buck, true).await
}

#[buck_test]
async fn test_basic_incremental_action_cached(buck: &Buck) -> anyhow::Result<()> {
    basic_incremental_action_cached_helper(buck, false).await
}

#[buck_test]
async fn test_basic_incremental_action_cached_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    basic_incremental_action_cached_helper(buck, true).await
}

#[buck_test]
async fn test_basic_incremental_action_after_cache_hit(buck: &Buck) -> anyhow::Result<()> {
    basic_incremental_action_after_cache_hit_helper(buck, false).await
}

#[buck_test]
async fn test_basic_incremental_action_after_cache_hit_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    basic_incremental_action_after_cache_hit_helper(buck, true).await
}

#[buck_test]
async fn test_incremental_action_interleave_platforms_aabb(buck: &Buck) -> anyhow::Result<()> {
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", false).await?;
    assert_eq!(result.stdout, "foo");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", false).await?;
    assert_eq!(result.stdout, "foo bar");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_cat", false).await?;
    assert_eq!(result.stdout, "foo");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_cat", false).await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

#[buck_test]
async fn test_incremental_action_different_platforms_abab(buck: &Buck) -> anyhow::Result<()> {
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", false).await?;
    assert_eq!(result.stdout, "foo");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_cat", false).await?;
    assert_eq!(result.stdout, "foo");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", false).await?;
    assert_eq!(result.stdout, "foo bar");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_cat", false).await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

#[buck_test]
async fn test_incremental_action_different_platforms_abba(buck: &Buck) -> anyhow::Result<()> {
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", false).await?;
    assert_eq!(result.stdout, "foo");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_cat", false).await?;
    assert_eq!(result.stdout, "foo");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_cat", false).await?;
    assert_eq!(result.stdout, "foo bar");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", false).await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

#[buck_test]
async fn test_incremental_action_interleave_platforms_aabb_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", true).await?;
    assert_eq!(result.stdout, "foo");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", true).await?;
    assert_eq!(result.stdout, "foo bar");
    let result = incremental_action_interleave_platforms_helper(buck, "root//:p_cat", true).await?;
    assert_eq!(result.stdout, "foo");
    let result = incremental_action_interleave_platforms_helper(buck, "root//:p_cat", true).await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

#[buck_test]
async fn test_incremental_action_interleave_platforms_abab_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", true).await?;
    assert_eq!(result.stdout, "foo");
    let result = incremental_action_interleave_platforms_helper(buck, "root//:p_cat", true).await?;
    assert_eq!(result.stdout, "foo");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", true).await?;
    assert_eq!(result.stdout, "foo bar");
    let result = incremental_action_interleave_platforms_helper(buck, "root//:p_cat", true).await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

#[buck_test]
async fn test_incremental_action_interleave_platforms_abba_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", true).await?;
    assert_eq!(result.stdout, "foo");
    let result = incremental_action_interleave_platforms_helper(buck, "root//:p_cat", true).await?;
    assert_eq!(result.stdout, "foo");
    let result = incremental_action_interleave_platforms_helper(buck, "root//:p_cat", true).await?;
    assert_eq!(result.stdout, "foo bar");
    let result =
        incremental_action_interleave_platforms_helper(buck, "root//:p_default", true).await?;
    assert_eq!(result.stdout, "foo bar");
    Ok(())
}

#[buck_test]
async fn test_incremental_action_with_metadata_opt_out(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:incremental_action_with_metadata_optout"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_incremental_action_persist_between_daemon_restart(buck: &Buck) -> anyhow::Result<()> {
    incremental_action_persist_between_daemon_restart_helper(buck, false).await
}

#[buck_test]
async fn test_incremental_action_persist_between_daemon_restart_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_persist_between_daemon_restart_helper(buck, true).await
}

#[buck_test]
async fn test_unmaterialized_incremental_action_not_persist_between_daemon_restart(
    buck: &Buck,
) -> anyhow::Result<()> {
    unmaterialized_incremental_action_not_persist_between_daemon_restart_helper(buck, false).await
}

#[buck_test]
async fn test_unmaterialized_incremental_action_not_persist_between_daemon_restart_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    unmaterialized_incremental_action_not_persist_between_daemon_restart_helper(buck, true).await
}

#[buck_test]
async fn test_incremental_action_clean_resets_state(buck: &Buck) -> anyhow::Result<()> {
    incremental_action_clean_resets_state_helper(buck, false).await
}

#[buck_test]
async fn test_incremental_action_clean_resets_state_with_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_clean_resets_state_helper(buck, true).await
}

#[buck_test]
async fn test_incremental_action_multi_outputs_with_daemon_restart(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_multi_outputs_with_daemon_restart_helper(buck, false).await
}

#[buck_test]
async fn test_incremental_action_multi_outputs_with_daemon_restart_and_content_based_path(
    buck: &Buck,
) -> anyhow::Result<()> {
    incremental_action_multi_outputs_with_daemon_restart_helper(buck, true).await
}

#[buck_test(extra_buck_config = [("buck2", "sqlite_incremental_state", "false")])]
async fn test_incremental_action_db_disabled(buck: &Buck) -> anyhow::Result<()> {
    basic_incremental_action_local_only_helper(buck, true).await
}

// BXL invocations with different arguments have different action owners, but the same BXL function
// label. Each invocation must get its own incremental state. Otherwise the second invocation starts
// from the output of the first.
#[buck_test(extra_buck_config = [("build", "execution_platforms", "root//:local_execution_platforms")])]
async fn test_bxl_incremental_state_keyed_by_arguments(buck: &Buck) -> anyhow::Result<()> {
    async fn run(buck: &Buck, name: &str) -> anyhow::Result<String> {
        let result = buck
            .bxl(&["root//incremental.bxl:incremental", "--", "--name", name])
            .await?;
        Ok(std::fs::read_to_string(
            buck.cwd().join(result.stdout.trim()),
        )?)
    }

    assert_eq!(run(buck, "a").await?, "foo");
    assert_eq!(run(buck, "b").await?, "foo");
    Ok(())
}
