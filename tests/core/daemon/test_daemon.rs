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
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use buck2_e2e::BuckError;
use buck2_e2e::BuckOutput;
use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;
use buck2_e2e::process::daemon_is_alive;
use buck2_e2e::utils::file_id;
use buck2_e2e::utils::filter_events;
use common_unidaemon::THIRTY_SECONDS;
use common_unidaemon::assert_daemon_exits;
use common_unidaemon::bool_cases;
use common_unidaemon::cases;
use common_unidaemon::daemon_pid;
use common_unidaemon::debug_tenants;
use common_unidaemon::generation_of;
use common_unidaemon::isolated_buck;
use common_unidaemon::isolation_of;
use common_unidaemon::materializer_identity;
use common_unidaemon::path_str;
use common_unidaemon::pid_of;
use common_unidaemon::proxy_slot;
use common_unidaemon::start_generation;
use common_unidaemon::status_json;
use common_unidaemon::tenant_entry;
use common_unidaemon::tenant_generation;
use common_unidaemon::wait_for_evicted_generation;
use common_unidaemon::wait_for_marker;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

/// Whether Python takes `value` as true.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

async fn server_status_pid(buck: &Buck) -> anyhow::Result<u32> {
    pid_of(&serde_json::from_str(
        &buck.server(&["--status"]).await?.stdout,
    )?)
}

fn extract_pid(stderr: &str) -> u64 {
    let regex = Regex::new(r"^.* PID: (\d+)").unwrap();
    let pids: Vec<u64> = stderr
        .lines()
        .filter_map(|line| regex.captures(line))
        .map(|c| c[1].parse().unwrap())
        .collect();
    assert_eq!(pids.len(), 1, "{pids:?}");
    pids[0]
}

fn touch(path: &Path) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(())
}

fn append_to_file(path: &Path, text: &str) -> anyhow::Result<()> {
    let contents = std::fs::read_to_string(path)?;
    std::fs::write(path, contents + text)?;
    Ok(())
}

/// The daemon holds the tenant's disk ownership lock, so no other process can take it.
fn assert_tenant_disk_locked(buck: &Buck, isolation: &str) -> anyhow::Result<()> {
    let path = proxy_slot(buck)
        .parent()
        .context("The proxy slot has no parent")?
        .join("._buck2_tenant_locks")
        .join(isolation);
    let probe = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    match probe.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => Ok(()),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        Ok(()) => {
            probe.unlock()?;
            anyhow::bail!("Tenant disk lock `{}` is not held", path.display())
        }
    }
}

fn isolations(entries: &[Value]) -> anyhow::Result<Vec<&str>> {
    entries.iter().map(isolation_of).collect()
}

/// What the Python tests' `asyncio.wait_for(asyncio.shield(build), timeout=30)` gave: the
/// command's outcome, or an error if it took longer.
async fn wait_within(
    process: BuckProcess,
    timeout: Duration,
) -> anyhow::Result<Result<BuckOutput, BuckError>> {
    tokio::time::timeout(timeout, process.wait())
        .await
        .with_context(|| format!("The command did not finish within {timeout:?}"))
}

/// Waits up to `timeout` for the commands to exit, whatever their outcome, as the Python tests'
/// `asyncio.gather(..., return_exceptions=True)` did.
async fn wait_for_all(
    processes: impl IntoIterator<Item = BuckProcess>,
    timeout: Duration,
) -> anyhow::Result<()> {
    let _outcomes = tokio::time::timeout(
        timeout,
        futures::future::join_all(processes.into_iter().map(BuckProcess::wait)),
    )
    .await
    .with_context(|| format!("The commands did not finish within {timeout:?}"))?;
    Ok(())
}

/// What `assertRaisesRegex(BuckException, "evicted")` checked: the command failed, saying so.
#[track_caller]
fn assert_evicted(outcome: Result<BuckOutput, BuckError>) {
    let error = outcome.expect_err("The command was expected to fail");
    assert!(error.to_string().contains("evicted"), "{error}");
}

fn generation_output(result: &BuckOutput) -> anyhow::Result<PathBuf> {
    result.build_report()?.output_for_target("//:generation")
}

fn read_generation(result: &BuckOutput) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(generation_output(result)?)?)
}

