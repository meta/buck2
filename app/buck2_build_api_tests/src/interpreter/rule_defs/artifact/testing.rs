/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::actions::key::ActionIndex;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::artifact_type::testing::BuildArtifactTestingExt;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_artifact::artifact::output_type::OutputType;
use buck2_artifact::artifact::source_artifact::SourceArtifact;
use buck2_build_api::actions::registry::ActionsRegistry;
use buck2_build_api::analysis::registry::AnalysisRegistry;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::artifact::associated::AssociatedArtifacts;
use buck2_build_api::interpreter::rule_defs::artifact::output_artifact_like::OutputArtifactArg;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::unpack_artifact::UnpackNonPromiseInputArtifact;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_core::category::CategoryRef;
use buck2_core::cells::paths::CellRelativePath;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::executor_config::PathSeparatorKind;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuckOutPathKind;
use buck2_core::fs::buck_out_path::BuckOutPathResolver;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::package::PackageLabel;
use buck2_core::package::package_relative_path::PackageRelativePath;
use buck2_core::package::source_path::SourcePath;
use buck2_core::pattern::pattern::ParsedPattern;
use buck2_core::pattern::pattern_type::TargetPatternExtra;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use buck2_hash::BuckMutMap;
use buck2_hash::buck_mut_insertion_set;
use buck2_interpreter_for_build::interpreter::build_context::BuildContext;
use buck2_interpreter_for_build::interpreter::testing::cells;
use buck2_util::arc_str::PackedStr;
use dupe::Dupe;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::list_or_tuple::ListOrTupleType;

use crate::actions::testings::SimpleUnregisteredAction;

fn get_label(eval: &Evaluator, target: &str) -> buck2_error::Result<ConfiguredTargetLabel> {
    let ctx = BuildContext::from_context(eval)?;
    match ParsedPattern::<TargetPatternExtra>::parse_precise(
        target,
        ctx.build_file_cell().name(),
        ctx.cell_resolver(),
        ctx.cell_info.cell_alias_resolver(),
    ) {
        Ok(ParsedPattern::Target(package, target_name, TargetPatternExtra)) => {
            Ok(TargetLabel::new(package, target_name.as_ref())
                .configure(ConfigurationData::testing_new()))
        }
        _ => panic!("expected a valid target"),
    }
}

