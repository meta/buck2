/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_error::BuckErrorContext;
use futures::future;
use futures::stream;
use futures::stream::StreamExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tonic::service::Routes;
use tonic::transport::server::Router;
use tower::Service;
use tower::layer::Layer;

use self::connection_with_extra::ConnectionWithExtra;
use self::drop_notifier::DropNotifier;

pub struct ServerHandle {
    channel: DropNotifier,
    handle: JoinHandle<buck2_error::Result<()>>,
}

impl ServerHandle {
    /// Tell the server to shutdown and wait for it to exit.
    pub async fn shutdown(self) -> buck2_error::Result<()> {
        self.channel.notify_now();
        self.handle
            .await
            .buck_error_context("Failed to join task")?
    }

    /// Obtain the JoinHandle to the task driving the server, without asking the server to
    /// shutdown.
    pub fn into_join_handle(self) -> JoinHandle<buck2_error::Result<()>> {
        self.channel.cancel();
        self.handle
    }
}

pub fn spawn_oneshot<T, L>(io: T, router: Router<L>) -> ServerHandle
where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static + tonic::transport::server::Connected,
    L: Layer<Routes> + Send + 'static,
    L::Service: Service<hyper::Request<tonic::body::Body>, Response = hyper::Response<tonic::body::Body>>
        + Clone
        + Send
        + 'static,
    <<L as Layer<Routes>>::Service as Service<hyper::Request<tonic::body::Body>>>::Future:
        Send + 'static,
    <<L as Layer<Routes>>::Service as Service<hyper::Request<tonic::body::Body>>>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    // We reserve 2 slots here: one for the connection and one for the ServerHandle's
    // shutdown.
    let (send, mut recv) = mpsc::channel(2);

    let io = ConnectionWithExtra {
        inner: io,
        notifier: DropNotifier::new(send.clone()),
    };

    let fut = async move {
        // NOTE: We immediately yield the connection we have on hand, then just report no further
        // connections going forward. This is important to ensure that the server doesn't just
        // exist when it sees no connections (it's a little weird that the server exists even with
        // open connections if they're not in the middle of a request, but thats's what it is).
        let incoming = stream::once(future::ready(io))
            .chain(stream::once(future::pending()))
            .map(std::io::Result::Ok);

        router
            .serve_with_incoming_shutdown(incoming, async move {
                let _ignored = recv.recv().await;
            })
            .await
            .buck_error_context("Server exited with an error")?;

        Ok(())
    };

    let handle = tokio::task::spawn(fut);

    ServerHandle {
        channel: DropNotifier::new(send),
        handle,
    }
}

mod drop_notifier {
    use tokio::sync::mpsc::Sender;

    /// Upon being dropped, this notifies the sender is contains.
    pub struct DropNotifier {
        sender: Option<Sender<()>>,
    }

    impl DropNotifier {
        pub fn new(sender: Sender<()>) -> Self {
            Self {
                sender: Some(sender),
            }
        }

        /// Notify the sender this contains (this is equivalent to dropping the DropNotifier).
        pub fn notify_now(self) {
            drop(self)
        }

        /// Drop this DropNotifier without notifying. This is useful in cases where "defusing" the
        /// DropNotifier is desired.
        pub fn cancel(mut self) {
            self.sender.take();
        }
    }

    impl Drop for DropNotifier {
        fn drop(&mut self) {
            if let Some(sender) = self.sender.as_ref() {
                let _ = sender.try_send(());
            }
        }
    }
}

mod connection_with_extra {
    use std::io;
    use std::pin::Pin;
    use std::task::Context;
    use std::task::Poll;

    use pin_project::pin_project;
    use tokio::io::AsyncRead;
    use tokio::io::AsyncWrite;
    use tokio::io::ReadBuf;
    use tonic::transport::server::Connected;

    use super::DropNotifier;

    /// A connection (AsyncWrite + AsyncRead + Connected), with a field for a DropNotifier. This
    /// acts a connection, but lets us observe when it's dropped.
    #[pin_project]
    pub struct ConnectionWithExtra<I> {
        #[pin]
        pub inner: I,
        pub notifier: DropNotifier,
    }

    impl<I> AsyncWrite for ConnectionWithExtra<I>
    where
        I: AsyncWrite,
    {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, io::Error>> {
            self.project().inner.poll_write(cx, buf)
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
            self.project().inner.poll_flush(cx)
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Result<(), io::Error>> {
            self.project().inner.poll_shutdown(cx)
        }
    }

    impl<I> AsyncRead for ConnectionWithExtra<I>
    where
        I: AsyncRead,
    {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            self.project().inner.poll_read(cx, buf)
        }
    }

    impl<I> Connected for ConnectionWithExtra<I>
    where
        I: Connected,
    {
        type ConnectInfo = I::ConnectInfo;

        fn connect_info(&self) -> Self::ConnectInfo {
            self.inner.connect_info()
        }
    }
}
