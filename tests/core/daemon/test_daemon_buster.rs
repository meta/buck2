/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;

async fn pid(buck: &Buck) -> anyhow::Result<u64> {
    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    status["process_info"]["pid"]
        .as_u64()
        .context("No daemon pid in the status")
}

#[buck_test]
async fn test_daemon_buster(buck: &Buck) -> anyhow::Result<()> {
    let settings = buck.cwd().join(".bucksettings.toml");

    buck.build(&[":"]).await?;
    let pid0 = pid(buck).await?;

    buck.build(&[":"]).await?;
    let pid1 = pid(buck).await?;
    assert_eq!(pid1, pid0);

    std::fs::write(&settings, "[daemon]\nbuster = \"1\"\n")?;
    let result = buck.build(&[":"]).await?;
    assert!(
        result
            .stderr
            .contains("Buck settings changed (daemon.buster: null -> \"1\"); restarting daemon")
    );
    let pid2 = pid(buck).await?;
    assert_ne!(pid2, pid1);

    buck.build(&[":"]).await?;
    let pid3 = pid(buck).await?;
    assert_eq!(pid3, pid2);

    std::fs::write(&settings, "[daemon]\nbuster = \"2\"\n")?;
    buck.build(&[":"]).await?;
    let pid4 = pid(buck).await?;
    assert_ne!(pid4, pid3);

    std::fs::remove_file(&settings)?;
    buck.build(&[":"]).await?;
    let pid5 = pid(buck).await?;
    assert_ne!(pid5, pid4);
    Ok(())
}
