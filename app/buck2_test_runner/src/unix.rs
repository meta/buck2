/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::os::unix::io::FromRawFd;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream as StdUnixStream;

use buck2_error::BuckErrorContext;
use buck2_grpc::DuplexChannel;
use clap::Parser;
use futures::future::BoxFuture;
use tokio::net::UnixStream;

#[derive(Debug, Parser)]
pub struct Buck2TestRunnerUnix {
    #[clap(long)]
    executor_fd: RawFd,

    #[clap(long)]
    orchestrator_fd: RawFd,

    args: Vec<String>,
}

impl Buck2TestRunnerUnix {
    /// Boxed and not inlinable so that the test runner service, which is generic over the
    /// stream types, is instantiated here rather than in the crate that awaits this.
    #[inline(never)]
    pub fn run(self) -> BoxFuture<'static, buck2_error::Result<()>> {
        Box::pin(async move {
            // NOTE: We assume the parameters we received from the caller are correct here. If
            // they're not, things are probably going to go wrong but that's on our caller.
            //
            // There are a few ways in which the params could be incorrect:
            // - The FDs don't exist
            // - The FDs aren't streams
            // - The FDs are the same
            //
            // In all those cases though, the unsafety below is going to result in bad file
            // descriptors at worse, which is basically the best we can do anyway.
            let orchestrator_io =
                UnixStream::from_std(unsafe { StdUnixStream::from_raw_fd(self.orchestrator_fd) })
                    .buck_error_context("Failed to create orchestrator_io")?;

            let executor_io =
                UnixStream::from_std(unsafe { StdUnixStream::from_raw_fd(self.executor_fd) })
                    .buck_error_context("Failed to create executor_io")?;

            let executor_io = {
                let (read, write) = tokio::io::split(executor_io);
                DuplexChannel::new(read, write)
            };

            crate::service::run(orchestrator_io, executor_io, self.args).await
        })
    }
}
