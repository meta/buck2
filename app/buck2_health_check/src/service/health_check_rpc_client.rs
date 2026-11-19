/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(dead_code)]

use std::net::Ipv4Addr;
use std::time::Duration;

use buck2_common::client_utils::get_channel_tcp;
use buck2_common::client_utils::retrying;
use buck2_core::async_once_cell::AsyncOnceCell;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::conversion::from_any_with_tag;
use buck2_fs::async_fs_util;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_health_check_proto::health_check_client::HealthCheckClient;
use buck2_util::properly_reaped_child::ProperlyReapedChild;
use buck2_util::properly_reaped_child::reap_on_drop_command;
use dupe::Dupe;
use futures::FutureExt;
use tonic::transport::Channel;

use crate::interface::HealthCheckContextEvent;
use crate::interface::HealthCheckService;
use crate::interface::HealthCheckSnapshotData;
use crate::report::Report;

const CLI_NAME: &str = "buck2-health-check";
const CLI_INFO_FILE: &str = "buck2-health-check-server-info";

struct HealthCheckServerConnection {
    pub(crate) rpc_client: HealthCheckClient<Channel>,
    // The process will be reaped when the client is dropped.
    pub(crate) process: ProperlyReapedChild,
}

pub(crate) struct HealthCheckRpcClient {
    // The connection is lazily created when the first event to update the context or to run checks is received.
    connection: AsyncOnceCell<buck2_error::Result<HealthCheckServerConnection>>,
    health_check_dir: AbsNormPathBuf,
}

impl HealthCheckRpcClient {
    pub fn new(health_check_dir: AbsNormPathBuf) -> Self {
        Self {
            connection: AsyncOnceCell::new(),
            health_check_dir,
        }
    }

    async fn connection(&self) -> buck2_error::Result<&HealthCheckServerConnection> {
        let init_fut = async move {
            let file_path = self.get_state_info_file_path().await?;
            Self::spawn_out_of_process_health_check_server(file_path)
                .boxed()
                .await
        };
        match self.connection.get_or_init(init_fut).await {
            Ok(v) => Ok(v),
            Err(e) => Err(e.dupe()),
        }
    }

    async fn rpc_client(&self) -> buck2_error::Result<HealthCheckClient<Channel>> {
        Ok(self.connection().await?.rpc_client.clone())
    }

    async fn spawn_out_of_process_health_check_server(
        file_path: AbsNormPathBuf,
    ) -> buck2_error::Result<HealthCheckServerConnection> {
        let initial_delay = Duration::from_millis(10);
        let max_delay = Duration::from_millis(100);
        let timeout = Duration::from_secs(20);

        let exe = Self::get_cli_path()?;
        let process =
            reap_on_drop_command(&exe, &["--state-info-file", &file_path.to_string()], None)?;

        let tcp_port = retrying(initial_delay, max_delay, timeout, || async {
            // Wait for the health check server to write the TCP port to the file.
            async_fs_util::read_to_string(&file_path)
                .await
                .categorize_internal()
        })
        .await
        .buck_error_context("Failed to find TCP socket from health check server")?;

        let tcp_port = tcp_port.trim().parse::<u16>()?;

        let channel = retrying(initial_delay, max_delay, timeout, || async {
            get_channel_tcp(Ipv4Addr::LOCALHOST, tcp_port).await
        })
        .await
        .buck_error_context("Failed to connect to the health check server using TCP")?;

        let rpc_client = HealthCheckClient::new(channel)
            .max_encoding_message_size(usize::MAX)
            .max_decoding_message_size(usize::MAX);
        Ok(HealthCheckServerConnection {
            rpc_client,
            process,
        })
    }

