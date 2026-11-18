/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_default_target_platform_is_subtarget(buck: &Buck) {
    // FIXME(JakobDegen): Bug. The target specifies a subtarget that does have an appropriate
    // provider.
    buck.cquery(&[":stub"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Expected `root//:alias_platform` to be a `platform\(\)` target");
}

#[buck_test]
async fn test_subtarget_in_select_key(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .uquery(&[
            "root//:with_constraint_key_dep",
            "-a",
            "buck.configuration_deps",
        ])
        .await?;
    let res: BTreeMap<String, BTreeMap<String, Vec<String>>> = serde_json::from_str(&res.stdout)?;
    // FIXME(JakobDegen): Bug. `buck.deps`-like attributes do not include subtargets
    assert_eq!(
        res.values().next().unwrap()["buck.configuration_deps"],
        ["root//:cat_alias[sub]"]
    );
    Ok(())
}
