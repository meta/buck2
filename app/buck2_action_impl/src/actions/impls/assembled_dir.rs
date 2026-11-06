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
use buck2_build_api::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_build_signals::env::WaitingData;
use buck2_core::category::CategoryRef;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_execute::artifact_utils::ArtifactValueBuilder;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::materializer::CopiedArtifact;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutInsertionSet;
use dupe::Dupe;
use itertools::Itertools;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::values::OwnedFrozen;
use starlark::values::Value;
use starlark::values::ValueError;
use starlark::values::ValueOf;
use starlark::values::dict::DictType;

use crate::actions::impls::cas_probe;
use crate::actions::impls::cas_probe::SourceFile;
use crate::actions::impls::copy::CopyMode;
use crate::actions::impls::produce;
use crate::actions::impls::produce::SourceCopy;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum AssembledDirError {
    #[error(
        "Paths to copied_dir/symlinked_dir/assembled_dir must be non-overlapping, but got `{0}` and `{1}`"
    )]
    OverlappingPaths(Box<ForwardRelativePath>, Box<ForwardRelativePath>),
    #[error("Paths to copied_dir/symlinked_dir/assembled_dir must not be empty")]
    EmptyPath,
    #[error(
        "Only artifact inputs are supported in copied_dir/symlinked_dir/assembled_dir actions, got {0}"
    )]
    UnsupportedInput(ArtifactGroup),
}

/// The shared implementation behind `copied_dir`, `symlinked_dir` and
/// `assembled_dir`: one directory assembled from input artifacts, each laid
/// out by its own `CopyMode`. `copied_dir` / `symlinked_dir` apply a uniform
/// mode to every entry; `assembled_dir` mixes modes per entry.
/// Which Starlark surface built this action: `symlinked_dir` for the
/// uniform-mode constructors (`copied_dir` / `symlinked_dir`, preserving
/// their pre-existing action category) and `assembled_dir` for the
/// mixed-mode form. Only affects the action's reported category.
#[derive(Debug, Allocative, Pagable, Clone, Copy, Dupe)]
pub(crate) enum DirActionCategory {
    SymlinkedDir,
    AssembledDir,
}

impl DirActionCategory {
    fn as_str(self) -> &'static str {
        match self {
            DirActionCategory::SymlinkedDir => "symlinked_dir",
            DirActionCategory::AssembledDir => "assembled_dir",
        }
    }
}

#[derive(Allocative)]
pub(crate) struct UnregisteredAssembledDirAction {
    args: Vec<(ArtifactGroup, Box<ForwardRelativePath>, CopyMode)>,
    // All associated artifacts of inputs unioned together
    unioned_associated_artifacts: AssociatedArtifacts,
    category: DirActionCategory,
}

impl UnregisteredAssembledDirAction {
    /// Validate that no output path is duplicated or overlapping.
    /// Duplicates are easy - overlapping only happens with symlinks to directories (a bad idea),
    /// and would look like `a` and `a/b` both being given.
    fn validate_args(
        args: &mut [(ArtifactGroup, Box<ForwardRelativePath>, CopyMode)],
    ) -> buck2_error::Result<()> {
        // We sort the inputs. They are morally a set, so it shouldn't matter too much,
        // and this lets us implement the overlap check more easily.
        args.sort_by(|x, y| x.1.iter().cmp(y.1.iter()));

        for ((_, x, _), (_, y, _)) in args.iter().zip(args.iter().skip(1)) {
            if y.starts_with(x) {
                return Err(AssembledDirError::OverlappingPaths(x.clone(), y.clone()).into());
            }
        }
        if args.len() == 1 && args[0].1.is_empty() {
            return Err(AssembledDirError::EmptyPath.into());
        }

        for (g, _, _) in args.iter() {
            // TODO: Exclude other variants once they become available here. For now, this is a
            // noop.
            match g {
                ArtifactGroup::Artifact(..) | ArtifactGroup::Promise(..) => {}
                other => return Err(AssembledDirError::UnsupportedInput(other.dupe()).into()),
            };
        }

        Ok(())
    }

