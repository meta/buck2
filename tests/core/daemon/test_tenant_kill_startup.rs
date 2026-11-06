/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;
use std::fs::File;
use std::fs::FileTimes;
use std::path::Path;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;
use buck2_e2e::process::daemon_is_alive;
use serde_json::Value;

async fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "Timed out after 5s");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn stop_process(process: Option<BuckProcess>) {
    if let Some(process) = process {
        let _ignored = tokio::time::timeout(Duration::from_secs(10), process.kill()).await;
    }
}

async fn status(buck: &Buck, args: &[&str]) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&buck.status(args).await?.stdout)?)
}

async fn tenant_isolations(buck: &Buck) -> anyhow::Result<BTreeSet<String>> {
    let tenants: Vec<Value> =
        serde_json::from_str(&buck.debug(&["tenants", "--json"]).await?.stdout)?;
    Ok(tenants
        .iter()
        .map(|tenant| tenant["isolation"].as_str().unwrap().to_owned())
        .collect())
}

fn modified_at_1ns(path: &Path) -> bool {
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|e| panic!("Reading the mtime of `{}`: {e}", path.display()));
    modified == SystemTime::UNIX_EPOCH + Duration::from_nanos(1)
}

#[buck_test(
    data_dir = "test_daemon_data",
    env = [("BUCK2_SHARE_DAEMON", "true")],
)]
async fn test_shared_kill_rechecks_owner_after_waiting_for_startup(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    buck.set_isolation_prefix("kill-race");
    let server_status: Value = serde_json::from_str(&buck.server(&["--status"]).await?.stdout)?;
    let startup_config = server_status["daemon_constraints"]["daemon_startup_config"]
        .as_str()
        .unwrap()
        .to_owned();
    let old_pid = u32::try_from(server_status["process_info"]["pid"].as_u64().unwrap())?;
    let daemon_dir = buck.daemon_dir().await?;
    buck.kill().await?;
    wait_until(|| !daemon_is_alive(old_pid)).await;
    let _ignored = std::fs::remove_file(daemon_dir.join("buckd.info"));
    let _ignored = std::fs::remove_file(daemon_dir.join("buckd.pid"));

    let mut pending_kill = None;
    let mut replacement = None;
    let buck = &*buck;
    let result = async {
        let lifecycle_path = daemon_dir.join("buckd.lifecycle");
        let lifecycle = File::options()
            .read(true)
            .append(true)
            .create(true)
            .open(&lifecycle_path)?;
        lifecycle.try_lock()?;
        let one_ns = SystemTime::UNIX_EPOCH + Duration::from_nanos(1);
        lifecycle.set_times(FileTimes::new().set_accessed(one_ns).set_modified(one_ns))?;
        let kill = pending_kill.insert(buck.kill().spawn());
        // File::create updates mtime before the client attempts the lock.
        wait_until(|| !modified_at_1ns(&lifecycle_path)).await;
        assert!(kill.try_wait().is_none());
        let daemon_id = uuid::Uuid::new_v4().to_string();
        let started = replacement.insert(
            buck.command(&[
                "daemon",
                "--dont-daemonize",
                "--daemon-id",
                &daemon_id,
                &startup_config,
            ])
            .spawn(),
        );
        wait_until(|| daemon_dir.join("buckd.info").exists()).await;
        let mut sibling = buck.clone();
        sibling.set_isolation_prefix("sibling");
        let replacement_status: Value =
            serde_json::from_str(&sibling.server(&["--status"]).await?.stdout)?;
        let replacement_pid =
            u32::try_from(replacement_status["process_info"]["pid"].as_u64().unwrap())?;
        assert_eq!(Some(replacement_pid), started.id());
        assert_ne!(replacement_pid, old_pid);
        sibling.targets(&["//:"]).await?;
        assert_eq!(
            tenant_isolations(&sibling).await?,
            BTreeSet::from(["kill-race".to_owned(), "sibling".to_owned()])
        );
        assert!(kill.try_wait().is_none());
        drop(lifecycle);

        let kill = pending_kill.take().unwrap();
        let killed = tokio::time::timeout(Duration::from_secs(15), kill.wait()).await?;
        let killed = match killed {
            Ok(output) => output,
            Err(error) => error.into_output(),
        };
        assert_ne!(killed.exit_code(), Some(0), "{}", killed.stderr);
        assert!(
            !killed.stderr.contains("Error locking buckd.lifecycle"),
            "{}",
            killed.stderr
        );
        assert!(replacement.as_mut().unwrap().try_wait().is_none());
        assert!(daemon_is_alive(replacement_pid));
        assert_eq!(
            status(&sibling, &[]).await?["process_info"]["pid"],
            replacement_pid
        );
        assert_eq!(
            tenant_isolations(&sibling).await?,
            BTreeSet::from(["kill-race".to_owned(), "sibling".to_owned()])
        );
        anyhow::Ok(())
    }
    .await;
    stop_process(pending_kill).await;
    stop_process(replacement).await;
    result
}