async fn wait_for_tenant_generation(
    buck: &Buck,
    isolation: &str,
    generation: u64,
) -> anyhow::Result<Value> {
    tokio::time::timeout(THIRTY_SECONDS, async {
        loop {
            let tenants = debug_tenants(buck).await?;
            let mut entry = None;
            for tenant in tenants {
                if isolation_of(&tenant)? == isolation {
                    entry = Some(tenant);
                    break;
                }
            }
            if let Some(entry) = entry
                && generation_of(&entry)? > generation
            {
                return Ok(entry);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("Timed out waiting for a new tenant generation")?
}

async fn dice_analysis_keys(buck: &Buck, path: &Path) -> anyhow::Result<BTreeMap<String, u64>> {
    buck.debug(&["dice-dump", "--path", path_str(path)]).await?;
    let mut nodes = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path.join("nodes.gz"))?)
        .read_to_string(&mut nodes)?;
    let mut keys = BTreeMap::new();
    for line in nodes.lines() {
        let row: Vec<&str> = line.split('\t').collect();
        let [index, kind, key] = row.as_slice() else {
            anyhow::bail!("Expected `index\\tkind\\tkey`, got `{line}`");
        };
        if *kind == "AnalysisKey" {
            let key = key.split_once(" (").map_or(*key, |(key, _)| key);
            keys.insert(key.to_owned(), index.parse()?);
        }
    }
    Ok(keys)
}

async fn tenant_value(buck: &Buck, config: &Path) -> anyhow::Result<PathBuf> {
    let result = buck
        .build(&[
            "//:tenant_value",
            "--config-file",
            path_str(config),
            "--build-report=-",
        ])
        .await?;
    result.build_report()?.output_for_target("//:tenant_value")
}

/// Each tenant with its isolation dir.
async fn assert_tenant_cached_paths(tenants: &[(&Buck, &str)]) -> anyhow::Result<()> {
    let mut outputs = BTreeSet::new();
    for first_run in [true, false] {
        for (tenant, isolation) in tenants {
            let result = tenant.bxl(&["//paths.bxl:paths"]).await?;
            let lines: Vec<&str> = result.stdout.lines().collect();
            let [root_json, output_path] = lines.as_slice() else {
                anyhow::bail!(
                    "Expected the root and the output path, got:\n{}",
                    result.stdout
                );
            };
            let paths: Value = serde_json::from_str(root_json)?;
            assert_eq!(
                paths.get("root").and_then(Value::as_str),
                Some(path_str(tenant.cwd()))
            );
            let output = PathBuf::from(output_path);
            assert!(output.starts_with(tenant.cwd().join("buck-out").join(isolation)));
            assert_eq!(std::fs::read_to_string(&output)?, path_str(tenant.cwd()));
            assert_eq!(result.stderr.contains("evaluating tenant paths"), first_run);
            if first_run {
                outputs.insert(output);
            }
        }
    }
    assert_eq!(outputs.len(), tenants.len());
    Ok(())
}

async fn assert_tenant_dice_graphs(
    tenant_one: &Buck,
    tenant_two: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let probe_one = "root//:tenant_one_probe";
    let probe_two = "root//:tenant_two_probe";
    tenant_one.build(&["//:tenant_one_probe"]).await?;
    let baseline = dice_analysis_keys(tenant_one, &tmp_path.join("before")).await?;
    assert!(baseline.contains_key(probe_one));
    assert!(!baseline.contains_key(probe_two));

    tenant_two.build(&["//:tenant_two_probe"]).await?;
    let event_log = tenant_two.log(&["show", "--recent", "0"]).await?;
    assert!(
        event_log
            .stdout
            .contains(&format!("dice-sharing:{share_dice}"))
    );
    let keys_one = dice_analysis_keys(tenant_one, &tmp_path.join("after-one")).await?;
    let keys_two = dice_analysis_keys(tenant_two, &tmp_path.join("after-two")).await?;
    assert!(keys_one.contains_key(probe_one));
    assert!(keys_two.contains_key(probe_two));
    if share_dice {
        assert!(keys_one.contains_key(probe_one));
        assert!(keys_one.contains_key(probe_two));
        assert!(keys_two.contains_key(probe_one));
        assert!(keys_two.contains_key(probe_two));
    } else {
        assert!(!keys_one.contains_key(probe_two));
        assert!(!keys_two.contains_key(probe_one));
    }
    Ok(())
}

#[buck_test(env = [("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")])]
async fn test_inactivity_timeout(buck: &Buck) -> anyhow::Result<()> {
    //######################################################
    // Recommend running this test in opt mode
    // Otherwise the command that is run here
    // could take longer than 1 second to finish
    // causing this test to be flaky
    //######################################################

    // this will start the daemon
    let pid = server_status_pid(buck).await?;
    let daemon_dir = buck.daemon_dir().await?;

    tokio::time::sleep(Duration::from_secs(1)).await; // 1 sec timeout

    // check it's dead
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if !daemon_is_alive(pid) {
            let result = buck.status(&[]).await?;
            assert_eq!(result.stderr.lines().last(), Some("no buckd running"));

            let stderr = std::fs::read_to_string(daemon_dir.join("buckd.stderr"))?;
            assert!(stderr.contains("inactivity timeout elapsed"));
            return Ok(());
        }
    }

    panic!("Server with pid {pid} did not die in 20 seconds");
}

#[buck_test]
async fn test_server_endpoint_output(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.server(&[]).await?;
    let stdout = result.stdout.trim();
    assert!(stdout.starts_with("buckd.endpoint="));
    assert!(
        !stdout
            .strip_prefix("buckd.endpoint=")
            .unwrap_or(stdout)
            .is_empty()
    );
    Ok(())
}

#[buck_test]
async fn test_server_status_output(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.server(&["--status"]).await?;
    let status: Value = serde_json::from_str(&result.stdout)?;
    let pid = status
        .get("process_info")
        .and_then(|info| info.get("pid"))
        .context("No `process_info.pid`")?;
    assert!(pid.is_i64() || pid.is_u64(), "{pid}");
    assert!(pid.as_u64().is_some_and(|pid| pid > 0), "{pid}");
    Ok(())
}

#[buck_test]
async fn test_server_status_snapshot_output(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.server(&["--status", "--snapshot"]).await?;
    let status: Value = serde_json::from_str(&result.stdout)?;
    let snapshot = status.get("snapshot").context("No `snapshot`")?;
    assert!(!snapshot.is_null());
    assert!(
        snapshot
            .as_object()
            .is_some_and(|snapshot| snapshot.contains_key("buck2_max_rss")),
        "{snapshot}"
    );
    Ok(())
}

#[buck_test]
async fn test_server_snapshot_requires_status(buck: &Buck) {
    buck.server(&["--snapshot"])
        .await
        .expect_failure()
        .assert_stderr_matches("--status");
}

async fn corrupted_buckd_info(buck: &Buck, corrupt: &str) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;

    let daemon_dir = buck.daemon_dir().await?;
    // Check file exists and valid.
    serde_json::from_str::<Value>(&std::fs::read_to_string(daemon_dir.join("buckd.info"))?)?;

    // Kill that daemon now to avoid having making a mess and leaving 2 daemons
    // around.
    buck.kill().await?;

    std::fs::write(daemon_dir.join("buckd.info"), corrupt)?;

    buck.targets(&["//:rule"]).await?;
    Ok(())
}

cases! { [buck] corrupted_buckd_info {
    #[buck_test]
    fn test_corrupted_buckd_info_not_json("not-json");
    #[buck_test]
    fn test_corrupted_buckd_info_valid_json_but_not_valid_data(r#"{"valid-json", "but-not-valid-data"}"#);
}}

#[buck_test]
async fn test_recovers_when_daemon_pid_cannot_be_killed(buck: &Buck) -> anyhow::Result<()> {
    // A stale buckd.info can name a pid we cannot kill (e.g. one reused by a
    // process owned by another user). Buck must report the failed kill and start
    // a fresh daemon rather than aborting, which used to leave buckd.info in
    // place so every later invocation failed the same way.
    buck.targets(&["//:rule"]).await?;

    let daemon_dir = buck.daemon_dir().await?;
    let mut info: Value =
        serde_json::from_str(&std::fs::read_to_string(daemon_dir.join("buckd.info"))?)?;

    // Kill the daemon so its endpoint stops accepting connections, forcing the
    // next invocation down the "could not connect, killing daemon" path.
    buck.kill().await?;

    // Point buckd.info at a pid that hard_kill_until cannot kill. An out-of-range
    // pid fails the kill deterministically on every platform, standing in for the
    // reused/foreign pid from the original bug report.
    info["pid"] = json!(9999999999999u64);
    std::fs::write(daemon_dir.join("buckd.info"), serde_json::to_string(&info)?)?;

    // Recovers by starting a new daemon; this used to fail to connect entirely.
    let result = buck.targets(&["//:rule"]).await?;
    assert!(result.stderr.contains("Failed to kill buckd"));
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_unkillable_daemon_that_owns_outputs_is_named(buck: &Buck) -> anyhow::Result<()> {
    // When the daemon that could not be killed still owns buck-out, no new daemon may
    // start; the error must say which process it is and how to get going again.
    buck.targets(&["//:rule"]).await?;
    let daemon_dir = buck.daemon_dir().await?;
    let mut info: Value =
        serde_json::from_str(&std::fs::read_to_string(daemon_dir.join("buckd.info"))?)?;
    buck.kill().await?;

    info["pid"] = json!(9999999999999u64);
    std::fs::write(daemon_dir.join("buckd.info"), serde_json::to_string(&info)?)?;
    let locks_dir = daemon_dir
        .parent()
        .context("the daemon directory has a parent")?
        .join("._buck2_tenant_locks");
    std::fs::create_dir_all(&locks_dir)?;
    let lock_path = locks_dir.join(
        daemon_dir
            .file_name()
            .context("the daemon directory has a name")?,
    );
    // Stands in for the unkillable daemon, which would still hold this lock.
    let lock = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&lock_path)?;
    lock.lock()?;
    buck.targets(&["//:rule"])
        .env("BUCKD_STARTUP_TIMEOUT", "3")
        .env("BUCKD_STARTUP_INIT_TIMEOUT", "3")
        .await
        .expect_failure()
        .assert_stderr_matches("recorded as pid 9999999999999, could not be stopped");
    drop(lock);
    buck.targets(&["//:rule"]).await?;
    Ok(())
}

fn ps(field: &str, pid: u32) -> anyhow::Result<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", field, &pid.to_string()])
        .output()?;
    anyhow::ensure!(output.status.success(), "`ps` failed: {output:?}");
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[buck_test]
async fn test_process_title(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[]).await?; // Start the daemon
    let pid = daemon_pid(buck).await?;

    if cfg!(target_os = "macos") {
        let out = ps("comm=", pid)?;
        assert!(out.starts_with("buck2d["), "{out}");
    } else if cfg!(target_os = "linux") {
        let out = ps("cmd=", pid)?;
        assert!(out.starts_with("buck2d["), "{out}");
    } else if cfg!(windows) {
        // We guarantee no value there.
    } else {
        panic!("Unknown platform");
    }
    Ok(())
}

