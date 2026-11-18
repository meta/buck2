/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::BaseArtifactKind;
use buck2_build_signals::env::WaitingCategory;
use buck2_build_signals::env::WaitingData;
use buck2_cli_proto::build_request::Materializations;
use buck2_cli_proto::build_request::Uploads;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_error::BuckErrorContext;
use buck2_execute::artifact_utils::ArtifactValueBuilder;
use buck2_execute::digest_config::HasDigestConfig;
use buck2_execute::directory::ActionDirectoryBuilder;
use buck2_execute::execute::blobs::ActionBlobs;
use buck2_execute::materialize::materializer::HasMaterializer;
use buck2_execute::re::invocation_re_settings::HasInvocationReSettings;
use buck2_hash::BuckDashSet;
use dice::DiceComputations;
use dice::UserComputationData;
use dice_futures::spawn::spawn_dropcancel;
use dupe::Dupe;
use futures::FutureExt;

use crate::actions::artifact::get_artifact_fs::GetArtifactFs;
use crate::actions::artifact::materializer::ArtifactMaterializer;
use crate::actions::execute::dice_data::GetReClient;
use crate::artifact_groups::ArtifactGroup;
use crate::artifact_groups::ArtifactGroupValues;
use crate::artifact_groups::calculation::ArtifactGroupCalculation;

pub async fn materialize_and_upload_artifact_group(
    ctx: &mut DiceComputations<'_>,
    artifact_group: &ArtifactGroup,
    contexts: MaterializationAndUploadContext,
    queue_tracker: &Arc<BuckDashSet<Artifact>>,
) -> buck2_error::Result<ArtifactGroupValues> {
    let (values, _) = {
        let fut = ctx.try_compute2(
            async |ctx| {
                materialize_artifact_group(ctx, artifact_group, contexts.0, queue_tracker).await
            },
            async |ctx| match contexts.1 {
                UploadContext::Skip => Ok(()),
                UploadContext::Upload => ensure_uploaded(ctx, artifact_group).boxed().await,
            },
        );

        tokio::task::unconstrained(fut).await?
    };

    Ok(values)
}

#[cfg(fbcode_build)] // Relies on fbcode future sizes
buck2_util::size_assert::words_of_async_fn_future!(
    materialize_and_upload_artifact_group,
    (_, _, _, _),
    ~39
);
// These are here just for understanding
#[cfg(fbcode_build)]
buck2_util::size_assert::words_of_async_fn_future!(materialize_artifact_group, (_, _, _, _), ~20);
#[cfg(fbcode_build)]
buck2_util::size_assert::words_of_async_fn_future!(ensure_uploaded, (_, _), ~390);

async fn materialize_artifact_group(
    ctx: &mut DiceComputations<'_>,
    artifact_group: &ArtifactGroup,
    materialization_context: MaterializationContext,
    queue_tracker: &Arc<BuckDashSet<Artifact>>,
) -> buck2_error::Result<ArtifactGroupValues> {
    let values = ctx.ensure_artifact_group(artifact_group).await?;

    let mut waiting_data = WaitingData::new();

    if let MaterializationContext::Materialize { force } = materialization_context {
        waiting_data.start_waiting_category_now(WaitingCategory::MaterializerPrepare);
        let artifact_fs = ctx.get_artifact_fs().await?;
        let digest_config = ctx.global_data().get_digest_config();

        let re_use_case = invocation_re_use_case(ctx);
        let data = ctx.data();
        let shared_data = Arc::new((
            data.dupe(),
            artifact_fs.dupe(),
            ctx.per_transaction_data().get_materializer_handle(),
        ));

        let mut materialize_futs = Vec::new();

        for (artifact, value) in values.iter() {
            let (base, projected_path) = artifact.as_parts();
            if let BaseArtifactKind::Build(build_artifact) = base {
                // Keyed by the projection too: each projection of an output is its own request,
                // for its own part of the output.
                if !queue_tracker.insert(artifact.dupe()) {
                    // We've already requested this artifact, no use requesting it again.
                    continue;
                }

                let fut = {
                    let waiting_data = waiting_data.clone();
                    let artifact = build_artifact.dupe();
                    let projected_path = projected_path.to_buf();
                    let value = value.dupe();
                    let shared_data = shared_data.dupe();
                    let artifact_group = artifact_group.dupe();

                    async move {
                        let (data, artifact_fs, materializer) = &*shared_data;

                        let configuration_hash_path = artifact_fs
                            .resolve_build_configuration_hash_path(artifact.get_path())?;

                        // A content-based output lives at its content path; the configuration
                        // path users know is a symlink to it, which nobody but this code
                        // declares. The request then names the symlink, whose deps reach the
                        // content path with the output's own value: a materializer that puts a
                        // symlink's deps on disk by their values puts the output there, not a
                        // stand-in for it. A projection knows only its part of the output, which
                        // is the dep it carries, at the part's own path under the content path.
                        let artifacts = if artifact.get_path().is_content_based_path() {
                            let content_based_path = artifact_fs.resolve_build(
                                artifact.get_path(),
                                Some(&value.content_based_path_hash()),
                            )?;
                            let mut builder =
                                ArtifactValueBuilder::new(artifact_fs.fs(), digest_config);
                            if projected_path.is_empty() {
                                builder.add_symlinked(
                                    &value,
                                    content_based_path,
                                    &configuration_hash_path,
                                )?;
                            } else {
                                builder
                                    .add_symlink(&content_based_path, &configuration_hash_path)?;
                                builder.add_input_value(
                                    content_based_path.join(&projected_path),
                                    &value,
                                )?;
                            }
                            let symlink_value = builder.build(&configuration_hash_path)?;

                            materializer
                            .declare_copy(configuration_hash_path.clone(), symlink_value.dupe(), Vec::new())
                            .await
                            .buck_error_context(
                                "Failed to declare configuration path to content-based path symlinks",
                            )?;
                            vec![(configuration_hash_path, symlink_value)]
                        } else {
                            // A projection's value is its part of the output, which goes at
                            // the part's own path, not the output's.
                            vec![(configuration_hash_path.join(&projected_path), value)]
                        };

                        data.try_materialize_requested_artifact(
                            &artifact,
                            waiting_data,
                            force,
                            artifacts,
                            re_use_case,
                            &artifact_group,
                        )
                        .await
                        .buck_error_context("Failed to materialize artifacts")?;
                        buck2_error::Ok(())
                    }
                };
                materialize_futs.push(spawn_dropcancel(
                    move |_cancellations| fut.boxed(),
                    &*data.per_transaction_data().spawner,
                    data.per_transaction_data(),
                ));
            }
        }

        buck2_util::future::try_join_all(materialize_futs).await?;
    }

    Ok(values)
}

