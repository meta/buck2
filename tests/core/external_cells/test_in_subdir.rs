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

// A bundled cell is served from memory; whether its directory exists on disk
// makes no difference.
#[buck_test(setup_eden = true)]
async fn test_in_subdir(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["test_bundled_cell//dir:"]).await?;
    buck.cquery(&["root//:"]).await?;

    std::fs::create_dir(buck.cwd().join("somedir"))?;
    buck.targets(&["test_bundled_cell//dir:"]).await?;
    buck.cquery(&["root//:"]).await?;
    Ok(())
}
