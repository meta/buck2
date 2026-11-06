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
use buck2_e2e::utils::random_string;

#[buck_test]
async fn test_build_id_env_var_is_set_locally(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "root//:top",
            "--local-only",
            "--no-remote-cache",
            "-c",
            &format!("test.cache_buster={}", random_string()),
        ])
        .await?;

    let output = result.build_report()?.output_for_target("root//:top")?;
    assert!(output.exists());
    assert_eq!(
        std::fs::read_to_string(&output)?.trim(),
        result.buck_build_id()?
    );
    Ok(())
}

#[buck_test]
async fn test_build_id_env_var_is_set_remotely(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "root//:top",
            "--remote-only",
            "--no-remote-cache",
            "-c",
            &format!("test.cache_buster={}", random_string()),
        ])
        .await?;

    let output = result.build_report()?.output_for_target("root//:top")?;
    assert!(output.exists());
    assert_eq!(
        std::fs::read_to_string(&output)?.trim(),
        result.buck_build_id()?
    );
    Ok(())
}
