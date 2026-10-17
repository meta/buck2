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
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;

use allocative::Allocative;
use arc_swap::ArcSwapOption;
use buck2_core::tag_error;
use buck2_error::BuckErrorContext;
use buck2_execute_local::CommandResult;
use buck2_execute_local::decode_command_event_stream;
use buck2_resource_control::ActionFreezeEvent;
use buck2_resource_control::ActionFreezeEventReceiver;
use dupe::Dupe;
use futures::future;
use futures::future::Future;
use futures::future::FutureExt;
use futures::stream;
use futures::stream::StreamExt;
use tokio::process::Child;
use tokio::sync::oneshot;
use tonic::Request;
use tonic::transport::Channel;

use crate::convert::decode_event_stream;

#[derive(Clone, Dupe, Allocative)]
pub struct ForkserverClient {
    inner: Arc<ForkserverClientInner>,
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
enum ForkserverError {
    #[error("Error on Forkserver wait()")]
    WaitError(#[source] io::Error),
    #[error("Forkserver exited {}", _0)]
    Exited(ExitStatus),
}

/// How long a finished command's response stream may take to reach its trailers before the client
/// gives up on it and lets h2 reset it. The forkserver writes the trailers right after the exit
/// event, so this only bounds a forkserver that has stopped answering.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Allocative)]
struct ForkserverClientInner {
    /// Error from the forkserver process, if any.
    #[allocative(skip)]
    error: Arc<ArcSwapOption<buck2_error::Error>>,
    pid: u32,
    #[allocative(skip)]
    rpc: buck2_forkserver_proto::forkserver_client::ForkserverClient<Channel>,
}

impl ForkserverClient {
    fn rpc(
        channel: Channel,
    ) -> buck2_forkserver_proto::forkserver_client::ForkserverClient<Channel> {
        buck2_forkserver_proto::forkserver_client::ForkserverClient::new(channel)
            .max_encoding_message_size(usize::MAX)
            .max_decoding_message_size(usize::MAX)
    }

    pub(crate) async fn new(mut child: Child, channel: Channel) -> buck2_error::Result<Self> {
        let rpc = Self::rpc(channel);

        let pid = child.id().expect("Child has not been polled");

        let error = Arc::new(ArcSwapOption::empty());

        tokio::task::spawn(buck2_util::async_move_clone!(error, {
            let err = match child.wait().await {
                Ok(status) => ForkserverError::Exited(status),
                Err(e) => ForkserverError::WaitError(e),
            };

            let err = buck2_error::Error::from(err).context("Forkserver is unavailable");
            error.swap(Some(Arc::new(err)));
        }));

        Ok(Self {
            inner: Arc::new(ForkserverClientInner { error, pid, rpc }),
        })
    }

    pub fn pid(&self) -> u32 {
        self.inner.pid
    }

    pub async fn execute<C>(
        &self,
        req: buck2_forkserver_proto::CommandRequest,
        cancel: C,
        freeze_rx: impl ActionFreezeEventReceiver,
    ) -> buck2_error::Result<CommandResult>
    where
        C: Future<Output = ()> + Send + 'static,
    {
        if let Some(err) = &*self.inner.error.load() {
            return Err(tag_error!(
                "forkserver_exit",
                err.as_ref().dupe(),
                task: false,
                daemon_in_memory_state_is_corrupted: true,
            ));
        }

        let cancel_stream = stream::once(cancel.map(|()| buck2_forkserver_proto::RequestEvent {
            data: Some(buck2_forkserver_proto::CancelRequest {}.into()),
        }));
        let freeze_stream = freeze_rx.map(|e| {
            let data = match e {
                ActionFreezeEvent::Freeze => buck2_forkserver_proto::FreezeRequest {}.into(),
                ActionFreezeEvent::Unfreeze => buck2_forkserver_proto::UnfreezeRequest {}.into(),
            };
            buck2_forkserver_proto::RequestEvent { data: Some(data) }
        });

        // The request stream ends once this call is done with the command, so the h2 stream closes
        // with END_STREAM from both sides. Left open, it stays open until `cancel` resolves, and
        // `cancel` is often a guard that outlives the command: `buck2 test` holds one for the
        // whole test command (buck2_test/src/command.rs), so every local test of the command
        // keeps a stream, two tasks and the forkserver's reader of it until the command ends, and
        // then all of them send their CancelRequest at once. The forkserver's single thread reads
        // that burst before any reader runs, and h2 >= 0.4.16 charges each unread DATA frame under
        // 256 bytes against a budget of half the connection window
        // (h2 src/proto/streams/counts.rs `record_data_frame`). Past it, h2 closes the connection
        // with GOAWAY ENHANCE_YOUR_CALM "too_many_data_frames" (h2 src/proto/streams/streams.rs
        // `recv_data`), and the forkserver, whose only connection that was, exits with status 0.
        // Dropping this future before the command finishes ends the request too, which the
        // forkserver treats as a cancellation and kills the command.
        let (finished_tx, finished_rx) = oneshot::channel::<()>();

        let stream = stream::once(future::ready(buck2_forkserver_proto::RequestEvent {
            data: Some(req.into()),
        }))
        .chain(futures::stream::select(cancel_stream, freeze_stream).take_until(finished_rx));

        let mut stream = self
            .inner
            .rpc
            .clone()
            .run(stream)
            .await
            .buck_error_context("Error dispatching command to Forkserver")?
            .into_inner();

        let res = decode_command_event_stream(decode_event_stream(stream.by_ref())).await;
        drop(finished_tx);

        // The exit event comes just before the server's trailers. Read up to them rather than
        // drop the stream, which h2 would reset; a reset stream is remembered for a second and only
        // 50 at a time (h2 src/proto/mod.rs DEFAULT_RESET_STREAM_*), and a frame for one it has
        // forgotten counts towards the 1024 that end the connection with "too_many_internal_resets"
        // (h2 src/proto/streams/streams.rs `recv_data`, `reset_on_recv_stream_err`), the fault
        // fork commit 5de4fc293e fixed for the remote execution client.
        tokio::task::spawn(async move {
            let _ignored = tokio::time::timeout(DRAIN_TIMEOUT, async {
                while let Some(_event) = stream.next().await {}
            })
            .await;
        });

        res
    }

