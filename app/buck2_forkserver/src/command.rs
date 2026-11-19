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
use std::sync::Arc;

use buck2_core::logging::LogConfigurationReloadHandle;
use buck2_error::BuckErrorContext;
use buck2_forkserver_proto::forkserver_server;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_grpc::DuplexChannel;
use tokio::net::UnixListener;
use tokio::net::UnixStream;

use super::service::UnixForkserverService;

pub async fn run_forkserver(
    fd: Option<RawFd>,
    socket_path: Option<String>,
    log_reload_handle: Arc<dyn LogConfigurationReloadHandle>,
    state_dir: AbsNormPathBuf,
) -> buck2_error::Result<()> {
    let io = match (fd, socket_path) {
        (Some(fd), None) => {
            // SAFETY: At worst, we just read (or close) the wrong FD.
            UnixStream::from_std(unsafe { StdUnixStream::from_raw_fd(fd) })
                .expect("Failed to create io")
        }
        (None, Some(socket_path)) => {
            let listener =
                UnixListener::bind(socket_path).buck_error_context("Failed to bind unix socket")?;
            let (stream, _addr) = listener
                .accept()
                .await
                .buck_error_context("Failed to accept unix socket")?;
            stream
        }
        _ => {
            unreachable!("Either fd or socket_path must be provided");
        }
    };

    let io = {
        let (read, write) = tokio::io::split(io);
        DuplexChannel::new(read, write)
    };

    let service = UnixForkserverService::new(log_reload_handle, &state_dir)
        .buck_error_context("Failed to create UnixForkserverService")?;

    let router = buck2_grpc::server_builder().add_service(
        forkserver_server::ForkserverServer::new(service)
            .max_encoding_message_size(usize::MAX)
            .max_decoding_message_size(usize::MAX),
    );

    buck2_grpc::spawn_oneshot(io, router)
        .into_join_handle()
        .await??;

    Ok(())
}
