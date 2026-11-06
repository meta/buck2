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
async fn test_peak_allocated_bytes(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["//:EEE"]).await?;
    let span_end_load_event =
        filter_events(buck, &["Event", "data", "SpanEnd", "data", "Load"]).await?;
    assert_eq!(span_end_load_event.len(), 1);
    let starlark_peak_allocated_bytes = span_end_load_event[0]["starlark_peak_allocated_bytes"]
        .as_u64()
        .expect("`starlark_peak_allocated_bytes` is a number");
    // list occupies pointer size (8) * number of elements (~10MB) + some extra overhead for bookkeeping
    assert!(starlark_peak_allocated_bytes >= (8 * 10) << 20);
    // check that it is no more than +10%
    assert!(starlark_peak_allocated_bytes < (8 * 11) << 20);
    Ok(())
}
