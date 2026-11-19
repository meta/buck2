/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use buck2_core::target::label::label::TargetLabel;
use buck2_node::attrs::coercion_context::AttrCoercionContext;
use buck2_node::call_stack::StarlarkCallStack;
use buck2_node::nodes::unconfigured::UnsealedTargetNode;
use buck2_node::package::Package;
use buck2_node::rule::Rule;
use dupe::Dupe;
use dupe::OptionDupedExt;
use starlark::eval::CallStack;
use starlark::eval::ParametersParser;
use starlark::values::Value;

use crate::call_stack::StarlarkCallStackWrapper;
use crate::interpreter::module_internals::ModuleInternals;
use crate::nodes::attr_spec::AttributeSpecExt;

pub trait UnsealedTargetNodeExt: Sized {
    fn from_params_ignore_attrs_for_profiling<'v>(
        rule: Arc<Rule>,
        package: Arc<Package>,
        internals: &ModuleInternals,
        param_parser: &mut ParametersParser<'v, '_>,
    ) -> buck2_error::Result<Self>;

    fn from_params<'v>(
        rule: Arc<Rule>,
        package: Arc<Package>,
        internals: &ModuleInternals,
        coercion_ctx: &dyn AttrCoercionContext<'v>,
        param_parser: &mut ParametersParser<'v, '_>,
        arg_count: usize,
        ignore_attrs_for_profiling: bool,
        call_stack: Option<CallStack>,
    ) -> buck2_error::Result<Self>;
}

impl UnsealedTargetNodeExt for UnsealedTargetNode {
    /// Extract only the name attribute from rule arguments, ignore the others.
    fn from_params_ignore_attrs_for_profiling<'v>(
        rule: Arc<Rule>,
        package: Arc<Package>,
        internals: &ModuleInternals,
        param_parser: &mut ParametersParser<'v, '_>,
    ) -> buck2_error::Result<Self> {
        let (name, indices, attributes) =
            rule.attributes.start_parse(param_parser, 1, internals)?;

        for (_, _, _) in indices {
            // Consume all the arguments.
            // We call `next_opt` even for non-optional parameters. starlark-rust doesn't check.
            param_parser.next_opt::<Value>()?;
        }

        let package_cfg_modifiers = internals.super_package.cfg_modifiers().duped();
        let label = TargetLabel::new(internals.buildfile_path().package().dupe(), name);
        Ok(UnsealedTargetNode {
            rule,
            package,
            label,
            attributes,
            call_stack: None,
            package_cfg_modifiers,
            test_config_unification_rollout: internals
                .super_package
                .test_config_unification_rollout(),
        })
    }

    /// The body of the callable returned by `rule()`. Records the target in this package's `TargetMap`
    #[allow(clippy::box_collection)] // Parameter `call_stack`, because this is the field type.
    fn from_params<'v>(
        rule: Arc<Rule>,
        package: Arc<Package>,
        internals: &ModuleInternals,
        coercion_ctx: &dyn AttrCoercionContext<'v>,
        param_parser: &mut ParametersParser<'v, '_>,
        arg_count: usize,
        ignore_attrs_for_profiling: bool,
        call_stack: Option<CallStack>,
    ) -> buck2_error::Result<Self> {
        if ignore_attrs_for_profiling {
            return Self::from_params_ignore_attrs_for_profiling(
                rule,
                package,
                internals,
                param_parser,
            );
        }

        let (target_name, attributes) =
            rule.attributes
                .parse_params(param_parser, arg_count, internals, coercion_ctx)?;
        let package_name = internals.buildfile_path().package();

        let label = TargetLabel::new(package_name.dupe(), target_name);
        let package_cfg_modifiers = internals.super_package.cfg_modifiers().duped();

        Ok(UnsealedTargetNode {
            rule,
            package,
            label,
            attributes,
            call_stack: call_stack
                .map(StarlarkCallStackWrapper)
                .map(StarlarkCallStack::new),
            package_cfg_modifiers,
            test_config_unification_rollout: internals
                .super_package
                .test_config_unification_rollout(),
        })
    }
}
