/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Instant;

use allocative::Allocative;
use allocative::FlameGraphBuilder;
use allocative::Visitor;
use allocative::ident_key;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::get_dispatcher;
use buck2_events::dispatch::get_dispatcher_opt;
use buck2_execute::materialize::materializer::CleanStaleArtifactsArgs;
use buck2_execute::materialize::materializer::CleanStaleArtifactsPolicy;
use buck2_execute::materialize::materializer::MaterializerEntry;
use buck2_execute::materialize::materializer::MaterializerIterItem;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use derivative::Derivative;
use dupe::Dupe;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use futures::stream::BoxStream;
use futures::stream::StreamExt;
use jiff::SignedDuration;
use jiff::Timestamp;
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::sync::oneshot::Sender;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::materializers::deferred::ArtifactMaterializationMethod;
use crate::materializers::deferred::ArtifactMaterializationStage;
use crate::materializers::deferred::DeferredMaterializerAccessor;
use crate::materializers::deferred::DeferredMaterializerCommandProcessor;
use crate::materializers::deferred::MaterializerCommand;
use crate::materializers::deferred::ProcessingFuture;
use crate::materializers::deferred::artifact_tree::ArtifactTree;
use crate::materializers::deferred::artifact_tree::artifact_metadata_size;
use crate::materializers::deferred::clean_stale::CleanScratchExtensionCommand;
use crate::materializers::deferred::clean_stale::CleanStaleArtifactsCommand;
use crate::materializers::deferred::clean_stale::CleanStaleArtifactsExtensionCommand;
use crate::materializers::deferred::clean_stale::CleanStaleArtifactsExtensionCommandKind;
use crate::materializers::deferred::command_processor::task_stopped;
use crate::materializers::deferred::io_handler::IoHandler;
use crate::materializers::deferred::io_handler::create_ttl_refresh;

pub(super) trait ExtensionCommand<T>: Debug + Sync + Send + 'static {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>);
}

struct AllocativeProfileRoot<'a> {
    artifact_tree: &'a ArtifactTree,
}

impl Allocative for AllocativeProfileRoot<'_> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_field(
            ident_key!(artifact_tree),
            &self.artifact_tree.allocative_dfs(),
        );
        visitor.exit();
    }
}

#[derive(Debug)]
struct PathData {
    stage: PathStage,
    processing: PathProcessing,
}

#[derive(Debug)]
enum PathStage {
    Materialized { ts: Timestamp, size: Option<u64> },
    Declared(Arc<ArtifactMaterializationMethod>),
}

#[derive(Debug)]
enum PathProcessing {
    Done,
    Materializing,
    Cleaning,
}

impl Display for PathData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.stage {
            PathStage::Materialized { ts, size } => {
                if let Some(size) = size {
                    write!(f, "materialized (ts={ts:?}, size={size})")?;
                } else {
                    write!(f, "materialized (ts={ts:?})")?;
                }
            }
            PathStage::Declared(method) => {
                write!(f, "declared: {method}")?;
            }
        }

        match &self.processing {
            PathProcessing::Done => {}
            PathProcessing::Materializing => {
                write!(f, " (materializing")?;
            }
            PathProcessing::Cleaning => {
                write!(f, " (cleaning)")?;
            }
        }

        Ok(())
    }
}

impl MaterializerEntry for PathData {}

#[derive(Derivative)]
#[derivative(Debug)]
struct Iterate {
    /// This is for debug commands so we use an unbounded channel to avoid locking up the
    /// materializer command thread.
    #[derivative(Debug = "ignore")]
    sender: UnboundedSender<MaterializerIterItem>,
}

