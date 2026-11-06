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
async fn test_output_artifact_twice_same(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["root//:test_output_artifact_twice_same"])
        .await?;
    assert_eq!(
        std::fs::read_to_string(
            res.build_report()?
                .output_for_target("root//:test_output_artifact_twice_same")?
        )?,
        "green lamp"
    );
    Ok(())
}

#[buck_test]
async fn test_output_artifact_twice_with_projection(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["root//:test_output_artifact_twice_with_projection"])
        .await?;
    assert_eq!(
        std::fs::read_to_string(
            res.build_report()?
                .output_for_target("root//:test_output_artifact_twice_with_projection")?
                .join("rel")
        )?,
        "red alert"
    );
    Ok(())
}
