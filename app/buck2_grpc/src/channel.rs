/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use buck2_error::BuckErrorContext as _;
use futures::future;
use pin_project::pin_project;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::ReadBuf;
use tonic::transport::Channel;
use tonic::transport::Endpoint;
use tonic::transport::Uri;
use tonic::transport::server::Connected;
use tower::service_fn;

use crate::flow_control::configure_endpoint;

#[pin_project]
pub struct DuplexChannel<R, W> {
    #[pin]
    read: R,
    #[pin]
    write: W,
}

impl<R, W> DuplexChannel<R, W> {
    pub fn new(read: R, write: W) -> Self {
        Self { read, write }
    }
}

impl<R, W> AsyncWrite for DuplexChannel<R, W>
where
    W: AsyncWrite,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        self.project().write.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.project().write.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        self.project().write.poll_shutdown(cx)
    }
}

impl<R, W> AsyncRead for DuplexChannel<R, W>
where
    R: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.project().read.poll_read(cx, buf)
    }
}

impl<R, W> Connected for DuplexChannel<R, W> {
    type ConnectInfo = ();

    fn connect_info(&self) {}
}

/// Create a channel using a pre-existing I/O instance. This will not support reconnecting since
/// there is no way to establish connections here. We're just using one that already exists.
pub async fn make_channel<T>(io: T, name: &str) -> buck2_error::Result<Channel>
where
    T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    // We have otherwise standardized on using tokio::io::Async*
    // to define our type constraints in the vicinity of this file,
    // (mostly because switching the impl to the Hyper equivalents isn't as
    // convenient - we cannot simply use the TokioIo wrapper when going
    // from Hyper->Tokio because the wrapper doesn't have an implementation for Connected)
    // but we need a Hyper object here for backward compatibility
    let io = hyper_util::rt::tokio::TokioIo::new(io);
    let mut io = Some(io);
    // NOTE: The uri here is only used to populate the requests we send. We don't actually connect
    // anywhere since we already have an I/O channel on hand.
    let endpoint = Endpoint::try_from(format!("http://{name}.invalid"))
        .buck_error_context("Invalid endpoint")?;
    let channel = configure_endpoint(endpoint)
        .connect_with_connector(service_fn(move |_: Uri| {
            let io = io
                .take()
                // Must be a `String` not a `&'static str`, the lifetime otherwise makes the
                // compiler very confused in very non-local ways
                .ok_or_else(|| "Cannot reconnect after connection loss".to_owned());
            future::ready(io)
        }))
        .await
        .buck_error_context("Failed to create channel")?;

    Ok(channel)
}
