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
use modifiers_util::get_cfg;

#[buck_test(inplace = false)]
async fn test_cfg_modifiers_attr(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&["root//:test", "--output-attribute=modifiers"])
        .await?;

    let targets: Vec<serde_json::Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    let target_modifiers = target.get("modifiers").context("No `modifiers`")?;
    assert_eq!(*target_modifiers, serde_json::json!(["root//:A_1"]));
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_cfg_modifiers_attr_ctargets(buck: &Buck) -> anyhow::Result<()> {
    let result = get_cfg(buck, &["root//:test2"]).await?;
    assert!(result.contains(":A_1"));
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_metadata_modifiers_is_hard_error(buck: &Buck) {
    let result = buck
        .ctargets(&["root//:test_metadata_modifiers"])
        .await
        .expect_failure();
    assert!(
        result
            .stderr
            .contains(r#"sets `metadata["buck.cfg_modifiers"]` which is no longer supported"#)
    );
}
