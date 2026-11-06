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
use modifiers_util::get_cfg;

macro_rules! data_dir {
    () => {
        "fbcode//buck2/tests/e2e/configurations/cfg_constructor/test_cli_modifiers_data"
    };
}
const TARGET: &str = concat!(data_dir!(), ":test_target");
const CONSTRAINT_A: &str = concat!(data_dir!(), ":A_1");
const CONSTRAINT_B: &str = concat!(data_dir!(), ":B_1");

#[buck_test(inplace = true)]
async fn test_one_cli_modifier(buck: &Buck) -> anyhow::Result<()> {
    // -m A
    assert!(
        get_cfg(buck, &[TARGET, "--modifier", CONSTRAINT_A])
            .await?
            .contains(CONSTRAINT_A)
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_two_cli_modifier(buck: &Buck) -> anyhow::Result<()> {
    // -m A,B
    let result = get_cfg(
        buck,
        &[
            TARGET,
            "--modifier",
            CONSTRAINT_A,
            "--modifier",
            CONSTRAINT_B,
        ],
    )
    .await?;
    assert!(result.contains(CONSTRAINT_A));
    assert!(result.contains(CONSTRAINT_B));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cli_modifiers_bad_input(buck: &Buck) {
    // -m A B (error)
    buck.cquery(&[
        &format!("deps({TARGET})"),
        "--modifier",
        CONSTRAINT_A,
        CONSTRAINT_B,
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(&format!("got args `{CONSTRAINT_B}`"));
}

#[buck_test(inplace = true)]
async fn test_cli_modifier_alias(buck: &Buck) -> anyhow::Result<()> {
    assert!(
        get_cfg(buck, &[TARGET, "--modifier", "linux"])
            .await?
            .contains("ovr_config//os/constraints:os[linux]")
    );
    Ok(())
}
