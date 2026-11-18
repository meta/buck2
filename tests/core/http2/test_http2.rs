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

#[buck_test]
async fn test_http2_enabled(buck: &Buck) -> anyhow::Result<()> {
    // Get a daemon to start
    buck.build(&[]).await?;
    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    assert_eq!(status["http2"], true, "http2 is enabled by default");

    // Insert necessary buckconfig to pick up http2 configuration.
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"[http]\nhttp2 = false\n")?;

    // Get a daemon to start
    buck.build(&[]).await?;
    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    assert_eq!(status["http2"], false, "http2 was disabled by buckconfig");
    Ok(())
}
