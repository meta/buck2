/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ops::ControlFlow;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::dispatch::get_dispatcher_opt;
use buck2_events::dispatch::with_dispatcher_opt_async;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_execute::execute::blocking::IoRequest;
use buck2_execute::execute::clean_output_paths::CleanOutputPaths;
use buck2_execute::execute::manager::CommandExecutionManager;
use buck2_execute::execute::manager::CommandExecutionManagerExt;
use buck2_execute::execute::manager::CommandExecutionManagerWithClaim;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::Materializer;
use buck2_execute::re::manager::ReConnectionManager;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use futures::future::Shared;

use crate::materializers::immediate::cas_download;

/// Download eagerly from RE, without taking a claim until the files are on disk.
#[derive(Allocative, Dupe, Clone)]
pub struct ParanoidDownloader {
    inner: Arc<ParanoidDownloaderInner>,
}

#[derive(Allocative)]
struct ParanoidDownloaderInner {
    fs: ProjectRoot,
    io: Arc<dyn BlockingExecutor>,
    re: Arc<ReConnectionManager>,
    cache_path: ProjectRelativePathBuf,
}

impl ParanoidDownloader {
    pub fn new(
        fs: ProjectRoot,
        io: Arc<dyn BlockingExecutor>,
        re: Arc<ReConnectionManager>,
        cache_path: ProjectRelativePathBuf,
    ) -> Self {
        Self {
            inner: Arc::new(ParanoidDownloaderInner {
                fs,
                io,
                re,
                cache_path,
            }),
        }
    }
}

impl ParanoidDownloader {
    pub async fn declare_cas_many(
        &self,
        materializer: &dyn Materializer,
        manager: CommandExecutionManager,
        info: CasDownloadInfo,
        artifacts: Vec<DeclareArtifactPayload>,
        cancellations: &CancellationContext,
    ) -> ControlFlow<CommandExecutionResult, CommandExecutionManagerWithClaim> {
        let inner = self.inner.dupe();
        let dispatcher = get_dispatcher_opt();
        let download_dispatcher = dispatcher.clone();

        let mut paths_to_clean = Vec::with_capacity(artifacts.len());

        let cache_artifacts = artifacts
            .iter()
            .map(
                |DeclareArtifactPayload {
                     path,
                     artifact: value,
                 }| {
                    let path = inner.cache_path.join(path);
                    paths_to_clean.push(path.clone());
                    (path, value.dupe())
                },
            )
            .collect::<Vec<_>>();

        let future =
            tokio::task::spawn(with_dispatcher_opt_async(download_dispatcher, async move {
                // We just spawned this!
                let cancellations = CancellationContext::never_cancelled();

                for (path, _) in cache_artifacts.iter() {
                    tracing::trace!(path = %path, "Materialize path");
                }

                cas_download(
                    &inner.fs,
                    inner.io.as_ref(),
                    inner.re.as_ref(),
                    &info,
                    cache_artifacts,
                    cancellations,
                )
                .await?;

                buck2_error::Result::Ok(())
            }))
            .map(|r| r.unwrap_or_else(|e| Err(e.into())))
            .boxed()
            .shared();

        let dl = CacheDownload {
            inner: Some(CacheDownloadInner {
                io: self.inner.io.dupe(),
                future,
                paths: paths_to_clean,
                dispatcher,
            }),
        };

        let download_result = dl.wait_until_materialized().await;

        match download_result {
            Ok(()) => (),
            Err(e) => {
                return ControlFlow::Break(manager.error("materialize_outputs", e));
            }
        };

        // Claim the request before copying the outputs.
        let manager = manager.claim().await;

        let output_paths = artifacts
            .iter()
            .map(|DeclareArtifactPayload { path, .. }| path.clone())
            .collect::<Vec<_>>();

        // The lease is what serializes the rename and the report against a local attempt that
        // lost the race and against any materialization of these paths. Clearing whatever is on
        // disk there is this producer's job, done under it.
        let outputs_lease = match materializer.prepare_outputs(output_paths.clone()).await {
            Ok(lease) => lease,
            Err(e) => {
                return ControlFlow::Break(manager.error("materialize_outputs", e));
            }
        };

        let res = cancellations
            .critical_section(|| async {
                self.inner
                    .io
                    .execute_io(
                        Box::new(CleanOutputPaths {
                            paths: output_paths,
                        }),
                        cancellations,
                    )
                    .await?;

                let mapping = artifacts
                    .iter()
                    .map(|DeclareArtifactPayload { path, .. }| {
                        (self.inner.cache_path.join(path), path.clone())
                    })
                    .collect::<Vec<_>>();

                self.inner
                    .io
                    .execute_io(Box::new(MoveOutputsIntoPlace { mapping }), cancellations)
                    .await?;

                materializer
                    .declare_existing(&outputs_lease, artifacts)
                    .await?;

                buck2_error::Ok(())
            })
            .await;

        drop(outputs_lease);

        match res {
            Ok(()) => (),
            Err(e) => {
                return ControlFlow::Break(manager.error("materialize_outputs", e));
            }
        };

        ControlFlow::Continue(manager)
    }
}

struct CacheDownload {
    inner: Option<CacheDownloadInner>,
}

impl CacheDownload {
    async fn wait_until_materialized(&self) -> buck2_error::Result<()> {
        self.inner
            .as_ref()
            .expect("wait_until_materialized called after drop")
            .future
            .clone()
            .await?;

        Ok(())
    }
}

struct CacheDownloadInner {
    io: Arc<dyn BlockingExecutor>,
    future: Shared<BoxFuture<'static, buck2_error::Result<()>>>,
    paths: Vec<ProjectRelativePathBuf>,
    dispatcher: Option<EventDispatcher>,
}

impl Drop for CacheDownload {
    fn drop(&mut self) {
        let inner = self.inner.take().expect("Dropped twice");

        tokio::task::spawn(with_dispatcher_opt_async(
            inner.dispatcher.clone(),
            async move {
                // We just spawned this!
                let cancellations = CancellationContext::never_cancelled();

                let CacheDownloadInner {
                    io, future, paths, ..
                } = inner;

                // Wait for the materialization to finish.
                let _ignored = future.await;

                for path in &paths {
                    tracing::trace!(path = %path, "Delete path");
                }

                // Delete the cache path.
                let _ignored = io
                    .execute_io(Box::new(CleanOutputPaths { paths }), cancellations)
                    .await;
            },
        ));
    }
}

struct MoveOutputsIntoPlace {
    mapping: Vec<(ProjectRelativePathBuf, ProjectRelativePathBuf)>,
}

impl IoRequest for MoveOutputsIntoPlace {
    fn execute(self: Box<Self>, project_fs: &ProjectRoot) -> buck2_error::Result<()> {
        for (from, to) in &self.mapping {
            let from = project_fs.resolve(from);
            let to = project_fs.resolve(to);

            if let Some(p) = to.parent() {
                fs_util::create_dir_all(p)?;
            }

            tracing::trace!(from = %from, to = %to, "Move path");

            fs_util::rename(&from, &to).categorize_internal()?;
        }

        Ok(())
    }
}
