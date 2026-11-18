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
async fn test_target_call_stacks_json(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .targets(&["--stack", "--output-attribute=.*", "root//:test"])
        .await?;

    let out: serde_json::Value = serde_json::from_str(&out.stdout)?;
    let call_stack = out[0]["buck.target_call_stack"].as_str().unwrap();
    assert!(call_stack.contains("stub"));
    Ok(())
}
