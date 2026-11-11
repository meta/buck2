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
    pub(crate) async fn new(mut child: Child, channel: Channel) -> buck2_error::Result<Self> {
        let rpc = buck2_forkserver_proto::forkserver_client::ForkserverClient::new(channel)
            .max_encoding_message_size(usize::MAX)
            .max_decoding_message_size(usize::MAX);

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

        let stream = stream::once(future::ready(buck2_forkserver_proto::RequestEvent {
            data: Some(req.into()),
        }))
        .chain(futures::stream::select(cancel_stream, freeze_stream));

        let stream = self
            .inner
            .rpc
            .clone()
            .run(stream)
            .await
            .buck_error_context("Error dispatching command to Forkserver")?
            .into_inner();
        let stream = decode_event_stream(stream);

        decode_command_event_stream(stream).await
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
