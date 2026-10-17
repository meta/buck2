/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::slice;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::Action;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::UnregisteredAction;
use buck2_build_api::actions::errors::execute_error::ExecuteError;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::execute::action_executor::ActionExecutionMetadata;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use buck2_build_signals::env::WaitingData;
use buck2_common::file_ops::metadata::FileDigestConfig;
use buck2_common::temp_path::TempPath;
use buck2_core::category::CategoryRef;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::directory::INTERNER;
use buck2_execute::entry::build_entry_from_disk_with_cancellation;
use buck2_execute::execute::clean_output_paths::CleanOutputPaths;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::buck_mut_insertion_map;
use dupe::Dupe;
use itertools::Itertools;
use pagable::Pagable;
use pagable::pagable_typetag;
use regex::Regex;
use starlark::values::OwnedFrozen;
use starlark::values::Value;

use crate::actions::impls::extract_archive::cache::ExtractionCache;
use crate::actions::impls::extract_archive::unpack;
use crate::actions::impls::extract_archive::unpack::ArchiveType;
use crate::actions::impls::extract_archive::unpack::ExtractOptions;
use crate::actions::impls::extract_archive::unpack::parse_excludes;
use crate::actions::impls::extract_archive::unpack::parse_strip_prefix;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ExtractArchiveActionError {
    #[error("Exactly one output must be specified for an extract_archive action, got {0}")]
    WrongNumberOfOutputs(usize),
}

#[derive(Debug, Allocative, Pagable)]
pub(crate) struct UnregisteredExtractArchiveAction {
    archive: ArtifactGroup,
    archive_type: Option<ArchiveType>,
    strip_prefix: Box<[String]>,
    #[allocative(skip)]
    excludes: Box<[Regex]>,
}

impl UnregisteredExtractArchiveAction {
    pub(crate) fn new(
        archive: ArtifactGroup,
        archive_type: Option<&str>,
        strip_prefix: Option<&str>,
        excludes: Vec<&str>,
    ) -> buck2_error::Result<Self> {
        Ok(Self {
            archive,
            archive_type: archive_type.map(ArchiveType::parse).transpose()?,
            strip_prefix: strip_prefix
                .map(parse_strip_prefix)
                .transpose()?
                .unwrap_or_default(),
            excludes: parse_excludes(&excludes)?,
        })
    }
}

impl UnregisteredAction for UnregisteredExtractArchiveAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        _starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        Ok(Box::new(ExtractArchiveAction::new(outputs, *self)?))
    }
}

#[derive(Debug, Allocative, Pagable)]
struct ExtractArchiveAction {
    output: BuildArtifact,
    inner: UnregisteredExtractArchiveAction,
}

impl ExtractArchiveAction {
    fn new(
        outputs: BuckMutInsertionSet<BuildArtifact>,
        inner: UnregisteredExtractArchiveAction,
    ) -> buck2_error::Result<Self> {
        let count = outputs.len();
        let output = outputs
            .into_iter()
            .exactly_one()
            .ok()
            .ok_or(ExtractArchiveActionError::WrongNumberOfOutputs(count))?;
        Ok(Self { output, inner })
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for ExtractArchiveAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::ExtractArchive
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        Ok(Cow::Borrowed(slice::from_ref(&self.inner.archive)))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(slice::from_ref(&self.output))
    }

    fn first_output(&self) -> &BuildArtifact {
        &self.output
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new("extract_archive")
    }

    fn identifier(&self) -> Option<&str> {
        Some(self.output.get_path().path().as_str())
    }

