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
use buck2_e2e::utils::filter_events;

#[buck_test]
async fn test_dice_is_not_invalidated_on_changes_in_ignored_directories(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.targets(&["root//..."]).await?;
    std::fs::write(buck.cwd().join("dir").join("fignore"), "xyz")?;
    buck.targets(&["root//..."]).await?;
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
    assert_eq!(dice_equal, [serde_json::Value::Bool(true)]);
    Ok(())
}
