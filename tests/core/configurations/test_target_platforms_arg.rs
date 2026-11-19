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
use regex::Regex;

#[buck_test]
async fn test_target_platforms_arg(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&[
            // Specifying platform without cell to make sure it is resolved against current cell
            "--target-platforms=//:p-clouds",
            "deps(//:the-test, 1)",
        ])
        .rel_cwd("subcell")
        .await?;
    let stdout = Regex::new(r":p-clouds#[a-f0-9]+\)")?.replace_all(&out.stdout, ":p-clouds#HASH)");
    assert_eq!(
        stdout,
        "\
subcell//:the-test (subcell//:p-clouds#HASH)
subcell//:t-clouds (subcell//:p-clouds#HASH)
"
    );
    Ok(())
}
