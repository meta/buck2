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
async fn test_configuration_info_api(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//test.bxl:configuration_info"]).await?;
    Ok(())
}

#[buck_test]
async fn test_select_resolves_constraint_setting_default(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&["root//:probe", "--output-attribute", "mode"])
        .await?;
    assert!(
        result.stdout.contains(r#""mode": "dev""#),
        "{}",
        result.stdout
    );
    Ok(())
}

#[buck_test]
async fn test_transition_can_insert_constraint_value(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "deps(root//:with_transitioned_dep)",
            "--output-attribute",
            "mode",
        ])
        .await?;
    assert!(
        result.stdout.contains(r#""mode": "dev""#),
        "{}",
        result.stdout
    );
    assert!(
        result.stdout.contains(r#""mode": "opt""#),
        "{}",
        result.stdout
    );
    Ok(())
}