#[buck_test]
async fn test_status_fields(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[]).await?; // Start the daemon
    let status = status_json(buck).await?;
    assert!(truthy(
        status
            .get("valid_working_directory")
            .context("No `valid_working_directory`")?
    ));
    assert!(truthy(
        status
            .get("valid_buck_out_mount")
            .context("No `valid_buck_out_mount`")?
    ));
    Ok(())
}

fn active_commands(status: &Value) -> anyhow::Result<&Vec<Value>> {
    status
        .get("active_commands")
        .and_then(Value::as_array)
        .with_context(|| format!("No `active_commands` in {status}"))
}

#[buck_test]
async fn test_status_active_commands(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[]).await?; // Start the daemon

    let status = status_json(buck).await?;
    // `status` is a oneshot request, not a registered command, so an idle
    // daemon reports no active commands.
    assert!(active_commands(&status)?.is_empty(), "{status}");

    let mut build = Some(
        buck.build(&[":long_running", "--local-only", "--no-remote-cache"])
            .spawn(),
    );
    let result = async {
        let mut found = None;
        // Generous budget: under heavy CI load, daemon warm-up plus the local
        // action launch can take a while to register as an active command.
        for _ in 0..600 {
            let status = status_json(buck).await?;
            if !active_commands(&status)?.is_empty() {
                found = Some(status);
                break;
            }
            if build
                .as_mut()
                .expect("the build is running")
                .try_wait()
                .is_some()
            {
                // Surface the real failure rather than timing out below.
                build.take().expect("the build is running").wait().await?;
                panic!("Build finished without appearing in active_commands");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let status = found.expect("Build never showed up in active_commands");

        let [command] = active_commands(&status)?.as_slice() else {
            panic!("Expected one active command: {status}");
        };
        assert!(
            command.get("trace_id").is_some_and(truthy),
            "Active command should report its trace id"
        );
        let argv = command
            .get("argv")
            .and_then(Value::as_array)
            .context("No `argv`")?;
        assert!(
            argv.iter()
                .any(|arg| arg.as_str().is_some_and(|arg| arg.contains("long_running"))),
            "Build argv should mention the target: {argv:?}"
        );
        assert!(!command.get("stats").context("No `stats`")?.is_null());
        anyhow::Ok(())
    }
    .await;
    // Consume the build's outcome so an underlying error is not silently
    // dropped at teardown.
    if let Some(build) = build.take() {
        let _ignored = build.kill().await;
    }
    result
}

#[buck_test]
async fn test_status_all(buck: &Buck) -> anyhow::Result<()> {
    // this will start the daemons
    buck.server(&[]).await?;

    let pid = daemon_pid(buck).await?;

    let status_all: Vec<Value> = serde_json::from_str(&buck.status(&["--all"]).await?.stdout)?;
    for status in &status_all {
        if pid_of(status)? == pid {
            return Ok(());
        }
    }
    anyhow::bail!(
        "buckd status for pid {pid} not found in {}",
        serde_json::to_string_pretty(&status_all)?
    )
}

#[buck_test(env = [("BUCK_LOG", "buck2_client_ctx::daemon::client::kill=debug")])]
async fn test_no_buckd_kills_existing_daemon(buck: &Buck) -> anyhow::Result<()> {
    buck.audit(&["cell"]).await?; // Start the daemon
    let result = buck.audit(&["cell", "--no-buckd"]).await?; // Kill the existing daemon
    assert!(result.stderr.contains("Killing daemon with PID"));
    Ok(())
}

#[buck_test(env = [("BUCK2_TEST_INIT_DATA_SLEEP_SECS", "2"), ("BUCKD_STARTUP_TIMEOUT", "1")])]
async fn test_no_buckd_waits_for_daemon_init(buck: &Buck) -> anyhow::Result<()> {
    // Initialization takes longer than connecting to an existing daemon may.
    buck.targets(&[":", "--no-buckd"]).await?;
    Ok(())
}

#[buck_test]
async fn test_no_buckd_is_a_noop_for_a_client_only_command(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?; // Start a resident daemon
    let pid = daemon_pid(buck).await?;
    // `log` never connects, so it must neither kill the resident daemon nor start a
    // daemon of its own.
    buck.log(&["last", "--no-buckd"]).await?;
    assert!(daemon_is_alive(pid));
    Ok(())
}

#[buck_test]
async fn test_no_buckd_without_daemon_is_quiet(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["cell", "--no-buckd"]).await?;
    assert!(!result.stderr.contains("no buckd server running"));
    Ok(())
}

#[buck_test]
async fn test_no_buckd_run_starts_in_the_invocation_dir(buck: &Buck) -> anyhow::Result<()> {
    // The in-process daemon moves the client process into buck-out; the exec'd run
    // target must still start where the user invoked `buck run`.
    let result = buck.run(&[":pwd_run", "--no-buckd"]).await?;
    assert_eq!(Path::new(result.stdout.trim()), buck.cwd());
    Ok(())
}

#[buck_test]
async fn test_no_buckd_runs_without_file_watcher(buck: &Buck) -> anyhow::Result<()> {
    // The test harness pins a file_watcher for every test; drop that line to get
    // the unconfigured default an ordinary --no-buckd invocation sees.
    let extra_config = PathBuf::from(
        buck.env_var("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG")
            .context("No `BUCK2_TEST_EXTRA_EXTERNAL_CONFIG` in the environment")?,
    );
    let text = std::fs::read_to_string(&extra_config)?;
    std::fs::write(
        &extra_config,
        text.split_inclusive('\n')
            .filter(|line| !line.contains("file_watcher"))
            .collect::<String>(),
    )?;
    // The in-process daemon serves exactly one command, so there is nothing to
    // invalidate between commands: it runs no file watcher and reads the tree as
    // this command's own snapshot.
    buck.targets(&[":", "--no-buckd"]).await?;
    let spans = filter_events(buck, &["Event", "data", "SpanEnd", "data", "FileWatcher"]).await?;
    assert_eq!(spans, Vec::<Value>::new());
    Ok(())
}

#[buck_test(extra_buck_config = [("buck2", "file_watcher", "notify")])]
async fn test_no_buckd_explicit_file_watcher_wins(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":", "--no-buckd"]).await?;
    let spans = filter_events(buck, &["Event", "data", "SpanEnd", "data", "FileWatcher"]).await?;
    assert!(
        !spans.is_empty(),
        "an explicitly configured watcher must still run"
    );
    Ok(())
}

#[buck_test]
async fn test_no_buckd_startup_is_quiet(buck: &Buck) -> anyhow::Result<()> {
    // The in-process daemon starts up on the user's terminal, not in a buckd.stderr
    // that a tailing client would need to catch up on.
    let result = buck.targets(&[":", "--no-buckd"]).await?;
    assert!(
        !result.stderr.contains("daemon_listener"),
        "{}",
        result.stderr
    );
    Ok(())
}

#[buck_test]
async fn test_no_buckd_leaves_no_daemon_record(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":", "--no-buckd"]).await?;
    let daemon_dir = buck.daemon_dir().await?;
    assert!(!daemon_dir.join("buckd.info").exists());
    // The next command starts a fresh daemon instead of chasing the dead record.
    let result = buck.targets(&[":"]).await?;
    assert!(
        !result.stderr.contains("Could not connect"),
        "{}",
        result.stderr
    );
    Ok(())
}

