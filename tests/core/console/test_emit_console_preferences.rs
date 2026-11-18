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
async fn test_emit_console_preferences(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["-c", "ui.thread_line_limit=30"]).await?;

    let max_lines = filter_events(
        buck,
        &[
            "Event",
            "data",
            "Instant",
            "data",
            "ConsolePreferences",
            "max_lines",
        ],
    )
    .await?;

    assert!(!max_lines.is_empty());
    assert_eq!(max_lines[0], 30);
    Ok(())
}
