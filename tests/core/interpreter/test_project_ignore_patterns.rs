/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;

use buck2_e2e::prelude::*;

/// A `project.ignore` entry that is not a glob is matched literally even when it contains a glob
/// metacharacter such as `}`; it must not break the cell's file operations.
#[buck_test]
async fn test_plain_ignore_name_with_a_closing_brace(buck: &Buck) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"[project]\nignore = a}\n")?;
    let result = buck.targets(&["//:"]).await?;
    assert_eq!(result.stdout.trim(), "root//:t");
    Ok(())
}
