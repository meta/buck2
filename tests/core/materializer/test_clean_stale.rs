/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use buck2_e2e::golden::sanitize_hashes;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::expect_exec_count;
use buck2_e2e::utils::replace_in_file;
use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use regex::Regex;

fn configure_active_unmaterialization(
    buck: &Buck,
    enabled: bool,
    scheduled: bool,
) -> std::io::Result<()> {
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        format!(
            "
[buck2]
ttl_refresh_enabled = true
clean_stale_enabled = {scheduled}
clean_stale_artifact_ttl_hours = 8
clean_stale_start_offset_hours = 0.001
clean_stale_period_hours = 0.0001
clean_stale_low_disk_threshold = 100.0
clean_stale_low_disk_adaptive_enabled = true
clean_stale_low_disk_adaptive_min_ttl_hours = 24
clean_stale_low_disk_adaptive_unmaterialize_active = {enabled}
clean_stale_unmaterialize_upload_enabled = {enabled}
        "
        ),
    )
}

fn configure_clean_stale(buck: &Buck, settings: &str) -> std::io::Result<()> {
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        format!("[buck2]\n{settings}"),
    )
}

async fn audit_entry(buck: &Buck, artifact_name: &str) -> anyhow::Result<String> {
    let entries = buck.audit(&["deferred-materializer", "list"]).await?.stdout;
    let matches: Vec<&str> = entries
        .lines()
        .filter(|entry| {
            entry
                .split_once('\t')
                .is_some_and(|(path, _)| path.contains(artifact_name))
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected one entry for {artifact_name}: {matches:?}"
    );
    Ok(matches[0].to_owned())
}

/// The path, relative to the project, that a `deferred-materializer list` entry is for.
fn entry_path(entry: &str) -> &str {
    entry.split_once('\t').map_or(entry, |(path, _)| path)
}

fn golden_audit_entries(entries: &[String], rel_path: &str) {
    let output = Regex::new(r"ts=[^,)]*")
        .unwrap()
        .replace_all(&entries.join("\n"), "ts=<TIMESTAMP>")
        .into_owned();
    let output = Regex::new(r"retrieved [0-9.]+ seconds ago with ttl = [0-9.]+ seconds")
        .unwrap()
        .replace_all(&output, "retrieved <AGE> ago with ttl = <TTL>")
        .into_owned();
    golden(&sanitize_hashes(&output), rel_path);
}

fn unix_time(time: SystemTime) -> anyhow::Result<u64> {
    Ok(time.duration_since(UNIX_EPOCH)?.as_secs())
}

/// Seven weeks from now, in seconds since the epoch.
fn future_time() -> anyhow::Result<u64> {
    unix_time(SystemTime::now() + Duration::from_secs(7 * 7 * 24 * 60 * 60))
}

/// Makes `path` writable by its owner, as `chmod u+w` does.
fn make_writable(path: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    #[expect(
        clippy::permissions_set_readonly_false,
        reason = "only the owner gets to write on Windows"
    )]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions)
}

/// Sets the permission bits of `path`; only a test that runs on Linux uses it.
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    unimplemented!("permission bits are Unix-only")
}

