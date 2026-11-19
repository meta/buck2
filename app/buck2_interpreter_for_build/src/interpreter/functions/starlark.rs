/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::none::NoneType;

use crate::interpreter::build_context::BuildContext;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum StarlarkPeakMemoryError {
    #[error("starlark peak memory already set in this file")]
    MemorySetInThisFile(),
}

#[starlark_module]
pub(crate) fn register_set_starlark_peak_allocated_byte_limit(globals: &mut GlobalsBuilder) {
    /// Set the peak allocated bytes during evaluation of build ctx.
    /// Err if it has already been set
    fn set_starlark_peak_allocated_byte_limit<'v>(
        #[starlark(require = pos)] value: u64,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        let build_ctx = BuildContext::from_context(eval)?;
        // Only a build file evaluation reads the limit; anywhere else it would be dropped.
        build_ctx
            .additional
            .require_build("set_starlark_peak_allocated_byte_limit")?;
        let limit = &build_ctx.starlark_peak_allocated_byte_limit;
        if limit.get().is_some() || limit.set(Some(value)).is_err() {
            return Err(
                buck2_error::Error::from(StarlarkPeakMemoryError::MemorySetInThisFile()).into(),
            );
        }
        Ok(NoneType)
    }
}