// There is no Ctrl-C for a single process on Windows.
#[cfg(unix)]
#[buck_test(skip_for_os = ["windows"])]
async fn test_no_buckd_interrupt_leaves_no_daemon_record(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let event_log = tmp_path.join("build.json-lines");
    let mut build = buck
        .build(&[
            ":long_running",
            "--no-buckd",
            "--local-only",
            "--no-remote-cache",
            "--event-log",
            event_log.to_str().context("event log path is not UTF-8")?,
        ])
        .spawn();
    // Interrupt in the middle of the command, once its action runs.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if build.try_wait().is_some() {
            let output = build.wait().await.unwrap_or_else(BuckError::into_output);
            panic!(
                "the build died before its action ran (exit_code={:?}): {}",
                output.exit_code(),
                output.stderr
            );
        }
        if std::fs::read_to_string(&event_log).is_ok_and(|log| log.contains("\"ActionExecution\""))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the build never started its action"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let daemon_dir = buck.daemon_dir().await?;
    assert!(daemon_dir.join("buckd.info").exists());
    let _interrupted = build.interrupt().await;
    assert!(!daemon_dir.join("buckd.info").exists());
    let result = buck.targets(&[":"]).await?;
    assert!(
        !result.stderr.contains("Could not connect"),
        "{}",
        result.stderr
    );
    Ok(())
}

// On Windows `run` waits for the target as a child instead of replacing the client with it,
// so the in-process daemon still owns buck-out while the target runs.
#[buck_test(skip_for_os = ["windows"])]
async fn test_no_buckd_run_nested_buck_is_its_own_invocation(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .run(&[":nested_buck", "--no-buckd"])
        .env("NESTED_BUCK2", buck.executable().as_os_str())
        .env(
            "NESTED_ISOLATION_DIR",
            buck.isolation_prefix().unwrap_or("v2"),
        )
        .await?;
    // `run` execs the target onto the client's pid after the daemon record is
    // gone, so the nested invocation starts its own daemon instead of chasing a
    // record that by now names the target itself, and killing it.
    assert!(result.stdout.contains("root//:rule"), "{}", result.stdout);
    assert!(
        !result.stderr.contains("killing daemon"),
        "{}",
        result.stderr
    );
    let daemon_dir = buck.daemon_dir().await?;
    assert!(!daemon_dir.join("buckd.info").exists());
    Ok(())
}

#[buck_test]
async fn test_buck_out_is_cache_dir(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?; // Start a daemon
    let root = buck.root(&[]).await?;
    assert!(
        std::fs::read_to_string(
            Path::new(root.stdout.trim())
                .join("buck-out")
                .join("v2")
                .join("CACHEDIR.TAG")
        )?
        .starts_with("Signature: 8a477f597d28d172789f06886806bc55")
    );
    Ok(())
}

#[buck_test]
async fn test_prev_daemon_dir(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_daemon_routing(false)?;
    buck.targets(&[":"]).await?; // Start a daemon
    buck.kill().await?;
    buck.targets(&[":"]).await?; // Start another daemon

    let new_daemon_stderr = buck.daemon_stderr().await?;
    let killed_daemon_stderr = buck.prev_daemon_stderr().await?;

    // check logs contain buckd pid and don't match
    assert_ne!(
        extract_pid(&new_daemon_stderr),
        extract_pid(&killed_daemon_stderr)
    );

    assert!(killed_daemon_stderr.contains("triggered shutdown: `buck kill` was invoked"));
    Ok(())
}

#[buck_test]
async fn test_unreachable_recorded_daemon_is_replaced(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?; // Start a daemon
    let daemon_dir = buck.daemon_dir().await?;
    let mut info: Value =
        serde_json::from_str(&std::fs::read_to_string(daemon_dir.join("buckd.info"))?)?;
    info["endpoint"] = json!("tcp:1");
    std::fs::write(daemon_dir.join("buckd.info"), serde_json::to_string(&info)?)?;

    // `kill` cannot reach the endpoint; the next command replaces the daemon.
    buck.kill().await?;
    buck.targets(&[":"]).await?; // Start another daemon

    let new_daemon_stderr = buck.daemon_stderr().await?;
    let killed_daemon_stderr = buck.prev_daemon_stderr().await?;

    // check logs contain buckd pid and don't match
    assert_ne!(
        extract_pid(&new_daemon_stderr),
        extract_pid(&killed_daemon_stderr)
    );
    Ok(())
}

#[buck_test(env = [("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true"), ("BUCKD_STARTUP_INIT_TIMEOUT", "20")])]
async fn test_recovers_promptly_after_inactivity_shutdown(buck: &Buck) -> anyhow::Result<()> {
    // A daemon that retires on its inactivity timeout leaves buckd.info behind
    // naming a pid that is gone. The next invocation must notice that quickly and
    // start a fresh daemon; it used to be suspected of spending the whole startup
    // budget here, which would turn an idle daemon into a 90s CLIENT_STARTUP_TIMEOUT.
    let pid = server_status_pid(buck).await?;
    let daemon_dir = buck.daemon_dir().await?;

    let mut died = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if !daemon_is_alive(pid) {
            died = true;
            break;
        }
    }
    assert!(died, "Server with pid {pid} did not die in 20 seconds");

    assert!(
        std::fs::read_to_string(daemon_dir.join("buckd.stderr"))?
            .contains("inactivity timeout elapsed")
    );
    assert!(
        daemon_dir.join("buckd.info").exists(),
        "stale buckd.info is the point"
    );

    let start = Instant::now();
    let result = buck.targets(&["//:rule"]).await?;
    let elapsed = start.elapsed().as_secs_f64();

    assert!(
        result.stderr.contains("buck2 daemon is not running"),
        "{}",
        result.stderr
    );
    // Well inside the 20s budget. A regression into the timeout path fails the
    // command outright, so this only guards against getting slow but succeeding.
    assert!(elapsed < 15.0, "took {elapsed:.2}s to recover");
    Ok(())
}

