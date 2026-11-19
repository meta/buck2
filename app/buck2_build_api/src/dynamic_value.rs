/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use dupe::Dupe;
use pagable::Pagable;
use starlark::values::StarlarkPagableViaPagable;

#[derive(
    Clone,
    Dupe,
    Eq,
    PartialEq,
    Hash,
    Debug,
    Allocative,
    derive_more::Display,
    Pagable,
    StarlarkPagableViaPagable
)]
#[display("{}", self.dynamic_lambda_results_key)]
pub struct DynamicValue {
    pub dynamic_lambda_results_key: DynamicLambdaResultsKey,
}