    fn get_cli_path() -> buck2_error::Result<String> {
        if let Ok(overridden_path) = std::env::var("BUCK2_HEALTH_CHECK_CLI_PATH") {
            // If there is a custom CLI path, use it. This is useful for testing new health checks and CLI changes.
            return Ok(overridden_path);
        }

        let exe = AbsPathBuf::new(
            std::env::current_exe().buck_error_context("Cannot get Buck2 executable")?,
        )?;
        let exe = fs_util::canonicalize(&exe)
            .categorize_internal()
            .buck_error_context(
                "Failed to canonicalize path to Buck2 executable. Try running `buck2 kill`.",
            )?;

        let exe = exe.as_abs_path();
        let exe_dir = exe
            .parent()
            .internal_error("Buck2 executable directory has no parent")?;

        let ext = if cfg!(windows) { ".exe" } else { "" };
        let cli_name = format!("{CLI_NAME}{ext}");

        Ok(exe_dir.join(cli_name).to_string())
    }

    async fn get_state_info_file_path(&self) -> buck2_error::Result<AbsNormPathBuf> {
        async fn create_state_info_file_path(
            dir: &AbsNormPathBuf,
        ) -> buck2_error::Result<AbsNormPathBuf> {
            async_fs_util::create_dir_all(dir).await?;
            let state_info_file = dir.join(ForwardRelativePath::unchecked_new(CLI_INFO_FILE));
            if fs_util::try_exists(&state_info_file)? {
                // Clean up any state info file from previous build.
                fs_util::remove_file(&state_info_file).categorize_internal()?;
            }
            Ok(state_info_file)
        }

        match std::env::var("BUCK2_HEALTH_CHECK_STATE_INFO_PATH") {
            Err(_) => create_state_info_file_path(&self.health_check_dir).await,
            Ok(path) => AbsNormPathBuf::try_from(path),
        }
    }
}

#[async_trait::async_trait]
impl HealthCheckService for HealthCheckRpcClient {
    async fn update_context(&mut self, event: HealthCheckContextEvent) -> buck2_error::Result<()> {
        let rpc_event: buck2_health_check_proto::HealthCheckContextEvent = event.try_into()?;
        self.rpc_client()
            .await?
            .update_context(rpc_event)
            .await
            .map_err(|e| from_any_with_tag(e, ErrorTag::HealthCheck))?;
        Ok(())
    }

    async fn run_checks(
        &mut self,
        snapshot: HealthCheckSnapshotData,
    ) -> buck2_error::Result<Vec<Report>> {
        let snapshot: buck2_health_check_proto::HealthCheckSnapshotData = snapshot.try_into()?;
        let mut reports = Vec::new();

        let response = self
            .rpc_client()
            .await?
            .run_checks(snapshot)
            .await
            .map_err(|e| from_any_with_tag(e, ErrorTag::HealthCheck))?;

        let result = response.into_inner();
        for report in result.reports {
            match Report::try_from(report) {
                Ok(report) => reports.push(report),
                Err(e) => {
                    tracing::debug!("Failed to parse health check report: {e:?}");
                }
            }
        }
        Ok(reports)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    async fn temp_state_info(
        temp_dir: &TempDir,
    ) -> buck2_error::Result<(AbsNormPathBuf, AbsNormPathBuf)> {
        let temp_path = AbsNormPathBuf::try_from(temp_dir.path().to_path_buf()).unwrap();
        Ok((
            temp_path.clone(),
            temp_path.join(ForwardRelativePath::unchecked_new(CLI_INFO_FILE)),
        ))
    }

    #[tokio::test]
    async fn test_server_state_reset() -> buck2_error::Result<()> {
        let temp_dir_guard = tempfile::tempdir()?;
        let (dir, file) = temp_state_info(&temp_dir_guard).await?;

        // Write some content representing a previous health check server state.
        async_fs_util::write(&file, "TestContent")
            .await
            .uncategorized()?;

        let client = HealthCheckRpcClient::new(dir.clone());
        client.get_state_info_file_path().await.unwrap();

        // Ensure that the file is deleted but the directory exists.
        assert!(fs_util::try_exists(&file).is_ok_and(|exists| !exists));
        assert!(dir.is_dir());
        drop(temp_dir_guard);
        Ok(())
    }
}
