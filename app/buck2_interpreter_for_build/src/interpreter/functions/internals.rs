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
use starlark::values::list::AllocList;
use starlark::values::none::NoneType;

use crate::interpreter::module_internals::ModuleInternals;

#[derive(buck2_error::Error, Debug)]
#[error("Fail: {0}")]
#[buck2(tag = Tier0)]
struct BuckFail(String);

/// Registers functions that are only available in the `__internal__` global and not meant to be
/// stable.
#[starlark_module]
pub(crate) fn register_internals(builder: &mut GlobalsBuilder) {
    /// `fail()` but implemented using a buck2 error type instead of starlark's, for testing
    /// purposes.
    fn buck2_fail<'v>(msg: &str, _eval: &mut Evaluator<'v, '_, '_>) -> starlark::Result<NoneType> {
        Err(buck2_error::Error::from(BuckFail(msg.to_owned())).into())
    }

    /// Returns a list of direct subpackage relative paths of current package.
    fn sub_packages<'v>(
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> starlark::Result<AllocList<Vec<String>>> {
        let extra = ModuleInternals::from_context(eval, "sub_packages")?;
        Ok(AllocList(
            extra
                .sub_packages()
                .map(|p| p.as_str().to_owned())
                .collect(),
        ))
    }
}