    // Map each artifact into an optional tuple of (artifact, path) and associated_artifacts, then collect
    // them into an optional tuple of vector and an index set respectively
    fn unpack_args<'v>(
        srcs: ValueOf<'v, DictType<&'v str, StarlarkArtifactView<'v>>>,
        copy: CopyMode,
    ) -> buck2_error::Result<(
        Vec<(ArtifactGroup, Box<ForwardRelativePath>, CopyMode)>,
        BuckMutInsertionSet<ArtifactGroup>,
    )> {
        let len = srcs.dict().len();
        srcs.try_items()
            .map(|entry| {
                let (k, as_artifact) = entry?;
                let as_artifact = as_artifact.as_input_artifact_like();
                let associates = as_artifact.get_associated_artifacts();
                buck2_error::Ok((
                    (
                        as_artifact.get_artifact_group()?,
                        ForwardRelativePathBuf::try_from(k.to_owned())
                            .buck_error_context("dict key must be a forward relative path")?
                            .into_box(),
                        copy,
                    ),
                    associates,
                ))
            })
            .fold_ok(
                (Vec::with_capacity(len), BuckMutInsertionSet::new()),
                |(mut aps, mut assocs), (ap, assoc)| {
                    aps.push(ap);
                    assoc.iter().flat_map(|v| v.iter()).for_each(|a| {
                        assocs.insert(a.dupe());
                    });
                    (aps, assocs)
                },
            )
    }

    pub(crate) fn new<'v>(
        copy: CopyMode,
        srcs: ValueOf<'v, DictType<&'v str, StarlarkArtifactView<'v>>>,
    ) -> buck2_error::Result<Self> {
        let (mut args, unioned_associated_artifacts) = Self::unpack_args(srcs, copy)
            // FIXME: This warning is talking about the Starlark-level argument name `srcs`.
            //        Once we use a proper Value parser this should all get cleaned up.
            .buck_error_context(
                ValueError::IncorrectParameterTypeNamed("srcs".to_owned()).to_string(),
            )?;
        // Overlapping check make sense for non-copy mode only.
        // When directories are copied into the same destination, the ordering defines how files are overwritten.
        match copy {
            CopyMode::Symlink => Self::validate_args(&mut args)?,
            CopyMode::Copy { .. } => (),
        };
        Ok(Self {
            args,
            unioned_associated_artifacts: AssociatedArtifacts::from(unioned_associated_artifacts),
            category: DirActionCategory::SymlinkedDir,
        })
    }

    /// Build a dir whose entries mix copied files and symlinks: `Copy`
    /// entries are laid out as real (content-addressed) bytes, `Symlink`
    /// entries point at their source artifacts. All destination paths must be
    /// non-overlapping -- mixing copies and symlinks under overlapping paths
    /// is ill-defined.
    pub(crate) fn new_assembled<'v>(
        contents: Vec<(&'v str, StarlarkArtifactView<'v>, CopyMode)>,
    ) -> buck2_error::Result<Self> {
        let mut args = Vec::with_capacity(contents.len());
        let mut assocs = BuckMutInsertionSet::new();
        for (k, as_artifact, mode) in contents {
            let as_artifact = as_artifact.as_input_artifact_like();
            let associates = as_artifact.get_associated_artifacts();
            args.push((
                as_artifact.get_artifact_group()?,
                ForwardRelativePathBuf::try_from(k.to_owned())
                    .buck_error_context("dict key must be a forward relative path")?
                    .into_box(),
                mode,
            ));
            associates.iter().flat_map(|v| v.iter()).for_each(|a| {
                assocs.insert(a.dupe());
            });
        }
        Self::validate_args(&mut args)?;
        Ok(Self {
            args,
            unioned_associated_artifacts: AssociatedArtifacts::from(assocs),
            category: DirActionCategory::AssembledDir,
        })
    }

    pub(crate) fn unioned_associated_artifacts(&self) -> AssociatedArtifacts {
        self.unioned_associated_artifacts.dupe()
    }
}

impl UnregisteredAction for UnregisteredAssembledDirAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        _starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        Ok(Box::new(AssembledDirAction {
            args: self.args.into(),
            outputs: BoxSliceSet::from(outputs),
            category: self.category,
        }))
    }
}

#[derive(Debug, Allocative, Pagable)]
struct AssembledDirAction {
    args: MiniBoxSlice<(ArtifactGroup, Box<ForwardRelativePath>, CopyMode)>,
    outputs: BoxSliceSet<BuildArtifact>,
    category: DirActionCategory,
}

impl AssembledDirAction {
    fn output(&self) -> &BuildArtifact {
        self.outputs
            .iter()
            .next()
            .expect("a single artifact by construction")
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for AssembledDirAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::SymlinkedDir
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        Ok(Cow::Owned(self.args.iter().map(|x| x.0.dupe()).collect()))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(self.outputs.as_slice())
    }

    fn first_output(&self) -> &BuildArtifact {
        self.output()
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new(self.category.as_str())
    }

