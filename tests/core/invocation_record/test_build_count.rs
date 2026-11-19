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

#[buck_test(setup_eden = true, write_invocation_record = true)]
async fn test_build_count_since_rebase(buck: &Buck) -> anyhow::Result<()> {
    // needed for mergebase to exist
    let status = std::process::Command::new("sl")
        .args(["bookmark", "main"])
        .current_dir(buck.cwd())
        .status()?;
    assert!(status.success(), "`sl bookmark main` failed: {status}");
    let res = buck.build(&["//:test"]).await?;
    let record = res.invocation_record()?;
    eprintln!("{}", record.get("hg_revision").context("No `hg_revision`")?);
    assert_eq!(record["min_attempted_build_count_since_rebase"], 1);
    assert_eq!(record["min_build_count_since_rebase"], 1);

    let res2 = buck
        .build(&["//:test", "-c test.fail=1"])
        .await
        .expect_failure();
    let record = res2.invocation_record()?;
    eprintln!("{}", record.get("hg_revision").context("No `hg_revision`")?);
    assert_eq!(record["min_attempted_build_count_since_rebase"], 2);
    assert_eq!(record["min_build_count_since_rebase"], 1);
    Ok(())
}