    pub async fn set_log_filter(&self, log_filter: String) -> buck2_error::Result<()> {
        self.inner
            .rpc
            .clone()
            .set_log_filter(Request::new(buck2_forkserver_proto::SetLogFilterRequest {
                log_filter,
            }))
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use buck2_core::logging::LogConfigurationReloadHandle;
    use buck2_execute_local::GatherOutputStatus;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use tokio::net::UnixStream;

    use super::*;
    use crate::service::ForkserverStats;
    use crate::service::UnixForkserverService;

    impl ForkserverClient {
        /// A client of a forkserver served in this process, which never exits.
        fn for_test(channel: Channel) -> Self {
            Self {
                inner: Arc::new(ForkserverClientInner {
                    error: Arc::new(ArcSwapOption::empty()),
                    pid: std::process::id(),
                    rpc: Self::rpc(channel),
                }),
            }
        }
    }

    /// A finished command leaves no stream open on the forkserver, even when its cancellation
    /// future never resolves, which is the case for every local test of a `buck2 test` until the
    /// command ends.
    #[tokio::test]
    async fn finished_commands_end_their_request_streams() {
        const COMMANDS: u64 = 200;

        let (client_io, server_io) = UnixStream::pair().unwrap();
        let stats = Arc::new(ForkserverStats::default());
        let state_dir = AbsNormPathBuf::new(std::env::temp_dir()).unwrap();
        let service = UnixForkserverService::new(
            <dyn LogConfigurationReloadHandle>::noop(),
            &state_dir,
            stats.dupe(),
        )
        .unwrap();
        let router = tonic::transport::Server::builder()
            .add_service(buck2_forkserver_proto::forkserver_server::ForkserverServer::new(service));
        let (read, write) = tokio::io::split(server_io);
        let _server =
            buck2_grpc::spawn_oneshot(buck2_grpc::DuplexChannel::new(read, write), router);
        let channel = buck2_grpc::make_channel(client_io, "forkserver")
            .await
            .unwrap();
        let client = ForkserverClient::for_test(channel);

        let cwd = std::env::current_dir().unwrap();
        for _ in 0..COMMANDS {
            let req = buck2_forkserver_proto::CommandRequest {
                exe: b"/bin/sh".to_vec(),
                argv: vec![b"-c".to_vec(), b"true".to_vec()],
                cwd: Some(buck2_forkserver_proto::WorkingDirectory {
                    path: cwd.as_os_str().as_encoded_bytes().to_vec(),
                }),
                ..Default::default()
            };
            let res = client
                .execute(
                    req,
                    future::pending(),
                    stream::pending::<ActionFreezeEvent>(),
                )
                .await
                .unwrap();
            assert!(
                matches!(
                    res.status,
                    GatherOutputStatus::Finished { exit_code: 0, .. }
                ),
                "{:?}",
                res.status
            );
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let open = stats.open_requests.load(Ordering::Relaxed);
            if open == 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{open} of {COMMANDS} finished commands still have a request stream open"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(stats.commands.load(Ordering::Relaxed), COMMANDS);
    }
}
