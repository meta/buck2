/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::OutputArtifact;
use buck2_artifact::artifact::output_type::OutputType;
use buck2_build_api::actions::ActionName;
use buck2_build_api::actions::impls::json::JsonUnpack;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use buck2_build_api::interpreter::rule_defs::artifact::output_artifact_like::OutputArtifactArg;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use buck2_build_api::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use buck2_build_api::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::InputProperties;
use buck2_build_api::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use buck2_build_api::interpreter::rule_defs::cmd_args::StarlarkCommandLineValueUnpack;
use buck2_build_api::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::value::CommandLineArg;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_build_api::interpreter::rule_defs::resolved_macro::ResolvedMacro;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_hash::BuckMutInsertionSet;
use buck2_hash::BuckMutMap;
use buck2_hash::buck_mut_insertion_set;
use buck2_interpreter::execution_group::ExecutionGroupArg;
use dupe::Dupe;
use either::Either;
use sha1::Digest;
use sha1::Sha1;
use starlark::environment::MethodsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::AllocValue;
use starlark::values::UnpackValue;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate::actions::impls::dep_file_fingerprint::DepFileFingerprintFormat;
use crate::actions::impls::dep_file_fingerprint::StarlarkDepFileFingerprint;
use crate::actions::impls::write::UnregisteredWriteAction;
use crate::actions::impls::write_hmap::HmapMappings;
use crate::actions::impls::write_hmap::UnregisteredWriteHmapAction;
use crate::actions::impls::write_hmap::visit_hmap_artifacts;
use crate::actions::impls::write_json::UnregisteredWriteJsonAction;
use crate::actions::impls::write_macros::UnregisteredWriteMacrosToFileAction;
use crate::context::execution_group::action_execution_group;
use crate::context::execution_group::check_default_execution_group_declared;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum WriteActionError {
    #[error(
        "Argument type attributes detected in a content to be written into a file, but support for arguments was not turned on. Use `allow_args` parameter to turn on the support for arguments."
    )]
    ArgAttrsDetectedButNotAllowed,
    #[error(
        "dep_files_fingerprint_using_canonical_paths cannot be combined with placeholder output"
    )]
    DepFileFingerprintWithPlaceholderOutput,
    #[error(
        "`with_inputs = True` cannot be combined with `dep_files_fingerprint_using_canonical_paths = True`: the fingerprint already forwards inputs with their original tags. Omit `with_inputs` and pass the fingerprint to `run(dep_file_fingerprints = [...])`."
    )]
    DepFileFingerprintWithInputs,
}

