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
use std::fmt;
use std::slice;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::Action;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::UnregisteredAction;
use buck2_build_api::actions::errors::execute_error::ExecuteError;
use buck2_build_api::actions::execute::action_executor::ActionExecutionMetadata;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_build_api::actions::impls::json;
use buck2_build_api::actions::impls::json::JsonUnpack;
use buck2_build_api::actions::impls::json::validate_json;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::command_line_arg_like_impl;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_build_api::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_build_signals::env::WaitingData;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::category::CategoryRef;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_error::BuckErrorOptionContext;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::buck_mut_insertion_map;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_complex_value;
use starlark::starlark_module;
use starlark::values::Demand;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::starlark_value;

use crate::actions::impls::declare_writes::declare_writes;
use crate::actions::impls::run::DepFilesPlaceholderArtifactPathMapper;
use crate::actions::impls::write::CommandLineContentBasedInputVisitor;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum WriteJsonActionValidationError {
    #[error("WriteJsonAction received no outputs")]
    NoOutputs,
    #[error("WriteJsonAction received more than one output")]
    TooManyOutputs,
}

#[derive(Allocative, Debug, Pagable)]
pub(crate) struct UnregisteredWriteJsonAction {
    pretty: bool,
    absolute: bool,
    use_dep_files_placeholder_for_content_based_paths: bool,
}

impl UnregisteredWriteJsonAction {
    pub(crate) fn new(
        pretty: bool,
        absolute: bool,
        use_dep_files_placeholder_for_content_based_paths: bool,
    ) -> Self {
        Self {
            pretty,
            absolute,
            use_dep_files_placeholder_for_content_based_paths,
        }
    }

    pub(crate) fn cli<'v>(
        artifact: Value<'v>,
        content: Value<'v>,
    ) -> buck2_error::Result<StarlarkWriteJsonCommandLineArg<'v>> {
        Ok(StarlarkWriteJsonCommandLineArg { artifact, content })
    }
}

impl UnregisteredAction for UnregisteredWriteJsonAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        let contents = starlark_data.expect("module data to be present");
        let action = WriteJsonAction::new(contents, outputs, *self)?;
        Ok(Box::new(action))
    }
}

#[derive(Debug, Allocative, Pagable)]
struct WriteJsonAction {
    contents: OwnedFrozen<Value<'static>>, // JSON value
    output: BuildArtifact,
    inner: UnregisteredWriteJsonAction,
}

impl WriteJsonAction {
    fn new(
        contents: OwnedFrozen<Value<'static>>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        inner: UnregisteredWriteJsonAction,
    ) -> buck2_error::Result<Self> {
        contents.by_ref(|v| -> buck2_error::Result<()> {
            validate_json(JsonUnpack::unpack_value_err(*v)?)
        })?;

        let mut outputs = outputs.into_iter();

        let output = match (outputs.next(), outputs.next()) {
            (Some(o), None) => o,
            (None, ..) => return Err(WriteJsonActionValidationError::NoOutputs.into()),
            (Some(..), Some(..)) => {
                return Err(WriteJsonActionValidationError::TooManyOutputs.into());
            }
        };

        Ok(WriteJsonAction {
            contents,
            output,
            inner,
        })
    }

