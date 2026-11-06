/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::PathBuf;

use buck2_e2e::http::StaticHttpServer;
use buck2_e2e::http::sha1_hex;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_event_roots;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::replace_in_file;

fn disable_sqlite_materializer_state(buck: &Buck) {
    let config_file = buck.cwd().join(".buckconfig");
    replace_in_file(
        "sqlite_materializer_state = true",
        "sqlite_materializer_state = false",
        &config_file,
    );
}

#[buck_test(data_dir = "modify_deferred_materialization")]
async fn test_modify_input_source(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:urandom_dep"]).await?;

    let targets_file = buck.cwd().join("TARGETS.fixture");

    // Change the label in Targets.
    replace_in_file(
        "__NOT_A_REAL_LABEL__",
        "buck2_test_local_exec",
        &targets_file,
    );

    buck.build(&["//:urandom_dep"]).await?;
    Ok(())
}

#[buck_test(
    data_dir = "modify_deferred_materialization_deps",
    skip_for_os = ["windows"], // TODO(marwhal): Fix and enable on Windows
)]
async fn test_modify_dep_materialization(buck: &Buck) -> anyhow::Result<()> {
    let target = "//:check";

    // Build, expect the symlink to work. We'll materialize the first time.

    let result = buck.build(&[target]).await?;
    let output = result.build_report()?.output_for_target(target)?;
    assert_eq!(std::fs::read_to_string(output)?.trim(), "TEXT");

    // Build again, expect the symlink to work. We'll materialize just deps.

    std::fs::write(buck.cwd().join("text"), "TEXT2")?;

    let result = buck.build(&[target]).await?;
    let output = result.build_report()?.output_for_target(target)?;
    assert_eq!(std::fs::read_to_string(output)?.trim(), "TEXT2");

    // Build again, expect the symlink to work. We'll materialize just deps
    // again. However this time our state is a little different since the
    // previous future was a check-deps only future.

    std::fs::write(buck.cwd().join("text"), "TEXT3")?;

    let result = buck.build(&[target]).await?;
    let output = result.build_report()?.output_for_target(target)?;
    assert_eq!(std::fs::read_to_string(output)?.trim(), "TEXT3");
    Ok(())
}

#[buck_test(
    data_dir = "deferred_materializer_matching_artifact_optimization",
    skip_for_materializer = ["v2"],
    env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")],
)]
async fn test_matching_artifact_optimization(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:copy";
    let result = buck.build(&[target]).await?;
    // Check output is correctly materialized
    assert!(result.build_report()?.output_for_target(target)?.exists());

    // In this case, modifying `hidden` does not change the output, so the output should not
    // need to be rematerialized
    std::fs::write(buck.cwd().join("hidden"), "HIDDEN2")?;

    let result = buck.build(&[target]).await?;
    // Check output still exists
    assert!(result.build_report()?.output_for_target(target)?.exists());
    // Check that materializer did not report any rematerialization
    assert!(
        result
            .stderr
            .contains("already materialized, updating deps only")
    );
    assert!(!result.stderr.contains("materialize artifact"));

    // In this case, modifying `src` changes the output, so the output should be rematerialized
    std::fs::write(buck.cwd().join("src"), "SRC2")?;

    let result = buck.build(&[target]).await?;
    // Check output still exists
    let output = result.build_report()?.output_for_target(target)?;
    assert!(output.exists());
    assert_eq!(std::fs::read_to_string(output)?.trim(), "SRC2");
    Ok(())
}

#[buck_test(data_dir = "deferred_materializer_matching_artifact_optimization")]
async fn test_cache_directory_cleanup(buck: &Buck) -> anyhow::Result<()> {
    // sqlite materializer state is already enabled
    let cache_dir = buck.cwd().join("buck-out").join("v2").join("cache");
    let materializer_state_dir = cache_dir.join("materializer_state");
    std::fs::create_dir_all(&materializer_state_dir)?;
    let incremental_state_dir = cache_dir.join("incremental_state");
    std::fs::create_dir_all(&incremental_state_dir)?;
    let command_hashes_dir = cache_dir.join("command_hashes");
    std::fs::create_dir_all(&command_hashes_dir)?;

    // Need to run a command to start the daemon.
    buck.audit_config(&[]).await?;

    let mut cache_dir_listing = std::fs::read_dir(&cache_dir)?
        .map(|entry| Ok(entry?.path()))
        .collect::<std::io::Result<Vec<PathBuf>>>()?;
    cache_dir_listing.sort();
    assert_eq!(
        cache_dir_listing,
        [incremental_state_dir.clone(), materializer_state_dir]
    );

    buck.kill().await?;
    disable_sqlite_materializer_state(buck);
    buck.audit_config(&[]).await?;

    let cache_dir_listing = std::fs::read_dir(&cache_dir)?
        .map(|entry| Ok(entry?.path()))
        .collect::<std::io::Result<Vec<PathBuf>>>()?;
    assert_eq!(cache_dir_listing, [incremental_state_dir]);
    Ok(())
}

