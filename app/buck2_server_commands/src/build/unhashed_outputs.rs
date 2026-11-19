/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::artifact_type::BaseArtifactKind;
use buck2_build_api::build::BuildProviderType;
use buck2_build_api::build::ProviderArtifacts;
use buck2_cli_proto::build_request::Materializations;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_execute::artifact_utils::ArtifactValueBuilder;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_execute::materialize::materializer::Materializer;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutInsertionMap;
use buck2_util::future::try_join_all;
use dupe::Dupe;
use itertools::Itertools;
use tracing::info;

/// For each unhashed path, the hashed paths that claim it, each with the artifact's value and the
/// path of that value under the hashed path: empty when the artifact is the whole of what is
/// there, the projection's path when it is a part of it.
type UnhashedOutputLinks = BuckMutInsertionMap<
    ProjectRelativePathBuf,
    BuckMutInsertionMap<ProjectRelativePathBuf, (ArtifactValue, ForwardRelativePathBuf)>,
>;

fn unhashed_output_links(
    provider_artifacts: Vec<ProviderArtifacts>,
    artifact_fs: &ArtifactFs,
) -> buck2_error::Result<UnhashedOutputLinks> {
    let mut unhashed_to_hashed: UnhashedOutputLinks = BuckMutInsertionMap::new();
    for provider_artifact in provider_artifacts {
        if !matches!(provider_artifact.provider_type, BuildProviderType::Default) {
            continue;
        }

        if let Ok((artifact, value)) = provider_artifact.values.iter().exactly_one()
            && let (BaseArtifactKind::Build(build), projected_path) = artifact.as_parts()
            && let Some(unhashed_path) = artifact_fs.retrieve_unhashed_location(build.get_path())
        {
            let path = artifact_fs.resolve_build(
                build.get_path(),
                if build.get_path().is_content_based_path() {
                    Some(value.content_based_path_hash())
                } else {
                    None
                }
                .as_ref(),
            )?;
            unhashed_to_hashed
                .entry(unhashed_path)
                .or_default()
                .insert(path, (value.clone(), projected_path.to_buf()));
        }
    }
    Ok(unhashed_to_hashed)
}

pub(crate) async fn create_unhashed_outputs_via_materializer(
    provider_artifacts: Vec<ProviderArtifacts>,
    artifact_fs: &ArtifactFs,
    digest_config: DigestConfig,
    materializer: &dyn Materializer,
    materializations: Materializations,
    re_use_case: RemoteExecutorUseCase,
) -> buck2_error::Result<()> {
    create_unhashed_outputs_via_materializer_impl(
        provider_artifacts,
        artifact_fs,
        digest_config,
        materializer,
        materializations,
        re_use_case,
    )
    .await
    .tag(ErrorTag::UnhashedOutputSymlink)
    .with_buck_error_context(|| "while creating materializer-managed unhashed output symlinks")
}

async fn create_unhashed_outputs_via_materializer_impl(
    provider_artifacts: Vec<ProviderArtifacts>,
    artifact_fs: &ArtifactFs,
    digest_config: DigestConfig,
    materializer: &dyn Materializer,
    materializations: Materializations,
    re_use_case: RemoteExecutorUseCase,
) -> buck2_error::Result<()> {
    let unhashed_to_hashed = unhashed_output_links(provider_artifacts, artifact_fs)?;
    let mut declarations = Vec::new();

    for (unhashed, hashed_set) in unhashed_to_hashed {
        if let Ok((hashed, (value, projected_path))) = hashed_set.iter().exactly_one() {
            let mut builder = ArtifactValueBuilder::new(artifact_fs.fs(), digest_config);
            // The symlink's target goes into the value as a dep, with what is there: a
            // materializer that materializes the symlink by its value must see the output as it
            // is, not a stand-in. A projection knows only its part of the output, which is the
            // dep it carries, at the part's own path under the hashed path.
            if projected_path.is_empty() {
                builder.add_symlinked(value, hashed.clone(), &unhashed)?;
            } else {
                builder.add_symlink(hashed, &unhashed)?;
                builder.add_input_value(hashed.join(projected_path), value)?;
            }
            let symlink_value = builder.build(&unhashed)?;
            declarations.push((unhashed, symlink_value));
        } else {
            info!(
                "The following outputs have a conflicting unhashed path at {}: {:?}",
                unhashed, hashed_set
            );
        }
    }
    // Nothing produces these symlinks, so they are declared rather than reported.
    try_join_all(
        declarations
            .iter()
            .map(|(path, value)| materializer.declare_copy(path.clone(), value.dupe(), Vec::new())),
    )
    .await?;
    let required = match materializations {
        Materializations::Skip => return Ok(()),
        Materializations::Default => false,
        Materializations::Materialize => true,
    };
    let response = materializer
        .materialize(MaterializeRequest {
            artifacts: declarations,
            outputs: Vec::new(),
            purpose: MaterializationPurpose::FinalOutput { required },
            re_use_case,
        })
        .await?;
    // Read after the command returns, so there is no scope to hold the lease over.
    drop(response.ensure_results_ok()?);

    Ok(())
}
