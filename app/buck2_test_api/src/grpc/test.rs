/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::assert_matches;
use std::time::Duration;

use buck2_error::BuckErrorContext as _;
use buck2_grpc::DuplexChannel;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;

use crate::data::ExternalRunnerSpec;
use crate::grpc::TestExecutorClient;
use crate::grpc::spawn_executor_server;
use crate::protocol::TestExecutor;

struct MockExecutor;

#[async_trait::async_trait]
impl TestExecutor for MockExecutor {
    async fn external_runner_spec(&self, _: ExternalRunnerSpec) -> buck2_error::Result<()> {
        Ok(())
    }

    async fn end_of_test_requests(&self) -> buck2_error::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_basic() -> buck2_error::Result<()> {
    let (client_io, server_io) = tokio::io::duplex(64);
    let server = spawn_executor_server(to_duplex_channel(server_io), MockExecutor);
    let client = TestExecutorClient::new(client_io)
        .await
        .buck_error_context("Failed to create client")?;

    client
        .end_of_test_requests()
        .await
        .buck_error_context("Call 1")?;
    client
        .end_of_test_requests()
        .await
        .buck_error_context("Call 2")?;
    client
        .end_of_test_requests()
        .await
        .buck_error_context("Call 3")?;

    server
        .shutdown()
        .await
        .buck_error_context("Failed to shutdown")?;

    assert_matches!(client.end_of_test_requests().await, Err(..));

    Ok(())
}

#[tokio::test]
async fn test_client_disconnect() -> buck2_error::Result<()> {
    let (client_io, server_io) = tokio::io::duplex(64);
    let mut server =
        spawn_executor_server(to_duplex_channel(server_io), MockExecutor).into_join_handle();

    assert_matches!(
        tokio::time::timeout(Duration::from_millis(10), &mut server).await,
        Err(..)
    );

    drop(client_io);

    assert_matches!(
        tokio::time::timeout(Duration::from_millis(10), &mut server).await,
        Ok(..)
    );

    Ok(())
}

fn to_duplex_channel(
    io: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
) -> impl AsyncRead + AsyncWrite + Send + Unpin + 'static + tonic::transport::server::Connected {
    let (read, write) = tokio::io::split(io);
    DuplexChannel::new(read, write)
}