    fn get_contents(
        &self,
        fs: &ExecutorFs,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<Vec<u8>> {
        let mut writer = Vec::new();
        self.contents.by_ref(|v| -> buck2_error::Result<()> {
            json::write_json(
                JsonUnpack::unpack_value_err(*v)?,
                Some(fs),
                &mut writer,
                self.inner.pretty,
                self.inner.absolute,
                artifact_path_mapping,
            )
        })?;
        Ok(writer)
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for WriteJsonAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::Write
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        if self.inner.use_dep_files_placeholder_for_content_based_paths {
            return Ok(Cow::Borrowed(&[]));
        }

        let mut visitor = CommandLineContentBasedInputVisitor::new();
        self.contents
            .by_ref(|v| json::visit_json_artifacts(*v, &mut visitor))?;
        Ok(Cow::Owned(
            visitor.content_based_inputs.into_iter().collect(),
        ))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(slice::from_ref(&self.output))
    }

    fn first_output(&self) -> &BuildArtifact {
        &self.output
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new("write_json")
    }

    fn identifier(&self) -> Option<&str> {
        Some(self.output.get_path().path().as_str())
    }

    fn aquery_attributes(
        &self,
        fs: &ExecutorFs,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<BuckMutInsertionMap<String, String>> {
        let content = self.get_contents(fs, artifact_path_mapping)?;
        Ok(buck_mut_insertion_map! {
            "contents".to_owned() => String::from_utf8(content)?,
            "absolute".to_owned() => self.inner.absolute.to_string(),
        })
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        let fs = ctx.fs();

        let mut execution_start = None;
        let (values, execution_kind) = declare_writes(
            ctx,
            Box::new(|| {
                execution_start = Some(Instant::now());
                let content = if self.inner.use_dep_files_placeholder_for_content_based_paths {
                    self.get_contents(
                        &ctx.executor_fs(),
                        &DepFilesPlaceholderArtifactPathMapper {},
                    )?
                } else {
                    self.get_contents(&ctx.executor_fs(), &ctx.artifact_path_mapping(None))?
                };
                let path = fs.resolve_build(
                    self.output.get_path(),
                    if self.output.get_path().is_content_based_path() {
                        let digest = TrackedFileDigest::from_content(
                            &content,
                            ctx.digest_config().cas_digest_config(),
                        );
                        Some(ContentBasedPathHash::new(digest.raw_digest().as_bytes())?)
                    } else {
                        None
                    }
                    .as_ref(),
                )?;
                Ok(vec![WriteRequest {
                    path,
                    content,
                    is_executable: false,
                    path_kind: self.output.get_path().path_resolution_method(),
                }])
            }),
        )
        .await?;
        let value = values
            .into_iter()
            .next()
            .internal_error("Write did not execute")?;

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

/// WriteJsonCommandLineArgGen represents the artifact produced by write_json in a way that it can
/// be added to commandlines while including the artifacts referenced by cmdargs in the content that
/// was written.
#[derive(
    Debug,
    Clone,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[derive(NoSerialize)] // TODO we should probably have a serialization for transitive set
pub(crate) struct StarlarkWriteJsonCommandLineArg<'v> {
    artifact: Value<'v>,
    // The list of artifacts here could be large and we don't want to hold those explicitly (due to
    // the memory cost) and so we hold the same content value that the write_json action itself will and
    // only traverse it when artifacts are requested.
    content: Value<'v>,
}

impl<'v> fmt::Display for StarlarkWriteJsonCommandLineArg<'v> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<WriteJsonCliArgs>")
    }
}

starlark_complex_value!(pub(crate) StarlarkWriteJsonCommandLineArg);

#[starlark_value(type = "WriteJsonCliArgs")]
impl<'v> StarlarkValue<'v> for StarlarkWriteJsonCommandLineArg<'v> {
    fn provide(&'v self, demand: &mut Demand<'_, 'v>) {
        demand.provide_value::<&dyn CommandLineArgLike>(self);
    }
}

impl<'v> StarlarkWriteJsonCommandLineArg<'v> {
    pub fn visit_contents(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        json::visit_json_artifacts(self.content, visitor)
    }
}

impl<'v> CommandLineArgLike<'v> for StarlarkWriteJsonCommandLineArg<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(StarlarkWriteJsonCommandLineArg::get_type_starlark_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        ValueAsCommandLineLike::unpack_value_err(self.artifact)?
            .0
            .add_to_command_line(fmt)
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        ValueAsCommandLineLike::unpack_value_err(self.artifact)?
            .0
            .visit_artifacts(visitor)?;
        json::visit_json_artifacts(self.content, visitor)
    }

    fn contains_arg_attr(&self) -> bool {
        // In the write_json implementation, the CommandLineBuilder we use don't support args.
        false
    }

    fn visit_write_to_file_macros(
        &self,
        _visitor: &mut dyn WriteToFileMacroVisitor,
        _artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        // In the write_json implementation, the CommandLineBuilder we use don't support args.
        Ok(())
    }
}

#[starlark_module]
#[starlark_types(
    StarlarkWriteJsonCommandLineArg<'_> as WriteJsonCliArgs
)]
pub(crate) fn register_write_json_cli_args(globals: &mut GlobalsBuilder) {}
