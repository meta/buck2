/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hasher;

use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_build_api::interpreter::rule_defs::cmd_args::SimpleCommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::StarlarkCommandLineInputs;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_build_api::interpreter::rule_defs::register_rule_defs;
use buck2_core::execution_types::executor_config::PathSeparatorKind;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuckOutPathResolver;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckMutMap;
use buck2_interpreter::types::regex::register_buck_regex;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::interpreter::testing::cells;
use buck2_interpreter_for_build::label::testing::label_creator;
use buck2_util::strong_hasher::Blake3StrongHasher;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::UnpackValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::list_or_tuple::ListOrTupleType;

use crate::interpreter::rule_defs::artifact::testing::artifactory;

fn artifact_fs() -> ArtifactFs {
    let cell_info = cells(None).unwrap();
    ArtifactFs::new(
        cell_info.1,
        BuckOutPathResolver::new(ProjectRelativePathBuf::unchecked_new(
            "buck-out/v2".to_owned(),
        )),
        ProjectRoot::new(AbsNormPathBuf::try_from(std::env::current_dir().unwrap()).unwrap())
            .unwrap(),
    )
}

fn get_command_line(value: Value) -> buck2_error::Result<Vec<String>> {
    let fs = artifact_fs();
    let executor_fs = ExecutorFs::new(&fs, PathSeparatorKind::Unix);
    let mut cli = Vec::<String>::new();
    let artifact_path_mapping = BuckMutMap::default();
    let mut fmt = CommandLineBuilder::new(&mut cli, &artifact_path_mapping, &executor_fs);

    match ValueAsCommandLineLike::unpack_value(value)? {
        Some(v) => v.0.add_to_command_line(&mut fmt),
        None => ValueAsCommandLineLike::unpack_value_err(value)?
            .0
            .add_to_command_line(&mut fmt),
    }?;
    Ok(cli)
}

#[starlark_module]
pub(crate) fn command_line_stringifier(builder: &mut GlobalsBuilder) {
    fn get_args<'v>(value: Value<'v>) -> starlark::Result<Vec<String>> {
        Ok(get_command_line(value)?)
    }

    fn stringify_cli_arg<'v>(value: Value<'v>) -> starlark::Result<String> {
        let fs = artifact_fs();
        let executor_fs = ExecutorFs::new(&fs, PathSeparatorKind::Unix);
        let mut cli = Vec::<String>::new();
        let artifact_path_mapping = BuckMutMap::default();
        let mut fmt = CommandLineBuilder::new(&mut cli, &artifact_path_mapping, &executor_fs);
        ValueAsCommandLineLike::unpack_value_err(value)?
            .0
            .add_to_command_line(&mut fmt)?;
        assert_eq!(1, cli.len());
        Ok(cli.first().unwrap().clone())
    }

    /// A hash of `parts` in order, in hex; prefix-free, so moving a boundary changes it.
    fn blake3_hex<'v>(parts: ValueOf<'v, ListOrTupleType<String>>) -> starlark::Result<String> {
        let mut hasher = Blake3StrongHasher::new();
        for part in parts.try_items() {
            let part = part?;
            hasher.write_usize(part.len());
            hasher.write(part.as_bytes());
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    /// `CommandLineBuilder::new_hashing`'s hash of a command line, in hex.
    fn hash_cli_args<'v>(value: Value<'v>) -> starlark::Result<String> {
        let mut hasher = Blake3StrongHasher::new();
        ValueAsCommandLineLike::unpack_value_err(value)?
            .0
            .add_to_command_line(&mut CommandLineBuilder::new_hashing(&mut hasher))?;
        Ok(hasher.finalize().to_hex().to_string())
    }
}

#[starlark_module]
pub(crate) fn inputs_helper(builder: &mut GlobalsBuilder) {
    fn make_inputs<'v>(
        values: ValueOf<'v, ListOrTupleType<Value<'v>>>,
    ) -> starlark::Result<StarlarkCommandLineInputs> {
        let mut visitor = SimpleCommandLineArtifactVisitor::new();
        for v in values.try_items() {
            let v = v?;
            let cli = ValueAsCommandLineLike::unpack_value_err(v)?.0;
            cli.visit_artifacts(&mut visitor)?;
        }

        Ok(StarlarkCommandLineInputs {
            inputs: visitor.inputs,
        })
    }
}

pub(crate) fn tester() -> buck2_error::Result<Tester> {
    let mut tester = Tester::new()?;
    tester.additional_globals(command_line_stringifier);
    tester.additional_globals(inputs_helper);
    tester.additional_globals(artifactory);
    tester.additional_globals(label_creator);
    tester.additional_globals(register_rule_defs);
    tester.additional_globals(register_buck_regex);
    Ok(tester)
}
