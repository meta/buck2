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
async fn test_projected_output_artifact_write(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&["root//:write"]).await?;
    // TODO(nga): this is a bug: we write into projected artifact, but return original artifact,
    //   and yet here we read from original non-projected artifact.
    assert_eq!(
        "ccoonntteenntt",
        std::fs::read_to_string(res.build_report()?.output_for_target("root//:write")?)?
    );
    Ok(())
}

#[buck_test]
async fn test_projected_output_artifact_run(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&["root//:run"]).await?;
    assert_eq!(
        "hello",
        std::fs::read_to_string(
            res.build_report()?
                .output_for_target("root//:run")?
                .join("rel")
        )?
    );
    Ok(())
}
