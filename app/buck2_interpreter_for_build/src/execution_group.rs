/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_interpreter::execution_group::EXEC_GROUP_TYPE;
use buck2_interpreter::execution_group::FrozenStarlarkExecutionGroup;
use buck2_interpreter::execution_group::StarlarkExecutionGroup;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::ValueTyped;
use starlark::values::starlark_value_as_type::StarlarkValueAsType;

use crate::interpreter::build_context::BuildContext;

#[starlark_module]
fn register_execution_group_globals(r: &mut GlobalsBuilder) {
    const ExecGroup: StarlarkValueAsType<StarlarkExecutionGroup> = EXEC_GROUP_TYPE;

    /// Create a new execution group.
    ///
    /// The value returned must be immediately bound to a global, which names the group:
    ///
    /// ```python
    /// Link = exec_group()
    ///
    /// my_rule = rule(
    ///     impl = _impl,
    ///     attrs = {
    ///         "_linker": attrs.exec_dep(default = ":linker", exec_group = Link),
    ///     },
    ///     exec_groups = [Link],
    /// )
    ///
    /// def _impl(ctx):
    ///     ctx.actions.run(cmd, category = "link", exec_group = Link)
    /// ```
    ///
    /// Each execution group a rule declares gets its own execution platform, resolved from the
    /// target's `exec_compatible_with` and the exec deps and toolchain deps in that group. Exec
    /// deps and toolchain deps without an `exec_group` are in `DefaultExecGroup`, whose
    /// platform is the target's execution platform. A rule that declares execution groups can
    /// use `DefaultExecGroup` only if it lists it in `exec_groups`, and must give each of its exec
    /// deps and toolchain deps an `exec_group`, `DefaultExecGroup` included. Its args need an
    /// `exec_group` only to refer to tools.
    fn exec_group<'v>(
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, StarlarkExecutionGroup>> {
        let cell_path = BuildContext::from_context(eval)?
            .starlark_path()
            .path()
            .into_owned();
        Ok(eval
            .heap()
            .alloc_representation(StarlarkExecutionGroup::new_unbound(cell_path)))
    }

    /// The execution group of every exec dep, toolchain dep and action that does not specify one.
    /// Its execution platform is the target's execution platform. A rule that declares named
    /// execution groups must list it in `exec_groups` to use it, and must name it on exec deps,
    /// toolchain deps and args that refer to tools.
    #[starlark(representation)]
    const DefaultExecGroup: FrozenStarlarkExecutionGroup = FrozenStarlarkExecutionGroup::DEFAULT;
}

pub(crate) fn register_execution_group(globals: &mut GlobalsBuilder) {
    register_execution_group_globals(globals);
}
