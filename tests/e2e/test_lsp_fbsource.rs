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

#[buck_test(inplace = true)]
async fn test_lsp_starts_fbsource(buck: &Buck) -> anyhow::Result<()> {
    let mut lsp = buck.lsp(&[]);
    // Will fail if the initialize response is not received
    lsp.init_connection().await?;
    lsp.close().await;
    Ok(())
}