    fn identifier(&self) -> Option<&str> {
        Some(self.output().get_path().path().as_str())
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        let fs = ctx.fs().fs();
        let temp_output = ctx.fs().resolve_build(
            self.output().get_path(),
            Some(&ContentBasedPathHash::for_output_artifact()),
        )?;
        let mut builder = ArtifactValueBuilder::new(fs, ctx.digest_config());
        let mut srcs = Vec::new();
        let mut source_files = Vec::new();

        for (group, relative_dest, copy) in &self.args {
            let (src_artifact, value) =
                ctx.artifact_values(group)
                    .iter()
                    .exactly_one()
                    .map_err(|_| {
                        internal_error!("Input did not dereference to exactly one artifact")
                    })?;

            let src = src_artifact.resolve_path(
                ctx.fs(),
                if src_artifact.path_resolution_requires_artifact_value() {
                    Some(value.content_based_path_hash())
                } else {
                    None
                }
                .as_ref(),
            )?;
            let temp_dest = temp_output.join(relative_dest);

            match copy {
                CopyMode::Copy {
                    executable_bit_override,
                } => {
                    let dest_entry = builder.add_copied(
                        value,
                        src.as_ref(),
                        temp_dest.as_ref(),
                        *executable_bit_override,
                    )?;
                    if src_artifact.is_source() {
                        for (path, metadata) in cas_probe::walk_files(&dest_entry) {
                            source_files.push((
                                src.join(&path),
                                relative_dest.join(&path),
                                metadata,
                                *executable_bit_override,
                            ));
                        }
                    }
                    srcs.push((
                        src,
                        relative_dest,
                        dest_entry.map_dir(|d| d.as_immutable()),
                        *executable_bit_override,
                    ));
                }
                CopyMode::Symlink => {
                    builder.add_symlinked(value, src, temp_dest.as_ref())?;
                }
            };
        }

        // The action defines the whole of the output directory, which is what lets the
        // materializer clear what a previous run left under it.
        let value = builder.build_exhaustive(temp_output.as_ref())?;
        let actual_output = ctx.fs().resolve_build(
            self.output().get_path(),
            if self.output().get_path().is_content_based_path() {
                Some(value.content_based_path_hash())
            } else {
                None
            }
            .as_ref(),
        )?;
        let materializer = ctx.materializer();
        let execution_kind = if materializer.is_disk_state() {
            // The output's structure is in its value; copies of buck-out content are nothing to
            // the core and symlinks copy nothing. Copied source files are not known to be in the
            // CAS, and the probe a `copy` makes asks about them a file at a time: the files are
            // action inputs the CAS usually has, while the directory assembled here is in the CAS
            // only if some action uploaded it in this exact shape. The files the CAS lacks are
            // made under their own paths, or uploaded, as a write's misses are; the rest is CAS
            // content.
            let files: Vec<SourceFile> = source_files
                .iter()
                .map(|(src, _, metadata, _)| (src.clone(), metadata.dupe()))
                .collect();
            let probed = if files.is_empty() {
                None
            } else {
                cas_probe::missing(ctx, &files).await
            };
            let (uploadable, to_make): (bool, Vec<usize>) = match &probed {
                Some((_, missing)) => (true, missing.clone()),
                None => (false, (0..source_files.len()).collect()),
            };
            let copies: Vec<SourceCopy> = to_make
                .into_iter()
                .map(|i| {
                    let (src, path, metadata, executable_bit_override) = &source_files[i];
                    SourceCopy {
                        src: src.clone(),
                        dest: actual_output.join(path),
                        value: ArtifactValue::file(metadata.dupe()),
                        executable_bit_override: *executable_bit_override,
                    }
                })
                .collect();
            if copies.is_empty() {
                if probed.is_some() {
                    ActionExecutionKind::Deferred
                } else {
                    ActionExecutionKind::Simple
                }
            } else {
                produce::produce_copies(ctx, copies, uploadable).await?
            }
        } else {
            // The deferred core is declared the copy, made when something needs the directory.
            let srcs = srcs
                .into_iter()
                .map(
                    |(src, relative_dest, dest_entry, executable_bit_override)| {
                        CopiedArtifact::new(
                            src,
                            actual_output.join(relative_dest),
                            dest_entry,
                            executable_bit_override,
                        )
                    },
                )
                .collect_vec();
            materializer
                .declare_copy(actual_output, value.dupe(), srcs)
                .await?;
            ActionExecutionKind::Simple
        };
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
    use buck2_artifact::artifact::artifact_type::Artifact;
    use buck2_artifact::artifact::source_artifact::SourceArtifact;
    use buck2_core::package::source_path::SourcePath;

    use super::*;

    fn mk_artifact() -> Artifact {
        let buck_path = SourcePath::testing_new("cell//pkg", "");
        Artifact::from(SourceArtifact::new(buck_path))
    }

    fn validate(paths: &[&str]) -> buck2_error::Result<()> {
        let a = ArtifactGroup::Artifact(mk_artifact());
        let mut xs = paths
            .iter()
            .map(|x| {
                (
                    a.dupe(),
                    ForwardRelativePath::new(x).unwrap().to_buf().into_box(),
                    CopyMode::Symlink,
                )
            })
            .collect::<Vec<_>>();
        UnregisteredAssembledDirAction::validate_args(&mut xs)
    }

    // TODO: This needs proper tests, but right now it's kind of a pain to get the
    //       action framework up and running to test actions
    #[test]
    fn assembled_dir_test() {}

    #[test]
    fn test_assembled_dir_validation() {
        // Check that error conditions are detected
        assert!(validate(&["test", "other"]).is_ok());
        assert!(validate(&["test", "test"]).is_err());
        assert!(validate(&["test", "other", "test"]).is_err());
        assert!(validate(&["test", "test/child"]).is_err());
    }

    #[test]
    fn test_assembled_dir_validation_overlap_separated_by_sibling() {
        assert!(validate(&["test", "test/child", "test_other"]).is_err());
        assert!(validate(&["test", "test.txt", "test/child"]).is_err());
        assert!(validate(&["test", "test-other", "test/child"]).is_err());
        assert!(validate(&["test/child", "test", "test.txt"]).is_err());
    }
}
