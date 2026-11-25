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

use crate::interpreter::build_context::BuildContext;

#[starlark_module]
pub(crate) fn register_read_config(globals: &mut GlobalsBuilder) {
    /// Read a configuration value from the `.buckconfig` of the cell of the file that is
    /// being evaluated: the `BUCK`, `PACKAGE` or `bxl` file, also when the call is in a
    /// function that the file loaded from another cell.
    ///
    /// At the top level of a `.bzl` file, that is the cell of the `.bzl` file if the root
    /// cell sets `buck2.eval_bzl_in_own_cell = true`. Otherwise it is the cell of the file
    /// that loads the `.bzl` file, and the `.bzl` file is evaluated once for each such cell.
    ///
    /// As an example, if that cell's .buckconfig contains:
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
    /// Only available while a file is loaded. A rule implementation, a transition or the
    /// implementation of a `bxl` script cannot call it; read the value at the top level of the
    /// file instead, or pass it in through an attribute.
    fn read_config<'v>(
        section: StringValue,
        key: StringValue,
        #[starlark(default = NoneType)] default: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<Value<'v>> {
        let buckconfigs = &BuildContext::from_context(eval)?.buckconfigs;
        match buckconfigs.current_cell_get(section, key, eval)? {
            Some(v) => Ok(v.to_value()),
            None => Ok(default),
        }
    }

    /// Like `read_config` but the project root `.buckconfig` is always consulted,
    /// regardless of the cell of the originating `BUCK` file.
    #[starlark(speculative_exec_safe)]
    fn read_root_config<'v>(
        #[starlark(require = pos)] section: StringValue,
        #[starlark(require = pos)] key: StringValue,
        // Unlike `read_config` we only allow string or `None` as default.
        #[starlark(require = pos, default = NoneType)] default: NoneOr<StringValue<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<NoneOr<StringValue<'v>>> {
        let buckconfigs = &BuildContext::from_context(eval)?.buckconfigs;
        match buckconfigs.root_cell_get(section, key, eval)? {
            Some(v) => Ok(NoneOr::Other(v)),
            None => Ok(default),
        }
    }
}