#[buck_test(env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")])]
async fn test_artifact_access_time(buck: &Buck) -> anyhow::Result<()> {
    // drop microseconds to match 1s precision from materializer
    let start = Timestamp::from_second(Timestamp::now().as_second())?;
    let target = "root//:copy";
    let result = buck.build(&[target]).await?;
    assert!(result.build_report()?.output_for_target(target)?.exists());

    // The deferred core lists an entry on disk as `materialized`, the disk-state core as
    // `tracked`; both carry the access time the same way.
    async fn audit_materialized(buck: &Buck) -> anyhow::Result<Vec<String>> {
        Ok(buck
            .audit(&["deferred-materializer", "list"])
            .await?
            .stdout
            .trim()
            .lines()
            .filter(|x| x.contains("\tmaterialized") || x.contains("\ttracked"))
            .map(str::to_owned)
            .collect())
    }

    fn parse_entry_ts(entry: &str) -> anyhow::Result<Timestamp> {
        let captures = Regex::new(r"\t(?:materialized|tracked) \(ts=([^ ,]*)")?
            .captures(entry)
            .unwrap_or_else(|| panic!("no timestamp in `{entry}`"));
        Ok(DateTime::strptime("%Y-%m-%dT%H:%M:%SZ", &captures[1])?
            .to_zoned(TimeZone::UTC)?
            .timestamp())
    }

    let materialized_entries = audit_materialized(buck).await?;
    assert_eq!(materialized_entries.len(), 1);
    let materialized_time = parse_entry_ts(&materialized_entries[0])?;
    assert!(materialized_time >= start);

    // Check that access time set after daemon restart
    buck.kill().await?;
    let materialized_entries = audit_materialized(buck).await?;
    assert_eq!(materialized_entries.len(), 1);
    let materialized_time = parse_entry_ts(&materialized_entries[0])?;
    assert!(materialized_time >= start);

    // Check that access time is updated following build
    tokio::time::sleep(Duration::from_secs(1)).await;
    buck.build(&[target]).await?;

    let materialized_entries = audit_materialized(buck).await?;

    assert_eq!(materialized_entries.len(), 1);
    let access_time = parse_entry_ts(&materialized_entries[0])?;
    assert!(access_time > materialized_time);
    Ok(())
}

#[buck_test(env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace"), ("BUCK_ACCESS_TIME_UPDATE_MAX_BUFFER_SIZE", "0")])]
async fn test_clean_stale_artifacts(buck: &Buck) -> anyhow::Result<()> {
    let target_1 = "root//:copy";
    let result_1 = buck.build(&[target_1]).await?;
    let output_1 = result_1.build_report()?.output_for_target(target_1)?;

    // ensure timestamp is after first materialization and before second
    // (resolution for access timestamps is 1 second)
    tokio::time::sleep(Duration::from_secs(1)).await;
    let after_first_build = unix_time(SystemTime::now())?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let target_2 = "root//:copy_2";
    let result_2 = buck.build(&[target_2]).await?;
    let output_2 = result_2.build_report()?.output_for_target(target_2)?;

    // Check output is correctly materialized
    assert!(output_1.exists());
    assert!(output_2.exists());

    let keep_since_first_build = format!("--keep-since-time={after_first_build}");
    buck.clean(&[&keep_since_first_build]).await?;
    // Check output_1 still materialized, it's stale but it was built by running daemon
    assert!(output_1.exists());

    buck.kill().await?;
    let res = buck.clean(&[&keep_since_first_build]).await?;
    // Check output_1 was cleaned because it's stale and not declared by running daemon
    assert!(res.stderr.contains("1 stale artifact") && res.stderr.contains("4 bytes cleaned"));
    assert!(!output_1.exists());
    assert!(output_2.exists());

    let keep_since_future = format!("--keep-since-time={}", future_time()?);

    // Check that a previously materialized output re-declared by new daemon is not cleaned
    buck.build(&[target_2]).await?;
    buck.clean(&[&keep_since_future]).await?;
    assert!(output_2.exists());

    // Check that setting keep-since-time in the future cleans non-active artifacts
    buck.kill().await?;
    let res = buck.clean(&[&keep_since_future]).await?;
    assert!(res.stderr.contains("1 stale artifact") && res.stderr.contains("4 bytes cleaned"));
    assert!(!output_2.exists());
    Ok(())
}

#[buck_test(env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace")])]
async fn test_clean_stale_artifact_dir(buck: &Buck) -> anyhow::Result<()> {
    let target_1 = "root//:copy";
    let result_1 = buck.build(&[target_1]).await?;
    let output_1 = result_1.build_report()?.output_for_target(target_1)?;
    assert!(output_1.exists());
    buck.kill().await?;
    let keep_since_future = format!("--keep-since-time={}", future_time()?);
    let res = buck.clean(&[&keep_since_future]).await?;
    assert!(res.stderr.contains("4 bytes cleaned"));
    assert!(!output_1.exists());
    // NOTE: Currently we require clean twice to delete empty dirs, which is ...
    // probably fine.
    buck.clean(&[&keep_since_future]).await?;
    let mut output_parent = output_1.parent().expect("an output has a parent");
    while !output_parent.exists() {
        output_parent = output_parent.parent().expect("buck-out exists");
    }
    assert!(output_parent.ends_with(Path::new("buck-out").join("v2").join("art")));
    Ok(())
}

