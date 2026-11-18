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
async fn test_no_package_call_does_not_reset_visibility(buck: &Buck) -> anyhow::Result<()> {
    // Test that PACKAGE file without package() call does not reset visibility inherited from parent PACKAGE file.

    buck.build(&["root//b:top"]).await?;
    Ok(())
}
