/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::values::Freeze;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListRef;
use starlark::values::type_repr::StarlarkTypeRepr;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::cmd_args::ArtifactPathMapper;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::CommandLineArtifactVisitor;
use crate::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgsView;
use crate::interpreter::rule_defs::cmd_args::StarlarkCommandLineValueUnpack;
use crate::interpreter::rule_defs::cmd_args::WriteToFileMacroVisitor;
use crate::interpreter::rule_defs::cmd_args::command_line_arg_like_type::command_line_arg_like_impl;

/// Provider that signals a rule is runnable and defines how to execute it.
///
/// `RunInfo` marks a target as executable and specifies how to run it. When a target
/// provides `RunInfo`, it can be executed with `buck2 run <target>`. The provider wraps the
/// command-line arguments that define the executable and its parameters.
///
/// # Usage
///
/// To run a target with `RunInfo`:
/// ```bash
/// buck2 run <target>                    # Run with the args specified in RunInfo
/// buck2 run <target> -- [extra args]    # Pass additional args after '--' to the command
/// ```
/// ## Examples
///
/// Basic executable with script:
/// ```python
/// def _sh_binary_impl(ctx):
///     script = ctx.actions.write("script.sh", ctx.attrs.script_content, is_executable = True)
///     return [
///         DefaultInfo(default_output = script),
///         RunInfo(args = cmd_args(script)),
///     ]
/// ```
///
/// Executable with arguments and resources:
/// ```python
/// def _binary_impl(ctx):
///     exe = ...
///
///     return [
///         DefaultInfo(default_output = exe),
///         RunInfo(
///             args = cmd_args(exe),
///         ),
///     ]
/// ```
#[internal_provider(run_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(transparent)]
pub struct RunInfo<'v> {
    /// The command to run.
    args: ValueOf<'v, StarlarkCmdArgsView<'v>>,
}

/// A `RunInfo` kept alive by its owning frozen heap; usable across threads and awaits.
pub type OwnedRunInfo = OwnedFrozen<ValueTyped<'static, RunInfo<'static>>>;

#[starlark_module]
fn run_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = RunInfo<'static>)]
    fn RunInfo<'v>(
        // TODO(nga): make the argument either named or positional.
        #[starlark(default = StarlarkCommandLineValueUnpack::List(ListRef::empty()))]
        args: StarlarkCommandLineValueUnpack<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<RunInfo<'v>> {
        let heap = eval.heap();
        let valid_args = StarlarkCmdArgs::try_from_value_typed(args)?;
        Ok(RunInfo {
            args: ValueOf::alloc(heap, valid_args),
        })
    }
}

impl<'v> CommandLineArgLike<'v> for RunInfo<'v> {
    fn register_me(&self) {
        command_line_arg_like_impl!(RunInfo::starlark_type_repr());
    }

    fn add_to_command_line(&self, fmt: &mut CommandLineBuilder<'v, '_>) -> buck2_error::Result<()> {
        self.args
            .unpack()
            .as_command_line_arg_like()
            .add_to_command_line(fmt)
    }

    fn visit_artifacts(
        &self,
        visitor: &mut dyn CommandLineArtifactVisitor<'v>,
    ) -> buck2_error::Result<()> {
        self.args
            .unpack()
            .as_command_line_arg_like()
            .visit_artifacts(visitor)
    }

    fn contains_arg_attr(&self) -> bool {
        self.args
            .unpack()
            .as_command_line_arg_like()
            .contains_arg_attr()
    }

    fn visit_write_to_file_macros(
        &self,
        _visitor: &mut dyn WriteToFileMacroVisitor,
        _artifact_path_mapping: &dyn ArtifactPathMapper,
    ) -> buck2_error::Result<()> {
        Ok(())
    }
}
