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
use std::time::Duration;
use std::time::Instant;

use buck2_e2e::prelude::*;
use serde_json::Value;

async fn start_daemon_and_get_snapshot(buck: &Buck) -> anyhow::Result<Value> {
    buck.targets(&[":"]).await?;
    let mut status: Value = serde_json::from_str(&buck.status(&["--snapshot"]).await?.stdout)?;
    Ok(status["snapshot"].take())
}

#[buck_test]
async fn test_stats_are_reported(buck: &Buck) -> anyhow::Result<()> {
    let snapshot = start_daemon_and_get_snapshot(buck).await?;
    assert_eq!(snapshot["standalone_ttl_refresh"]["passes"], 0);
    Ok(())
}

#[buck_test]
async fn test_a_pass_runs(buck: &Buck) -> anyhow::Result<()> {
    // A local-only build gives no digest an expiration, so the pass has nothing to ask the CAS
    // about; what is under test is that the loop sweeps, scans and reports on schedule.
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"\n[buck2]\nttl_refresh_frequency_seconds = 1\n")?;
    start_daemon_and_get_snapshot(buck).await?;

    let deadline = Instant::now() + Duration::from_secs(60);
    let stats = loop {
        let stats = start_daemon_and_get_snapshot(buck).await?["standalone_ttl_refresh"].take();
        if stats["passes"].as_u64().unwrap() > 0 {
            break stats;
        }
        assert!(
            Instant::now() < deadline,
            "no refresh pass completed, last stats: {stats}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    };

    assert_eq!(stats["errors"], 0);
    assert_eq!(stats["last_pass_candidates"], 0);
    Ok(())
}
