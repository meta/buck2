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

#[buck_test]
async fn test_root_cell_with_ignored_buckconfig(buck: &Buck) -> anyhow::Result<()> {
    let r = buck.root(&["--kind=cell"]).rel_cwd("abc").await?;
    assert_eq!(Path::new(r.stdout.trim()), buck.cwd());
    Ok(())
}
