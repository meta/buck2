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
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::Action;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::UnregisteredAction;
use buck2_build_api::actions::errors::execute_error::ExecuteError;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::execute::action_executor::ActionExecutionMetadata;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_build_api::actions::impls::write_action_cache_stats::WRITE_ACTION_CACHE_STATS;
use buck2_build_api::actions::impls::write_action_cache_stats::WriteActionCacheStats;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineSink;
use buck2_build_api::interpreter::rule_defs::cmd_args::InputProperties;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_build_signals::env::WaitingData;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::buck2_env;
use buck2_core::category::CategoryRef;
use buck2_core::soft_error;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::artifact_value::file_content_based_path_hash;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::buck_mut_insertion_map;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::values::OwnedFrozen;
use starlark::values::UnpackValue;
use starlark::values::Value;

use crate::actions::impls::declare_writes::declare_writes;
use crate::actions::impls::run::DepFilesPlaceholderArtifactPathMapper;
use crate::actions::impls::write_cache;
use crate::actions::impls::write_cache::CacheUse;
use crate::actions::impls::write_cache::KeyInputs;
use crate::actions::impls::write_cache::WriteCache;
use crate::actions::impls::write_cache::WriteCacheKey;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum WriteActionValidationError {
    #[error("WriteAction received no outputs")]
    NoOutputs,
    #[error("WriteAction received more than one output")]
    TooManyOutputs,
    #[error("Expected command line value, got {0}")]
    ContentsNotCommandLineValue(String),
}

pub(crate) struct CommandLineContentBasedInputVisitor {
    pub(crate) content_based_inputs: BuckMutInsertionSet<ArtifactGroup>,
}

impl CommandLineContentBasedInputVisitor {
    pub(crate) fn new() -> Self {
        Self {
            content_based_inputs: Default::default(),
        }
    }
}

impl<'v> CommandLineArtifactVisitor<'v> for CommandLineContentBasedInputVisitor {
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        _props: InputProperties,
        _tags: Vec<&ArtifactTag>,
    ) {
        if input.path_resolution_may_require_artifact_value() {
            self.content_based_inputs.insert(input);
        }
    }

    fn visit_declared_output(&mut self, _artifact: OutputArtifact<'v>, _tags: Vec<&ArtifactTag>) {}

    fn visit_frozen_output(&mut self, _artifact: Artifact, _tags: Vec<&ArtifactTag>) {}

    fn visit_declared_artifact(
        &mut self,
        declared_artifact: buck2_artifact::artifact::artifact_type::DeclaredArtifact<'v>,
        props: InputProperties,
        tags: Vec<&ArtifactTag>,
    ) -> buck2_error::Result<()> {
        if declared_artifact.has_content_based_path() {
            let artifact = declared_artifact.ensure_bound()?.into_artifact();
            self.visit_input(ArtifactGroup::Artifact(artifact), props, tags);
        }

        Ok(())
    }

    fn inputs_for_path_resolution(&self) -> bool {
        true
    }
}

#[derive(Allocative, Debug, Pagable)]
pub(crate) struct UnregisteredWriteAction {
    pub(crate) is_executable: bool,
    pub(crate) absolute: bool,
    pub(crate) macro_files: Option<MiniBoxSlice<Artifact>>,
    pub(crate) use_dep_files_placeholder_for_content_based_paths: bool,
}

impl UnregisteredAction for UnregisteredWriteAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        let contents = starlark_data.expect("module data to be present");

        let write_action = WriteAction::new(contents, outputs, *self)?;
        Ok(Box::new(write_action))
    }
}

#[derive(Debug, Allocative, Pagable)]
struct WriteAction {
    contents: OwnedFrozen<Value<'static>>, // StarlarkCmdArgs
    output: BuildArtifact,
    inner: UnregisteredWriteAction,
}

impl WriteAction {
    fn new(
        contents: OwnedFrozen<Value<'static>>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        inner: UnregisteredWriteAction,
    ) -> buck2_error::Result<Self> {
        let mut outputs = outputs.into_iter();

        let output = match (outputs.next(), outputs.next()) {
            (Some(o), None) => o,
            (None, ..) => return Err(WriteActionValidationError::NoOutputs.into()),
            (Some(..), Some(..)) => return Err(WriteActionValidationError::TooManyOutputs.into()),
        };

        let is_command_line = contents.by_ref(|v| -> buck2_error::Result<bool> {
            Ok(ValueAsCommandLineLike::unpack_value(*v)?.is_some())
        })?;
        if !is_command_line {
            return Err(WriteActionValidationError::ContentsNotCommandLineValue(
                contents.by_ref(|v| v.to_repr()),
            )
            .into());
        }

        Ok(WriteAction {
            contents,
            output,
            inner,
        })
    }

