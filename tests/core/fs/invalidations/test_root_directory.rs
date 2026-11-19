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
use buck2_e2e::utils::json_get;

#[buck_test]
async fn test_no_dice_invalidation_on_root_directory_changes(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//dir:"]).await?;

    // Add a file to the root directory
    std::fs::write(buck.cwd().join("file.txt"), "hello world")?;

    buck.build(&["root//dir:"]).await?;

    let log = buck.log(&["show"]).await?.stdout;

    for line in log.lines() {
        let event: serde_json::Value = serde_json::from_str(line)?;
        let e = json_get(&event, &["Event", "data", "SpanEnd", "data", "Load"]);
        assert!(e.is_none(), "Should not have loaded anything");
    }
    Ok(())
}
