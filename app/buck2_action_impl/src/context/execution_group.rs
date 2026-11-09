/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_interpreter::execution_group::StarlarkExecutionGroupView;
use starlark::values::none::NoneOr;

/// The execution group of an action from its `exec_group` argument: the default group when
/// it is `None`.
pub(crate) fn action_execution_group(
    execution_group: NoneOr<StarlarkExecutionGroupView<'_>>,
) -> buck2_error::Result<ExecutionGroup> {
    match execution_group.into_option() {
        None => Ok(ExecutionGroup::Default),
        Some(group) => group.expect_bound(),
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ActionExecutionGroupError {
    #[error(
        "The action runs in the default execution group, which the rule does not declare. Pass `exec_group`, or add `DefaultExecGroup` to `exec_groups`"
    )]
    DefaultExecutionGroupNotDeclared,
}

/// Rejects an action in the default execution group of a rule that declares named execution
/// groups but not `DefaultExecGroup`.
pub(crate) fn check_default_execution_group_declared(
    execution_platform: &ExecutionPlatformResolution,
    execution_group: &ExecutionGroup,
) -> buck2_error::Result<()> {
    if *execution_group == ExecutionGroup::Default
        && !execution_platform.default_execution_group_declared()
    {
        return Err(ActionExecutionGroupError::DefaultExecutionGroupNotDeclared.into());
    }
    Ok(())
}
