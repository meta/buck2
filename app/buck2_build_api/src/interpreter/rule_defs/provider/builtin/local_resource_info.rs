/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_hash::BuckMutInsertionMap;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::OwnedFrozen;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::dict::DictType;
use starlark::values::float::UnpackFloat;
use starlark::values::none::NoneOr;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgsView;
use crate::interpreter::rule_defs::cmd_args::StarlarkCommandLineValueUnpack;

#[internal_provider(local_resource_info_creator)]
#[derive(
    Clone,
    Debug,
    Freeze,
    Trace,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[freeze(validator = validate_local_resource_info)]
#[repr(C)]
pub struct LocalResourceInfo<'v> {
    /// Command to run to initialize a local resource.
    ///
    /// Running this command writes a JSON to stdout.
    /// This JSON represents a pool of local resources which are ready to be used.
    /// Example JSON would be:
    ///
    /// ```text
    /// {
    ///   "pid": 42,
    ///   "resources": [
    ///     {"socket_address": "foo:1"},
    ///     {"socket_address": "bar:2"}
    ///   ]
    /// }
    /// ```
    ///
    /// Where `"pid"` maps to a PID of a process which should be sent SIGTERM to release the pool of resources
    /// when they are no longer needed. `"resources"` maps to the pool of resources.
    /// When a local resource from this particular pool is needed for an execution command, single entity
    /// will be reserved from the pool, for example `{"socket_address": "bar:2"}` and environment variable with
    /// name resolved using mapping in `resource_env_vars` field and `"socket_address"` key will be added to
    /// execution command.
    setup: ValueOf<'v, StarlarkCmdArgsView<'v>>,
    /// Mapping from environment variable (appended to an execution command which is dependent on this local resource)
    /// to keys in setup command JSON output.
    resource_env_vars: ValueOf<'v, DictType<StringValue<'v>, StringValue<'v>>>,
    /// Timeout in seconds for `setup` command.
    setup_timeout_seconds: Option<ValueOf<'v, UnpackFloat>>,
}

fn validate_local_resource_info<'v>(info: &LocalResourceInfo<'v>) -> buck2_error::Result<()> {
    if info.resource_env_vars.dict().is_empty() {
        return Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Value for `resource_env_vars` field is an empty dictionary: `{}`",
            info.resource_env_vars
        ));
    }

    if info.setup.unpack().is_empty() {
        return Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Value for `setup` field is an empty command line: `{}`",
            info.setup
        ));
    }

    Ok(())
}

#[starlark_module]
fn local_resource_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = LocalResourceInfo<'static>)]
    fn LocalResourceInfo<'v>(
        #[starlark(require = named)] setup: StarlarkCommandLineValueUnpack<'v>,
        #[starlark(require = named)] resource_env_vars: ValueOf<
            'v,
            DictType<StringValue<'v>, StringValue<'v>>,
        >,
        #[starlark(require = named, default = NoneOr::None)] setup_timeout_seconds: NoneOr<
            ValueOf<'v, UnpackFloat>,
        >,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<LocalResourceInfo<'v>> {
        let setup = StarlarkCmdArgs::try_from_value_typed(setup)?;
        let result = LocalResourceInfo {
            setup: ValueOf::alloc(eval.heap(), setup),
            resource_env_vars,
            setup_timeout_seconds: setup_timeout_seconds.into_option(),
        };
        validate_local_resource_info(&result)?;
        Ok(result)
    }
}

/// A `LocalResourceInfo` kept alive by its owning frozen heap; usable across threads and awaits.
pub type OwnedLocalResourceInfo = OwnedFrozen<ValueTyped<'static, LocalResourceInfo<'static>>>;

impl<'v> LocalResourceInfo<'v> {
    /// Mapping from keys in setup command JSON output to environment variables keys which
    /// should be appended to execution commands dependent on this local resource.
    pub fn env_var_mapping(&self) -> buck2_error::Result<BuckMutInsertionMap<String, String>> {
        self.resource_env_vars
            .iter()
            .map(|entry| {
                let (k, v) = entry?;
                Ok((
                    k.unpack().as_str().to_owned(),
                    v.unpack().as_str().to_owned(),
                ))
            })
            .collect()
    }

    pub fn setup_command_line(&self) -> &'v dyn CommandLineArgLike<'v> {
        self.setup.unpack().as_command_line_arg_like()
    }

    pub fn setup_timeout(&self) -> Option<Duration> {
        self.setup_timeout_seconds
            .map(|f| Duration::from_secs_f64(f.unpack().0))
    }
}
