/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_package_values_missing_buck_file(buck: &Buck) -> anyhow::Result<()> {
    let stdout = buck.audit(&["package-values", "//"]).await?.stdout;
    golden(
        &stdout,
        "audit-package-values-missing-buck-file.golden.json",
    );
    Ok(())
}
