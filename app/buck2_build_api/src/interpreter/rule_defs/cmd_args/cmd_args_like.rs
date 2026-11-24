/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_interpreter::types::cell_path::StarlarkCellPath;
use buck2_interpreter::types::cell_root::CellRoot;
use buck2_interpreter::types::configured_providers_label::StarlarkConfiguredProvidersLabel;
use buck2_interpreter::types::project_root::StarlarkProjectRoot;
use buck2_interpreter::types::target_label::StarlarkTargetLabel;
use either::Either;
use starlark::starlark_predicate_type;
use starlark::starlark_recursive_type;
use starlark::values::list::StarlarkList;
use starlark::values::string::StarlarkStr;

use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;

starlark_predicate_type! {
    /// A value that can be put on a command line.
    ///
    /// A string is one, and so are an artifact, which stands for its path, a `cmd_args`, a
    /// label, a `RunInfo`, and the other types that know how to add themselves to a command
    /// line. One value can be several arguments, as a `cmd_args` is. It is what `cmd_args`
    /// holds, and what a command or an environment variable of an action is made of.
    ///
    /// A list is not one. `cmd_args()` and `cmd_args.add()` take lists as well, and copy
    /// the items out.
    pub StarlarkCmdArgsLike<'v>;
    name "CmdArgsLike";
    get &'v dyn CommandLineArgLike<'v>;
    // Strings come first: most of what is put on a command line is one.
    upstream_member StarlarkStr;
    upstream_member StarlarkCellPath;
    upstream_member StarlarkTargetLabel;
    upstream_member StarlarkConfiguredProvidersLabel;
    upstream_member CellRoot;
    upstream_member StarlarkProjectRoot;
}

starlark_recursive_type! {
    /// What `cmd_args` takes: a value that can be put on a command line, or a list of these.
    ///
    /// The lists may hold lists, to any depth. `cmd_args()` and `cmd_args.add()` copy the
    /// items out when they are called, so a list that changes afterwards does not change
    /// the command line.
    pub StarlarkCmdArgsInput<'v>;
    name "CmdArgsInput";
    is Either<StarlarkList<'v, StarlarkCmdArgsInput>, StarlarkCmdArgsLike>;
}