#[buck_test(env = [("BUCK_ACCESS_TIME_UPDATE_MAX_BUFFER_SIZE", "0")])]
async fn test_clean_stale_buck_out_empty(buck: &Buck) -> anyhow::Result<()> {
    let output = buck.clean(&["--stale"]).await?;
    assert!(output.stderr.contains("Nothing to clean"));
    Ok(())
}

#[buck_test(env = [("BUCK_LOG", "buck2_execute_impl::materializers=trace"), ("BUCK_ACCESS_TIME_UPDATE_MAX_BUFFER_SIZE", "0")])]
async fn test_clean_stale_actions(buck: &Buck) -> anyhow::Result<()> {
    let query_res = buck.cquery(&["root//..."]).await?;
    let targets: Vec<&str> = query_res
        .stdout
        .split('\n')
        .filter(|target| !target.is_empty())
        .map(|target| target.split(' ').next().unwrap_or(target))
        .collect();

    let mut outputs: Vec<PathBuf> = Vec::new();
    for target in &targets {
        let res = buck.build(&[target]).await?;
        outputs.extend(res.build_report()?.outputs_for_target(target, "DEFAULT")?);
    }

    assert!(outputs.len() >= targets.len());
    for output in &outputs {
        assert!(output.exists());
    }

    buck.clean(&["--stale"]).await?;
    for output in &outputs {
        assert!(output.exists());
    }
    Ok(())
}