#[starlark_module]
pub(crate) fn artifactory(builder: &mut GlobalsBuilder) {
    fn source_artifact(
        package: &str,
        path: &str,
        eval: &mut Evaluator,
    ) -> starlark::Result<StarlarkArtifact> {
        let ctx = BuildContext::from_context(eval)?;
        let package = PackageLabel::new(
            ctx.build_file_cell().name(),
            CellRelativePath::from_path(package).unwrap(),
        )?;
        let path = PackageRelativePath::new(path)?;
        let path = SourcePath::new(package, PackedStr::new(path));
        Ok(StarlarkArtifact::new(SourceArtifact::new(path).into()))
    }

    fn bound_artifact(
        target: &str,
        path: &str,
        eval: &mut Evaluator,
    ) -> starlark::Result<StarlarkArtifact> {
        let target_label = get_label(eval, target)?;
        let id = ActionIndex::new(0);
        let artifact = Artifact::from(BuildArtifact::testing_new(target_label, path, id));
        Ok(StarlarkArtifact::new(artifact))
    }

    fn declared_artifact<'v>(
        path: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkDeclaredArtifact<'v>> {
        let target_label = get_label(eval, "//foo:bar")?;
        let mut registry = ActionsRegistry::new(
            DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target_label)),
            ExecutionPlatformResolution::unspecified(),
        );
        let artifact = registry.declare_artifact(
            None,
            ForwardRelativePathBuf::try_from(path.to_owned()).unwrap(),
            OutputType::File,
            None,
            BuckOutPathKind::default(),
            eval.heap(),
        )?;
        Ok(StarlarkDeclaredArtifact::new(
            None,
            artifact,
            AssociatedArtifacts::new(),
        ))
    }

    fn declared_bound_artifact<'v>(
        target: &str,
        path: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<StarlarkDeclaredArtifact<'v>> {
        let target_label = get_label(eval, target)?;
        let mut registry = ActionsRegistry::new(
            DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target_label.dupe())),
            ExecutionPlatformResolution::unspecified(),
        );
        let artifact = registry.declare_artifact(
            None,
            ForwardRelativePathBuf::try_from(path.to_owned()).unwrap(),
            OutputType::File,
            None,
            BuckOutPathKind::default(),
            eval.heap(),
        )?;
        let outputs = buck_mut_insertion_set![artifact.as_output()];
        registry.register(
            &DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target_label.dupe())),
            outputs,
            SimpleUnregisteredAction::new(
                buck_mut_insertion_set![],
                vec![],
                CategoryRef::new("fake_action").unwrap().to_owned(),
                None,
            ),
        )?;
        Ok(StarlarkDeclaredArtifact::new(
            None,
            artifact,
            AssociatedArtifacts::new(),
        ))
    }

    fn stringify_for_cli<'v>(artifact: StarlarkArtifactView<'v>) -> starlark::Result<String> {
        let cell_info = cells(None).unwrap();
        let project_fs =
            ProjectRoot::new(AbsNormPathBuf::try_from(std::env::current_dir().unwrap()).unwrap())
                .unwrap();
        let fs = ArtifactFs::new(
            cell_info.1,
            BuckOutPathResolver::new(ProjectRelativePathBuf::unchecked_new(
                "buck-out/v2".to_owned(),
            )),
            project_fs,
        );
        let executor_fs = ExecutorFs::new(&fs, PathSeparatorKind::Unix);
        let mut cli = Vec::<String>::new();
        let artifact_path_mapping = BuckMutMap::default();
        let mut fmt = CommandLineBuilder::new(&mut cli, &artifact_path_mapping, &executor_fs);
        artifact
            .as_input_artifact_like()
            .as_command_line_like()
            .add_to_command_line(&mut fmt)
            .unwrap();
        assert_eq!(1, cli.len());
        Ok(cli.first().unwrap().to_owned())
    }

    // Mainly tests get_or_declare_output function that can transfer associated artifacts
    // artifact parameter can be either string or artifact
    fn declared_bound_artifact_with_associated_artifacts<'v>(
        // TODO(nga): parameters should be either positional or named, not both.
        artifact: OutputArtifactArg<'v>,
        associated_artifacts: ValueOf<'v, ListOrTupleType<UnpackNonPromiseInputArtifact<'v>>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let target_label = get_label(eval, "//foo:bar")?;
        let mut analysis_registry = AnalysisRegistry::new_from_owner(
            BaseDeferredKey::TargetLabel(target_label.dupe()),
            ExecutionPlatformResolution::unspecified(),
        )?;
        let mut actions_registry = ActionsRegistry::new(
            DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target_label.dupe())),
            ExecutionPlatformResolution::unspecified(),
        );

        let associated_artifacts = AssociatedArtifacts::from(
            associated_artifacts
                .try_items()
                .map(|a| ArtifactGroup::Artifact(a.unwrap().artifact().unwrap())),
        );
        let (declaration, output_artifact) =
            analysis_registry.get_or_declare_output(eval, artifact, OutputType::File, None)?;

        actions_registry.register(
            &DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target_label.dupe())),
            buck_mut_insertion_set![output_artifact],
            SimpleUnregisteredAction::new(
                buck_mut_insertion_set![],
                vec![],
                CategoryRef::new("fake_action").unwrap().to_owned(),
                None,
            ),
        )?;

        let value = declaration
            .into_declared_artifact(associated_artifacts)
            .to_value();
        Ok(value)
    }

    fn get_associated_artifacts_as_string<'v>(
        artifact: StarlarkArtifactView<'v>,
    ) -> starlark::Result<String> {
        let associated_artifacts = artifact.as_input_artifact_like().get_associated_artifacts();
        let s: String = associated_artifacts
            .iter()
            .flat_map(|v| v.iter())
            .map(|a| a.to_string())
            .collect();
        Ok(s)
    }
}
