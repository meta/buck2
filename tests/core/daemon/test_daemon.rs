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
use buck2_e2e::process::wait_for_exit;
use buck2_e2e::utils::file_id;
use buck2_e2e::utils::filter_events;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

const THIRTY_SECONDS: Duration = Duration::from_secs(30);

/// A test for each case of a parametrized Python test, named as the Python harness named
/// the case, running the test's body with the test's fixture (`[buck]`, `[mut buck]`,
/// `[buck, tmp_path]` or `[mut buck, tmp_path]`) and the case's arguments.
macro_rules! cases {
    ([buck] $body:ident { $($(#[$attr:meta])* fn $name:ident($($arg:expr),*);)* }) => {
        $(
            $(#[$attr])*
            async fn $name(buck: &Buck) -> anyhow::Result<()> {
                $body(buck, $($arg),*).await
            }
        )*
    };
    ([mut buck] $body:ident { $($(#[$attr:meta])* fn $name:ident($($arg:expr),*);)* }) => {
        $(
            $(#[$attr])*
            async fn $name(buck: &mut Buck) -> anyhow::Result<()> {
                $body(buck, $($arg),*).await
            }
        )*
    };
    ([buck, tmp_path] $body:ident { $($(#[$attr:meta])* fn $name:ident($($arg:expr),*);)* }) => {
        $(
            $(#[$attr])*
            async fn $name(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
                $body(buck, tmp_path, $($arg),*).await
            }
        )*
    };
    ([mut buck, tmp_path] $body:ident { $($(#[$attr:meta])* fn $name:ident($($arg:expr),*);)* }) => {
        $(
            $(#[$attr])*
            async fn $name(buck: &mut Buck, tmp_path: &Path) -> anyhow::Result<()> {
                $body(buck, tmp_path, $($arg),*).await
            }
        )*
    };
}

/// [`cases!`] whose names carry Python's `False` and `True`.
macro_rules! bool_cases {
    ($fixture:tt $body:ident { $($(#[$attr:meta])* fn $name:ident($($arg:expr),*);)* }) => {
        cases! {
            $fixture $body {
                $(
                    $(#[$attr])*
                    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
                    fn $name($($arg),*);
                )*
            }
        }
    };
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

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

/// `status["process_info"]["pid"]` of a `buck2 status` or `buck2 server --status`.
fn pid_of(status: &Value) -> anyhow::Result<u32> {
    let pid = status
        .get("process_info")
        .and_then(|info| info.get("pid"))
        .and_then(Value::as_u64)
        .with_context(|| format!("No `process_info.pid` in {status}"))?;
    Ok(u32::try_from(pid)?)
}

async fn status_json(buck: &Buck) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&buck.status(&[]).await?.stdout)?)
}

async fn daemon_pid(buck: &Buck) -> anyhow::Result<u32> {
    pid_of(&status_json(buck).await?)
}

async fn server_status_pid(buck: &Buck) -> anyhow::Result<u32> {
    pid_of(&serde_json::from_str(
        &buck.server(&["--status"]).await?.stdout,
    )?)
}

/// The `pid` of a `buckd.info`.
fn info_pid(info: &str) -> anyhow::Result<u32> {
    let info: Value = serde_json::from_str(info)?;
    let pid = info
        .get("pid")
        .and_then(Value::as_u64)
        .with_context(|| format!("No `pid` in {info}"))?;
    Ok(u32::try_from(pid)?)
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

fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => Ok(result?),
    }
}

fn isolated_buck(buck: &Buck, isolation: &str) -> Buck {
    let mut isolated = buck.clone();
    isolated.set_isolation_prefix(isolation);
    isolated
}

/// The entries of `buck2 debug tenants --json`.
async fn debug_tenants(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    Ok(serde_json::from_str(
        &buck.debug(&["tenants", "--json"]).await?.stdout,
    )?)
}

fn isolation_of(entry: &Value) -> anyhow::Result<&str> {
    entry
        .get("isolation")
        .and_then(Value::as_str)
        .with_context(|| format!("No `isolation` in {entry}"))
}

fn generation_of(entry: &Value) -> anyhow::Result<u64> {
    entry
        .get("generation")
        .and_then(Value::as_u64)
        .with_context(|| format!("No `generation` in {entry}"))
}

fn isolations(entries: &[Value]) -> anyhow::Result<Vec<&str>> {
    entries.iter().map(isolation_of).collect()
}

fn tenant_entry<'a>(entries: &'a [Value], isolation: &str) -> anyhow::Result<&'a Value> {
    for entry in entries {
        if isolation_of(entry)? == isolation {
            return Ok(entry);
        }
    }
    anyhow::bail!("No tenant `{isolation}` in {entries:?}")
}

/// The generation of the tenant `isolation`.
fn tenant_generation(entries: &[Value], isolation: &str) -> anyhow::Result<u64> {
    generation_of(tenant_entry(entries, isolation)?)
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

/// Waits up to 30 seconds for `path`, failing if `build` exits first.
async fn wait_for_marker(path: &Path, build: &mut Option<BuckProcess>) -> anyhow::Result<()> {
    tokio::time::timeout(THIRTY_SECONDS, async {
        while !path.exists() {
            let process = build.as_mut().expect("the build is running");
            if process.try_wait().is_some() {
                build.take().expect("the build is running").wait().await?;
                anyhow::bail!("Build exited without creating {}", path.display());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    })
    .await
    .with_context(|| format!("Timed out waiting for {}", path.display()))?
}

fn start_generation(buck: &Buck, directory: &Path, value: &str) -> anyhow::Result<BuckProcess> {
    let config = directory.join(format!("{value}.bcfg"));
    std::fs::write(
        &config,
        format!(
            "[generation]\nstarted = {}\nrelease = {}\nvalue = {value}\n",
            directory.join(format!("{value}.started")).display(),
            directory.join(format!("{value}.release")).display(),
        ),
    )?;
    Ok(buck
        .build(&[
            "//:generation",
            "--config-file",
            path_str(&config),
            "--no-remote-cache",
        ])
        .spawn())
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

async fn wait_for_evicted_generation(
    buck: &Buck,
    isolation: &str,
    generation: u64,
) -> anyhow::Result<()> {
    tokio::time::timeout(THIRTY_SECONDS, async {
        loop {
            let mut present = false;
            for entry in debug_tenants(buck).await? {
                if isolation_of(&entry)? == isolation && generation_of(&entry)? == generation {
                    present = true;
                }
            }
            if !present {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("Timed out waiting for the tenant generation to be evicted")?
}

fn materializer_identity(buck: &Buck, isolation: &str) -> anyhow::Result<rusqlite::types::Value> {
    let db = buck
        .cwd()
        .join("buck-out")
        .join(isolation)
        .join("cache/materializer_state/db.sqlite");
    let connection = rusqlite::Connection::open(db)?;
    Ok(connection.query_row(
        "SELECT value FROM created_by WHERE key = 'timestamp_on_initialization'",
        [],
        |row| row.get(0),
    )?)
}

async fn exited_process_pid() -> anyhow::Result<u32> {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd");
        command.args(["/c", "exit 0"]);
        command
    } else {
        tokio::process::Command::new("true")
    };
    let mut process = command.spawn()?;
    let pid = process.id().context("The process has no pid")?;
    assert!(process.wait().await?.success());
    assert!(!daemon_is_alive(pid));
    Ok(pid)
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

async fn assert_daemon_exits(pid: u32) {
    assert!(
        wait_for_exit(pid, Duration::from_secs(10)).await,
        "Daemon survived final tenant eviction"
    );
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
    buck.set_env("BUCK2_SHARE_DAEMON", "false");
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
async fn test_unreachable_recorded_daemon_is_replaced(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_env("BUCK2_SHARE_DAEMON", "false");
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
async fn test_daemon_start_waits_for_tenant_outputs(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_env("BUCK2_SHARE_DAEMON", "false");
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
    buck.set_env("BUCK2_SHARE_DAEMON", "false");
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

#[buck_test]
async fn test_unshared_daemon_preserves_shared_daemon(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_env("BUCK2_SHARE_DAEMON", "true");
    let buck = &*buck;
    let shared_pid = server_status_pid(buck).await?;
    let mut isolated = isolated_buck(buck, "unshared");
    isolated.set_env("BUCK2_SHARE_DAEMON", "false");
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

async fn bad_shared_metadata_preserves_live_daemon(
    buck: &Buck,
    metadata: &str,
    pid_file: &str,
) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    let sibling = isolated_buck(buck, "survivor");
    sibling.targets(&["//:rule"]).await?;
    let newcomer = isolated_buck(buck, "newcomer");
    let daemon_dir = buck.daemon_dir().await?;
    let info_path = daemon_dir.join("buckd.info");
    let pid_path = daemon_dir.join("buckd.pid");
    let original_info = std::fs::read_to_string(&info_path)?;
    let original_pid = std::fs::read_to_string(&pid_path)?;
    let pid = info_pid(&original_info)?;
    let damaged_pid = match pid_file {
        "unchanged" => Some(original_pid.clone()),
        "invalid" => Some("not a PID".to_owned()),
        "exited" => Some(exited_process_pid().await?.to_string()),
        _ => None,
    };
    let damaged = match metadata {
        "missing" => None,
        "not-json" => Some("not JSON"),
        "invalid-json" => Some("[]"),
        _ => panic!("unknown metadata `{metadata}`"),
    };
    let result = async {
        match damaged {
            None => std::fs::remove_file(&info_path)?,
            Some(damaged) => std::fs::write(&info_path, damaged)?,
        }
        match &damaged_pid {
            None => std::fs::remove_file(&pid_path)?,
            Some(damaged_pid) => std::fs::write(&pid_path, damaged_pid)?,
        }
        newcomer
            .targets(&["//:rule"])
            .await
            .expect_failure()
            .assert_stderr_matches("Cannot verify shared daemon inactivity");
        buck.kill().await.expect_failure();
        assert!(daemon_is_alive(pid));
        assert!(
            buck.cwd()
                .join("buck-out")
                .join("v2")
                .join("cache")
                .is_dir()
        );
        assert!(
            buck.cwd()
                .join("buck-out")
                .join("survivor")
                .join("cache")
                .is_dir()
        );
        assert!(
            !buck
                .cwd()
                .join("buck-out")
                .join("newcomer")
                .join("cache")
                .exists()
        );
        match damaged {
            None => assert!(!info_path.exists()),
            Some(damaged) => assert_eq!(std::fs::read_to_string(&info_path)?, damaged),
        }
        if pid_file == "missing" {
            assert!(!pid_path.exists());
        } else {
            assert_eq!(Some(std::fs::read_to_string(&pid_path)?), damaged_pid);
        }
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&info_path, &original_info)?;
    std::fs::write(&pid_path, &original_pid)?;
    result?;
    let result = async {
        for tenant in [buck, &sibling, &newcomer] {
            tenant.targets(&["//:rule"]).await?;
            assert_eq!(daemon_pid(tenant).await?, pid);
        }
        anyhow::Ok(())
    }
    .await;
    newcomer.kill().await?;
    sibling.kill().await?;
    result
}

const BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV: [(&str, &str); 2] = [
    ("BUCK2_SHARE_DAEMON", "true"),
    ("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "120"),
];

cases! { [buck] bad_shared_metadata_preserves_live_daemon {
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_unchanged_missing("missing", "unchanged");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_unchanged_not_json("not-json", "unchanged");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_unchanged_invalid_json("invalid-json", "unchanged");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_missing_missing("missing", "missing");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_missing_not_json("not-json", "missing");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_missing_invalid_json("invalid-json", "missing");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_invalid_missing("missing", "invalid");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_invalid_not_json("not-json", "invalid");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_invalid_invalid_json("invalid-json", "invalid");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_exited_missing("missing", "exited");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_exited_not_json("not-json", "exited");
    #[buck_test(env = BAD_SHARED_METADATA_PRESERVES_LIVE_DAEMON_ENV)]
    fn test_bad_shared_metadata_preserves_live_daemon_exited_invalid_json("invalid-json", "exited");
}}

fn read_if_exists(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    Ok(if path.exists() {
        Some(std::fs::read(path)?)
    } else {
        None
    })
}

async fn bad_shared_metadata_requires_repair_after_daemon_exit(
    buck: &Buck,
    metadata: &str,
    pid_file: &str,
) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    let daemon_dir = buck.daemon_dir().await?;
    let info_path = daemon_dir.join("buckd.info");
    let pid_path = daemon_dir.join("buckd.pid");
    let original_info = std::fs::read_to_string(&info_path)?;
    let original_pid = std::fs::read_to_string(&pid_path)?;
    let old_pid = info_pid(&original_info)?;
    buck.kill().await?;
    for _ in 0..100 {
        if !daemon_is_alive(old_pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!daemon_is_alive(old_pid));
    let result = async {
        if metadata == "missing" {
            std::fs::remove_file(&info_path)?;
        } else {
            std::fs::write(&info_path, "not JSON")?;
        }
        if pid_file == "missing" {
            std::fs::remove_file(&pid_path)?;
        } else if pid_file == "invalid" {
            std::fs::write(&pid_path, "not a PID")?;
        } else if pid_file == "exited" {
            std::fs::write(&pid_path, exited_process_pid().await?.to_string())?;
        }
        let damaged_info = read_if_exists(&info_path)?;
        let damaged_pid = read_if_exists(&pid_path)?;
        buck.targets(&["//:rule"]).await.expect_failure();
        assert!(!daemon_is_alive(old_pid));
        assert_eq!(read_if_exists(&info_path)?, damaged_info);
        assert_eq!(read_if_exists(&pid_path)?, damaged_pid);
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&info_path, &original_info)?;
    std::fs::write(&pid_path, &original_pid)?;
    result?;
    buck.targets(&["//:rule"]).await?;
    let new_pid = daemon_pid(buck).await?;
    assert_ne!(new_pid, old_pid);
    Ok(())
}

cases! { [buck] bad_shared_metadata_requires_repair_after_daemon_exit {
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_bad_shared_metadata_requires_repair_after_daemon_exit_unchanged_missing("missing", "unchanged");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_bad_shared_metadata_requires_repair_after_daemon_exit_unchanged_not_json("not-json", "unchanged");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_bad_shared_metadata_requires_repair_after_daemon_exit_missing_missing("missing", "missing");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_bad_shared_metadata_requires_repair_after_daemon_exit_missing_not_json("not-json", "missing");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_bad_shared_metadata_requires_repair_after_daemon_exit_invalid_missing("missing", "invalid");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_bad_shared_metadata_requires_repair_after_daemon_exit_invalid_not_json("not-json", "invalid");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_bad_shared_metadata_requires_repair_after_daemon_exit_exited_missing("missing", "exited");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_bad_shared_metadata_requires_repair_after_daemon_exit_exited_not_json("not-json", "exited");
}}

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
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_evicted_generation_cannot_overlap_outputs_False_False_False(false, false, false);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_evicted_generation_cannot_overlap_outputs_False_False_True(true, false, false);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_evicted_generation_cannot_overlap_outputs_False_True_False(false, true, false);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_evicted_generation_cannot_overlap_outputs_False_True_True(true, true, false);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_evicted_generation_cannot_overlap_outputs_True_False_False(false, false, true);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_evicted_generation_cannot_overlap_outputs_True_False_True(true, false, true);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_evicted_generation_cannot_overlap_outputs_True_True_False(false, true, true);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
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
        let pending = wait_for_tenant_generation(buck, "initializing", generation).await?;
        assert!(!truthy(
            pending.get("initialized").context("No `initialized`")?
        ));
        assert!(!tmp_path.join("new.started").exists());
        assert!(
            client
                .as_mut()
                .expect("eviction is running")
                .try_wait()
                .is_none(),
            "Eviction returned before its I/O released ownership"
        );
        if disconnect_client {
            let _ignored = client.take().expect("eviction is running").kill().await;
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
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_eviction_during_generation_initialization_False_False(false, false);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_eviction_during_generation_initialization_False_True(true, false);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_eviction_during_generation_initialization_True_False(false, true);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
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
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_isolated_tenants_share_daemon_without_sharing_config_False(false);
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_isolated_tenants_share_daemon_without_sharing_config_True(true);
}}

#[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
async fn test_unreachable_shared_daemon_is_not_killed(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    let sibling = isolated_buck(buck, "sibling");
    sibling.targets(&["//:rule"]).await?;
    let daemon_dir = buck.daemon_dir().await?;
    let info_path = daemon_dir.join("buckd.info");
    let original = std::fs::read_to_string(&info_path)?;
    let mut info: Value = serde_json::from_str(&original)?;
    let pid = info_pid(&original)?;
    info["endpoint"] = json!("tcp:1");
    std::fs::write(&info_path, serde_json::to_string(&info)?)?;
    let result = async {
        buck.kill().await.expect_failure();
        buck.targets(&["//:rule"])
            .await
            .expect_failure()
            .assert_stderr_matches("Shared daemon is alive");
        let cache = buck.cwd().join("buck-out").join("v2").join("cache");
        assert!(cache.exists());
        buck.clean(&[]).await.expect_failure();
        assert!(cache.exists());
        assert!(daemon_is_alive(pid));
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&info_path, &original)?;
    result?;
    sibling.targets(&["//:rule"]).await?;
    sibling.kill().await?;
    Ok(())
}

async fn killall_repo_reaches_all_isolation_directories(
    buck: &mut Buck,
    sharing: &str,
) -> anyhow::Result<()> {
    buck.set_env("BUCK2_SHARE_DAEMON", sharing);
    let buck = &*buck;
    let tenants = ["tenant-one", "tenant-two"].map(|isolation| isolated_buck(buck, isolation));
    let result = async {
        for tenant in &tenants {
            tenant.targets(&["//:rule"]).await?;
        }
        buck.command(&["killall", "--repo", "--in-isolation-dir", "tenant-one"])
            .env("BUCK2_SHARE_DAEMON", sharing)
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
    #[buck_test]
    fn test_killall_repo_reaches_all_isolation_directories_false("false");
    #[buck_test]
    fn test_killall_repo_reaches_all_isolation_directories_true("true");
}}

async fn mixed_daemon_modes_cannot_own_same_outputs(
    buck: &mut Buck,
    tmp_path: &Path,
    owner_sharing: &str,
    isolation: &str,
) -> anyhow::Result<()> {
    buck.set_env("BUCK2_SHARE_DAEMON", owner_sharing);
    buck.set_isolation_prefix(isolation);
    let buck = &*buck;
    let mut other = isolated_buck(buck, isolation);
    other.set_env(
        "BUCK2_SHARE_DAEMON",
        if owner_sharing == "false" {
            "true"
        } else {
            "false"
        },
    );
    let mut old = Some(start_generation(buck, tmp_path, "old")?);
    let result = async {
        wait_for_marker(&tmp_path.join("old.started"), &mut old).await?;
        let new = start_generation(&other, tmp_path, "new")?;
        // A shared intruder's daemon cannot take the tenant's disk lock; an unshared intruder
        // cannot take the lifecycle lock that the shared owner holds for the tenant.
        new.wait()
            .await
            .expect_failure()
            .assert_stderr_matches(if owner_sharing == "false" {
                "another process still owns it"
            } else {
                "Error locking buckd.lifecycle"
            });
        assert!(!tmp_path.join("new.started").exists());
        touch(&tmp_path.join("old.release"))?;
        let result = wait_within(
            old.take().expect("the old build is running"),
            THIRTY_SECONDS,
        )
        .await??;
        let output = generation_output(&result)?;
        assert_eq!(std::fs::read_to_string(&output)?, "old");
        buck.kill().await?;
        touch(&tmp_path.join("new.release"))?;
        let result =
            wait_within(start_generation(&other, tmp_path, "new")?, THIRTY_SECONDS).await??;
        let new_output = generation_output(&result)?;
        assert_eq!(new_output, output);
        assert_eq!(std::fs::read_to_string(&new_output)?, "new");
        let repeated = other
            .build(&[
                "//:generation",
                "--config-file",
                path_str(&tmp_path.join("new.bcfg")),
                "--no-remote-cache",
            ])
            .await?;
        assert_eq!(read_generation(&repeated)?, "new");
        anyhow::Ok(())
    }
    .await;
    for value in ["old", "new"] {
        touch(&tmp_path.join(format!("{value}.release")))?;
    }
    wait_for_all(old, THIRTY_SECONDS).await?;
    other.kill().await?;
    result
}

cases! { [mut buck, tmp_path] mixed_daemon_modes_cannot_own_same_outputs {
    #[buck_test(skip_for_os = ["windows"])]
    fn test_mixed_daemon_modes_cannot_own_same_outputs_collision_false("false", "collision");
    #[buck_test(skip_for_os = ["windows"])]
    fn test_mixed_daemon_modes_cannot_own_same_outputs_collision_true("true", "collision");
    #[buck_test(skip_for_os = ["windows"])]
    fn test_mixed_daemon_modes_cannot_own_same_outputs_v2_false("false", "v2");
    #[buck_test(skip_for_os = ["windows"])]
    fn test_mixed_daemon_modes_cannot_own_same_outputs_v2_true("true", "v2");
}}

async fn clean_preserves_sibling_tenant(
    buck: &mut Buck,
    tmp_path: &Path,
    sharing: &str,
    background: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    buck.set_env("BUCK2_SHARE_DAEMON", sharing);
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
    #[buck_test]
    fn test_clean_preserves_sibling_tenant_False_False_false("false", false, false);
    #[buck_test]
    fn test_clean_preserves_sibling_tenant_False_False_true("true", false, false);
    #[buck_test]
    fn test_clean_preserves_sibling_tenant_False_True_false("false", true, false);
    #[buck_test]
    fn test_clean_preserves_sibling_tenant_False_True_true("true", true, false);
    #[buck_test]
    fn test_clean_preserves_sibling_tenant_True_False_false("false", false, true);
    #[buck_test]
    fn test_clean_preserves_sibling_tenant_True_False_true("true", false, true);
    #[buck_test]
    fn test_clean_preserves_sibling_tenant_True_True_false("false", true, true);
    #[buck_test]
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
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_clean_waits_for_tenant_disk_work_False(false);
    #[buck_test(skip_for_os = ["windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_clean_waits_for_tenant_disk_work_True(true);
}}

async fn repository_eviction_reports_and_continues_after_failures(
    buck: &Buck,
    fault: &str,
    sharing: &str,
) -> anyhow::Result<()> {
    let tenants: Vec<Buck> = ["tenant-one", "tenant-two"]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut tenant = isolated_buck(buck, name);
            tenant.set_env("BUCK2_SHARE_DAEMON", if i == 0 { sharing } else { "false" });
            tenant
        })
        .collect();
    for tenant in &tenants {
        tenant.targets(&["//:rule"]).await?;
    }
    let info_path = tenants[0].daemon_dir().await?.join("buckd.info");
    let original = std::fs::read_to_string(&info_path)?;
    let mut info: Value = serde_json::from_str(&original)?;
    let pid = info_pid(&original)?;
    let damaged_paths = [info_path.clone()];
    let saved = damaged_paths
        .iter()
        .map(|path| Ok((path.clone(), std::fs::read(path)?)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let daemons_dir = info_path
        .parent()
        .and_then(Path::parent)
        .context("The daemon dir has no parent")?;
    std::fs::create_dir(daemons_dir.join("empty"))?;
    if fault == "unreachable" || fault == "stale-pid" {
        info["endpoint"] = json!("tcp:1");
        if fault == "stale-pid" {
            info["pid"] = json!(exited_process_pid().await?);
        }
        std::fs::write(&info_path, serde_json::to_string(&info)?)?;
    } else if fault == "missing-info" {
        for path in &damaged_paths {
            std::fs::remove_file(path)?;
        }
    }
    let mut args = vec!["killall", "--repo"];
    if fault == "rpc" {
        args.extend(["--in-isolation-dir", "invalid/name"]);
    }
    let outcome = async {
        let result = buck.command(&args).await.expect_failure();
        result.assert_stderr_matches("Repository tenant eviction failed");
        if sharing == "true" && (fault == "unreachable" || fault == "stale-pid") {
            tenants[0].kill().await.expect_failure();
        }
        assert!(daemon_is_alive(pid));
        if fault != "rpc" {
            if fault == "unreachable" || fault == "stale-pid" {
                assert!(result.stderr.contains("Failed to contact daemon"));
            } else {
                assert!(result.stderr.contains("buckd.info is missing"));
                assert!(damaged_paths.iter().all(|path| !path.exists()));
            }
            assert!(result.stderr.contains("failed for 1 daemon(s)"));
            assert!(result.stderr.contains("Evicted 1 tenant(s)"));
            assert!(
                tenants[1]
                    .status(&[])
                    .await?
                    .stderr
                    .contains("no buckd running")
            );
        } else {
            assert!(result.stderr.contains("failed for 2 daemon(s)"));
            assert_eq!(
                result
                    .stderr
                    .matches("Failed to evict tenants from daemon")
                    .count(),
                2
            );
            assert!(
                result
                    .stderr
                    .contains(path_str(info_path.parent().expect("a daemon dir")))
                    && result.stderr.contains("tenant-two")
            );
        }
        anyhow::Ok(())
    }
    .await;
    for (path, contents) in &saved {
        std::fs::write(path, contents)?;
    }
    for tenant in &tenants {
        tenant.kill().await?;
    }
    outcome
}

const REPOSITORY_EVICTION_ENV: [(&str, &str); 2] = [
    ("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "120"),
    ("BUCK2_SHARE_DAEMON", "true"),
];

cases! { [buck] repository_eviction_reports_and_continues_after_failures {
    #[buck_test(env = REPOSITORY_EVICTION_ENV)]
    fn test_repository_eviction_reports_and_continues_after_failures_unreachable_false("unreachable", "false");
    #[buck_test(env = REPOSITORY_EVICTION_ENV)]
    fn test_repository_eviction_reports_and_continues_after_failures_unreachable_true("unreachable", "true");
    #[buck_test(env = REPOSITORY_EVICTION_ENV)]
    fn test_repository_eviction_reports_and_continues_after_failures_stale_pid_false("stale-pid", "false");
    #[buck_test(env = REPOSITORY_EVICTION_ENV)]
    fn test_repository_eviction_reports_and_continues_after_failures_stale_pid_true("stale-pid", "true");
    #[buck_test(env = REPOSITORY_EVICTION_ENV)]
    fn test_repository_eviction_reports_and_continues_after_failures_rpc_false("rpc", "false");
    #[buck_test(env = REPOSITORY_EVICTION_ENV)]
    fn test_repository_eviction_reports_and_continues_after_failures_rpc_true("rpc", "true");
    #[buck_test(env = REPOSITORY_EVICTION_ENV)]
    fn test_repository_eviction_reports_and_continues_after_failures_missing_info_false("missing-info", "false");
    #[buck_test(env = REPOSITORY_EVICTION_ENV)]
    fn test_repository_eviction_reports_and_continues_after_failures_missing_info_true("missing-info", "true");
}}

#[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
async fn test_shared_startup_mismatch_preserves_siblings(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["//:rule"]).await?;
    let sibling = isolated_buck(buck, "sibling");
    let result = async {
        sibling.targets(&["//:rule"]).await?;
        let pid = daemon_pid(&sibling).await?;
        let config = buck.cwd().join(".buckconfig");
        let original = std::fs::read_to_string(&config)?;
        let mismatched = async {
            std::fs::write(
                &config,
                format!("{original}\n[buck2]\ndaemon_buster = mismatch\n"),
            )?;
            buck.targets(&["//:rule"])
                .await
                .expect_failure()
                .assert_stderr_matches("Shared daemon constraint mismatch");
            assert!(daemon_is_alive(pid));
            anyhow::Ok(())
        }
        .await;
        std::fs::write(&config, &original)?;
        mismatched?;
        sibling.targets(&["//:rule"]).await?;
        assert_eq!(daemon_pid(&sibling).await?, pid);
        anyhow::Ok(())
    }
    .await;
    sibling.kill().await?;
    result
}

#[buck_test]
async fn test_host_killall_isolation_filter_preserves_other_daemons(
    buck: &Buck,
) -> anyhow::Result<()> {
    let prefix = format!("tenant-filter-{}", uuid::Uuid::new_v4().simple());
    let tenants = ["selected", "survivor"].map(|name| {
        let mut tenant = isolated_buck(buck, &format!("{prefix}-{name}"));
        tenant.set_env("BUCK2_SHARE_DAEMON", "false");
        tenant
    });
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

#[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
async fn test_live_legacy_owner_blocks_shared_initialization(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    buck.set_isolation_prefix("legacy");
    let buck = &*buck;
    let shared_dir = buck.daemon_dir().await?;
    let legacy_dir = shared_dir
        .parent()
        .context("The daemon dir has no parent")?
        .join("legacy");
    std::fs::create_dir_all(&legacy_dir)?;
    let pid_file = legacy_dir.join("buckd.pid");
    std::fs::write(&pid_file, std::process::id().to_string())?;
    let result = async {
        buck.targets(&["//:rule"])
            .await
            .expect_failure()
            .assert_stderr_matches("already owned by a legacy daemon");
        assert!(
            !buck
                .cwd()
                .join("buck-out")
                .join("legacy")
                .join("cache")
                .exists()
        );
        anyhow::Ok(())
    }
    .await;
    std::fs::remove_file(&pid_file)?;
    result
}

#[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
async fn test_clean_cannot_unlink_sibling_disk_lock(buck: &mut Buck) -> anyhow::Result<()> {
    let sibling = isolated_buck(buck, "sibling");
    let result = async {
        sibling.targets(&["//:rule"]).await?;
        let daemon_dir = sibling.daemon_dir().await?;
        let mut lock_files = Vec::new();
        for entry in std::fs::read_dir(
            daemon_dir
                .parent()
                .context("The daemon dir has no parent")?,
        )? {
            let path = entry?.path();
            if path.is_dir() && path.join("sibling").is_file() {
                lock_files.push(path.join("sibling"));
            }
        }
        assert_eq!(lock_files.len(), 1, "{lock_files:?}");
        let inode = file_id(&lock_files[0])?;
        buck.set_env("BUCK2_SHARE_DAEMON", "false");
        buck.set_isolation_prefix(".tenant-locks");
        buck.clean(&[]).await?;
        assert_eq!(file_id(&lock_files[0])?, inode);
        sibling.targets(&["//:rule"]).await?;
        anyhow::Ok(())
    }
    .await;
    sibling.kill().await?;
    result
}

async fn garbage_legacy_pid_refuses_shared_initialization(
    buck: &mut Buck,
    bad_pid: &str,
) -> anyhow::Result<()> {
    buck.set_isolation_prefix("legacy-garbage");
    let buck = &*buck;
    let shared_dir = buck.daemon_dir().await?;
    let legacy_dir = shared_dir
        .parent()
        .context("The daemon dir has no parent")?
        .join("legacy-garbage");
    std::fs::create_dir_all(&legacy_dir)?;
    let pid_file = legacy_dir.join("buckd.pid");
    std::fs::write(&pid_file, bad_pid)?;
    let outputs = buck.cwd().join("buck-out").join("legacy-garbage");
    let sentinel = outputs.join("sentinel.txt");
    std::fs::create_dir_all(&outputs)?;
    std::fs::write(&sentinel, "sentinel")?;
    let result = async {
        let failure = buck.targets(&["//:rule"]).await.expect_failure();
        failure.assert_stderr_matches("Cannot verify legacy daemon ownership");
        assert!(failure.stderr.contains("restore a consistent"));
        assert!(failure.stderr.contains("Stop the legacy daemon"));
        assert_eq!(std::fs::read_to_string(&pid_file)?, bad_pid);
        assert_eq!(std::fs::read_to_string(&sentinel)?, "sentinel");
        assert!(!outputs.join("cache").exists());
        std::fs::write(&pid_file, exited_process_pid().await?.to_string())?;
        buck.targets(&["//:rule"]).await?;
        assert!(outputs.join("cache").is_dir());
        anyhow::Ok(())
    }
    .await;
    remove_file_if_exists(&pid_file)?;
    result
}

cases! { [mut buck] garbage_legacy_pid_refuses_shared_initialization {
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_garbage_legacy_pid_refuses_shared_initialization_empty("");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_garbage_legacy_pid_refuses_shared_initialization_garbage("not a PID");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_garbage_legacy_pid_refuses_shared_initialization_zero("0");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_garbage_legacy_pid_refuses_shared_initialization_negative("-1");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_garbage_legacy_pid_refuses_shared_initialization_overflow("99999999999999999999");
}}

async fn missing_and_exited_legacy_pid_allows_shared_initialization(
    buck: &mut Buck,
    legacy_pid: &str,
) -> anyhow::Result<()> {
    buck.set_isolation_prefix("legacy-allowed");
    let buck = &*buck;
    let shared_dir = buck.daemon_dir().await?;
    let legacy_dir = shared_dir
        .parent()
        .context("The daemon dir has no parent")?
        .join("legacy-allowed");
    std::fs::create_dir_all(&legacy_dir)?;
    let pid_file = legacy_dir.join("buckd.pid");
    let result = async {
        if legacy_pid == "exited" {
            std::fs::write(&pid_file, exited_process_pid().await?.to_string())?;
        } else {
            remove_file_if_exists(&pid_file)?;
        }
        buck.targets(&["//:rule"]).await?;
        assert!(
            buck.cwd()
                .join("buck-out")
                .join("legacy-allowed")
                .join("cache")
                .is_dir()
        );
        anyhow::Ok(())
    }
    .await;
    remove_file_if_exists(&pid_file)?;
    result
}

cases! { [mut buck] missing_and_exited_legacy_pid_allows_shared_initialization {
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_missing_and_exited_legacy_pid_allows_shared_initialization_missing("missing");
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_missing_and_exited_legacy_pid_allows_shared_initialization_exited("exited");
}}

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
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_isolated_tenants_keep_cached_paths_local_False(false);
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
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
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_isolated_tenants_select_dice_engine_ownership_False(false);
    #[buck_test(env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_isolated_tenants_select_dice_engine_ownership_True(true);
}}

async fn hg(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("hg")
        .args(args)
        .current_dir(cwd)
        .stderr(std::process::Stdio::inherit())
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "`hg {}` failed: {}",
        args.join(" "),
        output.status
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

async fn build_value(tenant: &Buck) -> anyhow::Result<PathBuf> {
    let result = tenant.build(&["//:tenant_value"]).await?;
    result.build_report()?.output_for_target("//:tenant_value")
}

async fn checkouts_share_daemon(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
    same_commit: bool,
) -> anyhow::Result<()> {
    let source = tmp_path.join("source");
    std::fs::create_dir(&source)?;
    for name in [".buckconfig", "TARGETS.fixture", "defs.bzl", "paths.bxl"] {
        std::fs::copy(buck.cwd().join(name), source.join(name))?;
    }
    touch(&source.join(".buckroot"))?;
    append_to_file(
        &source.join(".buckconfig"),
        &format!("\n[buck2]\nshare_dice = {share_dice}\nfile_watcher = fs_hash_crawler\n"),
    )?;
    let targets = source.join("TARGETS.fixture");
    let template = std::fs::read_to_string(&targets)?;

    hg(&source, &["init"]).await?;
    let mut revisions = Vec::new();
    for value in ["commit-one", "commit-two"] {
        std::fs::write(
            &targets,
            template.replace(
                r#"read_config("tenant", "value", "unset")"#,
                &format!("\"{value}\""),
            ),
        )?;
        if revisions.is_empty() {
            hg(
                &source,
                &[
                    "add",
                    ".buckconfig",
                    ".buckroot",
                    "TARGETS.fixture",
                    "defs.bzl",
                    "paths.bxl",
                ],
            )
            .await?;
        }
        hg(&source, &["commit", "-m", value, "-u", "Buck2 E2E"]).await?;
        revisions.push(hg(&source, &["log", "-r", ".", "-T", "{node}"]).await?);
    }
    assert_ne!(revisions[0], revisions[1]);
    if same_commit {
        revisions[1] = revisions[0].clone();
    }
    let values = [
        "commit-one",
        if same_commit {
            "commit-one"
        } else {
            "commit-two"
        },
    ];

    let mut tenants = Vec::new();
    for (name, revision) in ["checkout-one", "checkout-two"].into_iter().zip(&revisions) {
        let checkout = tmp_path.join(name);
        hg(
            &source,
            &[
                "clone",
                "--updaterev",
                revision,
                path_str(&source),
                path_str(&checkout),
            ],
        )
        .await?;
        assert_eq!(
            &hg(&checkout, &["log", "-r", ".", "-T", "{node}"]).await?,
            revision
        );
        assert!(
            hg(
                &checkout,
                &["status", "--modified", "--added", "--removed", "--deleted"]
            )
            .await?
            .is_empty()
        );
        let mut tenant = buck.clone();
        tenant.set_env("BUCK2_TEST_HOME_DIR", tmp_path.join("home"));
        tenant.set_env("BUCK2_DAEMON_GROUP", "different-commits");
        tenant.set_cwd(checkout);
        tenant.set_isolation_prefix("checkout");
        tenants.push(tenant);
    }
    let [tenant_one, tenant_two] = tenants.as_slice() else {
        unreachable!("two checkouts");
    };

    let result = async {
        let mut outputs = Vec::new();
        for (tenant, value) in tenants.iter().zip(values) {
            let output = build_value(tenant).await?;
            assert_eq!(std::fs::read_to_string(&output)?, value);
            assert!(output.starts_with(tenant.cwd()));
            outputs.push(output);
        }
        assert_ne!(outputs[0], outputs[1]);
        let daemon_pid_one = daemon_pid(tenant_one).await?;
        assert_eq!(daemon_pid(tenant_two).await?, daemon_pid_one);
        assert_tenant_dice_graphs(tenant_one, tenant_two, tmp_path, share_dice).await?;

        assert_tenant_cached_paths(&[(tenant_one, "checkout"), (tenant_two, "checkout")]).await?;

        let startup_config = tenant_two.cwd().join(".buckconfig");
        let original_config = std::fs::read_to_string(&startup_config)?;
        std::fs::write(
            &startup_config,
            format!("{original_config}\n[buck2]\nshare_dice = {}\n", !share_dice),
        )?;
        let mut other_group = isolated_buck(tenant_two, "other-mode");
        other_group.set_env("BUCK2_DAEMON_GROUP", "other-mode");
        let other_mode = async {
            tenant_two
                .build(&["//:tenant_value"])
                .await
                .expect_failure()
                .assert_stderr_matches("daemon group .* has incompatible constraints");
            assert_eq!(
                std::fs::read_to_string(build_value(&other_group).await?)?,
                values[1]
            );
            assert_ne!(daemon_pid(&other_group).await?, daemon_pid_one);
            anyhow::Ok(())
        }
        .await;
        other_group.kill().await?;
        std::fs::write(&startup_config, &original_config)?;
        other_mode?;
        assert_eq!(daemon_pid(tenant_one).await?, daemon_pid_one);

        for (tenant, value) in tenants.iter().zip(values) {
            assert_eq!(std::fs::read_to_string(build_value(tenant).await?)?, value);
        }
        tenant_one.kill().await?;
        assert_eq!(
            std::fs::read_to_string(build_value(tenant_two).await?)?,
            values[1]
        );
        assert_eq!(daemon_pid(tenant_two).await?, daemon_pid_one);
        assert_eq!(
            std::fs::read_to_string(build_value(tenant_one).await?)?,
            "commit-one"
        );
        assert_eq!(daemon_pid(tenant_one).await?, daemon_pid_one);
        for (tenant, revision) in tenants.iter().zip(&revisions) {
            assert_eq!(
                &hg(tenant.cwd(), &["log", "-r", ".", "-T", "{node}"]).await?,
                revision
            );
            assert!(
                hg(
                    tenant.cwd(),
                    &["status", "--modified", "--added", "--removed", "--deleted"]
                )
                .await?
                .is_empty()
            );
        }
        anyhow::Ok(daemon_pid_one)
    }
    .await;
    for tenant in &tenants {
        tenant.kill().await?;
    }
    assert_daemon_exits(result?).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] checkouts_share_daemon {
    #[buck_test(skip_for_os = ["darwin", "windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_checkouts_share_daemon_False_False(false, false);
    #[buck_test(skip_for_os = ["darwin", "windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_checkouts_share_daemon_False_True(true, false);
    #[buck_test(skip_for_os = ["darwin", "windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_checkouts_share_daemon_True_False(false, true);
    #[buck_test(skip_for_os = ["darwin", "windows"], env = [("BUCK2_SHARE_DAEMON", "true")])]
    fn test_checkouts_share_daemon_True_True(true, true);
}}