#[buck_test]
async fn test_clean_stale_uses_configured_ttl_when_scheduling_disabled(
    buck: &Buck,
) -> anyhow::Result<()> {
    configure_clean_stale(buck, "clean_stale_artifact_ttl_hours = 0\n")?;
    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());

    buck.kill().await?;
    buck.clean(&["--stale"]).await?;
    assert!(!output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_uses_configured_adaptive_policy(buck: &Buck) -> anyhow::Result<()> {
    configure_clean_stale(
        buck,
        "clean_stale_artifact_ttl_hours = 8
clean_stale_low_disk_threshold = 100.0
clean_stale_low_disk_adaptive_enabled = true
clean_stale_low_disk_adaptive_min_ttl_hours = 0
",
    )?;
    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());

    buck.kill().await?;
    buck.clean(&["--stale"]).await?;
    assert!(!output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_uses_configured_dry_run(buck: &Buck) -> anyhow::Result<()> {
    configure_clean_stale(
        buck,
        "clean_stale_artifact_ttl_hours = 0
clean_stale_dry_run = true
",
    )?;
    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());

    buck.kill().await?;
    buck.clean(&["--stale"]).await?;
    assert!(output.exists());
    Ok(())
}

#[buck_test]
async fn test_explicit_clean_stale_duration_ignores_configured_adaptive_policy(
    buck: &Buck,
) -> anyhow::Result<()> {
    configure_clean_stale(
        buck,
        "clean_stale_artifact_ttl_hours = 0
clean_stale_low_disk_threshold = 100.0
clean_stale_low_disk_adaptive_enabled = true
clean_stale_low_disk_adaptive_min_ttl_hours = 0
",
    )?;
    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());

    buck.kill().await?;
    buck.clean(&["--stale=10000d"]).await?;
    assert!(output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_declared(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//declared:declared"]).await?;
    buck.kill().await?;

    // Drop the state. The path exists on disk.
    std::fs::remove_dir_all(buck.cwd().join("buck-out/v2/cache/materializer_state"))?;

    // Build again, start by declaring, then clean, then require locally.
    buck.build(&["//declared:remote"]).await?;
    buck.clean(&["--stale"]).await?;
    buck.build(&["//declared:local"]).await?;
    Ok(())
}

#[buck_test]
async fn test_clean_stale_scheduled(buck: &Buck) -> anyhow::Result<()> {
    // Need to write to .buckconfig instead of passing cmd line args because
    // the config used when creating daemon state does not include cmd line args (but maybe it should).
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "
[buck2]
clean_stale_enabled = true
clean_stale_artifact_ttl_hours = 0
clean_stale_start_offset_hours = 0
# 0.0001h = 360ms
clean_stale_period_hours = 0.0001
        ",
    )?;

    // Just test that a clean runs if enabled via config.
    // Build a target, output is stale immediately but won't be cleaned until restart.
    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());
    buck.kill().await?;
    // Create a new daemon and build something else (could be any command that starts a daemon).
    buck.build(&["//declared:declared"]).await?;
    // Wait for at least one clean to run (but should have finished multiple cleans).
    tokio::time::sleep(Duration::from_secs(3)).await;
    // Original output should be cleaned.
    assert!(!output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_scheduled_does_not_run_during_command(buck: &Buck) -> anyhow::Result<()> {
    configure_clean_stale(
        buck,
        "
clean_stale_enabled = true
clean_stale_artifact_ttl_hours = 0
clean_stale_start_offset_hours = 0.0001
clean_stale_period_hours = 0.0001
        ",
    )?;

    let result = buck
        .build(&["root//:slow_write", "--no-remote-cache"])
        .await?;
    let output = result
        .build_report()?
        .output_for_target("root//:slow_write")?;
    assert_eq!(std::fs::read_to_string(output)?.trim(), "finished");
    Ok(())
}

#[buck_test]
async fn test_clean_stale_scheduled_high_disk_usage(buck: &Buck) -> anyhow::Result<()> {
    // Need to write to .buckconfig instead of passing cmd line args because
    // the config used when creating daemon state does not include cmd line args (but maybe it should).
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "
[buck2]
clean_stale_enabled = true
clean_stale_artifact_ttl_hours = 8
clean_stale_start_offset_hours = 0
# 0.0001h = 360ms
clean_stale_period_hours = 0.0001
clean_stale_low_disk_threshold = 100.0
clean_stale_low_disk_artifact_ttl_hours = 0.0
        ",
    )?;

    // Just test that a clean runs if enabled via config.
    // Build a target, output is stale immediately but won't be cleaned until restart.
    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());
    buck.kill().await?;
    // Create a new daemon and build something else (could be any command that starts a daemon).
    buck.build(&["//declared:declared"]).await?;
    // Wait for at least one clean to run (but should have finished multiple cleans).
    tokio::time::sleep(Duration::from_secs(3)).await;
    // Original output should be cleaned.
    assert!(!output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_scheduled_adaptive_high_disk_usage(buck: &Buck) -> anyhow::Result<()> {
    // Threshold of 100.0 guarantees free disk % is always "below" it, so the
    // adaptive loop must promote retained, non-active artifacts to stale even
    // though the regular ttl (8h) would have kept them.
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "
[buck2]
clean_stale_enabled = true
clean_stale_artifact_ttl_hours = 8
clean_stale_start_offset_hours = 0
# 0.0001h = 360ms
clean_stale_period_hours = 0.0001
clean_stale_low_disk_threshold = 100.0
clean_stale_low_disk_adaptive_enabled = true
clean_stale_low_disk_adaptive_min_ttl_hours = 0
        ",
    )?;

    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());
    buck.kill().await?;
    buck.build(&["//declared:declared"]).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_scheduled_adaptive_threshold_not_tripped(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Threshold of 0.0 guarantees free disk % is always above it, so the
    // adaptive loop must never engage and the retained artifact survives.
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "
[buck2]
clean_stale_enabled = true
clean_stale_artifact_ttl_hours = 8
clean_stale_start_offset_hours = 0
# 0.0001h = 360ms
clean_stale_period_hours = 0.0001
clean_stale_low_disk_threshold = 0.0
clean_stale_low_disk_adaptive_enabled = true
clean_stale_low_disk_adaptive_min_ttl_hours = 0
        ",
    )?;

    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());
    buck.kill().await?;
    buck.build(&["//declared:declared"]).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_scheduled_adaptive_min_ttl_protects_recent(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Threshold of 100.0 always trips adaptive promotion, but the freshly
    // built artifact is well within the 24h adaptive min-TTL floor — it must
    // survive even though disk pressure persists.
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "
[buck2]
clean_stale_enabled = true
clean_stale_artifact_ttl_hours = 8
clean_stale_start_offset_hours = 0
# 0.0001h = 360ms
clean_stale_period_hours = 0.0001
clean_stale_low_disk_threshold = 100.0
clean_stale_low_disk_adaptive_enabled = true
clean_stale_low_disk_adaptive_min_ttl_hours = 24
        ",
    )?;

    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());
    buck.kill().await?;
    buck.build(&["//declared:declared"]).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_cli_adaptive_promotes_retained(buck: &Buck) -> anyhow::Result<()> {
    // `--stale=10000d` alone would not clean a freshly-built artifact, but
    // `--adaptive-low-disk-threshold=100.0` always trips the adaptive branch
    // (free disk % is always <= 100%) and `--adaptive-min-ttl=0s` protects
    // nothing, so the retained, non-active artifact must be promoted to stale
    // and removed.
    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());
    buck.kill().await?;
    // New daemon — original artifact is retained but no longer active.
    buck.build(&["//declared:declared"]).await?;
    let res = buck
        .clean(&[
            "--stale=10000d",
            "--adaptive-low-disk-threshold=100.0",
            "--adaptive-min-ttl=0s",
        ])
        .await?;
    assert!(
        res.stderr
            .contains("Adaptive low-disk promotion enabled at 100%")
    );
    assert!(!output.exists());
    Ok(())
}

