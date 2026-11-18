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
use buck2_build_api::actions::impls::json::JsonUnpack;
use buck2_build_api::actions::impls::json::SerializeValue;
use buck2_build_api::actions::impls::json::visit_json_artifacts;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_build_api::interpreter::rule_defs::artifact_tagging::ArtifactTag;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use buck2_build_api::interpreter::rule_defs::cmd_args::InputProperties;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckMutMap;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use dupe::Dupe;
use indoc::indoc;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::UnpackValue;
use starlark::values::Value;

use crate::interpreter::rule_defs::artifact::testing::artifactory;
use crate::interpreter::rule_defs::artifact_tagging::testing::artifact_tag_factory;

#[test]
fn test_tagging() -> buck2_error::Result<()> {
    let _guard = buck2_util::threads::ignore_stack_overflow_checks_for_current_thread();

    struct AssertVisitor {
        tag: ArtifactTag,
        artifact: Artifact,
    }

    impl<'v> CommandLineArtifactVisitor<'v> for AssertVisitor {
        fn visit_input(
            &mut self,
            input: ArtifactGroup,
            _props: InputProperties,
            tags: Vec<&ArtifactTag>,
        ) {
            assert_eq!(tags, vec![&self.tag]);
            assert_eq!(input, ArtifactGroup::Artifact(self.artifact.dupe()));
        }

        fn visit_declared_output(
            &mut self,
            _artifact: OutputArtifact<'v>,
            _tags: Vec<&ArtifactTag>,
        ) {
        }

        fn visit_frozen_output(&mut self, _artifact: Artifact, _tags: Vec<&ArtifactTag>) {}
    }

    #[starlark_module]
    fn assertions(builder: &mut GlobalsBuilder) {
        fn check_artifact_is_tagged<'v>(
            tagged: Value<'v>,
            tag: Value<'v>,
            artifact: StarlarkArtifactView<'v>,
        ) -> starlark::Result<Value<'v>> {
            let tag = ArtifactTag::from_value(tag)
                .internal_error("Invalid tag")?
                .dupe();

            let artifact = artifact
                .as_input_artifact_like()
                .get_bound_artifact()
                .buck_error_context("Not a bound artifact")?
                .dupe();

            visit_json_artifacts(tagged, &mut AssertVisitor { tag, artifact })?;
            Ok(Value::new_none())
        }

        fn check_passthrough<'v>(
            tagged: Value<'v>,
            value: Value<'v>,
        ) -> starlark::Result<Value<'v>> {
            let json1 = serde_json::to_string(&SerializeValue {
                value: JsonUnpack::unpack_value_err(tagged)?,
                fs: None,
                absolute: false,
                artifact_path_mapping: &BuckMutMap::default(),
            })
            .map_err(buck2_error::Error::from)?;

            let json2 = serde_json::to_string(&SerializeValue {
                value: JsonUnpack::unpack_value_err(value)?,
                fs: None,
                absolute: false,
                artifact_path_mapping: &BuckMutMap::default(),
            })
            .map_err(buck2_error::Error::from)?;

            assert_eq!(json1, json2);

            Ok(Value::new_none())
        }
    }

    let mut tester = Tester::new()?;
    tester.additional_globals(artifact_tag_factory);
    tester.additional_globals(artifactory);
    tester.additional_globals(assertions);

    tester.run_starlark_bzl_test(indoc!(
        r#"
        def test():
            t1 = make_tag()
            a1 = source_artifact("foo", "bar")
            v1 = {"foo": "bar"}

            check_artifact_is_tagged(t1.tag_artifacts(a1), t1, a1)
            check_passthrough(t1.tag_artifacts(v1), v1)
        "#
    ))?;

    Ok(())
}