    fn get_contents(
        &self,
        fs: &ExecutorFs,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<String> {
        let mut cli = Vec::<String>::new();

        self.contents.by_ref(|v| {
            WriteCommandLineOptions {
                absolute: self.inner.absolute,
                macro_files: self.inner.macro_files.as_deref(),
            }
            .render(*v, fs, artifact_path_mapping, &mut cli)
        })?;

        Ok(cli.join("\n"))
    }

    /// The write as the materializer performs it.
    fn write_request(&self, ctx: &dyn ActionExecutionCtx) -> buck2_error::Result<WriteRequest> {
        let content = if self.inner.use_dep_files_placeholder_for_content_based_paths {
            self.get_contents(
                &ctx.executor_fs(),
                &DepFilesPlaceholderArtifactPathMapper {},
            )?
        } else {
            self.get_contents(&ctx.executor_fs(), &ctx.artifact_path_mapping(None))?
        }
        .into_bytes();
        let path = ctx.fs().resolve_build(
            self.output.get_path(),
            if self.output.get_path().is_content_based_path() {
                let digest = TrackedFileDigest::from_content(
                    &content,
                    ctx.digest_config().cas_digest_config(),
                );
                Some(file_content_based_path_hash(
                    digest.raw_digest().as_bytes(),
                    self.inner.is_executable,
                )?)
            } else {
                None
            }
            .as_ref(),
        )?;
        Ok(WriteRequest {
            path,
            content,
            is_executable: self.inner.is_executable,
            path_kind: self.output.get_path().path_resolution_method(),
        })
    }

    fn cache_key(&self, ctx: &dyn ActionExecutionCtx) -> buck2_error::Result<WriteCacheKey> {
        let mapping;
        let artifact_path_mapping = if self.inner.use_dep_files_placeholder_for_content_based_paths
        {
            None
        } else {
            mapping = ctx.artifact_path_mapping(None);
            Some(&mapping)
        };
        let key = self.contents.by_ref(|v| {
            write_cache::key(
                *v,
                &KeyInputs {
                    is_executable: self.inner.is_executable,
                    path_separator: ctx.executor_fs().path_separator(),
                    macro_files: self.inner.macro_files.as_deref(),
                    artifact_path_mapping,
                },
                ctx.fs(),
            )
        })?;
        if let Some(dump) = buck2_env!(
            "BUCK2_TEST_WRITE_ACTION_CACHE_KEY_DUMP",
            applicability = testing
        )? {
            write_cache::dump_key(
                dump,
                self.output.get_path().owner(),
                self.output.get_path().path(),
                &key,
            )?;
        }
        Ok(key)
    }

    /// Declares the output as a write without the cache is declared.
    async fn execute_plain(
        &self,
        ctx: &dyn ActionExecutionCtx,
    ) -> buck2_error::Result<(ArtifactValue, ActionExecutionKind)> {
        let (values, execution_kind) =
            declare_writes(ctx, Box::new(|| Ok(vec![self.write_request(ctx)?]))).await?;
        Ok((single(values)?, execution_kind))
    }

    /// Declares the output through `cache`: as the content the cache has for it, or else the
    /// way a write without the cache is declared, recorded for later first where the write is
    /// one to record.
    async fn execute_cached(
        &self,
        ctx: &dyn ActionExecutionCtx,
        cache: WriteCache<'_>,
    ) -> buck2_error::Result<(ArtifactValue, ActionExecutionKind)> {
        let key = self.cache_key(ctx)?;
        let populate = match cache.use_for(&key) {
            CacheUse::None => return self.execute_plain(ctx).await,
            CacheUse::Lookup => false,
            CacheUse::Populate => true,
        };
        if let Some(cached) = cache.lookup(ctx, &key).await {
            let value = cached.value(self.inner.is_executable);
            write_cache::declare(ctx, &self.output, value.dupe(), cached.info).await?;
            return Ok((value, ActionExecutionKind::Deferred));
        }
        let request = self.write_request(ctx)?;
        if populate {
            match cache.populate(ctx, &key, &request).await {
                Ok((value, info)) => {
                    write_cache::declare(ctx, &self.output, value.dupe(), info).await?;
                    return Ok((value, ActionExecutionKind::Deferred));
                }
                Err(e) => {
                    WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.populate_failures);
                    // Producing the write anyway is the right fallback, but a cache that
                    // cannot be populated would silently stay empty, which should be visible.
                    soft_error!(
                        "write_action_cache_populate_failed",
                        e.context("Recording a write's content in the action cache failed")
                    );
                }
            }
        }
        write_cache::declare_miss(ctx, request).await
    }

