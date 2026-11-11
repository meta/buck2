/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Two checkouts of one project under each file watcher that works across checkouts, with
//! separate daemons, a shared daemon, and a shared daemon with shared DICE.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::BuckProcess;
use buck2_e2e::eden::Eden;
use buck2_e2e::prelude::*;
use buck2_e2e::process::Signal;
use buck2_e2e::process::send_signal;
use buck2_e2e::utils::filter_events;
use common_unidaemon::debug_tenants;
use common_unidaemon::generation_of;
use common_unidaemon::isolation_of;
use common_unidaemon::path_str;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonMode {
    SeparateDaemons,
    Unidaemon,
    SharedDice,
}

impl DaemonMode {
    fn unidaemon(self) -> bool {
        self != Self::SeparateDaemons
    }
}

/// The file watchers that serve two checkouts, with the number the event log reports them as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Watchman,
    EdenFs,
}

impl Provider {
    fn config_name(self) -> &'static str {
        match self {
            Self::Watchman => "watchman",
            Self::EdenFs => "edenfs",
        }
    }

    /// `FileWatcherProvider` in the event log.
    fn event_value(self) -> u64 {
        match self {
            Self::Watchman => 0,
            Self::EdenFs => 3,
        }
    }

    fn io_provider(self) -> &'static str {
        match self {
            Self::Watchman => "fs",
            Self::EdenFs => "eden",
        }
    }
}

/// The two checkouts, `one` in the test's own project and `two` in `peer` next to it.
struct CheckoutPair {
    tenants: Vec<Buck>,
    workspace: PathBuf,
    peer: PathBuf,
    provider: Provider,
}

impl CheckoutPair {
    async fn set_up(buck: &Buck, mode: DaemonMode, provider: Provider) -> anyhow::Result<Self> {
        let workspace = buck
            .cwd()
            .parent()
            .context("the project has a parent")?
            .to_path_buf();
        let peer = workspace.join("peer");
        if provider == Provider::EdenFs {
            Eden::of(buck)?.clone_checkout(&peer)?;
        } else {
            std::fs::create_dir(&peer)?;
        }
        for name in [
            ".buckconfig",
            ".buckroot",
            ".gitignore",
            ".watchmanconfig",
            "TARGETS.fixture",
            "defs.bzl",
            "src.txt",
        ] {
            std::fs::copy(buck.cwd().join(name), peer.join(name))?;
        }
        let override_config = workspace.join("smoke.bcfg");
        let current = PathBuf::from(
            buck.env_var("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG")
                .context("the test fixture sets `BUCK2_TEST_EXTRA_EXTERNAL_CONFIG`")?,
        );
        std::fs::write(
            &override_config,
            format!(
                "{}\n[buck2]\nfile_watcher = {}\nallow_eden_io = {}\nuse_eden_thrift_read = true\nshare_dice = {}\nshare_daemon = {}\n\n[smoke]\nstarted = {}\nrelease = {}\n",
                std::fs::read_to_string(&current)?,
                provider.config_name(),
                provider == Provider::EdenFs,
                mode == DaemonMode::SharedDice,
                mode.unidaemon(),
                workspace.join("writer.started").display(),
                workspace.join("writer.release").display(),
            ),
        )?;
        let mut tenants = Vec::new();
        for (root, value) in [(buck.cwd().to_path_buf(), "one"), (peer.clone(), "two")] {
            std::fs::write(root.join("src.txt"), value)?;
            if provider == Provider::Watchman {
                // A commit gives the checkout a dirstate, which the daemon reads for its
                // revision telemetry; a bare `sl init` has none, and the harness turns
                // that soft error into a daemon crash.
                for args in [
                    vec!["init", path_str(&root)],
                    vec!["commit", "--addremove", "-m", "init"],
                ] {
                    let status = tokio::process::Command::new("sl")
                        .args(&args)
                        .current_dir(&root)
                        .status()
                        .await?;
                    anyhow::ensure!(status.success(), "`sl {args:?}` failed: {status}");
                }
            }
            let mut tenant = buck.clone();
            tenant.set_cwd(&root);
            tenant.set_isolation_prefix("tenant");
            tenant.set_env("BUCK2_TEST_HOME_DIR", workspace.join("home"));
            tenant.set_env("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG", &override_config);
            tenants.push(tenant);
        }
        Ok(Self {
            tenants,
            workspace,
            peer,
            provider,
        })
    }

    /// Stops the tenants and unmounts the peer checkout, after the test body.
    async fn tear_down(self) -> anyhow::Result<()> {
        touch(&self.workspace.join("writer.release"))?;
        let mut killed = Ok(());
        for tenant in &self.tenants {
            if let Err(e) = tenant.kill().await {
                killed = Err(e.into());
            }
        }
        if self.provider == Provider::EdenFs && self.peer.exists() {
            Eden::of(&self.tenants[0])?.remove_checkout(&self.peer)?;
        }
        killed
    }
}

