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
use starlark::values::StringValue;
use starlark::values::Value;
use starlark::values::none::NoneOr;
use starlark::values::none::NoneType;

use crate::interpreter::module_internals::ModuleInternals;

#[starlark_module]
pub(crate) fn register_module_natives(globals: &mut GlobalsBuilder) {
    /// Check if the target with `name` has already been defined,
    /// returns `True` if it has.
    ///
    /// Note that this function checks for the existence of a _target_ rather than a _rule_.
    /// In general use of this function is discouraged, as it makes definitions of rules not compose.
    fn rule_exists(name: &str, eval: &mut Evaluator) -> starlark::Result<bool> {
        Ok(ModuleInternals::from_context(eval, "rule_exists")?.target_exists(name))
    }

    /// Called in a `BUCK` file to declare the oncall contact details for
    /// all the targets defined. Must be called at most once, before any targets
    /// have been declared. Errors if called from a `.bzl` file.
    fn oncall(
        #[starlark(require = pos)] name: &str,
        eval: &mut Evaluator,
    ) -> starlark::Result<NoneType> {
        let internals = ModuleInternals::from_context(eval, "oncall")?;
        internals.set_oncall(name)?;
        Ok(NoneType)
    }

    /// Called in a `BUCK` file to retrieve the previously set `oncall`, or `None` if none has been set.
    /// It is an error to call `oncall` after calling this function.
    fn read_oncall<'v>(
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneOr<StringValue<'v>>> {
        let internals = ModuleInternals::from_context(eval, "read_oncall")?;
        match internals.get_oncall() {
            None => Ok(NoneOr::None),
            Some(oncall) => Ok(NoneOr::Other(eval.heap().alloc(oncall.as_str()))),
        }
    }

    fn implicit_package_symbol<'v>(
        name: &str,
        default: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let internals = ModuleInternals::from_context(eval, "implicit_package_symbol")?;
        match internals.get_package_implicit(name) {
            None => Ok(default.unwrap_or_else(Value::new_none)),
            Some(v) => Ok(v.add_to_heap(eval.heap())),
        }
    }
}
