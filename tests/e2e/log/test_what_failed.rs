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

#[buck_test(inplace = true)]
async fn test_what_failed(buck: &Buck) -> anyhow::Result<()> {
    let pkg = "fbcode//buck2/tests/targets/rules/genrule/bad";
    let bad = "my_genrule_bad_with_dep";
    let good = "stub";

    buck.build(&[&format!("{pkg}:{bad}")])
        .await
        .expect_failure();
    let out = buck.log(&["what-failed"]).await?;

    // Only the failed command should be in what-failed.
    assert!(out.stdout.contains(&format!("{pkg}:{bad}")));
    assert!(!out.stdout.contains(&format!("{pkg}:{good}")));

    // Even though both commands are here.
    let out = buck.log(&["what-ran"]).await?;
    assert!(out.stdout.contains(&format!("{pkg}:{bad}")));
    assert!(out.stdout.contains(&format!("{pkg}:{good}")));
    Ok(())
}