    /// Declares the output as it would be declared without the cache, and checks the content
    /// the cache has for the write, if any, against what the write renders.
    async fn execute_verifying(
        &self,
        ctx: &dyn ActionExecutionCtx,
        cache: WriteCache<'_>,
    ) -> buck2_error::Result<(ArtifactValue, ActionExecutionKind)> {
        let key = self.cache_key(ctx)?;
        let cached = cache.lookup(ctx, &key).await;
        let request = self.write_request(ctx)?;
        if let Some(cached) = cached {
            let rendered = TrackedFileDigest::from_content(
                &request.content,
                ctx.digest_config().cas_digest_config(),
            );
            WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.verified);
            if cached.digest != rendered {
                WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.mismatches);
                soft_error!(
                    "write_action_cache_mismatch",
                    buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "Write action cache holds `{}` for `{}`, which renders as `{}`",
                        cached.digest,
                        request.path,
                        rendered
                    )
                );
            }
        }
        // Rendered once above; `declare_writes` asks again only for a probe miss.
        let mut rendered = Some(request);
        let (values, execution_kind) = declare_writes(
            ctx,
            Box::new(move || match rendered.take() {
                Some(request) => Ok(vec![request]),
                None => Ok(vec![self.write_request(ctx)?]),
            }),
        )
        .await?;
        Ok((single(values)?, execution_kind))
    }
}

fn single(values: Vec<ArtifactValue>) -> buck2_error::Result<ArtifactValue> {
    values
        .into_iter()
        .next()
        .internal_error("Write did not execute")
}

#[pagable_typetag]
#[async_trait]
impl Action for WriteAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::Write
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        if self.inner.use_dep_files_placeholder_for_content_based_paths {
            return Ok(Cow::Borrowed(&[]));
        }

        let mut visitor = CommandLineContentBasedInputVisitor::new();
        self.contents.by_ref(|v| -> buck2_error::Result<()> {
            ValueAsCommandLineLike::unpack_value_err(*v)?
                .0
                .visit_artifacts(&mut visitor)
        })?;
        let mut content_based_inputs = visitor.content_based_inputs;
        if let Some(macro_files) = &self.inner.macro_files {
            for artifact in macro_files {
                if artifact.path_resolution_requires_artifact_value() {
                    content_based_inputs.insert(ArtifactGroup::Artifact(artifact.dupe()));
                }
            }
        }
        Ok(Cow::Owned(content_based_inputs.into_iter().collect()))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(slice::from_ref(&self.output))
    }

    fn first_output(&self) -> &BuildArtifact {
        &self.output
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new("write")
    }

    fn identifier(&self) -> Option<&str> {
        Some(self.output.get_path().path().as_str())
    }

    fn aquery_attributes(
        &self,
        fs: &ExecutorFs,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<BuckMutInsertionMap<String, String>> {
        Ok(buck_mut_insertion_map! {
            "contents".to_owned() => self.get_contents(fs, artifact_path_mapping)?,
            "absolute".to_owned() => self.inner.absolute.to_string(),
        })
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        let mut execution_start = None;

        // A write with `absolute` renders the project root, which is not the same between
        // hosts, so the cache is not for it.
        let cache = if self.inner.absolute {
            None
        } else {
            WriteCache::for_ctx(ctx)
        };
        let (value, execution_kind) = match cache {
            Some(cache) if cache.verifies() => {
                execution_start = Some(Instant::now());
                self.execute_verifying(ctx, cache).await?
            }
            Some(cache) => {
                execution_start = Some(Instant::now());
                self.execute_cached(ctx, cache).await?
            }
            None => {
                let (values, execution_kind) = declare_writes(
                    ctx,
                    Box::new(|| {
                        execution_start = Some(Instant::now());
                        Ok(vec![self.write_request(ctx)?])
                    }),
                )
                .await?;
                (single(values)?, execution_kind)
            }
        };

        let wall_time = Instant::now()
            - execution_start.internal_error("Action did not set execution_start")?;

        Ok((
            ActionOutputs::new(buck_mut_insertion_map![self.output.get_path().dupe() => value]),
            ActionExecutionMetadata {
                dep_file_db_writes_queued: 0,
                execution_kind,
                timing: ActionExecutionTimingData { wall_time },
                input_files_bytes: None,
                waiting_data,
            },
        ))
    }
}

pub(crate) struct WriteCommandLineOptions<'a> {
    pub(crate) absolute: bool,
    pub(crate) macro_files: Option<&'a [Artifact]>,
}

impl WriteCommandLineOptions<'_> {
    pub(crate) fn render(
        &self,
        content: Value<'_>,
        fs: &ExecutorFs<'_>,
        artifact_path_mapping: &dyn ArtifactPathMapper,
        sink: &mut dyn CommandLineSink,
    ) -> buck2_error::Result<()> {
        let macro_files = self
            .macro_files
            .map(|files| {
                files
                    .iter()
                    .map(|a| a.resolve_path(fs.fs(), artifact_path_mapping.get(a)))
                    .collect::<buck2_error::Result<Vec<_>>>()
            })
            .transpose()?;
        let mut builder = CommandLineBuilder::new_with_options(
            sink,
            artifact_path_mapping,
            fs,
            self.absolute,
            macro_files,
        );
        ValueAsCommandLineLike::unpack_value_err(content)?
            .0
            .add_to_command_line(&mut builder)
    }
}

#[cfg(test)]
mod tests {
    // TODO: This needs proper tests, but right now it's kind of a pain to get the
    //       action framework up and running to test actions
    #[test]
    fn writes_file() {}
}