#[buck_test]
async fn test_clean_stale_cli_adaptive_min_ttl_protects_recent(buck: &Buck) -> anyhow::Result<()> {
    // Adaptive is tripped (threshold=100%), but `--adaptive-min-ttl=24h`
    // protects every retained artifact accessed within the last 24h, so the
    // freshly-built output survives.
    let result = buck.build(&["root//:copy"]).await?;
    let output = result.build_report()?.output_for_target("root//:copy")?;
    assert!(output.exists());
    buck.kill().await?;
    buck.build(&["//declared:declared"]).await?;
    buck.clean(&[
        "--stale=10000d",
        "--adaptive-low-disk-threshold=100.0",
        "--adaptive-min-ttl=24h",
    ])
    .await?;
    assert!(output.exists());
    Ok(())
}

#[buck_test(skip_for_materializer = ["v2"])]
async fn test_adaptive_unmaterializes_active_remote_intermediate(
    buck: &Buck,
) -> anyhow::Result<()> {
    configure_active_unmaterialization(buck, true, true)?;
    let result = buck
        .build(&["root//:consume_remote", "--no-remote-cache"])
        .await?;
    let output = result
        .build_report()?
        .output_for_target("root//:consume_remote")?;
    assert!(output.exists());

    // Whether an adaptive pass has already run by the time the build returns is a race, so the
    // intermediate's state is only recorded once it has certainly had the chance to.
    tokio::time::sleep(Duration::from_secs(30)).await;
    let mut audit_entries = vec![audit_entry(buck, "__remote_write__").await?];

    let remote = buck.build(&["root//:remote_write"]).await?;
    expect_exec_count(buck, 0).await?;
    assert!(
        remote
            .build_report()?
            .output_for_target("root//:remote_write")?
            .exists()
    );
    audit_entries.push(audit_entry(buck, "__remote_write__").await?);
    golden_audit_entries(
        &audit_entries,
        "golden/test_adaptive_unmaterializes_active_remote_intermediate.golden.txt",
    );
    Ok(())
}

