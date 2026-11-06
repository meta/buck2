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
use buck2_e2e::utils::replace_hash;

#[buck_test]
async fn test_cquery_filter_should_not_include_configuration(buck: &Buck) -> anyhow::Result<()> {
    // First, self-check.
    let result = buck.cquery(&["//..."]).await?;
    assert_eq!(
        vec![
            "root//:aaaaa (<unbound>)",
            "root//:bbbbb (root//:aaaaa#<HASH>)",
        ],
        replace_hash(&result.stdout).lines().collect::<Vec<_>>()
    );

    // Now check the behavior of `filter()`.
    // `filter()` function checks unconfigured target label, as Buck1 does.
    let result = buck.cquery(&[r"filter('^root//:bbbbb$', //...)"]).await?;
    assert_eq!(
        vec!["root//:bbbbb (root//:aaaaa#<HASH>)"],
        replace_hash(&result.stdout).lines().collect::<Vec<_>>()
    );
    Ok(())
}
