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
use std::sync::atomic;
use std::sync::atomic::AtomicU64;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::values::Freeze;
use starlark::values::FreezeError;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictType;
use starlark::values::list::AllocList;
use starlark::values::none::NoneOr;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::cmd_args::CommandLineArgLike;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgsView;
use crate::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;

/// Provider that signals that a rule is a worker tool
#[internal_provider(worker_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[freeze(validator = validate_worker_info)]
#[repr(C)]
pub struct WorkerInfo<'v> {
    // Command to spawn a new worker
    pub exe: ValueOf<'v, StarlarkCmdArgsView<'v>>,
    /// Environment for the command that spawns the worker.
    env: Option<ValueOf<'v, DictType<StringValue<'v>, ValueAsCommandLineLike<'v>>>>,
    // Maximum number of concurrent commands to execute on a worker instance without queuing
    pub concurrency: Option<ValueOf<'v, usize>>,
    // Whether to always run actions using this worker via the streaming API
    pub streaming: Option<ValueOf<'v, bool>>,
    // Bazel remote persistent worker protocol capable worker
    pub supports_bazel_remote_persistent_worker_protocol: ValueOf<'v, bool>,

    pub id: u64,
}

fn next_id() -> u64 {
    static LAST_ID: AtomicU64 = AtomicU64::new(0);
    LAST_ID.fetch_add(1, atomic::Ordering::Relaxed) + 1
}

#[starlark_module]
fn worker_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = WorkerInfo<'static>)]
    fn WorkerInfo<'v>(
        #[starlark(default = AllocList::EMPTY)] exe: Value<'v>,
        #[starlark(default = NoneOr::None)] env: NoneOr<
            ValueOf<'v, DictType<StringValue<'v>, ValueAsCommandLineLike<'v>>>,
        >,
        #[starlark(require = named, default = NoneOr::None)] concurrency: NoneOr<
            ValueOf<'v, usize>,
        >,
        #[starlark(require = named, default = NoneOr::None)] streaming: NoneOr<ValueOf<'v, bool>>,
        #[starlark(require = named, default = false)]
        supports_bazel_remote_persistent_worker_protocol: bool,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<WorkerInfo<'v>> {
        let heap = eval.heap();
        let valid_exe = StarlarkCmdArgs::try_from_value(exe)?;
        let id = next_id();
        Ok(WorkerInfo {
            exe: ValueOf::alloc(heap, valid_exe),
            env: env.into_option(),
            id,
            concurrency: concurrency.into_option(),
            streaming: streaming.into_option(),
            supports_bazel_remote_persistent_worker_protocol: ValueOf::new_unchecked(
                heap.alloc(supports_bazel_remote_persistent_worker_protocol),
            ),
        })
    }
}

impl<'v> WorkerInfo<'v> {
    pub fn exe_command_line(&self) -> &'v dyn CommandLineArgLike<'v> {
        self.exe.unpack().as_command_line_arg_like()
    }

    pub fn env(&self) -> buck2_error::Result<Vec<(&'v str, &'v dyn CommandLineArgLike<'v>)>> {
        let Some(env) = self.env else {
            return Ok(Vec::new());
        };
        env.iter()
            .map(|entry| {
                let (key, value) = entry?;
                Ok((key.unpack().as_str(), value.unpack().0))
            })
            .collect()
    }

    pub fn concurrency(&self) -> Option<usize> {
        self.concurrency.map(|v| v.unpack())
    }

    pub fn streaming(&self) -> bool {
        self.streaming.is_some_and(|v| v.unpack())
    }

    pub fn supports_bazel_remote_persistent_worker_protocol(&self) -> bool {
        self.supports_bazel_remote_persistent_worker_protocol
            .unpack()
    }
}

fn validate_worker_info<'v>(info: &WorkerInfo<'v>) -> buck2_error::Result<()> {
    if info.exe.unpack().is_empty() {
        return Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Input,
            "Value for `exe` field is an empty command line: `{}`",
            info.exe
        ));
    }
    Ok(())
}
