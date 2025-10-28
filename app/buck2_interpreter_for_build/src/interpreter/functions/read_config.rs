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

use crate::interpreter::build_context::BuildContext;

#[starlark_module]
pub(crate) fn register_read_config(globals: &mut GlobalsBuilder) {
    /// Read a configuration value from the .buckconfig for the current
    /// cell of the `BUCK` file that started evaluation of this code.
    ///
    /// As an example, if the current cell's .buckconfig contains:
    ///
    /// ```toml
    /// [package_options]
    /// compile = super_fast
    /// ```
    ///
    /// Then you would get the following results:
    ///
    /// ```python
    /// read_config("package_options", "compile") == "super_fast"
    /// read_config("package_options", "linker") == None
    /// read_config("package_options", "linker", "a_default") == "a_default"
    /// ```
    ///
    /// In general the use of `.buckconfig` is discouraged in favour of `select`,
    /// but it can still be useful.
    ///
    /// ### Cell segmentation
    ///
    /// Historically, Buck has re-evaluated each .bzl file once per cell it is imported
    /// into. That meant that `read_config` would read from a cell based on where the
    /// chain of imports started. Re-evaluating came with undesirable behaviour for
    /// cross-cell type checking so this behaviour can now be disabled, but be aware
    /// that this affects read_config().
    ///
    /// In the prelude, and when you disable this behaviour for all cells with the
    /// buckconfig `buck2.disable_cell_segmentation`, the cell that this reads from
    /// depends on the function call stack.
    ///
    /// ```python
    /// # root .buckconfig
    /// [buck2]
    ///     disable_cell_segmentation = true
    ///
    /// # hello//:world.bzl
    /// ABC_DEF = read_config("abc", "def")
    /// def dynamic() -> str:
    ///     return read_config("abc", "def")
    ///
    /// # root//BUCK
    /// load("@hello//:world.bzl", "ABC_DEF", "dynamic")
    /// print(ABC_DEF)      # from hello/.buckconfig
    /// print(dynamic())    # from root .buckconfig
    /// ```
    ///
    /// Please note that you can no longer read_config during analysis.
    /// Previously this may have worked occasionally due to inlining.
    //
    // Unlike read_root_config, this is NOT speculative_exec_safe.
    // Its return value depends on the call stack.
    fn read_config<'v>(
        section: StringValue,
        key: StringValue,
        default: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let buckconfigs = &BuildContext::from_context(eval)?.buckconfigs;
        match buckconfigs.current_cell_get(section, key, eval)? {
            Some(v) => Ok(v.to_value()),
            None => Ok(default.unwrap_or_else(Value::new_none)),
        }
    }

    /// Like `read_config` but the project root `.buckconfig` is always consulted,
    /// regardless of the cell of the originating `BUCK` file.
    #[starlark(speculative_exec_safe)]
    fn read_root_config<'v>(
        #[starlark(require = pos)] section: StringValue,
        #[starlark(require = pos)] key: StringValue,
        // Unlike `read_config` we only allow string or `None` as default.
        #[starlark(require = pos, default = NoneOr::None)] default: NoneOr<StringValue<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneOr<StringValue<'v>>> {
        let buckconfigs = &BuildContext::from_context(eval)?.buckconfigs;
        match buckconfigs.root_cell_get(section, key, eval)? {
            Some(v) => Ok(NoneOr::Other(v)),
            None => Ok(default),
        }
    }
}
