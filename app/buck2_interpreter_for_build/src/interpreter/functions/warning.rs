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
use starlark::starlark_module;
use starlark::values::none::NoneType;

#[starlark_module]
pub(crate) fn register_warning(builder: &mut GlobalsBuilder) {
    /// Print a warning. The line will be decorated with the timestamp and other details,
    /// including the word `WARN` (colored, if the console supports it).
    ///
    /// If you are not writing a warning, use `print` instead. Be aware that printing
    /// lots of output (warnings or not) can be cause all information to be ignored by the user.
    fn warning(#[starlark(require = pos)] x: &str) -> starlark::Result<NoneType> {
        tracing::warn!("{}", x);
        Ok(NoneType)
    }
}
