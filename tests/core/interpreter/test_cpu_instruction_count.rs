/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

#[buck_test]
async fn test_cpu_instruction_count(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["//:"]).await?;
    let span_end_load_event =
        filter_events(buck, &["Event", "data", "SpanEnd", "data", "Load"]).await?;
    assert_eq!(span_end_load_event.len(), 1);

    let cpu_instruction_count = span_end_load_event[0]
        .get("cpu_instruction_count")
        .context("No `cpu_instruction_count` in the `Load` span end")?;

    // We only populate counters on Linux
    if cfg!(target_os = "linux") {
        if cpu_instruction_count.is_null() {
            eprintln!(
                "cpu_instruction_count is None, but we expect it to be populated on Linux most of the time. This is not a failure."
            );
        } else {
            assert!(cpu_instruction_count.as_u64().unwrap() >= 1000);
        }
    } else {
        assert!(cpu_instruction_count.is_null());
    }
    Ok(())
}
