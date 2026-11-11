/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Daemon sharing across checkouts: clients whose startup constraints agree join one daemon.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use buck2_e2e::BuckError;
use buck2_e2e::BuckOutput;
use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;
use buck2_e2e::process::Signal;
use buck2_e2e::process::daemon_is_alive;
use buck2_e2e::process::send_signal;
use buck2_e2e::utils::file_id;
use buck2_e2e::utils::remove_output;
use common_unidaemon::THIRTY_SECONDS;
use common_unidaemon::assert_daemon_exits;
use common_unidaemon::bool_cases;
use common_unidaemon::cases;
use common_unidaemon::daemon_pid;
use common_unidaemon::debug_tenants;
use common_unidaemon::generation_of;
use common_unidaemon::isolated_buck;
use common_unidaemon::isolation_of;
use common_unidaemon::isolations;
use common_unidaemon::materializer_identity;
use common_unidaemon::only_unidaemon_namespace;
use common_unidaemon::path_str;
use common_unidaemon::pid_of;
use common_unidaemon::proxy_slot;
use common_unidaemon::start_generation;
use common_unidaemon::status_json;
use common_unidaemon::tenant_entry;
use common_unidaemon::tenant_generation;
use common_unidaemon::unidaemon_checkouts;
use common_unidaemon::wait_for_evicted_generation;
use common_unidaemon::wait_for_marker;
use regex::Regex;
use serde_json::Value;

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

fn touch(path: &Path) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => Ok(result?),
    }
}

/// The `pid` of the `buckd.info` at `path`, if it is there and complete.
fn slot_pid(path: &Path) -> Option<u32> {
    let info: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    u32::try_from(info["pid"].as_u64()?).ok()
}

/// The pid of the proxy published in the tenant's slot.
fn proxy_pid(tenant: &Buck) -> anyhow::Result<u32> {
    let path = proxy_slot(tenant).join("buckd.info");
    slot_pid(&path).with_context(|| format!("No proxy is published at {}", path.display()))
}

/// What the Python tests' `asyncio.wait_for(asyncio.shield(task), timeout)` gave: the command's
/// outcome, or an error if it took longer.
async fn wait_within(
    process: BuckProcess,
    timeout: Duration,
) -> anyhow::Result<Result<BuckOutput, BuckError>> {
    tokio::time::timeout(timeout, process.wait())
        .await
        .with_context(|| format!("The command did not finish within {timeout:?}"))
}

/// Lets a started command finish, whatever its outcome, as the Python tests'
/// `asyncio.gather(task, return_exceptions=True)` did.
async fn finish(process: Option<BuckProcess>) {
    if let Some(process) = process {
        let _ignored = process.wait().await;
    }
}

/// What `assertRaisesRegex(BuckException, "evicted")` checked: the command failed, saying so.
#[track_caller]
fn assert_evicted(outcome: Result<BuckOutput, BuckError>) {
    let error = outcome.expect_err("The command was expected to fail");
    assert!(error.to_string().contains("evicted"), "{error}");
}

/// Builds `//:tenant_value` in `tenant` and checks that the output holds `value` and lies in
/// the tenant's own checkout.
async fn assert_tenant_value(tenant: &Buck, value: &str) -> anyhow::Result<()> {
    let result = tenant.build(&["//:tenant_value"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//:tenant_value")?;
    assert_eq!(std::fs::read_to_string(&output)?, value);
    assert!(output.starts_with(tenant.cwd()));
    Ok(())
}

fn namespace_keys(tmp_path: &Path) -> anyhow::Result<Vec<String>> {
    std::fs::read_dir(
        tmp_path
            .join("home")
            .join(".buck")
            .join("buckd")
            .join(".unidaemons"),
    )?
    .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
    .collect::<anyhow::Result<_>>()
}

async fn kill_all<'a>(tenants: impl IntoIterator<Item = &'a Buck>) -> anyhow::Result<()> {
    for tenant in tenants {
        tenant.kill().await?;
    }
    Ok(())
}

async fn unidaemon_compatibility_discovery(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let result = async {
        let mut pids = Vec::new();
        for (tenant, value) in tenants.iter().zip(["one", "two"]) {
            assert_tenant_value(tenant, value).await?;
            pids.push(daemon_pid(tenant).await?);
        }
        assert_eq!(pids[0], pids[1]);
        let keys = namespace_keys(tmp_path)?;
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(Regex::new("^[0-9a-f]{64}$")?.is_match(&keys[0]), "{keys:?}");
        tenants[0].kill().await?;
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, pids[0]);
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, pids[0]);
        anyhow::Ok(pids[0])
    }
    .await;
    let killed = kill_all(&tenants).await;
    let pid = result?;
    killed?;
    assert_daemon_exits(pid).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] unidaemon_compatibility_discovery {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_compatibility_discovery_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_compatibility_discovery_True(true);
}}

async fn unidaemon_incompatible_settings_select_another_process(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let config = tenants[1].cwd().join(".buckconfig");
    let original = std::fs::read_to_string(&config)?;
    let alternative = isolated_buck(&tenants[1], "other-mode");
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let pid = daemon_pid(&tenants[0]).await?;
        std::fs::write(
            &config,
            format!("{original}\n[buck2]\nshare_dice = {}\n", !share_dice),
        )?;
        alternative.build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_pid(&alternative).await?, pid);
        assert_eq!(namespace_keys(tmp_path)?.len(), 2);
        // Killed while its configuration still names its namespace: a client finds the
        // tenant's daemon by hashing the configuration it reads.
        alternative.kill().await?;
        std::fs::write(&config, &original)?;
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, pid);
        anyhow::Ok(pid)
    }
    .await;
    let killed = async {
        alternative.kill().await?;
        std::fs::write(&config, &original)?;
        kill_all(&tenants).await
    }
    .await;
    let pid = result?;
    killed?;
    assert_daemon_exits(pid).await;
    Ok(())
}

bool_cases! { [buck, tmp_path] unidaemon_incompatible_settings_select_another_process {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_incompatible_settings_select_another_process_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_incompatible_settings_select_another_process_True(true);
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_unawaited_final_eviction_stops_daemon(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    tenant.build(&["//:tenant_value"]).await?;
    let pid = daemon_pid(&tenant).await?;
    let entries = debug_tenants(&tenant).await?;
    let [entry] = entries.as_slice() else {
        anyhow::bail!("Expected one tenant, got {entries:?}");
    };
    tenant
        .debug(&[
            "tenants",
            "--evict-generation",
            &generation_of(entry)?.to_string(),
            "--no-wait",
        ])
        .await?;
    assert_daemon_exits(pid).await;
    Ok(())
}

/// The tenant's slot is served while its generation still initializes: the proxy there
/// describes the daemon as the tenant will see it afterwards, and another client of the
/// tenant joins the pending generation instead of starting one of its own.
#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_proxy_publishes_before_tenant_initializes(
    buck: &mut Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let gate = tmp_path.join("initialization");
    buck.set_env(
        "BUCK2_TEST_BLOCK_TENANT_INITIALIZATION",
        format!("initializing,{}", gate.display()),
    );
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let victim = isolated_buck(&tenants[0], "initializing");
    let mut first = None;
    let mut second = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        let status = status_json(&tenants[1]).await?;
        let daemon = pid_of(&status)?;
        first = Some(victim.build(&["//:tenant_value"]).spawn());
        wait_for_marker(&tmp_path.join("initialization.started"), &mut first).await?;
        let entries = debug_tenants(&tenants[1]).await?;
        let entry = tenant_entry(&entries, "initializing")?;
        assert!(!truthy(&entry["initialized"]));
        let generation = generation_of(entry)?;
        let slot = proxy_slot(&victim);
        assert!(slot.join("buckd.info").exists());
        let spec: Value =
            serde_json::from_str(&std::fs::read_to_string(slot.join("proxy-spec.json"))?)?;
        assert_eq!(spec["reference"]["generation"].as_u64(), Some(generation));
        assert_eq!(
            spec["constraints"]["daemon_id"],
            status["daemon_constraints"]["daemon_id"]
        );
        assert!(!truthy(&spec["constraints"]["extra"]["trace_io_enabled"]));
        second = Some(victim.build(&["//:tenant_value"]).spawn());
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(second.as_mut().unwrap().try_wait().is_none());
        touch(&tmp_path.join("initialization.release"))?;
        wait_within(first.take().unwrap(), THIRTY_SECONDS).await??;
        wait_within(second.take().unwrap(), THIRTY_SECONDS).await??;
        let entries = debug_tenants(&tenants[1]).await?;
        let initializing: Vec<(u64, bool)> = entries
            .iter()
            .filter(|entry| entry["isolation"].as_str() == Some("initializing"))
            .map(|entry| Ok((generation_of(entry)?, truthy(&entry["initialized"]))))
            .collect::<anyhow::Result<_>>()?;
        assert_eq!(initializing, [(generation, true)]);
        let status = status_json(&victim).await?;
        assert_eq!(pid_of(&status)?, daemon);
        assert!(!truthy(
            &status["daemon_constraints"]["extra"]["trace_io_enabled"]
        ));
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("initialization.release"))?;
    finish(first).await;
    finish(second).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_proxy_crash_during_initialization_evicts_generation(
    buck: &mut Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let gate = tmp_path.join("initialization");
    buck.set_env(
        "BUCK2_TEST_BLOCK_TENANT_INITIALIZATION",
        format!("initializing,{}", gate.display()),
    );
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let victim = isolated_buck(&tenants[0], "initializing");
    let mut build = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&tenants[1]).await?;
        build = Some(victim.build(&["//:tenant_value"]).spawn());
        wait_for_marker(&tmp_path.join("initialization.started"), &mut build).await?;
        let generation = tenant_generation(&debug_tenants(&tenants[1]).await?, "initializing")?;
        let slot = proxy_slot(&victim);
        let proxy = proxy_pid(&victim)?;
        send_signal(proxy, Signal::Kill)?;
        wait_for_evicted_generation(&tenants[1], "initializing", generation).await?;
        touch(&tmp_path.join("initialization.release"))?;
        assert_evicted(wait_within(build.take().unwrap(), THIRTY_SECONDS).await?);
        victim.build(&["//:tenant_value"]).await?;
        let entries = debug_tenants(&tenants[1]).await?;
        let newer: Vec<bool> = entries
            .iter()
            .filter(|entry| entry["isolation"].as_str() == Some("initializing"))
            .map(|entry| Ok(generation_of(entry)? > generation))
            .collect::<anyhow::Result<_>>()?;
        assert_eq!(newer, [true]);
        assert_ne!(slot_pid(&slot.join("buckd.info")), Some(proxy));
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("initialization.release"))?;
    finish(build).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

