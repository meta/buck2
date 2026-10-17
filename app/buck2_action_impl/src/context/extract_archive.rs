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
use buck2_hash::buck_mut_insertion_set;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneOr;
use starlark::values::string::StarlarkStr;

use crate::actions::impls::extract_archive::action::UnregisteredExtractArchiveAction;

#[starlark_module]
pub(crate) fn analysis_actions_methods_extract_archive(methods: &mut MethodsBuilder) {
    /// This action extracts the `archive` artifact into the directory `output` (a string or an output
    /// `artifact`) and returns the output `artifact`.
    ///
    /// Buck2 extracts the archive itself, without running a command, so the action needs no tools
    /// on the host and always runs where Buck2 runs.
    ///
    /// * `type`: one of `tar`, `tar.gz`, `tar.bz2`, `tar.xz`, `tar.zst` or `zip`. When it is not
    ///   given, the type comes from the extension of the archive's file name.
    /// * `strip_prefix`: only the entries below this path in the archive are extracted, to the
    ///   root of the output directory.
    /// * `excludes`: regular expressions. An entry is skipped when one of the regular expressions
    ///   matches its name in the archive, before `strip_prefix` is removed. Zip entry names use
    ///   `/` as the separator here, even when the archive stores `\`. Skipping a directory entry
    ///   does not skip the entries below it, except for tar directory names without a trailing
    ///   `/`. Excluding a tar directory with that spelling also excludes its descendants.
    ///
    /// Only file contents, symlinks and the executable bit are kept. A hard link to a file becomes
    /// a separate file in the output artifact. Entry names with a `..` component fail the action,
    /// and so does an entry that would be written through a symlink from the same archive. Tar
    /// entries selected for extraction fail if they are FIFOs, devices or sparse files that GNU tar
    /// stored in the pax format. Buffered tar metadata is limited to 1 MiB per entry. Global pax
    /// metadata is skipped. Windows symlink targets must be fully absolute or have no drive or root.
    fn extract_archive<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = pos)] archive: StarlarkArtifactView<'v>,
        #[starlark(require = named, default = NoneOr::None)] r#type: NoneOr<&str>,
        #[starlark(require = named, default = NoneOr::None)] strip_prefix: NoneOr<&str>,
        #[starlark(require = named, default = ValueOf::empty_list())] excludes: ValueOf<
            'v,
            ListOrTupleType<StarlarkStr>,
        >,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkDeclaredArtifact<'v>>> {
        let action = UnregisteredExtractArchiveAction::new(
            archive.as_input_artifact_like().get_artifact_group()?,
            r#type.into_option(),
            strip_prefix.into_option(),
            excludes
                .try_items::<&str>()
                .collect::<starlark::Result<Vec<_>>>()?,
        )?;

        let mut this = this.state()?;
        let (declaration, output_artifact) = this.get_or_declare_output(
            eval,
            output,
            OutputType::Directory,
            has_content_based_path.into_option(),
        )?;
        this.register_action(buck_mut_insertion_set![output_artifact], action, None, None)?;

        Ok(declaration.into_declared_artifact(AssociatedArtifacts::new()))
    }
}
