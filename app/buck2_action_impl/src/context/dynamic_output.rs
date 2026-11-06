/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::dynamic_value::DynamicValue;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact::StarlarkArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_value::StarlarkArtifactValue;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_declared_artifact::StarlarkDeclaredArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::starlark_output_artifact::StarlarkOutputArtifact;
use buck2_build_api::interpreter::rule_defs::artifact::unpack_artifact::UnpackNonPromiseInputArtifact;
use buck2_build_api::interpreter::rule_defs::context::AnalysisActions;
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use buck2_dynamic_actions::attrs::dedupe_output_artifacts;
use buck2_dynamic_actions::dynamic_actions::StarlarkDynamicActions;
use buck2_dynamic_actions::dynamic_actions::StarlarkDynamicActionsData;
use buck2_dynamic_actions::dynamic_value::StarlarkDynamicValue;
use buck2_dynamic_actions::output_binder::DynamicActionsOutputArtifactBinder;
use buck2_dynamic_actions::params::DynamicLambdaParams;
use buck2_dynamic_actions::params::DynamicLambdaStaticFields;
use buck2_dynamic_actions::storage::DynamicLambdaParamsStorageImpl;
use buck2_error::conversion::from_any_with_tag;
use buck2_error::internal_error;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::list_or_tuple::ListOrTupleType;
use starlark::values::none::NoneType;
use starlark::values::typing::StarlarkCallable;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum DynamicOutputError {
    #[error("Output list may not be empty")]
    EmptyOutput,
}

fn output_artifacts_to_lambda_build_artifacts<'v>(
    dynamic_key: &DynamicLambdaResultsKey,
    outputs: Vec<ValueTyped<'v, StarlarkOutputArtifact<'v>>>,
) -> buck2_error::Result<MiniBoxSlice<ValueTyped<'v, StarlarkOutputArtifact<'v>>>> {
    let outputs = dedupe_output_artifacts(outputs);
    let mut bind = DynamicActionsOutputArtifactBinder::new(dynamic_key);

    for output in &outputs {
        bind.bind(output.artifact())?;
    }
    Ok(outputs)
}

