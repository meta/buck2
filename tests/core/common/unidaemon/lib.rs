/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Helpers shared by the daemon test suites that exercise daemon sharing.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::Buck;
use buck2_e2e::BuckProcess;
use buck2_e2e::process::wait_for_exit;
use serde_json::Value;

pub const THIRTY_SECONDS: Duration = Duration::from_secs(30);

/// A test for each case of a parametrized Python test, named as the Python harness named
/// the case, running the test's body with the test's fixture (`[buck]`, `[mut buck]`,
/// `[buck, tmp_path]` or `[mut buck, tmp_path]`) and the case's arguments.
#[macro_export]
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
#[macro_export]
macro_rules! bool_cases {
    ($fixture:tt $body:ident { $($(#[$attr:meta])* fn $name:ident($($arg:expr),*);)* }) => {
        $crate::cases! {
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

pub fn path_str(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// `status["process_info"]["pid"]` of a `buck2 status` or `buck2 server --status`.
pub fn pid_of(status: &Value) -> anyhow::Result<u32> {
    let pid = status
        .get("process_info")
        .and_then(|info| info.get("pid"))
        .and_then(Value::as_u64)
        .with_context(|| format!("No `process_info.pid` in {status}"))?;
    Ok(u32::try_from(pid)?)
}

pub async fn status_json(buck: &Buck) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&buck.status(&[]).await?.stdout)?)
}

pub async fn daemon_pid(buck: &Buck) -> anyhow::Result<u32> {
    pid_of(&status_json(buck).await?)
}

pub fn isolated_buck(buck: &Buck, isolation: &str) -> Buck {
    let mut isolated = buck.clone();
    isolated.set_isolation_prefix(isolation);
    isolated
}

/// The tenant's own daemon directory under the test's home: where a shared tenant's proxy
/// publishes its connection files.
pub fn proxy_slot(buck: &Buck) -> PathBuf {
    let home = buck
        .env_var("BUCK2_TEST_HOME_DIR")
        .expect("a shared tenant has a home of its own");
    let cwd = buck.cwd().to_string_lossy();
    Path::new(home)
        .join(".buck/buckd")
        .join(cwd.trim_start_matches('/'))
        .join(buck.isolation_prefix().unwrap_or("v2"))
}

/// The `v2` directory of the one namespace under the test's home.
pub fn only_unidaemon_namespace(tmp_path: &Path) -> anyhow::Result<PathBuf> {
    let namespaces: Vec<PathBuf> = std::fs::read_dir(
        tmp_path
            .join("home")
            .join(".buck")
            .join("buckd")
            .join(".unidaemons"),
    )?
    .map(|entry| Ok(entry?.path()))
    .collect::<anyhow::Result<_>>()?;
    let [namespace] = namespaces.as_slice() else {
        anyhow::bail!("Expected one namespace, found {namespaces:?}");
    };
    Ok(namespace.join("v2"))
}

/// Two checkouts of the test's project, `one` and `two`, whose clients share a daemon under a home
/// of their own.
pub fn unidaemon_checkouts(
    buck: &Buck,
    tmp_path: &Path,
    share_dice: bool,
) -> anyhow::Result<Vec<Buck>> {
    ["one", "two"]
        .iter()
        .map(|value| {
            let checkout = tmp_path.join(value);
            std::fs::create_dir(&checkout)?;
            for name in [".buckconfig", "TARGETS.fixture", "defs.bzl", "paths.bxl"] {
                std::fs::copy(buck.cwd().join(name), checkout.join(name))?;
            }
            std::fs::write(checkout.join(".buckroot"), "")?;
            let config = checkout.join(".buckconfig");
            let contents = std::fs::read_to_string(&config)?;
            std::fs::write(
                &config,
                format!(
                    "{contents}\n[buck2]\nshare_dice = {share_dice}\nfile_watcher = fs_hash_crawler\n\n[tenant]\nvalue = {value}\n"
                ),
            )?;
            let mut tenant = buck.clone();
            tenant.set_cwd(checkout);
            tenant.set_isolation_prefix("tenant");
            tenant.set_env("BUCK2_TEST_HOME_DIR", tmp_path.join("home"));
            tenant.set_daemon_routing(true)?;
            Ok(tenant)
        })
        .collect()
}

/// The entries of `buck2 debug tenants --json`.
pub async fn debug_tenants(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    Ok(serde_json::from_str(
        &buck.debug(&["tenants", "--json"]).await?.stdout,
    )?)
}

pub fn isolation_of(entry: &Value) -> anyhow::Result<&str> {
    entry
        .get("isolation")
        .and_then(Value::as_str)
        .with_context(|| format!("No `isolation` in {entry}"))
}

pub fn generation_of(entry: &Value) -> anyhow::Result<u64> {
    entry
        .get("generation")
        .and_then(Value::as_u64)
        .with_context(|| format!("No `generation` in {entry}"))
}

pub fn isolations(entries: &[Value]) -> anyhow::Result<Vec<&str>> {
    entries.iter().map(isolation_of).collect()
}

pub fn tenant_entry<'a>(entries: &'a [Value], isolation: &str) -> anyhow::Result<&'a Value> {
    for entry in entries {
        if isolation_of(entry)? == isolation {
            return Ok(entry);
        }
    }
    anyhow::bail!("No tenant `{isolation}` in {entries:?}")
}

/// The generation of the tenant `isolation`.
pub fn tenant_generation(entries: &[Value], isolation: &str) -> anyhow::Result<u64> {
    generation_of(tenant_entry(entries, isolation)?)
}

/// Waits up to 30 seconds for `path`, failing if `build` exits first.
pub async fn wait_for_marker(path: &Path, build: &mut Option<BuckProcess>) -> anyhow::Result<()> {
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

/// Starts a build of `//:generation` that writes `<value>.started` in `directory` and waits for
/// `<value>.release` there before it finishes.
pub fn start_generation(buck: &Buck, directory: &Path, value: &str) -> anyhow::Result<BuckProcess> {
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

pub async fn wait_for_evicted_generation(
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

/// The materializer state's own identity: what tells one `buck-out` initialization from another.
pub fn materializer_identity(
    buck: &Buck,
    isolation: &str,
) -> anyhow::Result<rusqlite::types::Value> {
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

pub async fn assert_daemon_exits(pid: u32) {
    assert!(
        wait_for_exit(pid, Duration::from_secs(10)).await,
        "Daemon survived final tenant eviction"
    );
}
