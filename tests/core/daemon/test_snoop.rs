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
use std::process::Stdio;

use anyhow::Context;
use buck2_e2e::prelude::*;
use common_unidaemon::THIRTY_SECONDS;
use common_unidaemon::start_generation;
use common_unidaemon::unidaemon_checkouts;
use common_unidaemon::wait_for_marker;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::sync::mpsc;

struct Snoop {
    _child: tokio::process::Child,
    lines: mpsc::UnboundedReceiver<String>,
    seen: Vec<String>,
}

impl Snoop {
    fn start(buck: &Buck) -> anyhow::Result<Self> {
        let mut command = tokio::process::Command::new(buck.executable());
        if let Some(isolation) = buck.isolation_prefix() {
            command.args(["--isolation-dir", isolation]);
        }
        let mut child = command
            .args(["log", "snoop", "--console", "simple"])
            .current_dir(buck.cwd())
            .env_clear()
            .envs(buck.envs())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stderr = child.stderr.take().context("stderr is piped")?;
        let (sender, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut stderr = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = stderr.next_line().await {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            _child: child,
            lines,
            seen: Vec::new(),
        })
    }

    async fn wait_for(&mut self, needle: &str) -> anyhow::Result<()> {
        let found = tokio::time::timeout(THIRTY_SECONDS, async {
            while let Some(line) = self.lines.recv().await {
                let found = line.contains(needle);
                self.seen.push(line);
                if found {
                    return true;
                }
            }
            false
        })
        .await;
        match found {
            Ok(true) => Ok(()),
            Ok(false) => {
                anyhow::bail!("snoop exited without printing `{needle}`: {:#?}", self.seen)
            }
            Err(_) => anyhow::bail!("snoop did not print `{needle}`: {:#?}", self.seen),
        }
    }
}

async fn snoop_one_build(
    snoop: &mut Snoop,
    buck: &Buck,
    tmp_path: &Path,
    value: &str,
) -> anyhow::Result<String> {
    let mut build = Some(start_generation(buck, tmp_path, value)?);
    wait_for_marker(&tmp_path.join(format!("{value}.started")), &mut build).await?;
    snoop.wait_for("Snooping").await?;
    std::fs::write(tmp_path.join(format!("{value}.release")), "")?;
    let build_id = build
        .take()
        .context("the build is running")?
        .wait()
        .await?
        .buck_build_id()?;
    snoop
        .wait_for(&format!("Finished snooping {build_id}"))
        .await?;
    Ok(build_id)
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_snoop_follows_running_command(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.server(&[]).await?;
    let mut snoop = Snoop::start(buck)?;
    snoop.wait_for("Waiting for one to start").await?;
    snoop_one_build(&mut snoop, buck, tmp_path, "first").await?;
    snoop_one_build(&mut snoop, buck, tmp_path, "second").await?;
    Ok(())
}

#[buck_test(data_dir = "test_daemon_data", skip_for_os = ["windows", "darwin"])]
async fn test_snoop_follows_its_tenant_of_a_shared_daemon(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tenants = unidaemon_checkouts(buck, tmp_path, false)?;
    let result = async {
        for tenant in &tenants {
            tenant.server(&[]).await?;
        }
        let mut snoop = Snoop::start(&tenants[0])?;
        snoop.wait_for("Waiting for one to start").await?;

        let mut other = Some(start_generation(&tenants[1], tmp_path, "other")?);
        wait_for_marker(&tmp_path.join("other.started"), &mut other).await?;

        snoop_one_build(&mut snoop, &tenants[0], tmp_path, "own").await?;

        std::fs::write(tmp_path.join("other.release"), "")?;
        let other_id = other
            .take()
            .context("the build is running")?
            .wait()
            .await?
            .buck_build_id()?;
        assert!(
            !snoop.seen.iter().any(|line| line.contains(&other_id)),
            "snoop followed another tenant's command: {:#?}",
            snoop.seen
        );
        anyhow::Ok(())
    }
    .await;
    for tenant in &tenants {
        tenant.kill().await?;
    }
    result
}