#[buck_test(skip_for_materializer = ["v2"])]
async fn test_adaptive_unmaterializes_active_write_intermediate(buck: &Buck) -> anyhow::Result<()> {
    configure_active_unmaterialization(buck, true, false)?;
    // This build materializes `root//:write` as intermediate artifact
    let result = buck
        .build(&["root//:consume_local", "--local-only", "--no-remote-cache"])
        .await?;
    assert!(
        result
            .build_report()?
            .output_for_target("root//:consume_local")?
            .exists()
    );
    let mut audit_entries = vec![audit_entry(buck, "__write__").await?];
    let artifact_path = entry_path(&audit_entries[0]).to_owned();

    // `root//:write` should get unmaterialized here
    buck.clean(&["--stale"]).await?;
    audit_entries.push(audit_entry(buck, &artifact_path).await?);

    // Now we request `root//:write` as final output, which should require the unmaterialized
    // artifact to be re-materialized.
    let write = buck.build(&["root//:write"]).await?;
    expect_exec_count(buck, 0).await?;
    assert!(
        write
            .build_report()?
            .output_for_target("root//:write")?
            .exists()
    );
    audit_entries.push(audit_entry(buck, &artifact_path).await?);
    golden_audit_entries(
        &audit_entries,
        "golden/test_adaptive_unmaterializes_active_write_intermediate.golden.txt",
    );
    Ok(())
}

