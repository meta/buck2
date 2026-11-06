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

// Note: for test scenarios where we want to ensure the `cpe` crate reports no
// vpnless support, we have to define the env var but =0. Otherwise these
// tests will erroneously fail on macOS.

#[buck_test(env = [
    ("CPE_RUST_X2P_SUPPORTS_VPNLESS", "0"),
    ("CPE_RUST_X2P_HTTP1_PROXY_PORT", "5555"),
])]
async fn test_vpnless_disabled_by_host(buck: &Buck) -> anyhow::Result<()> {
    // Get a daemon to start
    buck.build(&[]).await?;
    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    assert_eq!(
        status["supports_vpnless"], false,
        "vpnless should be disabled by non-supporting host"
    );
    Ok(())
}

#[buck_test(env = [
    ("CPE_RUST_X2P_SUPPORTS_VPNLESS", "1"),
    // Need to set this so Windows doesn't go down the unix socket codepath.
    ("CPE_RUST_X2P_HTTP1_PROXY_PORT", "5555"),
])]
async fn test_vpnless_enabled(buck: &Buck) -> anyhow::Result<()> {
    // Get a daemon to start
    buck.build(&[]).await?;
    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    assert_eq!(
        status["supports_vpnless"], true,
        "vpnless should be enabled by host"
    );
    Ok(())
}
