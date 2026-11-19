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
async fn test_replay(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:EEE"]).await?;
    let replay = buck.log(&["replay", "-v2"]).await?;
    assert!(replay.stderr.contains("//:EEE"));
    Ok(())
}

#[buck_test]
async fn test_partial_result_replay(buck: &Buck) -> anyhow::Result<()> {
    // `audit cell` is an easy way to produce partial results
    let res = buck.audit(&["cell"]).await?;
    let res2 = buck.log(&["replay"]).await?;

    assert_ne!(res.stdout, res2.stdout);
    assert_eq!(res2.stdout, "");
    Ok(())
}