#[derive(UnpackValue, StarlarkTypeRepr)]
enum WriteContentArg<'v> {
    CommandLineArg(CommandLineArg<'v>),
    StarlarkCommandLineValueUnpack(StarlarkCommandLineValueUnpack<'v>),
}

type WriteOutput<'v> = Either<
    ValueTyped<'v, StarlarkDeclaredArtifact<'v>>,
    (
        ValueTyped<'v, StarlarkDeclaredArtifact<'v>>,
        StarlarkDepFileFingerprint<'v>,
    ),
>;

type WriteReturnValue<'v> =
    Either<WriteOutput<'v>, (WriteOutput<'v>, Vec<StarlarkDeclaredArtifact<'v>>)>;

/// We don't need to run this visitor in order to provide the inputs to the write actions,
/// because that is done lazily when we run the action.
/// However, we do need to always run this visitor, because it verifies that any content-based
/// inputs are bound. It will also collect "associated artifacts", if requested.
struct CommandLineInputVisitor {
    associated_artifacts: BuckMutInsertionSet<ArtifactGroup>,
    with_associated_artifacts: bool,
}

impl CommandLineInputVisitor {
    fn new(with_associated_artifacts: bool) -> Self {
        Self {
            associated_artifacts: Default::default(),
            with_associated_artifacts,
        }
    }
}

impl<'v> CommandLineArtifactVisitor<'v> for CommandLineInputVisitor {
    fn visit_input(
        &mut self,
        input: ArtifactGroup,
        _props: InputProperties,
        _tags: Vec<&ArtifactTag>,
    ) {
        if self.with_associated_artifacts {
            self.associated_artifacts.insert(input.dupe());
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
        if self.with_associated_artifacts || declared_artifact.has_content_based_path() {
            let artifact = declared_artifact.ensure_bound()?.into_artifact();
            self.visit_input(ArtifactGroup::Artifact(artifact), props, tags);
        }

        Ok(())
    }
}

#[starlark_module]
pub(crate) fn analysis_actions_methods_write(methods: &mut MethodsBuilder) {
    /// Returns an `artifact` whose contents are `content` written as a JSON value.
    ///
    /// * `output`: can be a string, or an existing artifact created with `declare_output`
    /// * `content`:  must be composed of the basic json types (boolean, number, string, list/tuple,
    ///   dictionary) plus artifacts and command lines
    ///     * An artifact will be written as a string containing the path
    ///     * A command line will be written as a list of strings, unless `joined=True` is set, in
    ///       which case it will be a string
    /// * If you pass `with_inputs = True`, you'll get back a `cmd_args` that expands to the JSON
    ///   file but carries all the underlying inputs as dependencies (so you don't have to use, for
    ///   example, `hidden` for them to be added to an action that already receives the JSON file)
    /// * `dep_files_fingerprint_using_canonical_paths` (optional): returns an `(artifact, DepFileFingerprint)` pair.
    ///   Pass the fingerprint to `run(dep_file_fingerprints = [...])` to fingerprint the JSON with
    ///   canonical content-based paths and forward its referenced inputs with their original tags.
    ///   Pass the artifact to the command separately, tagged for dep-file filtering.
    ///   Cannot be combined with `with_inputs = True`.
    /// * `pretty` (optional): write formatted JSON (defaults to `False`)
    /// * `absolute` (optional): if set, this action will produce absolute paths in its output when
    ///   rendering artifact paths. You generally shouldn't use this if you plan to use this action
    ///   as the input for anything else, as this would effectively result in losing all shared
    ///   caching. (defaults to `False`)
    /// * `exec_group` (optional): the execution group whose platform the written paths are for,
    ///   such as the path separator. Pass the group of the actions that read the output. Defaults
    ///   to the default execution group.
    fn write_json<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = pos)] content: ValueOf<'v, JsonUnpack<'v>>,
        #[starlark(require = named, default = false)] with_inputs: bool,
        #[starlark(require = named, default = false)]
        dep_files_fingerprint_using_canonical_paths: bool,
        #[starlark(require = named, default = false)] pretty: bool,
        #[starlark(require = named, default = false)] absolute: bool,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        #[starlark(require = named, default = false)]
        use_dep_files_placeholder_for_content_based_paths: bool,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<impl AllocValue<'v> + use<'v>> {
        let exec_group = action_execution_group(exec_group);
        if dep_files_fingerprint_using_canonical_paths && with_inputs {
            return Err(
                buck2_error::Error::from(WriteActionError::DepFileFingerprintWithInputs).into(),
            );
        }
        if dep_files_fingerprint_using_canonical_paths
            && use_dep_files_placeholder_for_content_based_paths
        {
            return Err(buck2_error::Error::from(
                WriteActionError::DepFileFingerprintWithPlaceholderOutput,
            )
            .into());
        }
        let mut this = this.state()?;
        check_default_execution_group_declared(this.execution_platform(), &exec_group)?;
        let (declaration, output_artifact) = this.get_or_declare_output(
            eval,
            output,
            OutputType::File,
            has_content_based_path.into_option(),
        )?;

        let value = declaration.into_declared_artifact(AssociatedArtifacts::new());
        let cli = UnregisteredWriteJsonAction::cli(value.to_value(), content.to_value())?;

        let mut visitor = CommandLineInputVisitor::new(false);
        cli.visit_contents(&mut visitor)?;

        let action_name = write_action_name("write_json", &output_artifact);
        this.register_action_in_group(
            buck_mut_insertion_set![output_artifact],
            UnregisteredWriteJsonAction::new(
                pretty,
                absolute,
                use_dep_files_placeholder_for_content_based_paths,
            ),
            Some(content.to_value()),
            None,
            exec_group,
            action_name,
        )?;

        // TODO(cjhopman): The with_inputs thing can go away once we have artifact dependencies (we'll still
        // need the UnregisteredWriteJsonAction::cli() to represent the dependency though).
        if dep_files_fingerprint_using_canonical_paths {
            let fingerprint = StarlarkDepFileFingerprint {
                artifact: value.to_value(),
                content: content.to_value(),
                absolute,
                format: DepFileFingerprintFormat::Json { pretty },
            };
            Ok(Either::Right((value, fingerprint)))
        } else if with_inputs {
            // TODO(nga): we use `AllocValue`, so this function return type for this branch
            //   is `write_json_cli_args`. We want just `cmd_args`,
            //   because users don't care about precise type.
            //   Do it when we migrate to new types not based on strings.
            Ok(Either::Left(Either::Right(cli)))
        } else {
            Ok(Either::Left(Either::Left(value)))
        }
    }

    /// Internal implementation of the prelude's Clang header-map writer.
    /// Do not call `ctx.actions._write_hmap` directly. Load `write_hmap` from
    /// `prelude//cxx:hmap.bzl` instead. We may move this implementation back
    /// into the prelude in the future.
    ///
    /// Writes a Clang header map from a dictionary of include names to
    /// `(artifact, path)` pairs. The path is relative to the artifact and may
    /// be empty to map to the artifact itself.
    /// Each destination also maps to itself, to stop Clang searching later maps.
    /// Header contents are not inputs, except when needed to resolve content-based paths.
    /// Callers must add the headers as inputs to actions that use the map.
    ///
    /// * `exec_group` (optional): the execution group whose platform the written paths are for,
    ///   such as the path separator. Pass the group of the actions that read the output. Defaults
    ///   to the default execution group.
    fn _write_hmap<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = pos)] content: ValueOf<'v, HmapMappings<'v>>,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<impl AllocValue<'v> + use<'v>> {
        let exec_group = action_execution_group(exec_group);
        let mut this = this.state()?;
        check_default_execution_group_declared(this.execution_platform(), &exec_group)?;
        let (declaration, output_artifact) = this.get_or_declare_output(
            eval,
            output,
            OutputType::File,
            has_content_based_path.into_option(),
        )?;
        let value = declaration.into_declared_artifact(AssociatedArtifacts::new());
        visit_hmap_artifacts(content.to_value(), &mut CommandLineInputVisitor::new(false))?;
        let action_name = write_action_name("write_hmap", &output_artifact);
        this.register_action_in_group(
            buck_mut_insertion_set![output_artifact],
            UnregisteredWriteHmapAction {},
            Some(content.to_value()),
            None,
            exec_group,
            action_name,
        )?;
        Ok(value)
    }

    /// Returns an `artifact` whose contents are `content`
    ///
    /// * `is_executable` (optional): indicates whether the resulting file should be marked with
    ///   executable permissions
    /// * `allow_args` (optional): must be set to `True` if you want to write parameter arguments to
    ///   the file (in particular, macros that write to file)
    ///     * If it is true, the result will be a pair of the `artifact` containing content and a
    ///       list of artifact values that were written by macros, which should be used in hidden
    ///       fields or similar
    /// * `with_inputs` (optional): if set, add artifacts in `content` as associated artifacts of the return `artifact`.
    /// * `dep_files_fingerprint_using_canonical_paths` (optional): returns an `(artifact, DepFileFingerprint)` pair in
    ///   place of the artifact (also inside the pair returned by `allow_args`). Pass the fingerprint
    ///   to `run(dep_file_fingerprints = [...])` and the artifact to the command separately, tagged
    ///   for dep-file filtering. The fingerprint forwards content and macro dependencies with their
    ///   original tags and includes the executable mode and canonical contents.
    ///   Cannot be combined with `with_inputs = True`.
    /// * `absolute` (optional): if set, this action will produce absolute paths in its output when
    ///   rendering artifact paths. You generally shouldn't use this if you plan to use this action
    ///   as the input for anything else, as this would effectively result in losing all shared
    ///   caching.
    /// * `exec_group` (optional): the execution group whose platform the written paths are for,
    ///   such as the path separator. Pass the group of the actions that read the output. Defaults
    ///   to the default execution group.
    ///
    /// The content is often a string, but can be any `ArgLike` value. This is occasionally useful
    /// for generating scripts to run as a part of another action. `cmd_args` in the content are
    /// newline separated unless another delimiter is explicitly specified.
    fn write<'v>(
        this: &AnalysisActions<'v>,
        #[starlark(require = pos)] output: OutputArtifactArg<'v>,
        #[starlark(require = pos)] content: WriteContentArg<'v>,
        #[starlark(require = named, default = false)] is_executable: bool,
        #[starlark(require = named, default = false)] allow_args: bool,
        // If set, add artifacts in content as associated artifacts of the output. This will only work for bound artifacts.
        #[starlark(require = named, default = false)] with_inputs: bool,
        #[starlark(require = named, default = false)]
        dep_files_fingerprint_using_canonical_paths: bool,
        #[starlark(require = named, default = false)] absolute: bool,
        #[starlark(require = named, default = NoneOr::None)] has_content_based_path: NoneOr<bool>,
        #[starlark(require = named, default = false)]
        use_dep_files_placeholder_for_content_based_paths: bool,
        #[starlark(require = named, default = NoneOr::None)] exec_group: NoneOr<ExecutionGroupArg>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<WriteReturnValue<'v>> {
        fn count_write_to_file_macros(
            args_allowed: bool,
            cli: &dyn CommandLineArgLike,
        ) -> buck2_error::Result<u32> {
            if !args_allowed && cli.contains_arg_attr() {
                return Err(WriteActionError::ArgAttrsDetectedButNotAllowed.into());
            }

            struct WriteToFileMacrosCounter {
                count: u32,
            }

            impl WriteToFileMacroVisitor for WriteToFileMacrosCounter {
                fn visit_write_to_file_macro(
                    &mut self,
                    _m: &ResolvedMacro,
                    _artifact_path_mapping: &dyn ArtifactPathMapper,
                ) -> buck2_error::Result<()> {
                    self.count += 1;
                    Ok(())
                }

                fn set_current_relative_to_path(&mut self, _p: ProjectRelativePathBuf) {}

                fn fs(&self) -> Option<&ExecutorFs<'_>> {
                    None
                }
            }

            let mut counter = WriteToFileMacrosCounter { count: 0 };
            // At this point the mapping doesn't matter because we're only doing a count
            cli.visit_write_to_file_macros(&mut counter, &BuckMutMap::default())?;
            Ok(counter.count)
        }

        fn get_cli_inputs(
            with_inputs: bool,
            cli: &dyn CommandLineArgLike,
        ) -> buck2_error::Result<BuckMutInsertionSet<ArtifactGroup>> {
            let mut visitor = CommandLineInputVisitor::new(with_inputs);
            cli.visit_artifacts(&mut visitor)?;
            Ok(visitor.associated_artifacts)
        }

        if dep_files_fingerprint_using_canonical_paths && with_inputs {
            return Err(
                buck2_error::Error::from(WriteActionError::DepFileFingerprintWithInputs).into(),
            );
        }
        if dep_files_fingerprint_using_canonical_paths
            && use_dep_files_placeholder_for_content_based_paths
        {
            return Err(buck2_error::Error::from(
                WriteActionError::DepFileFingerprintWithPlaceholderOutput,
            )
            .into());
        }
        let exec_group = action_execution_group(exec_group);
        let mut this = this.state()?;
        check_default_execution_group_declared(this.execution_platform(), &exec_group)?;
        let (declaration, output_artifact) = this.get_or_declare_output(
            eval,
            output,
            OutputType::File,
            has_content_based_path.into_option(),
        )?;

        let (content_cli, written_macro_count, mut associated_artifacts) = match content {
            WriteContentArg::CommandLineArg(content) => {
                let content_arg = content.as_command_line_arg();
                let count = count_write_to_file_macros(allow_args, content_arg)?;
                let associated_artifacts = get_cli_inputs(with_inputs, content_arg)?;
                (content, count, associated_artifacts)
            }
            WriteContentArg::StarlarkCommandLineValueUnpack(content) => {
                let cli = StarlarkCmdArgs::try_from_value_typed(content)?;
                let count = count_write_to_file_macros(allow_args, &cli)?;
                let associated_artifacts = get_cli_inputs(with_inputs, &cli)?;
                (
                    CommandLineArg::from_cmd_args(eval.heap().alloc_typed(cli)),
                    count,
                    associated_artifacts,
                )
            }
        };

        let path_resolution_method = output_artifact.path_resolution_method();

        let written_macro_files = if written_macro_count > 0 {
            let macro_directory_path = {
                // There might be several write actions at once, use write action output hash to deterministically avoid collisions for .macro files.
                let digest = output_artifact
                    .get_path()
                    .with_full_path(|path| Sha1::digest(path.as_str().as_bytes()));
                let sha = hex::encode(digest);
                format!("__macros/{sha}")
            };

            let mut written_macro_files = buck_mut_insertion_set![];
            for i in 0..written_macro_count {
                let macro_file = this.declare_output(
                    None,
                    &format!("{}/{}.macro", macro_directory_path, i),
                    OutputType::File,
                    eval.call_stack_top_location(),
                    path_resolution_method,
                    eval.heap(),
                )?;
                written_macro_files.insert(macro_file);
            }

            let state = &mut *this;
            let identifier = output_artifact
                .get_path()
                .with_short_path(|p| p.to_string());
            let action = UnregisteredWriteMacrosToFileAction::new(
                identifier.clone(),
                use_dep_files_placeholder_for_content_based_paths,
            );
            state.register_action_in_group(
                written_macro_files.iter().map(|a| a.as_output()).collect(),
                action,
                Some(content_cli.to_value()),
                None,
                exec_group.dupe(),
                || ActionName {
                    category: "write_macros_to_file".to_owned(),
                    identifier: Some(identifier),
                },
            )?;

            written_macro_files
        } else {
            buck_mut_insertion_set![]
        };

        let action = {
            let maybe_macro_files = if allow_args {
                let mut macro_files = buck_mut_insertion_set![];
                for a in &written_macro_files {
                    let artifact = a.dupe().ensure_bound()?.into_artifact();
                    macro_files.insert(artifact.dupe());
                }
                Some(macro_files.into_iter().collect())
            } else {
                None
            };
            UnregisteredWriteAction {
                is_executable,
                macro_files: maybe_macro_files,
                absolute,
                use_dep_files_placeholder_for_content_based_paths,
            }
        };
        let fingerprint_macro_files =
            dep_files_fingerprint_using_canonical_paths.then(|| action.macro_files.clone());
        let action_name = write_action_name("write", &output_artifact);
        this.register_action_in_group(
            buck_mut_insertion_set![output_artifact],
            action,
            Some(content_cli.to_value()),
            None,
            exec_group,
            action_name,
        )?;

        if allow_args {
            for a in &written_macro_files {
                associated_artifacts.insert(ArtifactGroup::Artifact(
                    a.dupe().ensure_bound()?.into_artifact(),
                ));
            }
        }

        let value =
            declaration.into_declared_artifact(AssociatedArtifacts::from(associated_artifacts));
        let value = if let Some(macro_files) = fingerprint_macro_files {
            let fingerprint = StarlarkDepFileFingerprint {
                artifact: value.to_value(),
                content: content_cli.to_value(),
                absolute,
                format: DepFileFingerprintFormat::Args {
                    is_executable,
                    macro_files,
                },
            };
            Either::Right((value, fingerprint))
        } else {
            Either::Left(value)
        };
        if allow_args {
            let macro_files: Vec<StarlarkDeclaredArtifact> = written_macro_files
                .into_iter()
                .map(|a| StarlarkDeclaredArtifact::new(None, a, AssociatedArtifacts::new()))
                .collect();
            Ok(Either::Right((value, macro_files)))
        } else {
            // Prefer simpler API when there is no possibility for write-to-file macros to be present in a content
            Ok(Either::Left(value))
        }
    }
}

/// Names a write action in registration errors: its identifier is the path of its output.
fn write_action_name<'v>(
    category: &'static str,
    output: &OutputArtifact<'v>,
) -> impl FnOnce() -> ActionName + use<'v> {
    let output = output.dupe();
    move || ActionName {
        category: category.to_owned(),
        identifier: Some(output.get_path().with_full_path(|p| p.to_string())),
    }
}
