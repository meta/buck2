/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_core::package::PackageLabel;
use buck2_node::super_package::SuperPackage;
use dice::DiceComputations;

use crate::interpreter::dice_calculation_delegate::HasCalculationDelegate;

#[async_trait]
pub trait EvalPackageFile {
    async fn eval_package_file(&mut self, path: PackageLabel) -> buck2_error::Result<SuperPackage>;
}

#[async_trait]
impl EvalPackageFile for DiceComputations<'_> {
    async fn eval_package_file(&mut self, path: PackageLabel) -> buck2_error::Result<SuperPackage> {
        self.get_interpreter_calculator_for_package(path)
            .await?
            .eval_package_file(path)
            .await
    }
}
