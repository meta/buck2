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

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::artifact::artifact_dyn::ArtifactDyn;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::Action;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::UnregisteredAction;
use buck2_build_api::actions::box_slice_set::BoxSliceSet;
use buck2_build_api::actions::errors::execute_error::ExecuteError;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::execute::action_executor::ActionExecutionMetadata;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_signals::env::WaitingData;
use buck2_core::category::CategoryRef;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_error::BuckErrorOptionContext;
use buck2_execute::artifact_utils::ArtifactValueBuilder;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::materializer::CopiedArtifact;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::buck_mut_insertion_set;
use dupe::Dupe;
use itertools::Itertools;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::values::OwnedFrozen;
use starlark::values::Value;

use crate::actions::impls::cas_probe;
use crate::actions::impls::produce;
use crate::actions::impls::produce::SourceCopy;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum CopyActionValidationError {
    #[error("Exactly one output file must be specified for a copy action, got {0}")]
    WrongNumberOfOutputs(usize),
    #[error("Only artifact inputs are supported in copy actions, got {0}")]
    UnsupportedInput(ArtifactGroup),
}

#[derive(Debug, Allocative, Pagable, Clone, Copy, Dupe)]
pub(crate) enum CopyMode {
    Copy {
        // Override the destination executable bit to +x (true) or -x (false)
        executable_bit_override: Option<bool>,
    },
    Symlink,
}

#[derive(Allocative)]
pub(crate) struct UnregisteredCopyAction {
    src: ArtifactGroup,
    copy: CopyMode,
}

impl UnregisteredCopyAction {
    pub(crate) fn new(src: ArtifactGroup, copy: CopyMode) -> Self {
        Self { src, copy }
    }
}

impl UnregisteredAction for UnregisteredCopyAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        _starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        Ok(Box::new(CopyAction::new(self.copy, self.src, outputs)?))
    }
}

#[derive(Debug, Allocative, Pagable)]
struct CopyAction {
    copy: CopyMode,
    inputs: BoxSliceSet<ArtifactGroup>,
    outputs: BoxSliceSet<BuildArtifact>,
}

impl CopyAction {
    fn new(
        copy: CopyMode,
        src: ArtifactGroup,
        outputs: BuckMutInsertionSet<BuildArtifact>,
    ) -> buck2_error::Result<Self> {
        // TODO: Exclude other variants once they become available here. For now, this is a noop.
        match src {
            ArtifactGroup::Artifact(..) | ArtifactGroup::Promise(..) => {}
            ArtifactGroup::TransitiveSetProjection(..) => {
                return Err(CopyActionValidationError::UnsupportedInput(src.dupe()).into());
            }
        };

        if outputs.len() != 1 {
            Err(CopyActionValidationError::WrongNumberOfOutputs(outputs.len()).into())
        } else {
            Ok(CopyAction {
                copy,
                inputs: BoxSliceSet::from(buck_mut_insertion_set![src]),
                outputs: BoxSliceSet::from(outputs),
            })
        }
    }

    fn input(&self) -> &ArtifactGroup {
        self.inputs
            .iter()
            .next()
            .expect("a single input by construction")
    }

    fn output(&self) -> &BuildArtifact {
        self.outputs
            .iter()
            .next()
            .expect("a single artifact by construction")
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for CopyAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::Copy
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        Ok(Cow::Borrowed(self.inputs.as_slice()))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(self.outputs.as_slice())
    }

    fn first_output(&self) -> &BuildArtifact {
        self.output()
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new("copy")
    }

    fn identifier(&self) -> Option<&str> {
        Some(self.output().get_path().path().as_str())
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        let (input, src_value) = ctx
            .artifact_values(self.input())
            .iter()
            .exactly_one()
            .ok()
            .internal_error("Input did not dereference to exactly one artifact")?;

        let artifact_fs = ctx.fs();
        let src = input.resolve_path(
            artifact_fs,
            if input.path_resolution_requires_artifact_value() {
                Some(src_value.content_based_path_hash())
            } else {
                None
            }
            .as_ref(),
        )?;
        let tmp_dest = artifact_fs.resolve_build(
            self.output().get_path(),
            Some(&ContentBasedPathHash::for_output_artifact()),
        )?;

        let value = {
            let fs = artifact_fs.fs();
            let mut builder = ArtifactValueBuilder::new(fs, ctx.digest_config());
            match self.copy {
                CopyMode::Copy {
                    executable_bit_override,
                } => {
                    builder.add_copied(
                        src_value,
                        src.as_ref(),
                        tmp_dest.as_ref(),
                        executable_bit_override,
                    )?;
                }
                CopyMode::Symlink => {
                    builder.add_symlinked(src_value, src.clone(), tmp_dest.as_ref())?;
                }
            }

            builder.build(tmp_dest.as_ref())?
        };

        let dest = if self.output().get_path().is_content_based_path() {
            artifact_fs.resolve_build(
                self.output().get_path(),
                Some(&value.content_based_path_hash()),
            )?
        } else {
            tmp_dest
        };

        // A source file's content is not known to be in the CAS. The disk-state core is declared
        // no copy of it: the probe spares making one when the CAS has the file, and a miss comes
        // to be as a write's does (`produce`). The deferred core is declared the copy, made when
        // something needs the file. A build artifact's content is the materializer's already, and
        // a symlink copies nothing; both are declared, which is nothing to the disk-state core.
        let source_copy = match self.copy {
            CopyMode::Copy {
                executable_bit_override,
            } if input.is_source() => Some(executable_bit_override),
            _ => None,
        };
        let (execution_kind, declare) = match source_copy {
            Some(executable_bit_override) if ctx.materializer().is_disk_state() => {
                let files = cas_probe::files_of(value.entry(), &src);
                match cas_probe::missing(ctx, &files).await {
                    Some((info, missing)) if missing.is_empty() => {
                        ctx.materializer()
                            .declare_cas_many(
                                info,
                                vec![DeclareArtifactPayload {
                                    path: dest.clone(),
                                    artifact: value.dupe(),
                                }],
                            )
                            .await?;
                        (ActionExecutionKind::Deferred, false)
                    }
                    probed => {
                        let kind = produce::produce_copies(
                            ctx,
                            vec![SourceCopy {
                                src: src.clone(),
                                dest: dest.clone(),
                                value: value.dupe(),
                                executable_bit_override,
                            }],
                            probed.is_some(),
                        )
                        .await?;
                        (kind, false)
                    }
                }
            }
            _ => (ActionExecutionKind::Simple, true),
        };
        if declare {
            ctx.materializer()
                .declare_copy(
                    dest.clone(),
                    value.dupe(),
                    // FIXME(JakobDegen): This is wrong in cases where the input artifact is a
                    // source directory with ignored paths, as the materializer will incorrectly
                    // assume that the source directory matches the artifact value when it
                    // doesn't.
                    vec![CopiedArtifact::new(
                        src,
                        dest,
                        value.entry().dupe().map_dir(|d| d.as_immutable()),
                        match self.copy {
                            CopyMode::Copy {
                                executable_bit_override,
                            } => executable_bit_override,
                            CopyMode::Symlink => None,
                        },
                    )],
                )
                .await?;
        }

        Ok((
            ActionOutputs::from_single(self.output().get_path().dupe(), value),
            ActionExecutionMetadata {
                dep_file_db_writes_queued: 0,
                execution_kind,
                timing: ActionExecutionTimingData::default(),
                input_files_bytes: None,
                waiting_data,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    // TODO: This needs proper tests, but right now it's kind of a pain to get the
    //       action framework up and running to test actions
    #[test]
    fn copies_file() {}
}
