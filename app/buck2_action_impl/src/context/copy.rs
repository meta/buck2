/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::output_type::OutputType;
use buck2_build_api::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use buck2_build_api::interpreter::rule_defs::artifact::output_artifact_like::OutputArtifactArg;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_error::internal_error;
use buck2_hash::buck_mut_insertion_set;
use dupe::OptionDupedExt;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::UnpackValue;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::none::NoneOr;

use crate::actions::impls::assembled_dir::UnregisteredAssembledDirAction;
use crate::actions::impls::copy::CopyMode;
use crate::actions::impls::copy::UnregisteredCopyAction;
use crate::context::assembled_dir::StarlarkAssembledDirEntry;

fn create_dir_tree<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    this: &AnalysisActions<'v>,
    output: OutputArtifactArg<'v>,
    srcs: ValueOf<'v, DictType<&'v str, StarlarkArtifactView<'v>>>,
    copy: CopyMode,
    has_content_based_path: Option<bool>,
) -> buck2_error::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
    let action = UnregisteredAssembledDirAction::new(copy, srcs)?;
    let unioned_associated_artifacts = action.unioned_associated_artifacts();

    let mut this = this.state()?;
    let (declaration, output_artifact) =
        this.get_or_declare_output(eval, output, OutputType::Directory, has_content_based_path)?;
    this.register_action(buck_mut_insertion_set![output_artifact], action, None, None)?;

    Ok(declaration.into_declared_artifact(unioned_associated_artifacts))
}

fn create_assembled_dir_tree<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    this: &AnalysisActions<'v>,
    output: OutputArtifactArg<'v>,
    contents: ValueOf<'v, DictType<&'v str, &'v StarlarkAssembledDirEntry<'v>>>,
    has_content_based_path: Option<bool>,
) -> buck2_error::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
    let entries = contents
        .try_items()
        .map(|entry| {
            let (k, entry) = entry?;
            // The entry constructors (`assembled_dir.copy` / `assembled_dir.symlink`)
            // only accept artifact-like values, so this re-unpack cannot fail.
            let as_artifact =
                StarlarkArtifactView::unpack_value_opt(entry.artifact).ok_or_else(|| {
                    internal_error!("assembled_dir entry holds a validated artifact-like value")
                })?;
            let mode = if entry.copy {
                CopyMode::Copy {
                    executable_bit_override: None,
                }
            } else {
                CopyMode::Symlink
            };
            buck2_error::Ok((k, as_artifact, mode))
        })
        .collect::<buck2_error::Result<Vec<_>>>()?;
    let action = UnregisteredAssembledDirAction::new_assembled(entries)?;
    let unioned_associated_artifacts = action.unioned_associated_artifacts();

    let mut this = this.state()?;
    let (declaration, output_artifact) =
        this.get_or_declare_output(eval, output, OutputType::Directory, has_content_based_path)?;
    this.register_action(buck_mut_insertion_set![output_artifact], action, None, None)?;

    Ok(declaration.into_declared_artifact(unioned_associated_artifacts))
}

fn copy_file_impl<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    this: &AnalysisActions<'v>,
    dest: OutputArtifactArg<'v>,
    src: StarlarkArtifactView<'v>,
    copy: CopyMode,
    output_type: OutputType,
    has_content_based_path: Option<bool>,
) -> buck2_error::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
    let src = src.as_input_artifact_like();

    let artifact = src.get_artifact_group()?;
    let associated_artifacts = src.get_associated_artifacts();
    let mut this = this.state()?;
    let (declaration, output_artifact) =
        this.get_or_declare_output(eval, dest, output_type, has_content_based_path)?;

    this.register_action(
        buck_mut_insertion_set![output_artifact],
        UnregisteredCopyAction::new(artifact, copy),
        None,
        None,
    )?;

    Ok(declaration.into_declared_artifact(
        associated_artifacts
            .duped()
            .unwrap_or_else(AssociatedArtifacts::new),
    ))
}