fn touch(path: &Path) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(())
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

async fn copy_value(
    tenant: &Buck,
    target: &str,
    value: &str,
    provider: Provider,
) -> anyhow::Result<PathBuf> {
    let result = tenant.build(&[target]).await?;
    let output = result.build_report()?.output_for_target(target)?;
    assert!(output.starts_with(tenant.cwd().join("buck-out")));
    assert_eq!(std::fs::read_to_string(&output)?, value);
    let providers = filter_events(
        tenant,
        &[
            "Event",
            "data",
            "SpanStart",
            "data",
            "FileWatcher",
            "provider",
        ],
    )
    .await?;
    assert!(
        !providers.is_empty()
            && providers
                .iter()
                .all(|actual| actual.as_u64() == Some(provider.event_value())),
        "{providers:?}"
    );
    Ok(output)
}

async fn status_pid(tenant: &Buck, provider: Provider) -> anyhow::Result<u32> {
    let result = tenant.status(&[]).await?;
    if result.stdout.is_empty() {
        let namespaces = PathBuf::from(tenant.env_var("BUCK2_TEST_HOME_DIR").unwrap())
            .join(".buck/buckd/.unidaemons");
        if let Ok(entries) = std::fs::read_dir(&namespaces) {
            for entry in entries.flatten() {
                if let Ok(log) = std::fs::read_to_string(entry.path().join("v2/buckd.stderr")) {
                    let tail = log.len().saturating_sub(12000);
                    eprintln!("{}", &log[tail..]);
                }
            }
        }
        anyhow::bail!("Status returned no daemon data: {}", result.stderr);
    }
    let status: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(status["io_provider"].as_str(), Some(provider.io_provider()));
    common_unidaemon::pid_of(&status)
}

fn proxy_slot(tenant: &Buck) -> PathBuf {
    PathBuf::from(tenant.env_var("BUCK2_TEST_HOME_DIR").unwrap())
        .join(".buck/buckd")
        .join(tenant.cwd().to_string_lossy().trim_start_matches('/'))
        .join("tenant")
}

/// The generation of `tenant` in the daemon, read through `via`, a client of the same daemon.
async fn tenant_generation(via: &Buck, tenant: &Buck) -> anyhow::Result<u64> {
    for entry in debug_tenants(via).await? {
        if entry["project_root"].as_str() == Some(path_str(tenant.cwd()))
            && isolation_of(&entry)? == tenant.isolation_prefix().unwrap_or("v2")
        {
            return generation_of(&entry);
        }
    }
    anyhow::bail!("{} has no tenant in the daemon", tenant.cwd().display())
}

fn proxy_pid(tenant: &Buck) -> Option<u32> {
    let info: Value =
        serde_json::from_str(&std::fs::read_to_string(proxy_slot(tenant).join("buckd.info")).ok()?)
            .ok()?;
    u32::try_from(info["pid"].as_u64()?).ok()
}

async fn analysis_keys(buck: &Buck, dump: &Path) -> anyhow::Result<BTreeSet<String>> {
    buck.debug(&["dice-dump", "--path", path_str(dump)]).await?;
    let mut nodes = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(dump.join("nodes.gz"))?)
        .read_to_string(&mut nodes)?;
    Ok(nodes
        .lines()
        .filter_map(|line| {
            let row: Vec<&str> = line.split('\t').collect();
            (row.len() == 3 && row[1] == "AnalysisKey").then(|| row[2].to_owned())
        })
        .collect())
}

async fn smoke_checkouts(pair: &CheckoutPair, mode: DaemonMode) -> anyhow::Result<()> {
    let provider = pair.provider;
    let [one, two] = pair.tenants.as_slice() else {
        unreachable!("two checkouts")
    };
    let targets = ["root//:one_probe", "root//:two_probe"];
    let outputs = [
        copy_value(one, targets[0], "one", provider).await?,
        copy_value(two, targets[1], "two", provider).await?,
    ];
    assert_ne!(outputs[0], outputs[1]);
    let pids = [
        status_pid(one, provider).await?,
        status_pid(two, provider).await?,
    ];
    assert_eq!(pids[0] == pids[1], mode.unidaemon());
    let analysis = analysis_keys(one, &pair.workspace.join("dice")).await?;
    assert!(analysis.iter().any(|key| key.contains("root//:one_probe")));
    assert_eq!(
        analysis.iter().any(|key| key.contains("root//:two_probe")),
        mode == DaemonMode::SharedDice
    );
    std::fs::write(one.cwd().join("src.txt"), "one-edited")?;
    copy_value(one, targets[0], "one-edited", provider).await?;
    copy_value(two, targets[1], "two", provider).await?;
    one.kill().await?;
    copy_value(two, targets[1], "two", provider).await?;
    assert_eq!(status_pid(two, provider).await?, pids[1]);
    copy_value(one, targets[0], "one-edited", provider).await?;
    if mode.unidaemon() {
        assert_eq!(status_pid(one, provider).await?, pids[0]);
    }
    let sentinel = one.cwd().join("buck-out/tenant/sentinel");
    std::fs::write(&sentinel, "old generation")?;
    one.clean(&[]).await?;
    assert!(!sentinel.exists());
    copy_value(two, targets[1], "two", provider).await?;
    assert_eq!(status_pid(two, provider).await?, pids[1]);
    copy_value(one, targets[0], "one-edited", provider).await?;
    if mode.unidaemon() {
        recover_lost_proxy(pair, pids[0]).await?;
    }
    Ok(())
}

