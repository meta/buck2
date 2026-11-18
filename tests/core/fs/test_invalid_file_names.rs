/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fs;

use buck2_e2e::prelude::*;

/// A file name buck2 does not accept, which is not a separator on the OSes the test runs on.
const BACKSLASH: &str = "\\";

/// This is a regression test for T136963408.
#[buck_test(skip_for_os = ["windows"], setup_eden = true)]
async fn test_invalid_file_invalidation(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":root"]).await?;

    let src = buck.cwd().join("src");
    let invalid = src.join(BACKSLASH);
    let invalid_nested = src.join(BACKSLASH).join("a");
    let invalid_nested_invalid = src.join(BACKSLASH).join(BACKSLASH);

    // Create an invalid file. Build should work.
    fs::File::create(&invalid)?;
    let output = buck.build(&[":root"]).await?;
    assert!(
        output.stderr.contains("is not valid, the entry is ignored"),
        "{}",
        output.stderr
    );

    // Delete it, build should work.
    fs::remove_file(&invalid)?;
    buck.build(&[":root"]).await?;

    // Create an invalid dir. Build should still work.
    fs::create_dir_all(&invalid_nested)?;
    let output = buck.build(&[":root"]).await?;
    assert!(
        output.stderr.contains("is not valid, the entry is ignored"),
        "{}",
        output.stderr
    );

    // And delete it. Things should work.
    fs::remove_dir(&invalid_nested)?;
    fs::remove_dir(&invalid)?;
    buck.build(&[":root"]).await?;

    // Finally, do an invalid file inside an invalid dir...
    fs::create_dir_all(&invalid_nested_invalid)?;
    let output = buck.build(&[":root"]).await?;
    assert!(
        output.stderr.contains("is not valid, the entry is ignored"),
        "{}",
        output.stderr
    );

    // And delete it. Things should again.
    fs::remove_dir(&invalid_nested_invalid)?;
    fs::remove_dir(&invalid)?;
    buck.build(&[":root"]).await?;
    Ok(())
}