impl<T: IoHandler> ExtensionCommand<T> for Iterate {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        // Ensure up to date access times
        processor.flush_access_times();
        for (path, data) in processor.tree.iter_with_paths() {
            let stage = match &data.stage {
                ArtifactMaterializationStage::Declared { method, .. } => {
                    PathStage::Declared(method.dupe())
                }
                ArtifactMaterializationStage::Materialized {
                    last_access_time,
                    metadata,
                    ..
                } => {
                    // drop nano-seconds
                    let ts = Timestamp::from_second(last_access_time.as_second())
                        .expect("whole seconds of a valid timestamp are in range");
                    PathStage::Materialized {
                        ts,
                        size: Some(artifact_metadata_size(metadata)),
                    }
                }
            };

            let processing = match data.processing.active_ref() {
                None => PathProcessing::Done,
                Some(active) => match &active.future {
                    ProcessingFuture::Materializing(..) => PathProcessing::Materializing,
                    ProcessingFuture::Cleaning(..) => PathProcessing::Cleaning,
                },
            };

            let path_data = PathData { stage, processing };

            let path = ProjectRelativePathBuf::from(path);

            let deps = match &data.deps {
                Some(deps) => processor.tree.find_artifacts_for_debug(deps),
                None => Vec::new(),
            };

            match self.sender.send(MaterializerIterItem {
                artifact_path: path,
                artifact_display: Box::new(path_data) as _,
                deps,
            }) {
                Ok(..) => {}
                Err(..) => break, // No use sending more if the client disconnected.
            }
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
struct AllocativeProfile {
    #[derivative(Debug = "ignore")]
    sender: Sender<allocative::FlameGraphOutput>,
}

impl<T> ExtensionCommand<T> for AllocativeProfile {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        let mut graph = FlameGraphBuilder::default();
        graph.visit_root(&AllocativeProfileRoot {
            artifact_tree: &processor.tree,
        });
        let _ignored = self.sender.send(graph.finish());
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
struct Fsck {
    /// This is for debug commands so we use an unbounded channel to avoid locking up the
    /// materializer command thread.
    #[derivative(Debug = "ignore")]
    sender: UnboundedSender<(ProjectRelativePathBuf, buck2_error::Error)>,
}

impl<T: IoHandler> ExtensionCommand<T> for Fsck {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        for (path, data) in processor.tree.iter_with_paths() {
            match &data.stage {
                ArtifactMaterializationStage::Declared { .. } => {
                    continue;
                }
                ArtifactMaterializationStage::Materialized { .. } => {}
            };

            // We actually block the thread here. This is to ensure we don't try to delete things
            // when we check them. This is primarily a debug command. We don't run this while
            // actual things are in flight.

            let path = ProjectRelativePathBuf::from(path);
            let res =
                fs_util::symlink_metadata(processor.io.fs().resolve(&path)).categorize_internal();
            match res {
                Ok(..) => {}
                Err(e) => {
                    let _ignored = self.sender.send((path, e.into()));
                }
            }
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
struct RefreshTtls {
    #[derivative(Debug = "ignore")]
    sender: Sender<Option<BoxFuture<'static, buck2_error::Result<()>>>>,
    min_ttl: i64,
}

impl<T: IoHandler> ExtensionCommand<T> for RefreshTtls {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        let dispatcher = get_dispatcher_opt().unwrap_or_else(EventDispatcher::error_on_event);
        let task = create_ttl_refresh(
            &processor.tree,
            processor.io.re_client_manager(),
            SignedDuration::from_secs(self.min_ttl),
            processor.io.digest_config(),
        )
        .map(|f| {
            processor
                .spawn(&dispatcher, move |_| f.boxed())
                .map(|result| result.unwrap_or_else(|error| Err(task_stopped(error))))
                .boxed()
        });
        let _ignored = self.sender.send(task);
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
struct GetTtlRefreshLog {
    sender: Sender<String>,
}

impl<T: IoHandler> ExtensionCommand<T> for GetTtlRefreshLog {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        // We normally poll this very lazily, so actually force it to happen here.
        processor.poll_current_ttl_refresh();

        let mut out = String::new();

        for entry in &processor.ttl_refresh_history {
            write!(&mut out, "{:?}\t", entry.at).unwrap();
            match &entry.outcome {
                None => {
                    writeln!(&mut out, "SKIP").unwrap();
                }
                Some(Ok(())) => {
                    writeln!(&mut out, "OK").unwrap();
                }
                Some(Err(e)) => {
                    writeln!(&mut out, "ERR\t{e:#}").unwrap();
                }
            }
        }

        let _ignored = self.sender.send(out);
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
struct TestIter {
    sender: Sender<String>,
    count: usize,
}

impl<T> ExtensionCommand<T> for TestIter {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        let mut out = String::new();

        let now = std::time::Instant::now();

        for _i in 0..self.count {
            let it = processor.tree.iter_without_paths();

            for e in it {
                let _e = e;
            }
        }

        writeln!(
            &mut out,
            "Elapsed for iter() ({} times): {:?}",
            self.count,
            Instant::now() - now
        )
        .unwrap();

        let now = std::time::Instant::now();

        for _i in 0..self.count {
            let it = processor.tree.iter_with_paths();

            for e in it {
                let _e = e;
            }
        }

        writeln!(
            &mut out,
            "Elapsed for iter().with_paths() ({} times): {:?}",
            self.count,
            Instant::now() - now
        )
        .unwrap();

        let _ignored = self.sender.send(out);
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
struct FlushAccessTimes {
    sender: Sender<String>,
}

impl<T: IoHandler> ExtensionCommand<T> for FlushAccessTimes {
    fn execute(self: Box<Self>, processor: &mut DeferredMaterializerCommandProcessor<T>) {
        let mut out = String::new();

        writeln!(&mut out, "{}", processor.flush_access_times()).unwrap();
        let _ignored = self.sender.send(out);
    }
}

impl<T: IoHandler> DeferredMaterializerAccessor<T> {
    pub(super) fn iterate_impl(
        &self,
    ) -> buck2_error::Result<BoxStream<'static, MaterializerIterItem>> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(Iterate { sender }) as _,
            get_dispatcher_opt(),
        ))?;
        Ok(UnboundedReceiverStream::new(receiver).boxed())
    }

    pub(super) async fn allocative_impl(
        &self,
    ) -> buck2_error::Result<allocative::FlameGraphOutput> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(AllocativeProfile { sender }) as _,
            get_dispatcher_opt(),
        ))?;
        receiver
            .await
            .buck_error_context("No response from materializer")
    }

    pub(super) fn fsck_impl(
        &self,
    ) -> buck2_error::Result<BoxStream<'static, (ProjectRelativePathBuf, buck2_error::Error)>> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(Fsck { sender }) as _,
            get_dispatcher_opt(),
        ))?;
        Ok(UnboundedReceiverStream::new(receiver).boxed())
    }

    pub(super) async fn refresh_ttls_impl(&self, min_ttl: i64) -> buck2_error::Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(RefreshTtls { sender, min_ttl }) as _,
            get_dispatcher_opt(),
        ))?;
        if let Some(task) = receiver
            .await
            .buck_error_context("No response from materializer")?
        {
            task.await.buck_error_context("Refresh failed")?;
        }
        Ok(())
    }

    pub(super) async fn get_ttl_refresh_log_impl(&self) -> buck2_error::Result<String> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(GetTtlRefreshLog { sender }) as _,
            get_dispatcher_opt(),
        ))?;
        receiver
            .await
            .buck_error_context("No response from materializer")
    }

    pub(super) async fn clean_stale_artifacts_impl(
        &self,
        args: CleanStaleArtifactsArgs,
    ) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse> {
        let dispatcher = get_dispatcher();
        let kind = match args.policy {
            CleanStaleArtifactsPolicy::Configured => {
                CleanStaleArtifactsExtensionCommandKind::Configured {
                    dispatcher,
                    dry_run: args.dry_run,
                    tracked_only: args.tracked_only,
                    origin: args.origin,
                }
            }
            CleanStaleArtifactsPolicy::Explicit {
                keep_since_time,
                adaptive_low_disk_threshold,
                adaptive_min_ttl,
                adaptive_unmaterialize_active,
            } => CleanStaleArtifactsExtensionCommandKind::Explicit(
                CleanStaleArtifactsCommand::explicit(
                    keep_since_time,
                    args.dry_run,
                    args.tracked_only,
                    adaptive_low_disk_threshold,
                    adaptive_min_ttl,
                    adaptive_unmaterialize_active,
                    args.origin,
                    dispatcher,
                    // Resolved to the processor's buck-out volume at execution.
                    None,
                ),
            ),
        };
        let (sender, recv) = oneshot::channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(CleanStaleArtifactsExtensionCommand { kind, sender }),
            get_dispatcher_opt(),
        ))?;
        recv.await?
            .await
            .map(|res| res.into())
            .map_err(|failure| failure.into_error())
    }

    pub(super) async fn clean_scratch_impl(
        &self,
    ) -> buck2_error::Result<buck2_cli_proto::CleanStaleResponse> {
        let (sender, recv) = oneshot::channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(CleanScratchExtensionCommand { sender }),
            get_dispatcher_opt(),
        ))?;
        recv.await?.await.map(|res| res.into())
    }

    pub(super) async fn test_iter_impl(&self, count: usize) -> buck2_error::Result<String> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(TestIter { sender, count }) as _,
            get_dispatcher_opt(),
        ))?;
        receiver
            .await
            .buck_error_context("No response from materializer")
    }

    pub(super) async fn flush_all_access_times_impl(&self) -> buck2_error::Result<String> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender.send(MaterializerCommand::Extension(
            Box::new(FlushAccessTimes { sender }) as _,
            get_dispatcher_opt(),
        ))?;
        receiver
            .await
            .buck_error_context("No response from materializer")
    }
}