#[starlark_module]
pub(crate) fn analysis_actions_methods_dynamic_output(methods: &mut MethodsBuilder) {
    /// `dynamic_output` allows a rule to use information that was not available when the rule was
    /// first run at analysis time. Examples include things like Distributed ThinLTO (where the
    /// index file is created by another action) or OCaml builds (where the dependencies are created
    /// by `ocamldeps`).
    ///
    /// The arguments are:
    ///
    /// * `dynamic` - a list of artifacts whose values will be available in the function. These will
    ///   be built before the function is run.
    /// * `inputs` - parameter is ignored.
    /// * `outputs` - a list of unbound artifacts (created with `declare_artifact`) which will be
    ///   bound by the function.
    /// * The function argument is given 3 arguments:
    ///   * `ctx` (context) - which is the same as that passed to the initial rule analysis.
    ///   * `artifacts` - using one of the artifacts from `dynamic` (example usage:
    ///     `artifacts[artifact_from_dynamic])` gives an artifact value containing the methods
    ///     `read_string`, `read_lines`, and `read_json` to obtain the values from the disk in
    ///     various formats.  Anything too complex should be piped through a Python script for
    ///     transformation to JSON.
    ///   * `outputs` - using one of the artifacts from the `dynamic_output`'s `outputs` (example
    ///     usage: `outputs[artifact_from_dynamic_output_outputs]`) gives an unbounded artifact. The
    ///     function argument must use its `outputs` argument to bind output artifacts, rather than
    ///     reusing artifacts from the outputs passed into `dynamic_output` directly.
    /// * The function must call `ctx.actions` (probably `ctx.actions.run`) to bind all outputs. It
    ///   can examine the values of the dynamic variables and depends on the inputs.
    ///   * The function will usually be a `def`, as `lambda` in Starlark does not allow statements,
    /// making it quite underpowered. For full details see
    /// https://buck2.build/docs/rule_authors/dynamic_dependencies/.
    ///
    /// Besides dynamic dependencies, there is a second use case for `dynamic_output`: say that you
    /// have some output artifact, and that the analysis to produce the action that outputs that
    /// artifact is expensive, ie takes a lot of CPU time; you would like to skip that work in
    /// builds that do not actually use that artifact.
    ///
    /// This can be accomplished by putting the analysis for that artifact behind a `dynamic_output`
    /// with an empty `dynamic` list. The `dynamic_output`'s function will not be run unless one of
    /// the actions it outputs is actually requested as part of the build.
    fn dynamic_output<'v>(
        this: &'v AnalysisActions<'v>,
        #[starlark(require = named)] dynamic: ValueOf<
            'v,
            ListOrTupleType<UnpackNonPromiseInputArtifact<'v>>,
        >,
        #[starlark(require = named)] inputs: Option<
            ValueOf<'v, ListOrTupleType<UnpackNonPromiseInputArtifact<'v>>>,
        >,
        #[starlark(require = named)] outputs: ValueOf<
            'v,
            ListOrTupleType<ValueTyped<'v, StarlarkOutputArtifact<'v>>>,
        >,
        #[starlark(require = named)] f: StarlarkCallable<
            'v,
            (
                Value<'static>,
                DictType<StarlarkArtifact, StarlarkArtifactValue>,
                DictType<StarlarkArtifact, StarlarkDeclaredArtifact>,
            ),
            NoneType,
        >,
    ) -> starlark::Result<NoneType> {
        // TODO(nga): delete.
        let _unused = inputs;

        // Parameter validation
        let outputs = outputs.try_to_vec()?;
        if outputs.is_empty() {
            return Err(buck2_error::Error::from(DynamicOutputError::EmptyOutput).into());
        }

        // Conversion
        let artifact_values = dynamic
            .try_items()
            .map(|x| x?.artifact())
            .collect::<buck2_error::Result<_>>()?;

        let attributes = this.attributes;
        let plugins = this.plugins;

        let mut this = this.state()?;

        let execution_platform = this.actions.execution_platform.dupe();

        let lambda_params_storage =
            DynamicLambdaParamsStorageImpl::get(&mut this.analysis_value_storage)?;

        let key = lambda_params_storage.next_dynamic_actions_key()?;
        let outputs = output_artifacts_to_lambda_build_artifacts(&key, outputs)?;

        // Registration
        let lambda_params = DynamicLambdaParams {
            attributes,
            plugins,
            lambda: f.erase(),
            attr_values: None,
            outputs,
            static_fields: DynamicLambdaStaticFields {
                artifact_values,
                dynamic_values: MiniBoxSlice::new(),
                execution_platform,
            },
        };
        lambda_params_storage.set_dynamic_actions(key, lambda_params)?;
        Ok(NoneType)
    }

    /// Declares a dynamic action that reads artifact contents to produce outputs.
    ///
    /// This is the new version of `dynamic_output` and will eventually replace it.
    /// Dynamic actions enable build decisions based on the actual content of intermediate
    /// artifacts.
    ///
    /// # Workflow
    ///
    /// 1. Define an implementation function with signature matching your dynamic attributes
    /// 2. Create a factory (`DynamicActionsCallable`) using `dynamic_actions(impl=..., attrs=...)`
    /// 3. Call that factory with concrete artifact values to create a `DynamicActions` instance
    /// 4. Pass the `DynamicActions` to `ctx.actions.dynamic_output_new()`
    ///
    /// # Arguments
    ///
    /// * `dynamic_actions` - A `DynamicActions` instance created by calling a `DynamicActionsCallable`
    ///
    /// # Returns
    ///
    /// A `DynamicValue` that can be consumed by other dynamic actions via `dynattrs.dynamic_value()`.
    ///
    /// # Example
    ///
    /// ```python
    /// # Step 1: Define the implementation function
    /// def _my_impl(actions: AnalysisActions, config: ArtifactValue, out: OutputArtifact):
    ///     content = config.read_string()
    ///     if "feature_enabled" in content:
    ///         actions.write(out, "feature output")
    ///     else:
    ///         actions.write(out, "default output")
    ///     return [DefaultInfo()]
    ///
    /// # Step 2: Create a factory
    /// _my_dynamic_action = dynamic_actions(
    ///     impl = _my_impl,
    ///     attrs = {
    ///         "config": dynattrs.artifact_value(),
    ///         "out": dynattrs.output(),
    ///     },
    /// )
    ///
    /// # Step 3 & 4: Use it in a rule or bxl script
    /// def _rule_impl(ctx: AnalysisContext):
    ///     config_file = ctx.actions.write("config.txt", "feature_enabled")
    ///     output = ctx.actions.declare_output("result")
    ///
    ///     # Call the factory to create a DynamicActions instance
    ///     dynamic_action = _my_dynamic_action(
    ///         config = config_file,
    ///         out = output.as_output(),
    ///     )
    ///
    ///     # Execute it
    ///     ctx.actions.dynamic_output_new(dynamic_action)
    ///     return [DefaultInfo(default_output = output)]
    /// ```
    ///
    /// See [Dynamic Dependencies](../../../rule_authors/dynamic_dependencies) for an overall
    /// overview of dynamic dependencies in buck2.
    ///
    /// For a guide on using this with BXL, see [How to run actions based on the content of artifacts](../../../bxl/how_tos/how_to_run_actions_based_on_the_content_of_artifact).
    fn dynamic_output_new<'v>(
        this: &'v AnalysisActions<'v>,
        #[starlark(require = pos)] dynamic_actions: ValueTyped<'v, StarlarkDynamicActions<'v>>,
    ) -> starlark::Result<StarlarkDynamicValue> {
        let dynamic_actions = dynamic_actions
            .data
            .try_borrow_mut()
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))?
            .take()
            .ok_or_else(|| {
                internal_error!(
                    "dynamic_action data can be used only in one `dynamic_output_new` call",
                )
            })?;
        let StarlarkDynamicActionsData {
            attr_values,
            callable,
        } = dynamic_actions;

        let mut this = this.state()?;

        let execution_platform = this.actions.execution_platform.dupe();

        let lambda_params_storage =
            DynamicLambdaParamsStorageImpl::get(&mut this.analysis_value_storage)?;
        let key = lambda_params_storage.next_dynamic_actions_key()?;

        attr_values.bind(&key)?;

        let outputs = attr_values.outputs()?;
        let artifact_values = attr_values.artifact_values();
        let dynamic_values = attr_values.dynamic_values();

        // Registration
        let lambda_params = DynamicLambdaParams {
            attributes: None,
            plugins: None,
            lambda: callable.implementation.erase(),
            attr_values: Some((attr_values, callable)),
            outputs,
            static_fields: DynamicLambdaStaticFields {
                artifact_values,
                dynamic_values,
                execution_platform,
            },
        };

        lambda_params_storage.set_dynamic_actions(key.dupe(), lambda_params)?;

        Ok(StarlarkDynamicValue {
            dynamic_value: DynamicValue {
                dynamic_lambda_results_key: key,
            },
        })
    }
}