#[buck_test(env = [("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")])]
async fn test_inactivity_shutdown_exits_with_a_command_in_flight(
    buck: &Buck,
) -> anyhow::Result<()> {
    // The inactivity timer is only reset when a command *starts*, so a long lived
    // streaming command lets the timeout fire underneath itself. The daemon then
    // cannot finish draining while the client holds the stream open, and it used
    // to sit alive forever - still accepting connections it would never answer,
    // so every later invocation on this isolation dir burned its startup budget.
    let pid = server_status_pid(buck).await?;
    let daemon_dir = buck.daemon_dir().await?;
    let daemon_stderr = || std::fs::read_to_string(daemon_dir.join("buckd.stderr"));

    let subscriber = buck.subscribe(&[]);
    let result = async {
        let mut timed_out = false;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if daemon_stderr()?.contains("inactivity timeout elapsed") {
                timed_out = true;
                break;
            }
        }
        assert!(timed_out, "daemon never reached its inactivity timeout");

        let mut exited = false;
        for _ in 0..15 {
            if !daemon_is_alive(pid) {
                exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert!(
            exited,
            "daemon {pid} announced shutdown but is still alive with a command in flight"
        );

        assert!(daemon_stderr()?.contains("Shutdown deadline exceeded"));
        anyhow::Ok(())
    }
    .await;
    // Exiting tears the stream down under the subscriber, so it reports a
    // broken connection. That is the deliberate trade: the command is
    // terminated rather than the daemon being left alive and unusable.
    let _ignored = subscriber.close().await;
    result
}

/// A new daemon waits within the startup deadline for whatever still owns its tenant's
/// outputs, as it waits for a running `clean`, rather than failing after a fixed time.
#[buck_test(skip_for_os = ["windows"])]
async fn test_daemon_start_waits_for_tenant_outputs(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?;
    let daemon_dir = buck.daemon_dir().await?;
    buck.kill().await?;
    let lock_path = daemon_dir
        .parent()
        .context("The daemon dir has no parent")?
        .join("._buck2_tenant_locks")
        .join(
            daemon_dir
                .file_name()
                .context("The daemon dir has no name")?,
        );
    std::fs::create_dir_all(lock_path.parent().unwrap())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lock_path)?;
    lock.lock()?;
    let mut starting = buck.targets(&[":"]).spawn();
    tokio::time::sleep(Duration::from_secs(12)).await;
    assert!(
        starting.try_wait().is_none(),
        "Daemon startup gave up while the outputs were owned"
    );
    drop(lock);
    tokio::time::timeout(Duration::from_secs(60), starting.wait())
        .await
        .context("Daemon startup did not finish within 60s")??;
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_unshared_forkserver_state_is_reclaimed(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_daemon_routing(false)?;
    let state_root = buck.cwd().join("buck-out").join("v2").join("forkserver");
    for _ in 0..3 {
        buck.targets(&["//:rule"]).await?;
        let mut states = Vec::new();
        for entry in std::fs::read_dir(&state_root)? {
            let path = entry?.path();
            if path.is_dir() {
                states.push(path);
            }
        }
        assert_eq!(states.len(), 1, "{states:?}");
        let state = &states[0];
        assert!(state.is_dir());
        buck.kill().await?;
        for _ in 0..100 {
            if !state.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!state.exists());
    }
    buck.clean(&[]).await?;
    assert!(!state_root.exists());
    Ok(())
}

#[buck_test(skip_for_os = ["darwin"])]
async fn test_unshared_daemon_preserves_shared_daemon(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_daemon_routing(true)?;
    let buck = &*buck;
    let shared_pid = server_status_pid(buck).await?;
    let mut isolated = isolated_buck(buck, "unshared");
    isolated.set_daemon_routing(false)?;
    let subscriber = buck.subscribe(&[]);
    let result = async {
        assert_ne!(server_status_pid(&isolated).await?, shared_pid);
        isolated.targets(&["//:rule"]).await?;
        isolated.kill().await?;
        assert!(daemon_is_alive(shared_pid));
        assert_eq!(daemon_pid(buck).await?, shared_pid);
        buck.targets(&["//:rule"]).await?;
        anyhow::Ok(())
    }
    .await;
    subscriber.close().await?;
    isolated.kill().await?;
    result
}

async fn evicted_generation_cannot_overlap_outputs(
    buck: &Buck,
    tmp_path: &Path,
    initial_tenant: bool,
    repository_eviction: bool,
    share_dice: bool,
) -> anyhow::Result<()> {
    append_to_file(
        &buck.cwd().join(".buckconfig"),
        &format!("\n[buck2]\nsqlite_dep_file_state = true\nshare_dice = {share_dice}\n"),
    )?;
    let sibling = isolated_buck(buck, "survivor");
    let recreated;
    let (tenant, isolation) = if initial_tenant {
        (buck, "v2")
    } else {
        recreated = isolated_buck(buck, "recreated");
        (&recreated, "recreated")
    };
    let instances = if initial_tenant {
        [tenant, &sibling]
    } else {
        [&sibling, tenant]
    };
    for instance in instances {
        instance.targets(&["//:rule"]).await?;
    }
    let pid = daemon_pid(&sibling).await?;
    let identity = materializer_identity(tenant, isolation)?;
    let generation = tenant_generation(&debug_tenants(&sibling).await?, isolation)?;
    let mut old = Some(start_generation(tenant, tmp_path, "old")?);
    let mut new = None;
    let mut killing = None;
    let result = async {
        wait_for_marker(&tmp_path.join("old.started"), &mut old).await?;
        let old_pid: u32 = std::fs::read_to_string(tmp_path.join("old.started.pid"))?
            .trim()
            .parse()?;
        assert!(daemon_is_alive(old_pid));
        killing = Some(if repository_eviction {
            tenant
                .command(&["killall", "--repo", "--in-isolation-dir", isolation])
                .spawn()
        } else {
            tenant.kill().spawn()
        });
        wait_for_evicted_generation(&sibling, isolation, generation).await?;
        new = Some(start_generation(tenant, tmp_path, "new")?);
        wait_within(killing.take().expect("eviction is running"), THIRTY_SECONDS).await??;
        assert!(
            wait_within(
                old.take().expect("the old build is running"),
                THIRTY_SECONDS
            )
            .await?
            .is_err(),
            "The old build was expected to fail"
        );
        assert!(
            !daemon_is_alive(old_pid),
            "Eviction returned while the old action was alive"
        );
        assert!(!tmp_path.join("old.release").exists());
        wait_for_marker(&tmp_path.join("new.started"), &mut new).await?;
        assert_eq!(
            std::fs::read_to_string(tmp_path.join("old.started"))?,
            std::fs::read_to_string(tmp_path.join("new.started"))?
        );
        touch(&tmp_path.join("new.release"))?;
        let result = wait_within(
            new.take().expect("the new build is running"),
            THIRTY_SECONDS,
        )
        .await??;
        assert_eq!(read_generation(&result)?, "new");
        assert_eq!(materializer_identity(tenant, isolation)?, identity);
        assert_eq!(daemon_pid(&sibling).await?, pid);
        anyhow::Ok(())
    }
    .await;
    for value in ["old", "new"] {
        touch(&tmp_path.join(format!("{value}.release")))?;
    }
    wait_for_all([old, new, killing].into_iter().flatten(), THIRTY_SECONDS).await?;
    tenant.kill().await?;
    sibling.kill().await?;
    result
}

bool_cases! { [buck, tmp_path] evicted_generation_cannot_overlap_outputs {
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_evicted_generation_cannot_overlap_outputs_False_False_False(false, false, false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_evicted_generation_cannot_overlap_outputs_False_False_True(true, false, false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_evicted_generation_cannot_overlap_outputs_False_True_False(false, true, false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_evicted_generation_cannot_overlap_outputs_False_True_True(true, true, false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_evicted_generation_cannot_overlap_outputs_True_False_False(false, false, true);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_evicted_generation_cannot_overlap_outputs_True_False_True(true, false, true);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_evicted_generation_cannot_overlap_outputs_True_True_False(false, true, true);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_evicted_generation_cannot_overlap_outputs_True_True_True(true, true, true);
}}

async fn eviction_during_generation_initialization(
    buck: &mut Buck,
    tmp_path: &Path,
    disconnect_client: bool,
    share_dice: bool,
) -> anyhow::Result<()> {
    let gate = tmp_path.join("initialization");
    buck.set_env(
        "BUCK2_TEST_BLOCK_TENANT_INITIALIZATION",
        format!("initializing,{}", gate.display()),
    );
    let buck = &*buck;
    append_to_file(
        &buck.cwd().join(".buckconfig"),
        &format!("\n[buck2]\nshare_dice = {share_dice}\n"),
    )?;
    buck.targets(&["//:rule"]).await?;
    let pid = daemon_pid(buck).await?;
    let tenant = isolated_buck(buck, "initializing");
    let mut old = Some(start_generation(&tenant, tmp_path, "old")?);
    let mut new = None;
    let mut client: Option<BuckProcess> = None;
    let result = async {
        wait_for_marker(&tmp_path.join("initialization.started"), &mut old).await?;
        let generation = tenant_generation(&debug_tenants(buck).await?, "initializing")?;
        client = Some(tenant.kill().spawn());
        wait_for_evicted_generation(buck, "initializing", generation).await?;
        new = Some(start_generation(&tenant, tmp_path, "new")?);
        assert!(!tmp_path.join("new.started").exists());
        assert!(
            client
                .as_mut()
                .expect("eviction is running")
                .try_wait()
                .is_none(),
            "Eviction returned before its I/O released ownership"
        );
        // The evicted generation's initialization I/O still owns the tenant's disk state, so
        // the pending generation has not taken the lock, let alone touched the outputs.
        assert_tenant_disk_locked(&tenant, "initializing")?;
        if disconnect_client {
            // Without the kill holding the tenant directory's lock, the replacement
            // registers its generation and waits in the daemon for the evicted one.
            let _ignored = client.take().expect("eviction is running").kill().await;
            let pending = wait_for_tenant_generation(buck, "initializing", generation).await?;
            assert!(!truthy(
                pending.get("initialized").context("No `initialized`")?
            ));
        }
        buck.targets(&["//:rule"]).await?;
        assert!(!tmp_path.join("new.started").exists());
        touch(&tmp_path.join("initialization.release"))?;
        if !disconnect_client {
            client.take().expect("eviction is running").wait().await?;
        }
        assert_evicted(
            wait_within(
                old.take().expect("the old build is running"),
                THIRTY_SECONDS,
            )
            .await?,
        );
        wait_for_marker(&tmp_path.join("new.started"), &mut new).await?;
        touch(&tmp_path.join("new.release"))?;
        let result = wait_within(
            new.take().expect("the new build is running"),
            THIRTY_SECONDS,
        )
        .await??;
        assert_eq!(read_generation(&result)?, "new");
        assert_eq!(daemon_pid(buck).await?, pid);
        // The evicted generation finished its initialization I/O after the release but never
        // ran the command that was waiting for it.
        assert!(!tmp_path.join("old.started").exists());
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("initialization.release"))?;
    for value in ["old", "new"] {
        touch(&tmp_path.join(format!("{value}.release")))?;
    }
    if let Some(client) = client.take() {
        let _ignored = client.kill().await;
    }
    wait_for_all([old, new].into_iter().flatten(), THIRTY_SECONDS).await?;
    tenant.kill().await?;
    buck.kill().await?;
    result
}

bool_cases! { [mut buck, tmp_path] eviction_during_generation_initialization {
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_eviction_during_generation_initialization_False_False(false, false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_eviction_during_generation_initialization_False_True(true, false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_eviction_during_generation_initialization_True_False(false, true);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_eviction_during_generation_initialization_True_True(true, true);
}}

async fn config_value(tenant: &Buck, config: &Path) -> anyhow::Result<String> {
    let result = tenant
        .audit_config(&[
            "tenant.value",
            "--style=json",
            "--config-file",
            path_str(config),
        ])
        .await?;
    Ok(result
        .audit_config_json()?
        .get("tenant.value")
        .and_then(Value::as_str)
        .context("No `tenant.value`")?
        .to_owned())
}

/// A client killed while its tenant initializes leaves the generation to a client that joined it.
/// Alone, it leaves the generation to the next client when the daemon did not notice its
/// departure, and the daemon evicts the generation once the initialization ends when it did, so
/// that the next client registers a fresh one. The daemon survives an initialization longer than
/// the proxy-loss budget either way.
async fn client_killed_during_tenant_initialization_leaves_the_tenant_usable(
    buck: &mut Buck,
    tmp_path: &Path,
    joined: bool,
) -> anyhow::Result<()> {
    let gate = tmp_path.join("initialization");
    buck.set_env(
        "BUCK2_TEST_BLOCK_TENANT_INITIALIZATION",
        format!("initializing,{}", gate.display()),
    );
    let buck = &*buck;
    buck.targets(&["//:rule"]).await?;
    let pid = daemon_pid(buck).await?;
    let tenant = isolated_buck(buck, "initializing");
    let mut first = Some(tenant.targets(&["//:rule"]).spawn());
    let mut joiner = None;
    let result = async {
        wait_for_marker(&tmp_path.join("initialization.started"), &mut first).await?;
        let generation = tenant_generation(&debug_tenants(buck).await?, "initializing")?;
        if joined {
            joiner = Some(tenant.targets(&["//:rule"]).spawn());
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let _killed = first
            .take()
            .expect("the first client is running")
            .kill()
            .await;
        // Longer than the proxy-loss eviction budget, which does not apply here.
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(daemon_is_alive(pid));
        touch(&tmp_path.join("initialization.release"))?;
        if let Some(joiner) = joiner.take() {
            wait_within(joiner, THIRTY_SECONDS).await??;
        }
        tenant.targets(&["//:rule"]).await?;
        let entries = debug_tenants(buck).await?;
        let entry = tenant_entry(&entries, "initializing")?;
        assert!(truthy(&entry["initialized"]));
        if joined {
            assert_eq!(generation_of(entry)?, generation);
        } else {
            assert!(generation_of(entry)? >= generation);
        }
        assert_eq!(daemon_pid(buck).await?, pid);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("initialization.release"))?;
    if let Some(first) = first.take() {
        let _ignored = first.kill().await;
    }
    if let Some(joiner) = joiner.take() {
        let _ignored = wait_within(joiner, THIRTY_SECONDS).await;
    }
    let killed = async {
        tenant.kill().await?;
        buck.kill().await?;
        anyhow::Ok(())
    }
    .await;
    result?;
    killed
}

bool_cases! { [mut buck, tmp_path] client_killed_during_tenant_initialization_leaves_the_tenant_usable {
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_client_killed_during_tenant_initialization_leaves_the_tenant_usable_False(false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_client_killed_during_tenant_initialization_leaves_the_tenant_usable_True(true);
}}

async fn isolated_tenants_share_daemon_without_sharing_config(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    append_to_file(
        &buck.cwd().join(".buckconfig"),
        &format!("\n[buck2]\nshare_dice = {share_dice}\n"),
    )?;
    let config_one = tmp_path.join("tenant-one.bcfg");
    std::fs::write(&config_one, "[tenant]\n  value = one\n")?;
    let config_two = tmp_path.join("tenant-two.bcfg");
    std::fs::write(&config_two, "[tenant]\n  value = two\n")?;

    let tenant_one = isolated_buck(buck, "tenant-one");
    let tenant_two = isolated_buck(buck, "tenant-two");

    assert_eq!(config_value(&tenant_one, &config_one).await?, "one");
    assert_eq!(config_value(&tenant_two, &config_two).await?, "two");

    let tenant_one_pid = daemon_pid(&tenant_one).await?;
    let tenant_two_pid = daemon_pid(&tenant_two).await?;
    assert_eq!(tenant_one_pid, tenant_two_pid);

    let tenants = debug_tenants(&tenant_one).await?;
    let tenant_one_generation = tenant_generation(&tenants, "tenant-one")?;
    assert!(isolations(&tenants)?.contains(&"tenant-two"));

    tenant_one.kill().await?;
    assert_eq!(config_value(&tenant_two, &config_two).await?, "two");
    assert_eq!(daemon_pid(&tenant_two).await?, tenant_two_pid);

    assert_eq!(config_value(&tenant_one, &config_one).await?, "one");
    let recreated_tenants = debug_tenants(&tenant_one).await?;
    assert!(tenant_generation(&recreated_tenants, "tenant-one")? > tenant_one_generation);
    Ok(())
}

bool_cases! { [buck, tmp_path] isolated_tenants_share_daemon_without_sharing_config {
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_isolated_tenants_share_daemon_without_sharing_config_False(false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_isolated_tenants_share_daemon_without_sharing_config_True(true);
}}

/// The tenant telemetry of one build: the invocation record's string metadata.
async fn tenant_telemetry(tenant: &Buck) -> anyhow::Result<BTreeMap<String, String>> {
    let record = tenant
        .build(&["//:tenant_value"])
        .await?
        .invocation_record()?;
    record["metadata"]["strings"]
        .as_object()
        .context("No `metadata.strings` in the invocation record")?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                value
                    .as_str()
                    .with_context(|| format!("`{key}` is not a string"))?
                    .to_owned(),
            ))
        })
        .collect::<anyhow::Result<_>>()
}

#[buck_test(write_invocation_record = true)]
async fn test_unshared_daemon_reports_single_tenant_telemetry(buck: &Buck) -> anyhow::Result<()> {
    let telemetry = tenant_telemetry(buck).await?;
    assert_eq!(telemetry["daemon_routing"], "unshared");
    assert_eq!(telemetry["tenant_count"], "1");
    assert_eq!(telemetry["tenant_generation"], "1");
    assert_eq!(telemetry["tenant_started_reason"], "initial");
    assert!(!telemetry["tenant_id"].is_empty());
    assert!(!telemetry["tenant_instance_id"].is_empty());
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_os = ["windows", "darwin"], share_daemon = true)]
async fn test_isolated_tenants_report_tenant_telemetry(buck: &Buck) -> anyhow::Result<()> {
    let tenant_one = isolated_buck(buck, "tenant-one");
    let tenant_two = isolated_buck(buck, "tenant-two");
    let result = async {
        let first = tenant_telemetry(&tenant_one).await?;
        let second = tenant_telemetry(&tenant_two).await?;
        let pid = daemon_pid(&tenant_one).await?;

        assert_eq!(first["daemon_uuid"], second["daemon_uuid"]);
        assert_eq!(first["daemon_routing"], "shared");
        assert_ne!(first["tenant_id"], second["tenant_id"]);
        assert_ne!(first["tenant_instance_id"], second["tenant_instance_id"]);
        assert_eq!(first["tenant_generation"], "1");
        assert_eq!(second["tenant_generation"], "2");
        assert_eq!(first["tenant_count"], "1");
        assert_eq!(second["tenant_count"], "2");
        assert_eq!(first["tenant_started_reason"], "initial");
        assert_eq!(second["tenant_started_reason"], "joined");

        let repeat = tenant_telemetry(&tenant_one).await?;
        assert_eq!(repeat["tenant_instance_id"], first["tenant_instance_id"]);
        assert!(!repeat.contains_key("tenant_started_reason"));

        tenant_one.kill().await?;
        let recreated = tenant_telemetry(&tenant_one).await?;
        assert_eq!(recreated["daemon_uuid"], first["daemon_uuid"]);
        assert_eq!(recreated["tenant_id"], first["tenant_id"]);
        assert_ne!(recreated["tenant_instance_id"], first["tenant_instance_id"]);
        assert_eq!(recreated["tenant_generation"], "3");
        assert_eq!(recreated["tenant_count"], "2");
        assert_eq!(recreated["tenant_started_reason"], "recreated:kill");
        anyhow::Ok(pid)
    }
    .await;
    let killed = async {
        tenant_one.kill().await?;
        tenant_two.kill().await?;
        anyhow::Ok(())
    }
    .await;
    let pid = result?;
    killed?;
    assert_daemon_exits(pid).await;
    Ok(())
}

async fn killall_repo_reaches_all_isolation_directories(
    buck: &mut Buck,
    sharing: &str,
) -> anyhow::Result<()> {
    buck.set_daemon_routing(sharing == "true")?;
    let buck = &*buck;
    let tenants = ["tenant-one", "tenant-two"].map(|isolation| isolated_buck(buck, isolation));
    let result = async {
        for tenant in &tenants {
            tenant.targets(&["//:rule"]).await?;
        }
        buck.command(&["killall", "--repo", "--in-isolation-dir", "tenant-one"])
            .await?;
        if sharing == "false" {
            assert!(
                tenants[0]
                    .status(&[])
                    .await?
                    .stderr
                    .contains("no buckd running")
            );
        } else {
            let registered = debug_tenants(&tenants[1]).await?;
            assert!(!isolations(&registered)?.contains(&"tenant-one"));
        }
        tenants[1].targets(&["//:rule"]).await?;
        buck.command(&["killall", "--repo"])
            .env("BUCK2_SHARE_DAEMON", sharing)
            .await?;
        for tenant in &tenants {
            assert!(
                tenant
                    .status(&[])
                    .await?
                    .stderr
                    .contains("no buckd running")
            );
        }
        buck.command(&["killall", "--repo"])
            .env("BUCK2_SHARE_DAEMON", sharing)
            .await?;
        anyhow::Ok(())
    }
    .await;
    for tenant in &tenants {
        tenant.kill().await?;
    }
    result
}

cases! { [mut buck] killall_repo_reaches_all_isolation_directories {
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_killall_repo_reaches_all_isolation_directories_false("false");
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_killall_repo_reaches_all_isolation_directories_true("true");
}}

async fn clean_preserves_sibling_tenant(
    buck: &mut Buck,
    tmp_path: &Path,
    sharing: &str,
    background: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    buck.set_daemon_routing(sharing == "true")?;
    let buck = &*buck;
    let sibling = isolated_buck(buck, "survivor");
    touch(&tmp_path.join("value.release"))?;
    let result = start_generation(buck, tmp_path, "value")?.wait().await?;
    let output = generation_output(&result)?;
    let outcome = async {
        let result = start_generation(&sibling, tmp_path, "value")?
            .wait()
            .await?;
        let sibling_output = generation_output(&result)?;
        let pid = daemon_pid(&sibling).await?;
        let mut args = Vec::new();
        if background {
            args.push("--background");
        }
        if dry_run {
            args.push("--dry-run");
        }
        buck.clean(&args).await?;
        assert_eq!(output.exists(), dry_run);
        assert_eq!(std::fs::read_to_string(&sibling_output)?, "value");
        assert_eq!(daemon_pid(&sibling).await?, pid);
        sibling
            .build(&[
                "//:generation",
                "--config-file",
                path_str(&tmp_path.join("value.bcfg")),
                "--no-remote-cache",
            ])
            .await?;
        assert_eq!(std::fs::read_to_string(&sibling_output)?, "value");
        touch(&tmp_path.join("after.release"))?;
        let after = start_generation(&sibling, tmp_path, "after")?
            .wait()
            .await?;
        assert_eq!(read_generation(&after)?, "after");
        let result = start_generation(buck, tmp_path, "value")?.wait().await?;
        assert_eq!(read_generation(&result)?, "value");
        anyhow::Ok(())
    }
    .await;
    sibling.kill().await?;
    outcome
}

bool_cases! { [mut buck, tmp_path] clean_preserves_sibling_tenant {
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_clean_preserves_sibling_tenant_False_False_false("false", false, false);
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_clean_preserves_sibling_tenant_False_False_true("true", false, false);
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_clean_preserves_sibling_tenant_False_True_false("false", true, false);
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_clean_preserves_sibling_tenant_False_True_true("true", true, false);
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_clean_preserves_sibling_tenant_True_False_false("false", false, true);
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_clean_preserves_sibling_tenant_True_False_true("true", false, true);
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_clean_preserves_sibling_tenant_True_True_false("false", true, true);
    #[buck_test(skip_for_os = ["darwin"])]
    fn test_clean_preserves_sibling_tenant_True_True_true("true", true, true);
}}

async fn clean_waits_for_tenant_disk_work(
    buck: &mut Buck,
    tmp_path: &Path,
    background: bool,
) -> anyhow::Result<()> {
    let gate = tmp_path.join("initialization");
    let isolation = "v2";
    buck.set_env(
        "BUCK2_TEST_BLOCK_TENANT_INITIALIZATION",
        format!("{isolation},{}", gate.display()),
    );
    let buck = &*buck;
    let sibling = isolated_buck(buck, "survivor");
    sibling.targets(&["//:rule"]).await?;
    let pid = daemon_pid(&sibling).await?;
    let mut old = Some(start_generation(buck, tmp_path, "old")?);
    let mut cleaning: Option<BuckProcess> = None;
    let result = async {
        wait_for_marker(&tmp_path.join("initialization.started"), &mut old).await?;
        let generation = tenant_generation(&debug_tenants(&sibling).await?, isolation)?;
        let sentinel = buck
            .cwd()
            .join("buck-out")
            .join(isolation)
            .join("preserved");
        std::fs::write(&sentinel, "old")?;
        let args: &[&str] = if background { &["--background"] } else { &[] };
        cleaning = Some(buck.clean(args).spawn());
        wait_for_evicted_generation(&sibling, isolation, generation).await?;
        assert!(
            cleaning
                .as_mut()
                .expect("clean is running")
                .try_wait()
                .is_none()
        );
        assert_tenant_disk_locked(buck, isolation)?;
        assert!(
            sentinel.exists(),
            "Clean deleted outputs still owned by cancelled I/O"
        );
        touch(&tmp_path.join("initialization.release"))?;
        assert_evicted(
            wait_within(
                old.take().expect("the old build is running"),
                THIRTY_SECONDS,
            )
            .await?,
        );
        wait_within(cleaning.take().expect("clean is running"), THIRTY_SECONDS).await??;
        assert!(!sentinel.exists());
        assert_eq!(daemon_pid(&sibling).await?, pid);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("initialization.release"))?;
    touch(&tmp_path.join("old.release"))?;
    wait_for_all(old.into_iter().chain(cleaning), Duration::from_secs(35)).await?;
    sibling.kill().await?;
    result
}

bool_cases! { [mut buck, tmp_path] clean_waits_for_tenant_disk_work {
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_clean_waits_for_tenant_disk_work_False(false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_clean_waits_for_tenant_disk_work_True(true);
}}

#[buck_test]
async fn test_host_killall_isolation_filter_preserves_other_daemons(
    buck: &Buck,
) -> anyhow::Result<()> {
    let prefix = format!("tenant-filter-{}", uuid::Uuid::new_v4().simple());
    let mut tenants =
        ["selected", "survivor"].map(|name| isolated_buck(buck, &format!("{prefix}-{name}")));
    for tenant in &mut tenants {
        tenant.set_daemon_routing(false)?;
    }
    let result = async {
        for tenant in &tenants {
            tenant.targets(&["//:rule"]).await?;
        }
        let pid = daemon_pid(&tenants[1]).await?;
        tenants[0]
            .command(&[
                "killall",
                "--in-isolation-dir",
                &format!("{prefix}-selected"),
            ])
            .await?;
        assert!(
            tenants[0]
                .status(&[])
                .await?
                .stderr
                .contains("no buckd running")
        );
        tenants[1].targets(&["//:rule"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, pid);
        anyhow::Ok(())
    }
    .await;
    for tenant in &tenants {
        tenant.kill().await?;
    }
    result
}

#[buck_test(skip_for_os = ["windows"], share_daemon = true)]
async fn test_clean_cannot_unlink_sibling_disk_lock(buck: &mut Buck) -> anyhow::Result<()> {
    let sibling = isolated_buck(buck, "sibling");
    let result = async {
        sibling.targets(&["//:rule"]).await?;
        let lock_file = proxy_slot(&sibling)
            .parent()
            .context("The proxy slot has no parent")?
            .join("._buck2_tenant_locks")
            .join("sibling");
        assert!(lock_file.is_file());
        let inode = file_id(&lock_file)?;
        buck.set_daemon_routing(false)?;
        buck.set_isolation_prefix(".tenant-locks");
        buck.clean(&[]).await?;
        assert_eq!(file_id(&lock_file)?, inode);
        sibling.targets(&["//:rule"]).await?;
        anyhow::Ok(())
    }
    .await;
    sibling.kill().await?;
    result
}

async fn isolated_tenants_keep_cached_paths_local(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    append_to_file(
        &buck.cwd().join(".buckconfig"),
        &format!("\n[buck2]\nshare_dice = {share_dice}\n"),
    )?;
    let tenants = ["tenant-one", "tenant-two"].map(|name| (isolated_buck(buck, name), name));
    let result = async {
        let mut outputs = Vec::new();
        for (tenant, _) in &tenants {
            let result = tenant.build(&["//:tenant_value"]).await?;
            let output = result
                .build_report()?
                .output_for_target("//:tenant_value")?;
            assert_eq!(std::fs::read_to_string(&output)?, "unset");
            outputs.push(output);
        }
        assert_ne!(outputs[0], outputs[1]);
        let pids = [
            daemon_pid(&tenants[0].0).await?,
            daemon_pid(&tenants[1].0).await?,
        ];
        assert_eq!(pids[0], pids[1]);
        assert_tenant_cached_paths(&tenants.each_ref().map(|(tenant, name)| (tenant, *name)))
            .await?;
        assert_tenant_dice_graphs(&tenants[0].0, &tenants[1].0, tmp_path, share_dice).await?;
        anyhow::Ok(pids[0])
    }
    .await;
    for (tenant, _) in &tenants {
        tenant.kill().await?;
    }
    assert_daemon_exits(result?).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] isolated_tenants_keep_cached_paths_local {
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_isolated_tenants_keep_cached_paths_local_False(false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_isolated_tenants_keep_cached_paths_local_True(true);
}}

async fn isolated_tenants_select_dice_engine_ownership(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    append_to_file(
        &buck.cwd().join(".buckconfig"),
        &format!("\n[buck2]\nshare_dice = {share_dice}\n"),
    )?;
    let config_one = tmp_path.join("tenant-one.bcfg");
    std::fs::write(&config_one, "[tenant]\nvalue = one\n")?;
    let config_two = tmp_path.join("tenant-two.bcfg");
    std::fs::write(&config_two, "[tenant]\nvalue = two\n")?;
    let tenant_one = isolated_buck(buck, "tenant-one");
    let tenant_two = isolated_buck(buck, "tenant-two");
    let result = async {
        let output_one = tenant_value(&tenant_one, &config_one).await?;
        let output_two = tenant_value(&tenant_two, &config_two).await?;
        assert_ne!(output_one, output_two);
        assert_eq!(std::fs::read_to_string(&output_one)?, "one");
        assert_eq!(std::fs::read_to_string(&output_two)?, "two");
        let daemon_pid_one = daemon_pid(&tenant_one).await?;
        assert_eq!(daemon_pid_one, daemon_pid(&tenant_two).await?);
        assert_tenant_dice_graphs(&tenant_one, &tenant_two, tmp_path, share_dice).await?;
        assert_eq!(
            std::fs::read_to_string(tenant_value(&tenant_one, &config_one).await?)?,
            "one"
        );
        assert_eq!(
            std::fs::read_to_string(tenant_value(&tenant_two, &config_two).await?)?,
            "two"
        );
        anyhow::Ok(daemon_pid_one)
    }
    .await;
    tenant_one.kill().await?;
    tenant_two.kill().await?;
    assert_daemon_exits(result?).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] isolated_tenants_select_dice_engine_ownership {
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_isolated_tenants_select_dice_engine_ownership_False(false);
    #[buck_test(skip_for_os = ["windows", "darwin"], share_daemon = true)]
    fn test_isolated_tenants_select_dice_engine_ownership_True(true);
}}
