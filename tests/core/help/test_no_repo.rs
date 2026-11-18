/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::prelude::*;

#[buck_test(skip_final_kill = true)]
async fn test_no_repo(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.help(&[]).await?;
    // And make sure this also works with absolute argfiles
    let arg_path = tmp_path.join("argsfile.txt");
    std::fs::write(&arg_path, "--help")?;
    buck.command(&[&format!("@{}", arg_path.display())]).await?;
    Ok(())
}