/// Waits up to 30 seconds for `path` to be gone.
async fn wait_for_missing_path(path: &Path) -> anyhow::Result<()> {
    tokio::time::timeout(THIRTY_SECONDS, async {
        while path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("Timed out waiting for {} to go", path.display()))
}

/// Waits up to 3 seconds for `path`.
async fn wait_for_path(path: &Path) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("Timed out waiting for {}", path.display()))
}

/// Whether the process `pid` has exited: it is gone, or a zombie not yet reaped by its parent.
fn process_terminated(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            let state = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next());
            matches!(state, Some("Z" | "X"))
        }
        Err(_) => true,
    }
}

async fn wait_for_terminated(pid: u32, timeout: Duration) -> anyhow::Result<()> {
    tokio::time::timeout(timeout, async {
        while !process_terminated(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("Process {pid} did not terminate within {timeout:?}"))
}

/// The daemon's record of `tenant`, read through `via`, a client of the same daemon.
async fn tenant_entry_via(via: &Buck, tenant: &Buck) -> anyhow::Result<Option<Value>> {
    let isolation = tenant.isolation_prefix().unwrap_or("v2");
    let entries = debug_tenants(via).await?;
    for entry in entries {
        if isolation_of(&entry)? == isolation
            && entry["project_root"].as_str() == Some(path_str(tenant.cwd()))
        {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

async fn tenant_generation_via(via: &Buck, tenant: &Buck) -> anyhow::Result<u64> {
    let entry = tenant_entry_via(via, tenant)
        .await?
        .with_context(|| format!("{} has no tenant in the daemon", tenant.cwd().display()))?;
    generation_of(&entry)
}

/// Waits until `tenant` has a generation above `previous` whose proxy is published and alive,
/// asking the daemon through `via`, a sibling tenant that is not being replaced.
async fn wait_for_proxy_generation(
    via: &Buck,
    tenant: &Buck,
    previous: u64,
) -> anyhow::Result<u64> {
    let path = proxy_slot(tenant).join("buckd.info");
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(entry) = tenant_entry_via(via, tenant).await?
                && let Some(pid) = slot_pid(&path)
                && generation_of(&entry)? > previous
                && !process_terminated(pid)
            {
                return generation_of(&entry);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("Timed out waiting for the tenant's new proxy")?
}

/// The rows of the namespace's `unidaemon-events.jsonl`.
fn unidaemon_events(tenant: &Buck) -> anyhow::Result<Vec<Value>> {
    let log = proxy_slot(tenant)
        .parent()
        .context("the slot has a parent")?
        .join("unidaemon-events.jsonl");
    std::fs::read_to_string(&log)
        .with_context(|| format!("Reading {}", log.display()))?
        .lines()
        .map(|row| Ok(serde_json::from_str(row)?))
        .collect()
}

fn has_event(events: &[Value], event: &str, generation: Option<u64>) -> bool {
    events.iter().any(|row| {
        row["event"].as_str() == Some(event)
            && generation.is_none_or(|generation| row["generation"].as_u64() == Some(generation))
    })
}

fn pid_file(path: &Path) -> anyhow::Result<u32> {
    Ok(std::fs::read_to_string(path)?.trim().parse()?)
}

fn kill_process(pid: u32, signal: Signal) -> anyhow::Result<()> {
    send_signal(pid, signal)
}

/// A client of the released buck2 the test target supplies as `BUCK2_TEST_LEGACY_BINARY`, routed
/// without sharing, for the tenant's checkout and isolation.
fn legacy_buck(tenant: &Buck, executable: &Path) -> anyhow::Result<Buck> {
    let mut legacy = tenant.clone();
    legacy.set_executable(executable);
    legacy.set_daemon_routing(false)?;
    Ok(legacy)
}

/// The released buck2 the test target supplies as `BUCK2_TEST_LEGACY_BINARY`, as the harness
/// gives it to `buck`'s commands: an absolute path. The target passes a DotSlash file, which is
/// not itself executable, so run it through `dotslash`.
fn legacy_executable(buck: &Buck, tmp_path: &Path) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(
        buck.env_var("BUCK2_TEST_LEGACY_BINARY")
            .context("the test target supplies BUCK2_TEST_LEGACY_BINARY")?,
    );
    if is_executable(&path) {
        return Ok(path);
    }
    let wrapper = tmp_path.join("legacy-buck2");
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\nexec dotslash \"{}\" \"$@\"\n", path.display()),
    )?;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
    Ok(wrapper)
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

fn set_gate(tenants: &mut [&mut Buck], variable: &str, isolation: &str, path: &Path) {
    for tenant in tenants {
        tenant.set_env(variable, format!("{isolation},{}", path.display()));
    }
}

/// A proxy that crashes or loses its liveness link is replaced by a fresh generation; one
/// stopped with a signal, as `buck2 killall` does, retires its tenant and clears its slot,
/// and the next command registers the tenant again.
async fn unidaemon_proxy_loss_retires_then_recovers_generation(
    buck: &Buck,
    tmp_path: &Path,
    failure: &str,
    share_dice: bool,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let mut victim = isolated_buck(&tenants[0], "crashing");
    let gate = tmp_path.join("eviction");
    let close = tmp_path.join("close-monitor");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "crashing",
            &gate,
        );
        set_gate(
            &mut all,
            "BUCK2_TEST_PROXY_CLOSE_LIVENESS",
            "crashing",
            &close,
        );
    }
    let mut writer = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        victim.build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&victim).await?;
        let slot = proxy_slot(&victim);
        let old = proxy_pid(&victim)?;
        let generation = tenant_generation_via(&tenants[1], &victim).await?;
        assert!(has_event(
            &unidaemon_events(&victim)?,
            "proxy-monitor-attached",
            Some(generation)
        ));
        writer = Some(start_generation(&victim, tmp_path, "old")?);
        wait_for_marker(&tmp_path.join("old.started"), &mut writer).await?;
        let writer_pid = pid_file(&tmp_path.join("old.started.pid"))?;
        match failure {
            "crash" => kill_process(old, Signal::Abort)?,
            "killed" => kill_process(old, Signal::Kill)?,
            _ => touch(&close)?,
        }
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        wait_for_evicted_generation(&tenants[1], "crashing", generation).await?;
        wait_for_terminated(writer_pid, Duration::from_secs(3)).await?;
        if failure == "socket-eof" {
            assert!(
                !process_terminated(old),
                "Live proxy exited before its tenant drained"
            );
        }
        let entries = debug_tenants(&tenants[1]).await?;
        assert!(!isolations(&entries)?.contains(&"crashing"));
        remove_file_if_exists(&close)?;
        touch(&tmp_path.join("eviction.release"))?;
        if failure == "killed" {
            for name in ["buckd.info", "buckd.pid", "proxy-spec.json"] {
                wait_for_missing_path(&slot.join(name)).await?;
            }
            victim.build(&["//:tenant_value"]).await?;
            assert!(tenant_generation_via(&tenants[1], &victim).await? > generation);
        } else {
            wait_for_proxy_generation(&tenants[1], &victim, generation).await?;
        }
        assert_ne!(proxy_pid(&victim)?, old);
        victim.build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    remove_file_if_exists(&close)?;
    touch(&tmp_path.join("eviction.release"))?;
    touch(&tmp_path.join("old.release"))?;
    finish(writer).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

cases! { [buck, tmp_path] unidaemon_proxy_loss_retires_then_recovers_generation {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_False_crash("crash", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_False_socket_eof("socket-eof", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_False_killed("killed", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_True_crash("crash", true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_True_socket_eof("socket-eof", true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_proxy_loss_retires_then_recovers_generation_True_killed("killed", true);
}}

async fn unidaemon_loss_recovery_respects_legacy_slot_owner(
    buck: &Buck,
    tmp_path: &Path,
    operation: &str,
) -> anyhow::Result<()> {
    let executable = legacy_executable(buck, tmp_path)?;
    let mut tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "crashing");
    let gate = tmp_path.join("eviction");
    let close = tmp_path.join("close-monitor");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "crashing",
            &gate,
        );
        set_gate(
            &mut all,
            "BUCK2_TEST_PROXY_CLOSE_LIVENESS",
            "crashing",
            &close,
        );
    }
    let legacy = legacy_buck(&victim, &executable)?;
    let mut transition = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        victim.build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&victim).await?;
        let generation = tenant_generation_via(&tenants[1], &victim).await?;
        touch(&close)?;
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        transition = Some(if operation == "clean" {
            legacy.clean(&[]).spawn()
        } else {
            legacy.build(&["//:tenant_value"]).spawn()
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(transition.as_mut().unwrap().try_wait().is_none());
        remove_file_if_exists(&close)?;
        touch(&tmp_path.join("eviction.release"))?;
        wait_within(transition.take().unwrap(), Duration::from_secs(15)).await??;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let entries = debug_tenants(&tenants[1]).await?;
        assert!(!isolations(&entries)?.contains(&"crashing"));
        if operation == "clean" {
            assert!(!proxy_slot(&victim).join("buckd.info").exists());
            assert!(!victim.cwd().join("buck-out/crashing/cache").exists());
        } else {
            let owner = daemon_pid(&legacy).await?;
            assert_ne!(owner, daemon);
            assert_eq!(proxy_pid(&victim)?, owner);
        }
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        assert!(generation > 0);
        anyhow::Ok(())
    }
    .await;
    remove_file_if_exists(&close)?;
    touch(&tmp_path.join("eviction.release"))?;
    finish(transition).await;
    let killed = kill_all([&legacy, &victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

cases! { [buck, tmp_path] unidaemon_loss_recovery_respects_legacy_slot_owner {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_loss_recovery_respects_legacy_slot_owner_clean("clean");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_loss_recovery_respects_legacy_slot_owner_startup("startup");
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_proxy_death_before_forwarding_kill(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "crashing");
    let gate = tmp_path.join("before-forward");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(&mut all, "BUCK2_TEST_PROXY_BEFORE_EVICT", "crashing", &gate);
    }
    let mut killing = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        victim.build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&victim).await?;
        let pid = proxy_pid(&victim)?;
        let legacy = legacy_buck(&victim, &legacy_executable(buck, tmp_path)?)?;
        killing = Some(legacy.kill().spawn());
        wait_for_path(&tmp_path.join("before-forward.started")).await?;
        kill_process(pid, Signal::Kill)?;
        let _outcome = wait_within(killing.take().unwrap(), Duration::from_secs(10)).await?;
        wait_for_path(
            &proxy_slot(&victim)
                .parent()
                .unwrap()
                .join("unidaemon-events.jsonl"),
        )
        .await?;
        tokio::time::timeout(Duration::from_secs(3), async {
            while isolations(&debug_tenants(&tenants[1]).await?)?.contains(&"crashing") {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            anyhow::Ok(())
        })
        .await
        .context("The tenant was not retired")??;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("before-forward.release"))?;
    finish(killing).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_later_eof_does_not_extend_eviction_deadline(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "crashing");
    let gate = tmp_path.join("eviction");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "crashing",
            &gate,
        );
    }
    let mut daemon = None;
    let result = async {
        tenants[1].build(&["//:tenant_value"]).await?;
        victim.build(&["//:tenant_value"]).await?;
        let pid = daemon_pid(&victim).await?;
        daemon = Some(pid);
        let generation = tenant_generation_via(&tenants[1], &victim).await?;
        victim
            .debug(&[
                "tenants",
                "--evict-generation",
                &generation.to_string(),
                "--eviction-timeout-ms",
                "2000",
                "--no-wait",
            ])
            .await?;
        // The deadline counts from the eviction's start in the daemon, not from the client
        // command that asked for it, which can take a while to start under load.
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        let start = Instant::now();
        tokio::time::sleep(Duration::from_millis(700)).await;
        kill_process(proxy_pid(&victim)?, Signal::Kill)?;
        wait_for_terminated(pid, Duration::from_secs(3)).await?;
        assert!(
            start.elapsed() < Duration::from_millis(3500),
            "EOF extended the original eviction deadline"
        );
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("eviction.release"))?;
    if let Some(daemon) = daemon
        && !process_terminated(daemon)
    {
        let _ignored = kill_process(daemon, Signal::Kill);
    }
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

async fn unidaemon_migrates_same_slot_between_configurations(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let config = tenants[0].cwd().join(".buckconfig");
    let original = std::fs::read_to_string(&config)?;
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let first = daemon_pid(&tenants[1]).await?;
        std::fs::write(
            &config,
            format!("{original}\n[buck2]\nshare_dice = {}\n", !share_dice),
        )?;
        tenants[0].build(&["//:tenant_value"]).await?;
        let second = daemon_pid(&tenants[0]).await?;
        assert_ne!(first, second);
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, first);
        std::fs::write(&config, &original)?;
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, first);
        assert_daemon_exits(second).await;
        assert_eq!(namespace_keys(tmp_path)?.len(), 1);
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&config, &original)?;
    let killed = kill_all(&tenants).await;
    result?;
    killed
}

bool_cases! { [buck, tmp_path] unidaemon_migrates_same_slot_between_configurations {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_migrates_same_slot_between_configurations_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_migrates_same_slot_between_configurations_True(true);
}}

async fn unidaemon_registration_waits_for_clean(
    buck: &Buck,
    background: bool,
) -> anyhow::Result<()> {
    let tmp_path = buck.cwd().parent().unwrap().join("sharing");
    std::fs::create_dir(&tmp_path)?;
    let mut tenants = unidaemon_checkouts(buck, &tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "evicting");
    let gate = tmp_path.join("eviction");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "evicting",
            &gate,
        );
    }
    let mut cleaning = None;
    let mut creating = None;
    let result = async {
        for tenant in [&victim, &tenants[1]] {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let daemon = daemon_pid(&victim).await?;
        let sentinel = victim.cwd().join("buck-out/evicting/preserved");
        std::fs::write(&sentinel, "old generation")?;
        let clean_args: &[&str] = if background { &["--background"] } else { &[] };
        cleaning = Some(victim.clean(clean_args).spawn());
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        creating = Some(victim.build(&["//:tenant_value"]).spawn());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(cleaning.as_mut().unwrap().try_wait().is_none());
        assert!(creating.as_mut().unwrap().try_wait().is_none());
        assert!(sentinel.exists());
        touch(&tmp_path.join("eviction.release"))?;
        wait_within(cleaning.take().unwrap(), Duration::from_secs(10)).await??;
        wait_within(creating.take().unwrap(), Duration::from_secs(10)).await??;
        assert!(!sentinel.exists());
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("eviction.release"))?;
    finish(cleaning).await;
    finish(creating).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

bool_cases! { [buck] unidaemon_registration_waits_for_clean {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_registration_waits_for_clean_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_registration_waits_for_clean_True(true);
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_stale_eviction_cannot_remove_replacement(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let old = tenant_generation_via(&tenants[0], &tenants[0]).await?;
        tenants[0].kill().await?;
        tenants[0].build(&["//:tenant_value"]).await?;
        let current = tenant_generation_via(&tenants[0], &tenants[0]).await?;
        assert!(current > old);
        let result = tenants[0]
            .debug(&["tenants", "--evict-generation", &old.to_string()])
            .await?;
        let evicted: Value = serde_json::from_str(&result.stdout)?;
        assert_eq!(evicted["generation"].as_u64(), Some(old));
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(
            tenant_generation_via(&tenants[0], &tenants[0]).await?,
            current
        );
        anyhow::Ok(())
    }
    .await;
    let killed = kill_all(&tenants).await;
    result?;
    killed
}

async fn unidaemon_eviction_fallback_terminates_writers(
    buck: &Buck,
    failure: &str,
) -> anyhow::Result<()> {
    let tmp_path = buck.cwd().parent().unwrap().join("sharing");
    std::fs::create_dir(&tmp_path)?;
    let mut tenants = unidaemon_checkouts(buck, &tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "evicting");
    let gate = tmp_path.join("eviction");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "evicting",
            &gate,
        );
    }
    tenants[1].build(&["//:tenant_value"]).await?;
    victim.build(&["//:tenant_value"]).await?;
    let daemon = daemon_pid(&victim).await?;
    let proxy = proxy_pid(&victim)?;
    // The fallback under test is the proxy's: it serves clients that kill through the
    // tenant's own daemon directory, as a released client does and as a client of this release
    // does with its longer grace.
    let compatible = failure == "compatible-unresponsive-daemon";
    let unresponsive = compatible || failure == "unresponsive-daemon";
    let cleaner = if compatible {
        victim.clone()
    } else {
        legacy_buck(&victim, &legacy_executable(buck, &tmp_path)?)?
    };
    let cleaner = &cleaner;
    // A released client gives the proxy four seconds; this release's gives it eleven.
    let grace = Duration::from_secs(if compatible { 11 } else { 4 });
    let mut writer = Some(start_generation(&tenants[1], &tmp_path, "old")?);
    let mut cleaning = None;
    let result = async {
        wait_for_marker(&tmp_path.join("old.started"), &mut writer).await?;
        let writer_pid = pid_file(&tmp_path.join("old.started.pid"))?;
        if unresponsive {
            kill_process(daemon, Signal::Stop)?;
        }
        let start = Instant::now();
        cleaning = Some(cleaner.clean(&[]).spawn());
        if !unresponsive {
            tokio::time::timeout(Duration::from_secs(3), async {
                while !tmp_path.join("eviction.started").exists() {
                    if cleaning.as_mut().unwrap().try_wait().is_some() {
                        cleaning.take().unwrap().wait().await?;
                        anyhow::bail!("Clean completed without arming eviction");
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                anyhow::Ok(())
            })
            .await
            .context("Clean did not arm the eviction")??;
        }
        if failure == "proxy-death" {
            kill_process(proxy, Signal::Kill)?;
        }
        let outcome = tokio::time::timeout(
            grace + Duration::from_secs(4),
            cleaning.take().unwrap().wait(),
        )
        .await;
        let Ok(outcome) = outcome else {
            eprintln!(
                "daemon_alive={} proxy_alive={} writer_alive={} root={}",
                daemon_is_alive(daemon),
                daemon_is_alive(proxy),
                daemon_is_alive(writer_pid),
                tmp_path.display()
            );
            if let Ok(events) = unidaemon_events(&victim) {
                eprintln!("{events:#?}");
            }
            anyhow::bail!(
                "Clean did not finish within {:?}",
                grace + Duration::from_secs(4)
            );
        };
        outcome?;
        let elapsed = start.elapsed();
        assert!(
            elapsed < grace + Duration::from_secs(6),
            "Clean did not finish: {elapsed:?}"
        );
        if failure != "proxy-death" {
            assert!(
                process_terminated(writer_pid),
                "Clean returned with a live output writer"
            );
        }
        assert_daemon_exits(daemon).await;
        wait_for_terminated(writer_pid, Duration::from_secs(3)).await?;
        finish(writer.take()).await;
        assert!(!proxy_slot(&victim).join("buckd.info").exists());
        assert!(!victim.cwd().join("buck-out/evicting/cache").exists());
        let events = unidaemon_events(&victim)?;
        assert!(has_event(&events, "eviction-start", None));
        if failure != "proxy-death" {
            let exits: Vec<&Value> = events
                .iter()
                .filter(|row| {
                    matches!(
                        row["event"].as_str(),
                        Some("forced-daemon-exit" | "daemon-exit")
                    )
                })
                .collect();
            assert!(
                !exits.is_empty(),
                "Missing verified daemon termination telemetry"
            );
            let within = grace.as_millis() as u64;
            assert!(
                exits
                    .iter()
                    .all(|row| row["elapsed_ms"].as_u64().is_some_and(|ms| ms < within)),
                "{exits:?}"
            );
        }
        touch(&tmp_path.join("eviction.release"))?;
        for tenant in [&victim, &tenants[1]] {
            tenant.build(&["//:tenant_value"]).await?;
            assert_ne!(daemon_pid(tenant).await?, daemon);
        }
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("eviction.release"))?;
    touch(&tmp_path.join("old.release"))?;
    if daemon_is_alive(daemon) {
        let _ignored = kill_process(daemon, Signal::Continue);
    }
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    if let Some(writer) = writer.take() {
        let _ignored = writer.kill().await;
    }
    if let Some(cleaning) = cleaning.take() {
        let _ignored = cleaning.kill().await;
    }
    result?;
    killed
}

cases! { [buck] unidaemon_eviction_fallback_terminates_writers {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_eviction_fallback_terminates_writers_deadline("deadline");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_eviction_fallback_terminates_writers_proxy_death("proxy-death");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_eviction_fallback_terminates_writers_unresponsive_daemon("unresponsive-daemon");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_eviction_fallback_terminates_writers_compatible_unresponsive_daemon("compatible-unresponsive-daemon");
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_legacy_client_build_kill_clean_and_return(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let executable = legacy_executable(buck, tmp_path)?;
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let legacy = legacy_buck(&tenants[0], &executable)?;
    let result = async {
        for tenant in &tenants {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let daemon = daemon_pid(&tenants[1]).await?;
        legacy.kill().await?;
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        tenants[0].build(&["//:tenant_value"]).await?;
        legacy.clean(&[]).await?;
        assert!(!tenants[0].cwd().join("buck-out/tenant/cache").exists());
        tenants[0].build(&["//:tenant_value"]).await?;
        legacy.build(&["//:tenant_value"]).await?;
        let legacy_pid = daemon_pid(&legacy).await?;
        assert_ne!(legacy_pid, daemon);
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[0]).await?, daemon);
        assert!(!daemon_is_alive(legacy_pid));
        tenants[1].build(&["//:tenant_value"]).await?;
        anyhow::Ok(())
    }
    .await;
    let killed = kill_all([&legacy, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

async fn unidaemon_final_tenant_can_restart_immediately(
    buck: &Buck,
    tmp_path: &Path,
    operation: &str,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        let old = daemon_pid(&tenant).await?;
        if operation == "kill" {
            tenant.kill().await?;
        } else {
            tenant.clean(&[]).await?;
        }
        assert!(process_terminated(old));
        tenant.build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_pid(&tenant).await?, old);
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

cases! { [buck, tmp_path] unidaemon_final_tenant_can_restart_immediately {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_final_tenant_can_restart_immediately_kill("kill");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_final_tenant_can_restart_immediately_clean("clean");
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_failed_proxy_publication_releases_generation(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let victim = isolated_buck(&tenants[0], "failed");
    tenants[1].build(&["//:tenant_value"]).await?;
    let daemon = daemon_pid(&tenants[1]).await?;
    let slot = proxy_slot(&victim);
    std::fs::create_dir_all(&slot)?;
    let spec = slot.join("proxy-spec.json");
    std::fs::create_dir(&spec)?;
    let result = async {
        victim
            .build(&["//:tenant_value"])
            .await
            .expect_failure()
            .assert_stderr_matches("Launching tenant proxy");
        assert!(!slot.join("buckd.info").exists());
        let entries = debug_tenants(&tenants[1]).await?;
        assert!(!isolations(&entries)?.contains(&"failed"));
        std::fs::remove_dir(&spec)?;
        victim.build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&victim).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    if spec.is_dir() {
        std::fs::remove_dir(&spec)?;
    }
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_repo_killall_preserves_other_checkout(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let sibling = isolated_buck(&tenants[0], "sibling");
    let result = async {
        for tenant in [&tenants[0], &tenants[1], &sibling] {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let daemon = daemon_pid(&tenants[1]).await?;
        tenants[0].command(&["killall", "--repo"]).await?;
        for tenant in [&tenants[0], &sibling] {
            assert!(!proxy_slot(tenant).join("buckd.info").exists());
        }
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    let killed = kill_all([&tenants[0], &tenants[1], &sibling]).await;
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_concurrent_first_commands_share_a_generation(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let result = async {
        let builds: Vec<BuckProcess> = (0..4)
            .map(|_| tenant.build(&["//:tenant_value"]).spawn())
            .collect();
        for outcome in futures::future::join_all(builds.into_iter().map(BuckProcess::wait)).await {
            outcome?;
        }
        tenant.build(&["//:tenant_value"]).await?;
        let entries = debug_tenants(&tenant).await?;
        assert_eq!(entries.len(), 1, "{entries:?}");
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

/// A client of the same release stops its tenant through the tenant's proxy, as a released
/// client does, with its own longer budget: the command returns once the eviction has
/// completed, and the proxy has removed its connection files.
async fn unidaemon_compatible_client_evicts_through_proxy(
    buck: &Buck,
    tmp_path: &Path,
    operation: &str,
    share_dice: bool,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let mut victim = isolated_buck(&tenants[0], "direct");
    let gate = tmp_path.join("eviction");
    let forwarding = tmp_path.join("proxy-forward");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            "direct",
            &gate,
        );
        set_gate(
            &mut all,
            "BUCK2_TEST_PROXY_BEFORE_EVICT",
            "direct",
            &forwarding,
        );
    }
    let mut writer = None;
    let mut retiring = None;
    let result = async {
        for tenant in [&tenants[1], &victim] {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let daemon = daemon_pid(&victim).await?;
        let slot = proxy_slot(&victim);
        let generation = tenant_generation_via(&tenants[1], &victim).await?;
        let proxy = proxy_pid(&victim)?;
        let inode = file_id(&slot.join("buckd.lifecycle"))?;
        writer = Some(start_generation(&victim, tmp_path, "old")?);
        wait_for_marker(&tmp_path.join("old.started"), &mut writer).await?;
        let writer_pid = pid_file(&tmp_path.join("old.started.pid"))?;
        let sentinel = victim.cwd().join("buck-out/direct/preserved");
        std::fs::write(&sentinel, "old generation")?;
        retiring = Some(match operation {
            "kill" => victim.kill().spawn(),
            "clean" => victim.clean(&[]).spawn(),
            _ => victim.clean(&["--background"]).spawn(),
        });
        // The proxy receives the Kill and forwards it as an eviction of its generation.
        wait_for_path(&tmp_path.join("proxy-forward.started")).await?;
        assert!(retiring.as_mut().unwrap().try_wait().is_none());
        touch(&tmp_path.join("proxy-forward.release"))?;
        wait_for_path(&tmp_path.join("eviction.started")).await?;
        assert!(retiring.as_mut().unwrap().try_wait().is_none());
        assert!(!process_terminated(proxy));
        assert!(sentinel.exists());
        touch(&tmp_path.join("eviction.release"))?;
        let result = wait_within(retiring.take().unwrap(), Duration::from_secs(15)).await??;
        assert!(result.stderr.contains("Evicted tenant generation"));
        assert!(process_terminated(writer_pid));
        assert!(process_terminated(proxy));
        finish(writer.take()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!slot.join("buckd.info").exists());
        assert!(!slot.join("buckd.pid").exists());
        assert!(!slot.join("proxy-spec.json").exists());
        assert_eq!(file_id(&slot.join("buckd.lifecycle"))?, inode);
        assert_eq!(sentinel.exists(), operation == "kill");
        assert!(has_event(
            &unidaemon_events(&victim)?,
            "eviction-start",
            Some(generation)
        ));
        victim.build(&["//:tenant_value"]).await?;
        assert!(tenant_generation_via(&tenants[1], &victim).await? > generation);
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("eviction.release"))?;
    touch(&tmp_path.join("proxy-forward.release"))?;
    touch(&tmp_path.join("old.release"))?;
    finish(writer).await;
    finish(retiring).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

cases! { [buck, tmp_path] unidaemon_compatible_client_evicts_through_proxy {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_compatible_client_evicts_through_proxy_False_kill("kill", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_compatible_client_evicts_through_proxy_False_clean("clean", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_compatible_client_evicts_through_proxy_False_background_clean("background-clean", false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_compatible_client_evicts_through_proxy_True_kill("kill", true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_compatible_client_evicts_through_proxy_True_clean("clean", true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_compatible_client_evicts_through_proxy_True_background_clean("background-clean", true);
}}

async fn unidaemon_incompatible_client_evicts_through_proxy(
    buck: &Buck,
    tmp_path: &Path,
    operation: &str,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let mut victim = isolated_buck(&tenants[0], "incompatible");
    let forwarding = tmp_path.join("proxy-forward");
    {
        let [one, two] = tenants.as_mut_slice() else {
            unreachable!()
        };
        let mut all = [one, two, &mut victim];
        set_gate(
            &mut all,
            "BUCK2_TEST_PROXY_BEFORE_EVICT",
            "incompatible",
            &forwarding,
        );
    }
    let config = victim.cwd().join(".buckconfig");
    let original = std::fs::read_to_string(&config)?;
    let mut retiring = None;
    let result = async {
        for tenant in [&tenants[1], &victim] {
            tenant.build(&["//:tenant_value"]).await?;
        }
        let daemon = daemon_pid(&victim).await?;
        std::fs::write(&config, format!("{original}\n[buck2]\nshare_dice = true\n"))?;
        retiring = Some(if operation == "kill" {
            victim.kill().spawn()
        } else {
            victim.clean(&[]).spawn()
        });
        wait_for_path(&tmp_path.join("proxy-forward.started")).await?;
        assert!(retiring.as_mut().unwrap().try_wait().is_none());
        touch(&tmp_path.join("proxy-forward.release"))?;
        wait_within(retiring.take().unwrap(), Duration::from_secs(10)).await??;
        assert_eq!(daemon_pid(&tenants[1]).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&config, &original)?;
    touch(&tmp_path.join("proxy-forward.release"))?;
    finish(retiring).await;
    let killed = kill_all([&victim, &tenants[0], &tenants[1]]).await;
    result?;
    killed
}

cases! { [buck, tmp_path] unidaemon_incompatible_client_evicts_through_proxy {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_incompatible_client_evicts_through_proxy_kill("kill");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_incompatible_client_evicts_through_proxy_clean("clean");
}}

async fn daemon_id(tenant: &Buck) -> anyhow::Result<String> {
    let status = status_json(tenant).await?;
    Ok(status["daemon_constraints"]["daemon_id"]
        .as_str()
        .context("No `daemon_constraints.daemon_id`")?
        .to_owned())
}

/// The compatibility namespace serving `tenant`, as `debug daemon-dir` resolves it.
async fn namespace_key(tenant: &Buck) -> anyhow::Result<String> {
    let daemon_dir = tenant.daemon_dir().await?;
    let namespace = daemon_dir.parent().context("the daemon dir has a parent")?;
    anyhow::ensure!(
        namespace.parent().and_then(Path::file_name) == Some(".unidaemons".as_ref()),
        "{} is not in a namespace",
        daemon_dir.display()
    );
    Ok(namespace
        .file_name()
        .context("the namespace has a name")?
        .to_string_lossy()
        .into_owned())
}

/// The namespace, daemon and generation serving `tenant`, as its proxy reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TenantRecord {
    compatibility_key: String,
    daemon_id: String,
    generation: u64,
}

async fn tenant_record(tenant: &Buck) -> anyhow::Result<TenantRecord> {
    Ok(TenantRecord {
        compatibility_key: namespace_key(tenant).await?,
        daemon_id: daemon_id(tenant).await?,
        generation: tenant_generation_via(tenant, tenant).await?,
    })
}

fn forkserver_pid(status: &Value) -> anyhow::Result<u32> {
    Ok(u32::try_from(
        status["forkserver_pid"]
            .as_u64()
            .context("No `forkserver_pid`")?,
    )?)
}

/// Process-wide corruption, a dead forkserver here, replaces the shared daemon. A corrupt
/// materializer database belongs to one tenant: that tenant's generation is replaced in
/// the running daemon while the other tenant's work goes on. A generation that cannot
/// drain within the eviction deadline brings the daemon's watchdog down on the whole
/// daemon.
async fn unidaemon_corruption_recovery_scope(
    buck: &Buck,
    failure: &str,
    share_dice: bool,
    trace_io: bool,
) -> anyhow::Result<()> {
    let tmp_path = buck.cwd().parent().unwrap().join("corruption");
    std::fs::create_dir(&tmp_path)?;
    let mut tenants = unidaemon_checkouts(buck, &tmp_path, share_dice)?;
    let gate = tmp_path.join("eviction");
    for tenant in &mut tenants {
        let config = tenant.cwd().join(".buckconfig");
        std::fs::write(
            &config,
            std::fs::read_to_string(&config)?.replace("execution_platforms = root//:platforms", "")
                + "\n[buck2]\nrestarter = true\nforkserver = true\n",
        )?;
        std::fs::write(tenant.cwd().join("restart-source"), "materializer recovery")?;
        let targets = tenant.cwd().join("TARGETS.fixture");
        std::fs::write(
            &targets,
            std::fs::read_to_string(&targets)?
                + "\nload(\":defs.bzl\", \"restart_copy\")\n\
                   restart_copy(name = \"restart_seed\", src = \"restart-source\")\n\
                   restart_copy(name = \"restart_producer\", src = \":restart_seed\")\n\
                   restart_copy(name = \"restart_consumer\", src = \":restart_producer\", local_only = True)\n",
        )?;
        tenant.set_env(
            "BUCK2_TEST_UNIDAEMON_EVICTION_GATE",
            format!("tenant,{}", gate.display()),
        );
    }
    let mut writer = None;
    let mut rebuilding = None;
    let result = async {
        if trace_io {
            for tenant in &tenants {
                tenant.debug(&["trace-io", "enable"]).await?;
            }
        }
        let mut outputs = Vec::new();
        for tenant in &tenants {
            let result = tenant.build(&["//:restart_producer"]).await?;
            outputs.push(
                result
                    .build_report()?
                    .output_for_target("//:restart_producer")?,
            );
        }
        let status = status_json(&tenants[0]).await?;
        let daemon = pid_of(&status)?;
        let forkserver = forkserver_pid(&status)?;
        let mut owners = Vec::new();
        for tenant in &tenants {
            owners.push(tenant_record(tenant).await?);
        }
        let proxies = [proxy_pid(&tenants[0])?, proxy_pid(&tenants[1])?];
        let identities = [
            materializer_identity(&tenants[0], "tenant")?,
            materializer_identity(&tenants[1], "tenant")?,
        ];
        assert_ne!(identities[0], identities[1]);
        assert_eq!(
            forkserver_pid(&status_json(&tenants[1]).await?)?,
            forkserver
        );
        if failure == "forkserver" {
            kill_process(forkserver, Signal::Kill)?;
            wait_for_terminated(forkserver, Duration::from_secs(3)).await?;
        } else {
            writer = Some(start_generation(&tenants[0], &tmp_path, "old")?);
            wait_for_marker(&tmp_path.join("old.started"), &mut writer).await?;
            remove_output(&outputs[1])?;
        }
        rebuilding = Some(tenants[1].build(&["//:restart_consumer"]).spawn());
        tokio::time::timeout(Duration::from_secs(10), async {
            while !tmp_path.join("eviction.started").exists() {
                if rebuilding.as_mut().unwrap().try_wait().is_some() {
                    let result = rebuilding.take().unwrap().wait().await?;
                    anyhow::bail!(
                        "Build completed without corruption recovery: {}",
                        result.stderr
                    );
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            anyhow::Ok(())
        })
        .await
        .context("The eviction did not start")??;
        assert!(rebuilding.as_mut().unwrap().try_wait().is_none());
        assert!(!process_terminated(daemon));
        if failure == "materializer-deadline" {
            // The eviction never drains, so the daemon's watchdog stops the daemon under the
            // command that asked for the recovery; the next command gets a fresh daemon.
            let outcome = wait_within(rebuilding.take().unwrap(), THIRTY_SECONDS).await?;
            assert!(outcome.is_err(), "{outcome:?}");
            wait_for_terminated(daemon, Duration::from_secs(3)).await?;
            for proxy in proxies {
                wait_for_terminated(proxy, Duration::from_secs(3)).await?;
            }
            // The database is still the corrupt one; the next command that needs the
            // missing output recovers it in a fresh daemon, with the eviction left to drain.
            touch(&tmp_path.join("eviction.release"))?;
            let result = tenants[1].build(&["//:restart_consumer"]).await?;
            assert!(result.stderr.contains("Your command will now restart"));
            assert_ne!(materializer_identity(&tenants[1], "tenant")?, identities[1]);
            assert_ne!(daemon_pid(&tenants[1]).await?, daemon);
            return anyhow::Ok(());
        }
        touch(&tmp_path.join("eviction.release"))?;
        let result = wait_within(rebuilding.take().unwrap(), Duration::from_secs(20)).await??;
        assert!(result.stderr.contains("Your command will now restart"));
        let replacement = tenant_record(&tenants[1]).await?;
        assert_eq!(replacement.compatibility_key, owners[1].compatibility_key);
        if failure == "forkserver" {
            assert!(result.stderr.contains("Restarting rejected unidaemon"));
            assert!(process_terminated(daemon));
            for proxy in proxies {
                wait_for_terminated(proxy, Duration::from_secs(3)).await?;
            }
            assert_ne!(replacement.daemon_id, owners[1].daemon_id);
            tenants[0].build(&["//:tenant_value"]).await?;
            assert_eq!(materializer_identity(&tenants[0], "tenant")?, identities[0]);
            let replacement_status = status_json(&tenants[0]).await?;
            assert_ne!(pid_of(&replacement_status)?, daemon);
            assert_ne!(forkserver_pid(&replacement_status)?, forkserver);
            let other = status_json(&tenants[1]).await?;
            if trace_io {
                // The client that replaced the daemon settled on its proxy while the proxy
                // still stood for the traced daemon, so its replacement traces too. The other
                // tenant's proxy died with the daemon, so with no preference of its own it
                // continues without tracing, in the namespace of that state.
                assert_ne!(pid_of(&other)?, pid_of(&replacement_status)?);
                assert!(truthy(
                    &other["daemon_constraints"]["extra"]["trace_io_enabled"]
                ));
                assert!(!truthy(
                    &replacement_status["daemon_constraints"]["extra"]["trace_io_enabled"]
                ));
            } else {
                assert_eq!(pid_of(&other)?, pid_of(&replacement_status)?);
            }
        } else {
            assert!(!result.stderr.contains("Restarting rejected unidaemon"));
            assert!(!process_terminated(daemon));
            // Only the corrupt tenant's generation went, with its proxy; the other tenant's
            // action kept running.
            wait_for_terminated(proxies[1], Duration::from_secs(3)).await?;
            assert!(!process_terminated(proxies[0]));
            assert!(!process_terminated(pid_file(
                &tmp_path.join("old.started.pid")
            )?));
            assert_eq!(replacement.daemon_id, owners[1].daemon_id);
            assert!(replacement.generation > owners[1].generation);
            assert_ne!(materializer_identity(&tenants[1], "tenant")?, identities[1]);
            touch(&tmp_path.join("old.release"))?;
            finish(writer.take()).await;
            tenants[0].build(&["//:tenant_value"]).await?;
            assert_eq!(materializer_identity(&tenants[0], "tenant")?, identities[0]);
            let status = status_json(&tenants[0]).await?;
            assert_eq!(pid_of(&status)?, daemon);
            assert_eq!(forkserver_pid(&status)?, forkserver);
        }
        anyhow::Ok(())
    }
    .await;
    touch(&tmp_path.join("eviction.release"))?;
    touch(&tmp_path.join("old.release"))?;
    finish(writer).await;
    finish(rebuilding).await;
    let killed = kill_all(&tenants).await;
    result?;
    killed
}

cases! { [buck] unidaemon_corruption_recovery_scope {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_False_False_forkserver("forkserver", false, false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_False_False_materializer("materializer", false, false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_False_False_materializer_deadline("materializer-deadline", false, false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_False_True_forkserver("forkserver", true, false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_False_True_materializer("materializer", true, false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_False_True_materializer_deadline("materializer-deadline", true, false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_True_False_forkserver("forkserver", false, true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_True_False_materializer("materializer", false, true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_True_False_materializer_deadline("materializer-deadline", false, true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_True_True_forkserver("forkserver", true, true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_True_True_materializer("materializer", true, true);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    #[expect(non_snake_case, reason = "named like the Python harness's parametrized case")]
    fn test_unidaemon_corruption_recovery_scope_True_True_materializer_deadline("materializer-deadline", true, true);
}}

async fn unidaemon_client_without_trace_preference_follows_its_proxy(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    let result = async {
        tenants[0].debug(&["trace-io", "enable"]).await?;
        let traced = daemon_id(&tenants[0]).await?;
        let traced_key = namespace_key(&tenants[0]).await?;
        // A command of the same tenant with no preference follows its proxy into the traced
        // namespace instead of hashing to the untraced one.
        tenants[0].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_id(&tenants[0]).await?, traced);
        assert_eq!(namespace_key(&tenants[0]).await?, traced_key);
        let status = status_json(&tenants[0]).await?;
        assert!(truthy(
            &status["daemon_constraints"]["extra"]["trace_io_enabled"]
        ));
        // A tenant with no proxy to follow starts without tracing, in its own namespace.
        assert!(!proxy_slot(&tenants[1]).join("buckd.info").exists());
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_id(&tenants[1]).await?, traced);
        assert_ne!(namespace_key(&tenants[1]).await?, traced_key);
        let status = status_json(&tenants[1]).await?;
        assert!(!truthy(
            &status["daemon_constraints"]["extra"]["trace_io_enabled"]
        ));
        anyhow::Ok(())
    }
    .await;
    let killed = kill_all(&tenants).await;
    result?;
    killed
}

bool_cases! { [buck, tmp_path] unidaemon_client_without_trace_preference_follows_its_proxy {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    fn test_unidaemon_client_without_trace_preference_follows_its_proxy_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], allow_soft_errors = true)]
    fn test_unidaemon_client_without_trace_preference_follows_its_proxy_True(true);
}}

async fn unidaemon_nested_client_inherits_parent_user_version(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, share_dice)?;
    tenants[0].set_env("SANDCASTLE_ID", "parent-job");
    tenants[1].set_env("SANDCASTLE_ID", "nested-job");
    let config = tenants[1].cwd().join(".buckconfig");
    let original = std::fs::read_to_string(&config)?;
    let parent_config = tenants[0].cwd().join(".buckconfig");
    let parent_original = std::fs::read_to_string(&parent_config)?;
    let result = async {
        tenants[0].build(&["//:tenant_value"]).await?;
        let parent = daemon_id(&tenants[0]).await?;
        let parent_key = namespace_key(&tenants[0]).await?;
        // What the parent's daemon exports to the actions it runs.
        tenants[1].set_env("BUCK2_DAEMON_UUID", &parent);
        tenants[1].set_env("BUCK2_DAEMON_USER_VERSION", "parent-job");
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_eq!(daemon_id(&tenants[1]).await?, parent);
        assert_eq!(namespace_key(&tenants[1]).await?, parent_key);
        // Still nested, a client with its own user version must not move the tenant.
        tenants[1].remove_env("BUCK2_DAEMON_USER_VERSION");
        tenants[1]
            .build(&["//:tenant_value"])
            .await
            .expect_failure()
            .assert_stderr_matches(
                "constraint mismatch during nested invocation: User version mismatch",
            );
        // Outside any action, the client's own user version selects another namespace.
        tenants[1].remove_env("BUCK2_DAEMON_UUID");
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_id(&tenants[1]).await?, parent);
        assert_ne!(namespace_key(&tenants[1]).await?, parent_key);
        // Nested in a tenant the parent's daemon does not serve, an incompatible client gets
        // a daemon of its own.
        tenants[1].set_env("BUCK2_DAEMON_UUID", &parent);
        tenants[1].set_env("BUCK2_DAEMON_USER_VERSION", "parent-job");
        std::fs::write(
            &config,
            format!("{original}\n[buck2]\nshare_dice = {}\n", !share_dice),
        )?;
        tenants[1].build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_id(&tenants[1]).await?, parent);
        // Nested in the parent's own tenant, it must not replace the parent's daemon.
        tenants[0].set_env("BUCK2_DAEMON_UUID", &parent);
        tenants[0].set_env("BUCK2_DAEMON_USER_VERSION", "parent-job");
        std::fs::write(
            &parent_config,
            format!("{parent_original}\n[buck2]\nshare_dice = {}\n", !share_dice),
        )?;
        let nested = tenants[0].build(&["//:tenant_value"]).await;
        std::fs::write(&parent_config, &parent_original)?;
        tenants[0].remove_env("BUCK2_DAEMON_UUID");
        tenants[0].remove_env("BUCK2_DAEMON_USER_VERSION");
        nested
            .expect_failure()
            .assert_stderr_matches("constraint mismatch during nested invocation");
        assert_eq!(daemon_id(&tenants[0]).await?, parent);
        anyhow::Ok(())
    }
    .await;
    std::fs::write(&config, &original)?;
    let killed = kill_all(&tenants).await;
    result?;
    killed
}

bool_cases! { [buck, tmp_path] unidaemon_nested_client_inherits_parent_user_version {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_nested_client_inherits_parent_user_version_False(false);
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_nested_client_inherits_parent_user_version_True(true);
}}

/// A sleeping process with a pid of its own, killed when dropped.
fn sleeper() -> anyhow::Result<(tokio::process::Child, u32)> {
    let child = tokio::process::Command::new("sleep")
        .arg("1000")
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id().context("The process has no pid")?;
    Ok((child, pid))
}

/// The start time of the process `pid`, as buck2 records it in `buckd.info`.
fn process_start_time(pid: u32) -> anyhow::Result<u64> {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new()?;
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    Ok(system
        .process(pid)
        .with_context(|| format!("No process {pid}"))?
        .start_time())
}

async fn exited_process_pid() -> anyhow::Result<u32> {
    let mut process = tokio::process::Command::new("true").spawn()?;
    let pid = process.id().context("The process has no pid")?;
    assert!(process.wait().await?.success());
    assert!(!daemon_is_alive(pid));
    Ok(pid)
}

fn namespace_record(pid: u32, pid_start_time: u64) -> String {
    serde_json::json!({
        "pid": pid,
        "endpoint": "tcp:127.0.0.1:0",
        "version": "other",
        "auth_token": "",
        "pid_start_time": pid_start_time,
    })
    .to_string()
}

async fn unidaemon_exited_namespace_restarts_whatever_its_metadata(
    buck: &Buck,
    tmp_path: &Path,
    leftover: &str,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let (mut unrelated, unrelated_pid) = sleeper()?;
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        let pid = daemon_pid(&tenant).await?;
        let namespace = only_unidaemon_namespace(tmp_path)?;
        // Only a daemon that dies leaves its metadata behind; one that exits removes it.
        kill_process(pid, Signal::Kill)?;
        wait_for_terminated(pid, Duration::from_secs(3)).await?;
        wait_for_missing_path(&proxy_slot(&tenant).join("buckd.info")).await?;
        if leftover == "reused-pid" {
            // A process took the exited daemon's pid: same pid, a start time an hour apart.
            let info_path = namespace.join("buckd.info");
            let mut info: Value = serde_json::from_str(&std::fs::read_to_string(&info_path)?)?;
            info["pid"] = unrelated_pid.into();
            info["pid_start_time"] = (info["pid_start_time"].as_u64().unwrap() - 3600).into();
            std::fs::write(&info_path, serde_json::to_string(&info)?)?;
            std::fs::write(namespace.join("buckd.pid"), unrelated_pid.to_string())?;
        } else {
            // A daemon that exited before writing its pid and connection records.
            std::fs::remove_file(namespace.join("buckd.info"))?;
            std::fs::remove_file(namespace.join("buckd.pid"))?;
            touch(&namespace.join("buckd.stdout"))?;
        }
        tenant.build(&["//:tenant_value"]).await?;
        assert_ne!(daemon_pid(&tenant).await?, pid);
        assert!(
            unrelated.try_wait()?.is_none(),
            "the unrelated process was left alone"
        );
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

cases! { [buck, tmp_path] unidaemon_exited_namespace_restarts_whatever_its_metadata {
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_exited_namespace_restarts_whatever_its_metadata_reused_pid("reused-pid");
    #[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
    fn test_unidaemon_exited_namespace_restarts_whatever_its_metadata_logs_only("logs-only");
}}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_live_namespace_with_damaged_metadata_is_left_alone(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let mut tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    for tenant in &mut tenants {
        // Keep the daemon from noticing its removed `buckd.pid` and exiting mid-test.
        tenant.set_env("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "600");
    }
    let mut pid = None;
    let result = async {
        tenants[0].build(&["//:tenant_value"]).await?;
        let daemon = daemon_pid(&tenants[0]).await?;
        pid = Some(daemon);
        let namespace = only_unidaemon_namespace(tmp_path)?;
        for name in ["buckd.info", "buckd.pid"] {
            std::fs::remove_file(namespace.join(name))?;
        }
        tenants[1]
            .build(&["//:tenant_value"])
            .await
            .expect_failure()
            .assert_stderr_matches("connection records are missing");
        assert!(daemon_is_alive(daemon), "the running daemon was left alone");
        assert!(
            !namespace.join("buckd.info").exists(),
            "no second daemon was started in the namespace"
        );
        anyhow::Ok(())
    }
    .await;
    let killed = kill_all(&tenants).await;
    if let Some(pid) = pid
        && daemon_is_alive(pid)
    {
        let _ignored = kill_process(pid, Signal::Kill);
    }
    result?;
    killed
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_pruning_spares_a_live_daemon_without_the_lifetime_lock(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let sibling = tmp_path
        .join("home")
        .join(".buck")
        .join("buckd")
        .join(".unidaemons")
        .join("0".repeat(64))
        .join("v2");
    std::fs::create_dir_all(&sibling)?;
    // A running daemon of a release that does not take the lifetime lock.
    let (_other_release, other_pid) = sleeper()?;
    let result = async {
        std::fs::write(
            sibling.join("buckd.info"),
            namespace_record(other_pid, process_start_time(other_pid)?),
        )?;
        std::fs::write(sibling.join("buckd.pid"), other_pid.to_string())?;
        tenant.build(&["//:tenant_value"]).await?;
        tenant.build(&["//:tenant_value"]).await?;
        assert!(
            sibling.join("buckd.info").exists(),
            "the live namespace was left alone"
        );
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_damaged_sibling_namespace_does_not_fail_commands(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let sibling = tmp_path
        .join("home")
        .join(".buck")
        .join("buckd")
        .join(".unidaemons")
        .join("0".repeat(64))
        .join("v2");
    std::fs::create_dir_all(&sibling)?;
    // Records of an exited daemon, with its `buckd.pid` lost.
    std::fs::write(
        sibling.join("buckd.info"),
        namespace_record(exited_process_pid().await?, 1),
    )?;
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        tenant.build(&["//:tenant_value"]).await?;
        assert!(!sibling.exists(), "the exited sibling namespace was pruned");
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_unidaemon_exited_daemon_removes_its_namespace(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let locks = tmp_path
        .join("home")
        .join(".buck")
        .join("buckd")
        .join(".unidaemon_locks");
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        let namespace = only_unidaemon_namespace(tmp_path)?
            .parent()
            .unwrap()
            .to_path_buf();
        let lock = locks.join(format!(
            "{}.alive",
            namespace.file_name().unwrap().to_string_lossy()
        ));
        assert!(lock.exists());
        let pid = daemon_pid(&tenant).await?;
        tenant.kill().await?;
        assert_daemon_exits(pid).await;
        assert!(
            !namespace.exists(),
            "the exited daemon removed its namespace"
        );
        assert!(lock.exists(), "lock files stay");
        tenant.build(&["//:tenant_value"]).await?;
        assert_eq!(
            only_unidaemon_namespace(tmp_path)?.parent().unwrap(),
            namespace
        );
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_commands_without_daemon_routing_work_under_sharing(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        // These commands never select a daemon, so they skip the routing; none of them may
        // depend on it.
        tenant.root(&[]).await?;
        tenant.root(&["--kind", "project"]).await?;
        tenant.log(&["last"]).await?;
        tenant.log(&["what-ran"]).await?;
        tenant.debug(&["exe"]).await?;
        tenant.debug(&["internal-version"]).await?;
        tenant.help_env(&[]).await?;
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_root_reports_shared_daemon_dir(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        let root = PathBuf::from(tenant.root(&["--kind", "daemon"]).await?.stdout.trim());
        assert_eq!(root, tenant.daemon_dir().await?);
        assert!(root.iter().any(|part| part == ".unidaemons"));
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], share_daemon = true)]
async fn test_killall_repo_reports_damaged_slot_metadata(buck: &Buck) -> anyhow::Result<()> {
    let tenant = isolated_buck(buck, "tenant-one");
    let result = async {
        tenant.targets(&["//:rule"]).await?;
        let slots = proxy_slot(&tenant)
            .parent()
            .context("The proxy slot has no parent")?
            .to_path_buf();
        // A pid of a process that exited, as a daemon that died while starting leaves, is
        // debris; a live one without a record is an owner `killall` cannot reach.
        let exited = slots.join("exited");
        std::fs::create_dir(&exited)?;
        std::fs::write(exited.join("buckd.pid"), "999999999")?;
        let live = slots.join("live");
        std::fs::create_dir(&live)?;
        std::fs::write(live.join("buckd.pid"), std::process::id().to_string())?;
        let failure = buck.command(&["killall", "--repo"]).await.expect_failure();
        failure.assert_stderr_matches("Repository tenant eviction failed for 1 tenant");
        assert!(
            failure
                .stderr
                .contains(&format!("`{}`: its owner is running", live.display())),
            "{}",
            failure.stderr
        );
        assert!(!failure.stderr.contains(path_str(&exited)));
        assert!(failure.stderr.contains("Evicted 1 tenant(s)"));
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], share_daemon = true)]
async fn test_killall_repo_evicts_remaining_tenants_after_a_failure(
    buck: &Buck,
) -> anyhow::Result<()> {
    let tenants = ["tenant-one", "tenant-two"].map(|name| isolated_buck(buck, name));
    let mut lock = None;
    let result = async {
        for tenant in &tenants {
            tenant.targets(&["//:rule"]).await?;
        }
        let path = proxy_slot(&tenants[0]).join("buckd.lifecycle");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0))?;
        lock = Some(path);
        let failure = buck.command(&["killall", "--repo"]).await.expect_failure();
        failure.assert_stderr_matches("Repository tenant eviction failed for 1 tenant");
        assert!(failure.stderr.contains("Evicted 1 tenant(s)"));
        let registered = debug_tenants(&tenants[0]).await?;
        assert_eq!(isolations(&registered)?, ["tenant-one"]);
        anyhow::Ok(())
    }
    .await;
    if let Some(lock) = &lock {
        std::fs::set_permissions(lock, std::fs::Permissions::from_mode(0o644))?;
    }
    let killed = kill_all(&tenants).await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_shared_forkserver_state_goes_with_its_daemon(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let root = tenant
        .cwd()
        .join("buck-out")
        .join("._buck2")
        .join("forkserver");
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        let key = namespace_name(tmp_path)?;
        assert!(root.join(&key).is_dir());
        let entries: Vec<String> = std::fs::read_dir(&root)?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<anyhow::Result<_>>()?;
        assert_eq!(entries, [key.clone()]);
        let pid = daemon_pid(&tenant).await?;
        tenant.kill().await?;
        assert_daemon_exits(pid).await;
        assert!(
            !root.join(&key).exists(),
            "the exited daemon removed its forkserver state"
        );
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_shared_daemon_leaves_forkserver_state_of_other_namespaces(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenant = unidaemon_checkouts(buck, tmp_path, false)?.swap_remove(0);
    let root = tenant
        .cwd()
        .join("buck-out")
        .join("._buck2")
        .join("forkserver");
    // Another namespace's state, whose daemon may run under another home.
    let other = root.join("0".repeat(64));
    std::fs::create_dir_all(&other)?;
    std::fs::write(other.join("miniperf"), "another daemon's")?;
    let result = async {
        tenant.build(&["//:tenant_value"]).await?;
        let key = namespace_name(tmp_path)?;
        assert!(root.join(&key).is_dir());
        assert!(
            other.join("miniperf").exists(),
            "the starting daemon left another namespace's state alone"
        );
        anyhow::Ok(())
    }
    .await;
    let killed = tenant.kill().await;
    result?;
    killed?;
    Ok(())
}

/// The name of the one namespace under the test's home: its compatibility key.
fn namespace_name(tmp_path: &Path) -> anyhow::Result<String> {
    Ok(only_unidaemon_namespace(tmp_path)?
        .parent()
        .context("the namespace's `v2` has a parent")?
        .file_name()
        .context("the namespace has a name")?
        .to_string_lossy()
        .into_owned())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"], share_daemon = true)]
async fn test_checkout_buckconfig_local_opts_out_of_external_sharing(
    buck: &Buck,
) -> anyhow::Result<()> {
    // The rollout turns sharing on from a user-level config file (the GK experiments file); a
    // checkout's `.buckconfig.local` must still be able to turn it off.
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2]\nshare_daemon = false\n",
    )?;
    buck.targets(&["//:rule"]).await?;
    let root = PathBuf::from(buck.root(&["--kind", "daemon"]).await?.stdout.trim());
    assert!(
        !root.iter().any(|part| part == ".unidaemons"),
        "{}",
        root.display()
    );
    assert_eq!(root, buck.daemon_dir().await?);
    Ok(())
}
