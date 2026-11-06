/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

async fn check_dice_equality(buck: &Buck) -> anyhow::Result<()> {
    let dice_equal = filter_events(
        buck,
        &[
            "Event",
            "data",
            "Instant",
            "data",
            "DiceEqualityCheck",
            "is_equal",
        ],
    )
    .await?;
    assert_eq!(dice_equal.len(), 1);
    assert_eq!(dice_equal[0], true);
    Ok(())
}

async fn check_config_is_the_same(buck: &Buck) -> anyhow::Result<()> {
    // We only fire this event where there are config invalidations.
    let has_new_configs = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "CellHasNewConfigs"],
    )
    .await?;
    assert_eq!(has_new_configs.len(), 0);
    Ok(())
}

async fn check_config_is_different(buck: &Buck) -> anyhow::Result<()> {
    // We only fire this event where there are config invalidations.
    let has_new_configs = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "CellHasNewConfigs"],
    )
    .await?;
    assert_eq!(has_new_configs.len(), 1);

    assert_eq!(has_new_configs[0]["cell"], "root");
    Ok(())
}

#[buck_test]
async fn test_ignore_state_invalidation_with_re_override_in_arg(buck: &Buck) -> anyhow::Result<()> {
    // Add arg to switch to buck2-user
    buck.build(&[
        "root//:simple",
        "--config",
        "buck2_re_client.override_use_case=buck2-user",
    ])
    .await?;
    // No arg, default is buck2-default
    buck.build(&["root//:simple"]).await?;
    check_dice_equality(buck).await?;
    check_config_is_the_same(buck).await?;
    // Add arg to switch to buck2-user again
    buck.build(&[
        "root//:simple",
        "--config",
        "buck2_re_client.override_use_case=buck2-user",
    ])
    .await?;
    check_dice_equality(buck).await?;
    check_config_is_the_same(buck).await?;
    Ok(())
}

#[buck_test]
async fn test_ignore_state_invalidation_with_re_override_in_config(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Default is buck2-default
    buck.build(&["root//:simple"]).await?;
    // Add config to switch to buck2-user
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2_re_client]\noverride_use_case = buck2-user\n",
    )?;
    buck.build(&["root//:simple"]).await?;
    check_config_is_different(buck).await?;
    // Add config to return to buck2-default
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2_re_client]\noverride_use_case = buck2-default\n",
    )?;
    buck.build(&["root//:simple"]).await?;
    check_config_is_different(buck).await?;
    Ok(())
}

#[buck_test]
async fn test_ignore_state_invalidation_with_re_override_in_external_config(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    // Default is buck2-default
    buck.build(&["root//:simple"]).await?;
    // Add config to switch to buck2-user
    let user = tmp_path.join("user.bcfg");
    std::fs::write(&user, "[buck2_re_client]\noverride_use_case = buck2-user\n")?;
    buck.build(&["root//:simple", "--config-file", &user.to_string_lossy()])
        .await?;
    check_config_is_different(buck).await?;
    // Add config to return to buck2-default
    let default = tmp_path.join("default.bcfg");
    std::fs::write(
        &default,
        "[buck2_re_client]\noverride_use_case = buck2-default\n",
    )?;
    buck.build(&["root//:simple", "--config-file", &default.to_string_lossy()])
        .await?;
    check_config_is_different(buck).await?;
    Ok(())
}

#[buck_test]
async fn test_ignore_state_invalidation_with_re_override_in_external_config_source(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let temp = tmp_path.join("external.bcfg");
    std::fs::write(&temp, "")?;

    // Default is buck2-default
    buck.build(&["root//:simple"])
        .env("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG", &temp)
        .await?;

    // Add config to switch to buck2-user
    std::fs::write(&temp, "[buck2_re_client]\noverride_use_case = buck2-user\n")?;
    buck.build(&["root//:simple"])
        .env("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG", &temp)
        .await?;
    check_config_is_different(buck).await?;

    // Add config to return to buck2-default
    std::fs::write(
        &temp,
        "[buck2_re_client]\noverride_use_case = buck2-default\n",
    )?;
    buck.build(&["root//:simple"])
        .env("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG", &temp)
        .await?;
    check_dice_equality(buck).await?;
    Ok(())
}