#[buck_test(
    data_dir = "deferred_materializer_matching_artifact_optimization",
    skip_for_materializer = ["v2"],
    env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")],
)]
async fn test_sqlite_materializer_state_matching_artifact_optimization(
    buck: &Buck,
) -> anyhow::Result<()> {
    // sqlite materializer state is already enabled
    let target = "root//:copy";
    let res = buck.build(&[target]).await?;
    // Check output is correctly materialized
    assert!(res.build_report()?.output_for_target(target)?.exists());

    buck.kill().await?;

    let res = buck.build(&[target]).await?;
    // Check that materializer did not report any rematerialization
    assert!(
        res.stderr
            .contains("already materialized, updating deps only"),
        "{}",
        res.stderr
    );
    assert!(!res.stderr.contains("materialize artifact"));

    buck.kill().await?;

    // In this case, modifying `src` changes the output, so the output should be rematerialized
    std::fs::write(buck.cwd().join("src"), "SRC2")?;

    let res = buck.build(&[target]).await?;
    // Check output still exists
    let output = res.build_report()?.output_for_target(target)?;
    assert!(output.exists());
    assert_eq!(std::fs::read_to_string(output)?.trim(), "SRC2");
    Ok(())
}

#[buck_test(data_dir = "deferred_materializer_matching_artifact_optimization")]
async fn test_download_file_not_repeated_after_restart(buck: &Buck) -> anyhow::Result<()> {
    // Fresh content, so the CAS cannot already have it and the download really happens.
    let content = random_string().into_bytes();
    let server = StaticHttpServer::start([("/file", content.clone())]).await?;
    let target = "root//:download";
    let url_config = format!("test.url={}", server.url("/file"));
    let sha1_config = format!("test.sha1={}", sha1_hex(&content));
    let args = [target, "-c", &url_config, "-c", &sha1_config];

    let res = buck.build(&args).await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert_eq!(std::fs::read(&output)?, content);
    assert_eq!(server.count("GET", "/file"), 1);

    // The sqlite materializer state tells the new daemon the file is already there.
    buck.kill().await?;
    buck.build(&args).await?;
    assert_eq!(std::fs::read(&output)?, content);
    assert_eq!(server.count("GET", "/file"), 1);
    Ok(())
}

#[buck_test(
    data_dir = "deferred_materializer_matching_artifact_optimization",
    skip_for_materializer = ["v2"],
    env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")],
)]
async fn test_sqlite_materializer_state_disabled(buck: &Buck) -> anyhow::Result<()> {
    disable_sqlite_materializer_state(buck);

    let target = "root//:copy";
    let result = buck.build(&[target]).await?;
    // Check output is correctly materialized
    assert!(result.build_report()?.output_for_target(target)?.exists());

    buck.kill().await?;

    let result = buck.build(&[target]).await?;
    // Check that materializer did have to rematerialize the same artifact
    assert!(
        !result
            .stderr
            .contains("already materialized, updating deps only")
    );
    assert!(result.stderr.contains("materialize artifact"));
    Ok(())
}

#[buck_test(
    data_dir = "deferred_materializer_matching_artifact_optimization",
    env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")],
)]
async fn test_sqlite_materializer_state_buckconfig_version_change(
    buck: &Buck,
) -> anyhow::Result<()> {
    // sqlite materializer state is already enabled
    let target = "root//:copy";
    let result = buck.build(&[target]).await?;
    // Check output is correctly materialized
    assert!(result.build_report()?.output_for_target(target)?.exists());

    buck.kill().await?;

    // Bump the buckconfig version of sqlite materializer state to invalidate the existing sqlite db
    replace_in_file(
        "sqlite_materializer_state_version = 0",
        "sqlite_materializer_state_version = 1",
        &buck.cwd().join(".buckconfig"),
    );

    // just starting the buck2 daemon should delete the sqlite materializer state
    buck.audit_config(&[]).await?;
    Ok(())
}

/// Materialization spans should be parented to the span that triggered them,
/// not appear as root spans with parent_id == 0.
#[buck_test(
    data_dir = "modify_deferred_materialization_deps",
    skip_for_os = ["windows"],
)]
async fn test_materialization_spans_have_parent_id(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:check"]).await?;

    let materialization_events = filter_event_roots(
        buck,
        &["Event", "data", "SpanStart", "data", "Materialization"],
    )
    .await?;

    assert!(
        !materialization_events.is_empty(),
        "Expected at least one Materialization span"
    );
    for event in &materialization_events {
        assert_ne!(
            event
                .pointer("/Event/parent_id")
                .unwrap_or_else(|| panic!("No `parent_id` in {event}")),
            0,
            "Materialization span has parent_id == 0 (no parent): {event}"
        );
    }
    Ok(())
}

/// MaterializerCommand instant events emitted on the synchronous command
/// processing thread should be parented to the span that sent the command,
/// not appear as root events with parent_id == 0.  This requires
/// verbose_materializer_event_log = true in .buckconfig.
#[buck_test(
    data_dir = "modify_deferred_materialization_deps",
    skip_for_os = ["windows"],
    skip_for_materializer = ["v2"],
)]
async fn test_materializer_command_events_have_parent_id(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:check"]).await?;

    let command_events = filter_event_roots(
        buck,
        &["Event", "data", "Instant", "data", "MaterializerCommand"],
    )
    .await?;

    assert!(
        !command_events.is_empty(),
        "Expected at least one MaterializerCommand instant event (is verbose_materializer_event_log enabled?)"
    );
    for event in &command_events {
        assert_ne!(
            event
                .pointer("/Event/parent_id")
                .unwrap_or_else(|| panic!("No `parent_id` in {event}")),
            0,
            "MaterializerCommand event has parent_id == 0 (no parent): {event}"
        );
    }
    Ok(())
}