#[starlark_module]
pub(crate) fn analysis_actions_methods_copy(methods: &mut MethodsBuilder) {
    /// Copies the source `artifact` to the destination (which can be a string representing a
    /// filename or an output `artifact`) and returns the output `artifact`. The copy works for
    /// files or directories.
    fn copy_file<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] dest: OutputArtifactArg<'v>,
        #[starlark(require = pos)] src: StarlarkArtifactView<'v>,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        #[starlark(require = named, default = NoneOr::None)] executable_bit_override: NoneOr<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        // `copy_file` can copy either a file or a directory, even though its name has the word
        // `file` in it
        Ok(copy_file_impl(
            eval,
            this,
            dest,
            src,
            CopyMode::Copy {
                executable_bit_override: executable_bit_override.into_option(),
            },
            OutputType::FileOrDirectory,
            has_content_based_path.into_option(),
        )?)
    }

    /// Creates a symlink to the source `artifact` at the destination (which can be a string
    /// representing a filename or an output `artifact`) and returns the output `artifact`. The
    /// symlink works for files or directories.
    fn symlink_file<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] dest: OutputArtifactArg<'v>,
        #[starlark(require = pos)] src: StarlarkArtifactView<'v>,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        // `copy_file` can copy either a file or a directory, even though its name has the word
        // `file` in it
        Ok(copy_file_impl(
            eval,
            this,
            dest,
            src,
            CopyMode::Symlink,
            OutputType::FileOrDirectory,
            has_content_based_path.into_option(),
        )?)
    }

    /// Make a copy of a directory.
    fn copy_dir<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] dest: OutputArtifactArg<'v>,
        #[starlark(require = pos)] src: StarlarkArtifactView<'v>,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        #[starlark(require = named, default = NoneOr::None)] executable_bit_override: NoneOr<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        Ok(copy_file_impl(
            eval,
            this,
            dest,
            src,
            CopyMode::Copy {
                executable_bit_override: executable_bit_override.into_option(),
            },
            OutputType::Directory,
            has_content_based_path.into_option(),
        )?)
    }

    /// Returns an `artifact` that is a directory containing symlinks.
    /// The srcs must be a dictionary of path (as string, relative to the result directory) to bound `artifact`, which will be laid out in the directory.
    fn symlinked_dir<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = pos)] srcs: ValueOf<'v, DictType<&'v str, StarlarkArtifactView<'v>>>,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        Ok(create_dir_tree(
            eval,
            this,
            output,
            srcs,
            CopyMode::Symlink,
            has_content_based_path.into_option(),
        )?)
    }

    /// Returns an `artifact` which is a directory containing copied files.
    /// The srcs must be a dictionary of path (as string, relative to the result directory) to the bound `artifact`, which will be laid out in the directory.
    fn copied_dir<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = pos)] srcs: ValueOf<'v, DictType<&'v str, StarlarkArtifactView<'v>>>,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        #[starlark(require = named, default = NoneOr::None)] executable_bit_override: NoneOr<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        Ok(create_dir_tree(
            eval,
            this,
            output,
            srcs,
            CopyMode::Copy {
                executable_bit_override: executable_bit_override.into_option(),
            },
            has_content_based_path.into_option(),
        )?)
    }

    /// Returns an `artifact` that is a directory whose entries are a mix of
    /// copied files and symlinks. `contents` is a dictionary of path (string,
    /// relative to the result directory) to entry, where an entry wraps a
    /// bound `artifact` in a materialization mode:
    /// `assembled_dir.copy(artifact)` entries are laid out as real
    /// (content-addressed) bytes, while `assembled_dir.symlink(artifact)`
    /// entries are symlinks pointing at their source artifacts.
    ///
    /// Use a copy for inputs that must be physically present in the directory --
    /// e.g. an executable whose `$ORIGIN`-relative RPATH or sibling
    /// `.resources.json` has to resolve inside this dir (a symlinked exe would
    /// resolve `$ORIGIN` against its realpath, not this dir). Use a symlink for
    /// inputs that already live at an immutable (content-based) path, to avoid
    /// duplicating their bytes while staying robust to rebuilds.
    ///
    /// ```python
    /// bundle = ctx.actions.assembled_dir(
    ///     "bundle",
    ///     contents = {
    ///         "app": assembled_dir.copy(exe),
    ///         "app.resources.json": assembled_dir.copy(manifest),
    ///         "res": assembled_dir.symlink(resource_dir),
    ///     },
    /// )
    /// ```
    fn assembled_dir<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = named)] contents: ValueOf<
            'v,
            DictType<&'v str, &'v StarlarkAssembledDirEntry<'v>>,
        >,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        Ok(create_assembled_dir_tree(
            eval,
            this,
            output,
            contents,
            has_content_based_path.into_option(),
        )?)
    }
}
