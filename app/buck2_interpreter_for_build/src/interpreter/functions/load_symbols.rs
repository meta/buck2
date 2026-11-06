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
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::dict::DictType;
use starlark::values::none::NoneType;

#[starlark_module]
pub(crate) fn register_load_symbols(builder: &mut GlobalsBuilder) {
    /// Used in a `.bzl` file to set exported symbols. In most cases just defining
    /// the symbol as a top-level binding is sufficient, but sometimes the names
    /// might be programatically generated.
    ///
    /// It is undefined behaviour if you try and use any of the symbols exported
    /// here later in the same module, or if they overlap with existing definitions.
    /// This function should be used rarely.
    fn load_symbols<'v>(
        symbols: ValueOf<'v, DictType<&'v str, Value<'v>>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneType> {
        for entry in symbols.try_items() {
            let (k, v) = entry?;
            eval.set_module_variable_at_some_point(k, v)?;
        }
        Ok(NoneType)
    }
}
