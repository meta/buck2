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
async fn test_build_system_info(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:test"]).await?;

    let system_info =
        filter_events(buck, &["Event", "data", "Instant", "data", "SystemInfo"]).await?;
    assert_eq!(system_info.len(), 1);
    assert!(
        system_info[0]["system_total_memory_bytes"]
            .as_u64()
            .is_some_and(|b| b > 0),
        "{}",
        system_info[0]
    );
    assert!(
        system_info[0]["total_disk_space_bytes"]
            .as_u64()
            .is_some_and(|b| b > 0),
        "{}",
        system_info[0]
    );
    Ok(())
}
