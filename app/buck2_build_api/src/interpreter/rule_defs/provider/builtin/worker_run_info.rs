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
use starlark::values::StarlarkPagable;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::AllocList;
use starlark::values::none::NoneOr;

use crate as buck2_build_api;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgs;
use crate::interpreter::rule_defs::cmd_args::StarlarkCmdArgsView;
use crate::interpreter::rule_defs::provider::builtin::worker_info::WorkerInfo;

/// Provider that signals that a rule can run using a worker
#[internal_provider(worker_run_info_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct WorkerRunInfo<'v> {
    // Configuration needed to spawn a new local worker
    worker: Option<ValueOf<'v, WorkerInfo<'v>>>,

    // Configuration needed to spawn a new remote worker
    remote_worker: Option<ValueOf<'v, WorkerInfo<'v>>>,

    // Command to execute without spawning a worker, when the build environment or configuration does not support workers
    exe: ValueOf<'v, StarlarkCmdArgsView<'v>>,
}

#[starlark_module]
fn worker_run_info_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = WorkerRunInfo<'static>)]
    fn WorkerRunInfo<'v>(
        #[starlark(require = named, default = NoneOr::None)] worker: NoneOr<
            ValueOf<'v, WorkerInfo<'v>>,
        >,
        #[starlark(require = named, default = NoneOr::None)] remote_worker: NoneOr<
            ValueOf<'v, WorkerInfo<'v>>,
        >,
        #[starlark(require = named, default = AllocList::EMPTY)] exe: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<ValueTyped<'v, WorkerRunInfo<'v>>> {
        let heap = eval.heap();
        let valid_exe = StarlarkCmdArgs::try_from_value(exe)?;

        Ok(heap.alloc_representation(WorkerRunInfo {
            worker: worker.into_option(),
            remote_worker: remote_worker.into_option(),
            exe: ValueOf::alloc(heap, valid_exe),
        }))
    }
}

impl<'v> WorkerRunInfo<'v> {
    pub fn worker(&self) -> Option<ValueTyped<'v, WorkerInfo<'v>>> {
        self.worker.map(|v| v.get())
    }

    pub fn remote_worker(&self) -> Option<ValueTyped<'v, WorkerInfo<'v>>> {
        self.remote_worker.map(|v| v.get())
    }

    pub fn exe(&self) -> ValueOf<'v, StarlarkCmdArgsView<'v>> {
        self.exe
    }
}
