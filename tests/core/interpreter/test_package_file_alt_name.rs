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
async fn test_package_file_alt_name(buck: &Buck) -> anyhow::Result<()> {
    let output = buck.build(&["//:"]).await?;
    assert!(output.stderr.contains("AAA from BUCK_TREE"));
    assert!(!output.stderr.contains("AAA from PACKAGE"));

    std::fs::remove_file(buck.cwd().join("BUCK_TREE"))?;

    let output = buck.build(&["//:"]).await?;
    assert!(!output.stderr.contains("AAA from BUCK_TREE"));
    assert!(output.stderr.contains("AAA from PACKAGE"));
    Ok(())
}