    fn aquery_attributes(
        &self,
        _fs: &ExecutorFs,
        _artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<BuckMutInsertionMap<String, String>> {
        let excludes: Vec<&str> = self.inner.excludes.iter().map(Regex::as_str).collect();
        Ok(buck_mut_insertion_map! {
            "type".to_owned() => self.inner.archive_type.map_or("", |t| t.name()).to_owned(),
            "strip_prefix".to_owned() => self.inner.strip_prefix.join("/"),
            "excludes".to_owned() => format!("{excludes:?}"),
        })
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        let start = Instant::now();

        let (input, archive_value) = ctx
            .artifact_values(&self.inner.archive)
            .iter()
            .exactly_one()
            .ok()
            .internal_error("Input did not dereference to exactly one artifact")?;
        let fs = ctx.fs();
        let archive_path = input.resolve_path(
            fs,
            if input.path_resolution_requires_artifact_value() {
                Some(archive_value.content_based_path_hash())
            } else {
                None
            }
            .as_ref(),
        )?;
        let archive_type = match self.inner.archive_type {
            Some(archive_type) => archive_type,
            None => ArchiveType::from_file_name(archive_path.as_str())?,
        };

        let cache = ExtractionCache::new(
            &self.output,
            archive_value,
            archive_type,
            &self.inner.strip_prefix,
            &self.inner.excludes,
        );
        if let Some(cache) = &cache
            && let Some(value) = cache.lookup(ctx, &self.output).await?
        {
            return Ok((
                ActionOutputs::from_single(self.output.get_path().dupe(), value),
                ActionExecutionMetadata {
                    dep_file_db_writes_queued: 0,
                    execution_kind: ActionExecutionKind::LocalActionCache,
                    timing: ActionExecutionTimingData {
                        wall_time: Instant::now() - start,
                    },
                    input_files_bytes: None,
                    waiting_data,
                },
            ));
        }

        // The path of a content-based output depends on the extracted content, so the archive
        // is extracted into a scratch directory first. Every configuration of the action shares
        // the placeholder path of the output and the usual scratch path of the action. Two
        // configurations that run at the same time would extract into the same directory, so
        // the action uses the configuration scratch path instead.
        let is_content_based = self.output.get_path().is_content_based_path();
        let dest = if is_content_based {
            fs.buck_out_path_resolver()
                .resolve_scratch(&ctx.target().configuration_scratch_path())?
        } else {
            fs.resolve_build(self.output.get_path(), None)?
        };

        let mut artifacts = Vec::new();
        if input.requires_materialization(fs) {
            artifacts.push((archive_path.clone(), archive_value.dupe()));
        }
        let (_archive_lease, output_lease) = ctx
            .materializer()
            .materialize(MaterializeRequest {
                artifacts,
                outputs: vec![dest.clone()],
                purpose: MaterializationPurpose::IntermediateOnly,
                re_use_case: ctx.invocation_re_use_case(),
            })
            .await?
            .into_leases()
            .map_err(buck2_error::Error::from)?;
        let output_lease =
            output_lease.internal_error("A request with outputs did not grant a write lease")?;

        let blocking_executor = ctx.blocking_executor();
        blocking_executor
            .execute_io(
                Box::new(CleanOutputPaths {
                    paths: vec![dest.clone()],
                }),
                ctx.cancellation_context(),
            )
            .await?;
        let archive_abs = fs.fs().resolve(&archive_path);
        let dest_abs = fs.fs().resolve(&dest);
        // The write lease must outlive scratch cleanup, including when extraction is cancelled.
        let scratch = is_content_based.then(|| TempPath::new_path(dest_abs.clone()));
        let cancellation = ctx.cancellation_context();
        let is_cancelled = || cancellation.is_cancelled();
        let options = ExtractOptions {
            archive_type,
            strip_prefix: &self.inner.strip_prefix,
            excludes: &self.inner.excludes,
        };
        // On Linux, `execute_io` runs on a few threads reserved for changes to the directory
        // structure. Decompression needs CPU, so it runs through `execute_io_inline`. On Linux,
        // `execute_io_inline` allows one concurrent call per available core.
        blocking_executor
            .execute_io_inline(|| {
                unpack::extract(
                    archive_abs.as_path(),
                    dest_abs.as_path(),
                    &options,
                    &is_cancelled,
                )
            })
            .await
            .with_buck_error_context(|| format!("Failed to extract `{archive_path}`"))?;

        let digest_config = ctx.digest_config();
        let (entry, _hashing_info) = build_entry_from_disk_with_cancellation(
            dest_abs,
            FileDigestConfig::build(digest_config.cas_digest_config()),
            blocking_executor,
            fs.fs().root(),
            cancellation,
        )
        .await?;
        let entry = entry
            .internal_error("The extracted archive is missing")?
            .map_dir(|dir| {
                dir.fingerprint(digest_config.as_directory_serializer())
                    .shared(&*INTERNER)
            });
        let value = ArtifactValue::new(entry, None);

        if let Some(scratch) = scratch {
            let content_path = fs.resolve_build(
                self.output.get_path(),
                Some(&value.content_based_path_hash()),
            )?;
            // Another configuration can await the same copy, so cancellation must not remove its source.
            cancellation
                .critical_section(|| async {
                    let _published = ctx
                        .materializer()
                        .publish(&output_lease, dest.clone(), content_path, value.dupe())
                        .await?;
                    blocking_executor
                        .execute_io_inline(|| scratch.close())
                        .await?;
                    // The deferred materializer still tracks scratch after copying it to the content path.
                    drop(output_lease);
                    let _scratch_lease = ctx.materializer().prepare_outputs(vec![dest]).await?;
                    buck2_error::Ok(())
                })
                .await?;
        } else {
            ctx.materializer()
                .declare_existing(
                    &output_lease,
                    vec![DeclareArtifactPayload {
                        path: dest,
                        artifact: value.dupe(),
                    }],
                )
                .await?;
        }

        let queued_write = cache.is_some_and(|cache| cache.store(ctx, &self.output, &value));

        Ok((
            ActionOutputs::from_single(self.output.get_path().dupe(), value),
            ActionExecutionMetadata {
                dep_file_db_writes_queued: u64::from(queued_write),
                execution_kind: ActionExecutionKind::Simple,
                timing: ActionExecutionTimingData {
                    wall_time: Instant::now() - start,
                },
                input_files_bytes: None,
                waiting_data,
            },
        ))
    }
}
