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
async fn test_select_refine(buck: &Buck) -> anyhow::Result<()> {
    // Smoke test for select refinement:
    // the most specific option is picked even if it is not listed first.
    let out = buck
        .cquery(&[
            "--target-platforms=//:p-good-domestic",
            "-a=labels",
            "//:the-test",
        ])
        .await?;
    let q: BTreeMap<String, BTreeMap<String, Vec<String>>> = serde_json::from_str(&out.stdout)?;
    assert_eq!(q.len(), 1);
    assert_eq!(q.values().next().unwrap()["labels"], ["good-domestic"]);
    Ok(())
}
