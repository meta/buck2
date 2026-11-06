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
async fn test_build_universe(buck: &Buck) -> anyhow::Result<()> {
    // Run the build without universe.
    let result = buck.build(&["//:test"]).await?;
    let output = result.build_report()?.output_for_target("//:test")?;
    assert_eq!(std::fs::read_to_string(output)?.trim_end(), "default");

    // Now build the same target, but with the universe.
    let result = buck
        .build(&["//:test", "--target-universe", "//:universe"])
        .await?;
    let output = result.build_report()?.output_for_target("//:test")?;
    assert_eq!(std::fs::read_to_string(output)?.trim_end(), "cat");
    Ok(())
}

#[buck_test]
async fn test_build_target_not_found_in_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["//:test", "--target-universe", "//:different_universe"])
        .await?;

    assert!(
        result
            .stderr
            .contains("No targets found inside the specified universe, nothing will be built")
    );
    Ok(())
}