#[buck_test(skip_for_materializer = ["v2"])]
async fn test_adaptive_unmaterialization_fails_for_modified_local_intermediate(
    buck: &Buck,
) -> anyhow::Result<()> {
    configure_active_unmaterialization(buck, true, false)?;
    let original = format!(
        "ORIGINAL-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    replace_in_file(
        r#"content = "HELLO""#,
        &format!(r#"content = "{original}""#),
        &buck.cwd().join("TARGETS.fixture"),
    );
    let result = buck
        .build(&["root//:consume_local", "--local-only", "--no-remote-cache"])
        .await?;
    assert!(
        result
            .build_report()?
            .output_for_target("root//:consume_local")?
            .exists()
    );

    let entry = audit_entry(buck, "__write__").await?;
    assert!(entry.contains("\tmaterialized"));
    let artifact = buck.cwd().join(entry_path(&entry));
    assert_eq!(std::fs::read_to_string(&artifact)?, original);
    // Tracked outputs are read-only (`buck2.readonly_outputs`); the edit is what a user
    // would do after a `chmod`.
    make_writable(&artifact)?;
    std::fs::write(&artifact, "EDITED")?;

    buck.clean(&["--stale"]).await?;

    assert!(
        audit_entry(buck, "__write__")
            .await?
            .contains("\tmaterialized")
    );
    assert_eq!(std::fs::read_to_string(&artifact)?, "EDITED");
    Ok(())
}

#[buck_test(skip_for_materializer = ["v2"])]
async fn test_adaptive_unmaterializes_active_local_copy_intermediate(
    buck: &Buck,
) -> anyhow::Result<()> {
    configure_active_unmaterialization(buck, true, false)?;
    let result = buck
        .build(&["root//:consume_copy", "--local-only", "--no-remote-cache"])
        .await?;
    assert!(
        result
            .build_report()?
            .output_for_target("root//:consume_copy")?
            .exists()
    );
    let mut audit_entries = vec![audit_entry(buck, "__consume_local__").await?];
    let artifact_path = entry_path(&audit_entries[0]).to_owned();

    buck.clean(&["--stale"]).await?;
    audit_entries.push(audit_entry(buck, &artifact_path).await?);

    let copied = buck.build(&["root//:consume_local"]).await?;
    expect_exec_count(buck, 0).await?;
    assert!(
        copied
            .build_report()?
            .output_for_target("root//:consume_local")?
            .exists()
    );
    audit_entries.push(audit_entry(buck, &artifact_path).await?);
    golden_audit_entries(
        &audit_entries,
        "golden/test_adaptive_unmaterializes_active_local_copy_intermediate.golden.txt",
    );
    Ok(())
}

#[buck_test(skip_for_materializer = ["v2"])]
async fn test_adaptive_unmaterializes_active_local_action_intermediate(
    buck: &Buck,
) -> anyhow::Result<()> {
    configure_active_unmaterialization(buck, true, false)?;
    let result = buck
        .build(&["root//:consume_action", "--local-only", "--no-remote-cache"])
        .await?;
    assert!(
        result
            .build_report()?
            .output_for_target("root//:consume_action")?
            .exists()
    );
    let mut audit_entries = vec![audit_entry(buck, "__copy_dir__").await?];

    buck.clean(&["--stale"]).await?;
    audit_entries.push(audit_entry(buck, "__copy_dir__").await?);

    let action = buck.build(&["root//:copy_dir"]).await?;
    expect_exec_count(buck, 0).await?;
    assert!(
        action
            .build_report()?
            .output_for_target("root//:copy_dir")?
            .exists()
    );
    audit_entries.push(audit_entry(buck, "__copy_dir__").await?);
    golden_audit_entries(
        &audit_entries,
        "golden/test_adaptive_unmaterializes_active_local_action_intermediate.golden.txt",
    );
    Ok(())
}

#[buck_test(skip_for_materializer = ["v2"])]
async fn test_adaptive_does_not_unmaterialize_active_final_output(
    buck: &Buck,
) -> anyhow::Result<()> {
    configure_active_unmaterialization(buck, true, true)?;
    let result = buck.build(&["root//:remote_write"]).await?;
    assert!(
        result
            .build_report()?
            .output_for_target("root//:remote_write")?
            .exists()
    );

    tokio::time::sleep(Duration::from_secs(30)).await;
    golden_audit_entries(
        &[audit_entry(buck, "__remote_write__").await?],
        "golden/test_adaptive_does_not_unmaterialize_active_final_output.golden.txt",
    );
    Ok(())
}

#[buck_test(skip_for_materializer = ["v2"])]
async fn test_adaptive_does_not_unmaterialize_when_disabled(buck: &Buck) -> anyhow::Result<()> {
    configure_active_unmaterialization(buck, false, true)?;
    let result = buck
        .build(&["root//:consume_remote", "--no-remote-cache"])
        .await?;
    assert!(
        result
            .build_report()?
            .output_for_target("root//:consume_remote")?
            .exists()
    );
    let mut audit_entries = vec![audit_entry(buck, "__remote_write__").await?];

    tokio::time::sleep(Duration::from_secs(30)).await;
    audit_entries.push(audit_entry(buck, "__remote_write__").await?);
    golden_audit_entries(
        &audit_entries,
        "golden/test_adaptive_does_not_unmaterialize_when_disabled.golden.txt",
    );
    Ok(())
}

/// Scratch (buck-out/<iso>/tmp) is swept once the daemon goes idle.
#[buck_test(skip_for_os = ["windows", "darwin"])]
async fn test_clean_scratch_on_idle(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2]\nclean_scratch_on_idle = true\n",
    )?;

    // Dead scratch from past actions: deleted regardless of age.
    let dead = buck
        .cwd()
        .join("buck-out")
        .join("v2")
        .join("tmp")
        .join("root")
        .join("aaaa")
        .join("cat")
        .join("dead_action");
    std::fs::create_dir_all(&dead)?;
    std::fs::write(dead.join("junk"), "x".repeat(16))?;

    // A sibling scratch root the sweep's scan cannot read: still deleted, since
    // the deletion adds owner permissions to the directories it removes.
    let unreadable = buck
        .cwd()
        .join("buck-out")
        .join("v2")
        .join("tmp")
        .join("unreadable");
    std::fs::create_dir(&unreadable)?;
    std::fs::write(unreadable.join("junk"), "y")?;
    set_mode(&unreadable, 0o000)?;

    let swept = async {
        buck.build(&["root//:copy"]).await?;
        // The sweep starts shortly after the command finishes, once the daemon
        // is idle.
        for _ in 0..60 {
            if !dead.exists() && !unreadable.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        anyhow::Ok(())
    }
    .await;
    if unreadable.exists() {
        set_mode(&unreadable, 0o755)?;
    }
    swept?;

    assert!(!dead.exists());
    assert!(!unreadable.exists());
    Ok(())
}
