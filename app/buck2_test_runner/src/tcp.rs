/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::net::SocketAddr;

use buck2_error::BuckErrorContext;
use buck2_grpc::DuplexChannel;
use clap::Parser;
use futures::future::BoxFuture;
use tokio::net::TcpStream;

#[derive(Debug, Parser)]
pub struct Buck2TestRunnerTcp {
    #[clap(long)]
    executor_addr: String,

    #[clap(long)]
    orchestrator_addr: String,

    args: Vec<String>,
}

impl Buck2TestRunnerTcp {
    /// See `Buck2TestRunnerUnix::run` for why this is boxed and not inlinable.
    #[inline(never)]
    pub fn run(self) -> BoxFuture<'static, buck2_error::Result<()>> {
        Box::pin(async move {
            // NOTE: We can remove this code once Tokio supports UNIX domain socket or Named pipe
            // https://github.com/tokio-rs/tokio/issues/2201

            // NOTE: We assume the parameters we received from the caller are correct here. If
            // they're not, things are probably going to go wrong but that's on our caller.
            //
            // There are a few ways in which the params could be incorrect:
            // - The TCP socket addresses aren't valid
            // - The TCP socket don't exist
            // - The TCP socket aren't alive
            let orchestrator_addr: SocketAddr = self.orchestrator_addr.parse().map_err(|e| {
                buck2_error::internal_error!("Invalid orchestrator address {:#}", e)
            })?;
            let executor_addr: SocketAddr = self
                .executor_addr
                .parse()
                .map_err(|e| buck2_error::internal_error!("Invalid executor address {:#}", e))?;

            let orchestrator_io = TcpStream::connect(&orchestrator_addr)
                .await
                .buck_error_context("Failed to create orchestrator_io")?;

            let executor_io = TcpStream::connect(&executor_addr)
                .await
                .buck_error_context("Failed to create executor_io")?;

            let executor_io = {
                let (read, write) = tokio::io::split(executor_io);
                DuplexChannel::new(read, write)
            };

            crate::service::run(orchestrator_io, executor_io, self.args).await
        })
    }
}
