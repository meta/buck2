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
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_build_api::interpreter::rule_defs::cmd_args::SingletonCommandLineSink;
use buck2_build_api::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use buck2_build_api::interpreter::rule_defs::resolved_macro::ResolvedMacro;
use buck2_build_signals::env::WaitingData;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::category::CategoryRef;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorOptionContext;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::values::OwnedFrozen;
use starlark::values::UnpackValue;
use starlark::values::Value;

use crate::actions::impls::declare_writes::declare_writes;
use crate::actions::impls::run::DepFilesPlaceholderArtifactPathMapper;
use crate::actions::impls::write::CommandLineContentBasedInputVisitor;

#[derive(Debug, Allocative, Pagable)]
pub(crate) struct UnregisteredWriteMacrosToFileAction {
    identifier: String,
    use_dep_files_placeholder_for_content_based_paths: bool,
}

impl UnregisteredWriteMacrosToFileAction {
    pub(crate) fn new(
        identifier: String,
        use_dep_files_placeholder_for_content_based_paths: bool,
    ) -> Self {
        Self {
            identifier,
            use_dep_files_placeholder_for_content_based_paths,
        }
    }
}

impl UnregisteredAction for UnregisteredWriteMacrosToFileAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        let contents = starlark_data.expect("Action data should be present");

        let action = WriteMacrosToFileAction::new(contents, outputs, *self)?;

        Ok(Box::new(action))
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum WriteMacrosActionValidationError {
    #[error("At least one output file must be specified for a write macros action")]
    NoOutputsSpecified,
    #[error("Expected command line value, got {0}")]
    ContentsNotCommandLineValue(String),
    #[error(
        "Number of write-to-file macro artifacts during analysis time should be consistent with when those artifacts are created"
    )]
    InconsistentNumberOfMacroArtifacts,
}

#[derive(Debug, Allocative, Pagable)]
struct WriteMacrosToFileAction {
    contents: OwnedFrozen<Value<'static>>, // StarlarkCmdArgs
    outputs: MiniBoxSlice<BuildArtifact>,
    inner: UnregisteredWriteMacrosToFileAction,
}

impl WriteMacrosToFileAction {
    fn new(
        contents: OwnedFrozen<Value<'static>>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        inner: UnregisteredWriteMacrosToFileAction,
    ) -> buck2_error::Result<Self> {
        let is_command_line = contents.by_ref(|v| -> buck2_error::Result<bool> {
            Ok(<&dyn CommandLineArgLike>::unpack_value(*v)?.is_some())
        })?;
        if outputs.is_empty() {
            Err(WriteMacrosActionValidationError::NoOutputsSpecified.into())
        } else if !is_command_line {
            Err(
                WriteMacrosActionValidationError::ContentsNotCommandLineValue(
                    contents.by_ref(|v| v.to_repr()),
                )
                .into(),
            )
        } else {
            Ok(Self {
                contents,
                outputs: outputs.into_iter().collect(),
                inner,
            })
        }
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for WriteMacrosToFileAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::WriteMacrosToFile
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        if self.inner.use_dep_files_placeholder_for_content_based_paths {
            return Ok(Cow::Borrowed(&[]));
        }

        let mut visitor = CommandLineContentBasedInputVisitor::new();
        self.contents.by_ref(|v| -> buck2_error::Result<()> {
            <&dyn CommandLineArgLike>::unpack_value(*v)?
                .unwrap()
                .visit_write_to_file_macros(&mut visitor, &BuckMutMap::default())
        })?;
        Ok(Cow::Owned(
            visitor.content_based_inputs.into_iter().collect(),
        ))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(&self.outputs)
    }

    fn first_output(&self) -> &BuildArtifact {
        // Required to have outputs on construction
        &self.outputs[0]
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new("write_macros_to_file")
    }