async fn recover_lost_proxy(pair: &CheckoutPair, daemon: u32) -> anyhow::Result<()> {
    let provider = pair.provider;
    let [one, two] = pair.tenants.as_slice() else {
        unreachable!("two checkouts")
    };
    let old = tenant_generation(two, one).await?;
    let old_proxy = proxy_pid(one).context("the tenant has a proxy")?;
    let mut writer: Option<BuckProcess> =
        Some(one.build(&["root//:writer", "--no-remote-cache"]).spawn());
    let result = async {
        let started = pair.workspace.join("writer.started");
        tokio::time::timeout(Duration::from_secs(30), async {
            while !started.exists() {
                if writer.as_mut().unwrap().try_wait().is_some() {
                    writer.take().unwrap().wait().await?;
                    anyhow::bail!("Writer ended before publishing its PID");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            anyhow::Ok(())
        })
        .await
        .context("The writer did not start")??;
        let writer_pid: u32 = std::fs::read_to_string(&started)?.trim().parse()?;
        // A crash; a proxy stopped with `SIGKILL` or `SIGTERM` retires its tenant instead.
        send_signal(old_proxy, Signal::Abort)?;
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let proxy = proxy_pid(one).unwrap_or(old_proxy);
                if proxy != old_proxy && !process_terminated(proxy) {
                    assert!(tenant_generation(two, one).await? > old);
                    assert!(process_terminated(writer_pid));
                    return anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("The tenant's proxy was not replaced")??;
        assert_eq!(status_pid(two, provider).await?, daemon);
        copy_value(two, "root//:two_probe", "two", provider).await?;
        copy_value(one, "root//:one_probe", "one-edited", provider).await?;
        assert_eq!(status_pid(one, provider).await?, daemon);
        anyhow::Ok(())
    }
    .await;
    touch(&pair.workspace.join("writer.release"))?;
    if let Some(writer) = writer {
        let _ignored = writer.wait().await;
    }
    result
}

async fn two_checkouts(buck: &Buck, mode: DaemonMode, provider: Provider) -> anyhow::Result<()> {
    let pair = CheckoutPair::set_up(buck, mode, provider).await?;
    let result = smoke_checkouts(&pair, mode).await;
    let torn_down = pair.tear_down().await;
    result?;
    torn_down
}

#[buck_test(setup_eden = true, skip_for_os = ["windows", "darwin"])]
async fn test_two_eden_checkouts_separate_daemons(buck: &Buck) -> anyhow::Result<()> {
    two_checkouts(buck, DaemonMode::SeparateDaemons, Provider::EdenFs).await
}

#[buck_test(setup_eden = true, skip_for_os = ["windows", "darwin"])]
async fn test_two_eden_checkouts_unidaemon(buck: &Buck) -> anyhow::Result<()> {
    two_checkouts(buck, DaemonMode::Unidaemon, Provider::EdenFs).await
}

#[buck_test(setup_eden = true, skip_for_os = ["windows", "darwin"])]
async fn test_two_eden_checkouts_shared_dice(buck: &Buck) -> anyhow::Result<()> {
    two_checkouts(buck, DaemonMode::SharedDice, Provider::EdenFs).await
}

#[buck_test(skip_for_os = ["windows", "darwin"])]
async fn test_two_watchman_checkouts_separate_daemons(buck: &Buck) -> anyhow::Result<()> {
    two_checkouts(buck, DaemonMode::SeparateDaemons, Provider::Watchman).await
}

#[buck_test(skip_for_os = ["windows", "darwin"])]
async fn test_two_watchman_checkouts_unidaemon(buck: &Buck) -> anyhow::Result<()> {
    two_checkouts(buck, DaemonMode::Unidaemon, Provider::Watchman).await
}

#[buck_test(skip_for_os = ["windows", "darwin"])]
async fn test_two_watchman_checkouts_shared_dice(buck: &Buck) -> anyhow::Result<()> {
    two_checkouts(buck, DaemonMode::SharedDice, Provider::Watchman).await
}