async fn ensure_uploaded(
    ctx: &mut DiceComputations<'_>,
    artifact_group: &ArtifactGroup,
) -> buck2_error::Result<()> {
    let digest_config = ctx.global_data().get_digest_config();
    let artifact_fs = ctx.get_artifact_fs().await?;
    let dir = {
        let mut dir = ActionDirectoryBuilder::empty_non_exhaustive();
        let values = ctx.ensure_artifact_group(artifact_group).await?;
        for (artifact, value) in values.iter() {
            let path = artifact.resolve_path(
                artifact_fs,
                if artifact.path_resolution_requires_artifact_value() {
                    Some(value.content_based_path_hash())
                } else {
                    None
                }
                .as_ref(),
            )?;
            buck2_execute::directory::insert_artifact(&mut dir, path, value)?;
        }
        dir.fingerprint(digest_config.as_directory_serializer())
    };
    let re_use_case = invocation_re_use_case(ctx);
    ctx.per_transaction_data()
        .get_re_client()
        .dupe()
        .with_use_case(re_use_case)
        .upload(
            artifact_fs.fs(),
            ctx.per_transaction_data().get_materializer(),
            &ActionBlobs::new(digest_config),
            ProjectRelativePath::empty(),
            &dir,
            None,
        )
        .await?;

    Ok(())
}

/// The RE use case the invocation talks to the CAS as on its own behalf; see
/// `InvocationReSettings`.
pub fn invocation_re_use_case(ctx: &DiceComputations<'_>) -> RemoteExecutorUseCase {
    ctx.per_transaction_data()
        .get_invocation_re_settings()
        .use_case
}

#[derive(Clone, Dupe, Copy)]
enum MaterializationContext {
    Skip,
    Materialize {
        /// Whether we should force the materialization of requested artifacts, or defer to the
        /// config.
        force: bool,
    },
}
impl From<Materializations> for MaterializationContext {
    fn from(value: Materializations) -> Self {
        match value {
            Materializations::Skip => MaterializationContext::Skip,
            Materializations::Default => MaterializationContext::Materialize { force: false },
            Materializations::Materialize => MaterializationContext::Materialize { force: true },
        }
    }
}

#[derive(Clone, Dupe, Copy)]
enum UploadContext {
    Skip,
    Upload,
}
impl From<Uploads> for UploadContext {
    fn from(value: Uploads) -> Self {
        match value {
            Uploads::Always => UploadContext::Upload,
            Uploads::Never => UploadContext::Skip,
        }
    }
}

#[derive(Clone, Dupe, Copy)]
pub struct MaterializationAndUploadContext(MaterializationContext, UploadContext);
impl MaterializationAndUploadContext {
    pub fn skip() -> Self {
        Self(MaterializationContext::Skip, UploadContext::Skip)
    }
    pub fn materialize() -> Self {
        Self(
            MaterializationContext::Materialize { force: true },
            UploadContext::Skip,
        )
    }
}
impl From<(Materializations, Uploads)> for MaterializationAndUploadContext {
    fn from(value: (Materializations, Uploads)) -> Self {
        Self(value.0.into(), value.1.into())
    }
}

/// This map contains all the artifacts that we enqueued for materialization. This ensures
/// we don't enqueue the same thing more than once. Should be shared across work done
/// in a single DICE transaction.
pub struct MaterializationQueueTrackerHolder(Arc<BuckDashSet<Artifact>>);

pub trait HasMaterializationQueueTracker {
    fn init_materialization_queue_tracker(&mut self);

    fn get_materialization_queue_tracker(&self) -> Arc<BuckDashSet<Artifact>>;
}

impl HasMaterializationQueueTracker for UserComputationData {
    fn init_materialization_queue_tracker(&mut self) {
        self.data.set(MaterializationQueueTrackerHolder(Arc::new(
            BuckDashSet::default(),
        )));
    }

    fn get_materialization_queue_tracker(&self) -> Arc<BuckDashSet<Artifact>> {
        self.data
            .get::<MaterializationQueueTrackerHolder>()
            .expect("MaterializationQueueTracker should be set")
            .0
            .dupe()
    }
}
