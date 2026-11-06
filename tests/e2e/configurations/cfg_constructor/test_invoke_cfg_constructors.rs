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

// The fixture writes modifiers as a `{setting: value}` dict, which is the
// `cfg_modifier_unresolvable` soft error.
#[buck_test(inplace = false, allow_soft_errors = true)]
async fn test_invoke_cfg_constructors(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["root//:test"]).await?;
    assert!(
        result
            .stdout
            .contains("root//:test (post_constraint_analysis_test_label")
    );
    Ok(())
}

// The fixture writes modifiers as a `{setting: value}` dict, which is the
// `cfg_modifier_unresolvable` soft error.
#[buck_test(inplace = false, allow_soft_errors = true)]
async fn test_invoke_cfg_constructors_without_aliases(buck: &Buck) -> anyhow::Result<()> {
    // This test ensures that for backwards compatibility, we can call
    // `set_cfg_constructor` without explicitly passing in aliases parameter.
    let result = buck
        .cquery(&["root//:test", "-c", "testing.no_aliases=true"])
        .await?;
    assert!(
        result
            .stdout
            .contains("root//:test (post_constraint_analysis_test_label")
    );
    Ok(())
}

// The fixture writes modifiers as a `{setting: value}` dict, which is the
// `cfg_modifier_unresolvable` soft error.
#[buck_test(inplace = false, allow_soft_errors = true)]
async fn test_invoke_cfg_constructors_unbound_platform(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["root//:test_unbound"]).await?;
    assert!(
        result
            .stdout
            .contains("root//:test_unbound (post_constraint_analysis_test_label")
    );
    Ok(())
}
