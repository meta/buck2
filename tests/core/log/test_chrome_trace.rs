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
async fn test_chrome_trace(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    // Just check it at least runs. More thorough coverage lives in the
    // buck2_log_common unit tests shared with `buck2 debug chrome-trace`.
    buck.build(&["//..."]).await?;
    let trace = tmp_path.join("trace.json");
    buck.log(&["chrome-trace", "--trace-path", trace.to_str().unwrap()])
        .await?;
    assert!(trace.exists());
    Ok(())
}