    fn identifier(&self) -> Option<&str> {
        Some(&self.inner.identifier)
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        let mut execution_start = None;

        let (values, execution_kind) = declare_writes(
            ctx,
            Box::new(|| {
                execution_start = Some(Instant::now());

                let fs = ctx.executor_fs();

                let mut output_contents = Vec::with_capacity(self.outputs.len());
                let mut macro_writer = MacroToFileWriter::new(&fs, &mut output_contents);

                self.contents.by_ref(|v| -> buck2_error::Result<()> {
                    let command_line = <&dyn CommandLineArgLike>::unpack_value(*v)?.unwrap();

                    if self.inner.use_dep_files_placeholder_for_content_based_paths {
                        command_line.visit_write_to_file_macros(
                            &mut macro_writer,
                            &DepFilesPlaceholderArtifactPathMapper {},
                        )
                    } else {
                        command_line.visit_write_to_file_macros(
                            &mut macro_writer,
                            &ctx.artifact_path_mapping(None),
                        )
                    }
                })?;

                if self.outputs.len() != output_contents.len() {
                    return Err(
                        WriteMacrosActionValidationError::InconsistentNumberOfMacroArtifacts.into(),
                    );
                }

                std::iter::zip(self.outputs.iter(), output_contents.into_iter())
                    .map(|(output, content)| {
                        let content = content.into_bytes();
                        let path = if output.get_path().is_content_based_path() {
                            let digest = TrackedFileDigest::from_content(
                                &content,
                                ctx.digest_config().cas_digest_config(),
                            );
                            fs.fs().resolve_build(
                                output.get_path(),
                                Some(&ContentBasedPathHash::new(digest.raw_digest().as_bytes())?),
                            )?
                        } else {
                            fs.fs().resolve_build(output.get_path(), None)?
                        };
                        Ok(WriteRequest {
                            path,
                            content,
                            is_executable: false,
                            path_kind: output.get_path().path_resolution_method(),
                        })
                    })
                    .collect::<buck2_error::Result<_>>()
            }),
        )
        .await?;

        let wall_time = Instant::now()
            - execution_start.internal_error("Action did not set execution_start")?;

        let output_values = std::iter::zip(self.outputs.iter(), values)
            .map(|(output, value)| (output.get_path().dupe(), value))
            .collect();

        Ok((
            ActionOutputs::new(output_values),
            ActionExecutionMetadata {
                dep_file_db_writes_queued: 0,
                execution_kind,
                timing: ActionExecutionTimingData {
                    wall_time,
                    ..Default::default()
                },
                input_files_bytes: None,
                waiting_data,
            },
        ))
    }
}

impl WriteToFileMacroVisitor for CommandLineContentBasedInputVisitor {
    fn visit_write_to_file_macro(
        &mut self,
        resolved_macro: &ResolvedMacro,
        _artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        resolved_macro.visit_artifacts(self)
    }

    fn set_current_relative_to_path(&mut self, _p: ProjectRelativePathBuf) {}

    fn fs(&self) -> Option<&ExecutorFs<'_>> {
        None
    }
}

struct MacroToFileWriter<'a> {
    fs: &'a ExecutorFs<'a>,
    outputs: &'a mut Vec<String>,
    relative_to_path: Option<ProjectRelativePathBuf>,
}

impl<'a> MacroToFileWriter<'a> {
    fn new(fs: &'a ExecutorFs<'a>, outputs: &'a mut Vec<String>) -> Self {
        Self {
            fs,
            outputs,
            relative_to_path: None,
        }
    }
}

impl WriteToFileMacroVisitor for MacroToFileWriter<'_> {
    fn visit_write_to_file_macro(
        &mut self,
        resolved_macro: &ResolvedMacro,
        artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        let content = {
            let mut writer = SingletonCommandLineSink::new();
            let mut fmt = CommandLineBuilder::new(&mut writer, artifact_path_mapping, self.fs);
            fmt.push_scope_delimiter("");
            if let Some(p) = self.relative_to_path.as_ref() {
                fmt.push_scope_relative_to(p.clone());
            }
            resolved_macro.add_to_arg(&mut fmt)?;
            if self.relative_to_path.is_some() {
                fmt.pop_scope();
            }
            fmt.pop_scope();
            writer.finalize()?
        };

        self.outputs.push(content);
        Ok(())
    }

    fn set_current_relative_to_path(&mut self, p: ProjectRelativePathBuf) {
        // FIXME(JakobDegen): Completely broken, doesn't respect the cmd_args nesting structure.
        self.relative_to_path = Some(p);
    }

    fn fs(&self) -> Option<&ExecutorFs<'_>> {
        Some(self.fs)
    }
}
