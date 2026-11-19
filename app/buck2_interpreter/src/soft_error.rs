/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::soft_error_or_panic;
use starlark::eval::SoftErrorHandler;
pub struct Buck2StarlarkSoftErrorHandler;

/// Starlark-rust reports inconsistencies it recovers from (corrupt spans) through this hook;
/// they become buck2 soft errors.
impl SoftErrorHandler for Buck2StarlarkSoftErrorHandler {
    fn soft_error(&self, category: &str, error: starlark::Error) -> Result<(), starlark::Error> {
        let error = buck2_error::Error::from(error);
        soft_error_or_panic!(&format!("starlark_rust_{category}"), error);
        Ok(())
    }
}
