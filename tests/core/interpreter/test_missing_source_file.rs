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
use buck2_e2e::utils::is_open_source;

/// Meta's buck2 tolerates a missing source file when it loads the package, for misconfigured
/// sparse checkouts; open source buck2 rejects it.
#[buck_test]
async fn test_missing_source_file_at_load_time(buck: &Buck) -> anyhow::Result<()> {
    let query = buck.uquery(&["//package1:"]);
    if is_open_source() {
        query.await.expect_failure().assert_stderr_matches(
            "Source file `non_existent_source_file.txt` does not exist as a member of package `prelude//package1`",
        );
    } else {
        query.await?;
    }
    Ok(())
}
