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

#[buck_test(inplace = true)]
async fn test_set_cfg_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&[
            "fbcode//buck2/tests/e2e/configurations/cfg_constructor/test_clear_package_modifiers_data/set_cfg_modifiers/dir:test",
            "--package-values",
        ])
        .await?;
    let targets: Vec<serde_json::Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    let cfg_modifiers = target
        .get("buck.package_values")
        .and_then(|values| values.get("buck.cfg_modifiers"))
        .context("No `buck.package_values.buck.cfg_modifiers`")?;
    assert_eq!(
        *cfg_modifiers,
        serde_json::json!([
            {
                "_type": "TaggedModifiers",
                "location": {
                    "_type": "ModifierPackageLocation",
                    "package_path": "fbcode//buck2/tests/e2e/configurations/cfg_constructor/test_clear_package_modifiers_data/set_cfg_modifiers/PACKAGE",
                },
                "modifiers": [
                    {
                        "_type": "ModifiersMatch",
                        "ovr_config//os/constraints:linux": "ovr_config//cpu/constraints:arm64",
                        "ovr_config//os/constraints:macos": "ovr_config//cpu/constraints:x86_64",
                    },
                    {
                        "DEFAULT": "ovr_config//os/constraints:linux",
                        "_type": "ModifiersMatch",
                    },
                ],
                "rule_name": null,
            },
            {
                "_type": "TaggedModifiers",
                "location": {
                    "_type": "ModifierPackageLocation",
                    "package_path": "fbcode//buck2/tests/e2e/configurations/cfg_constructor/test_clear_package_modifiers_data/set_cfg_modifiers/PACKAGE",
                },
                "modifiers": [
                    "ovr_config//cpu/constraints:x86_64",
                ],
                "rule_name": "python_binary",
            },
            {
                "_type": "TaggedModifiers",
                "location": {
                    "_type": "ModifierPackageLocation",
                    "package_path": "fbcode//buck2/tests/e2e/configurations/cfg_constructor/test_clear_package_modifiers_data/set_cfg_modifiers/dir/PACKAGE",
                },
                "modifiers": [
                    {
                        "_type": "ModifiersMatch",
                        "ovr_config//os/constraints:windows": "ovr_config//cpu/constraints:x86_64",
                    },
                    "ovr_config//os/constraints:macos",
                ],
                "rule_name": null,
            },
        ])
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_set_cfg_modifiers_from_package_file_only(buck: &Buck) {
    buck.targets(&[
        "fbcode//buck2/tests/e2e/configurations/cfg_constructor/test_clear_package_modifiers_data/set_cfg_modifiers/package_file_check:test",
        "-c",
        "buck_e2e.testing_failure=true",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "set_cfg_modifiers is only allowed to be used from a PACKAGE or BUCK_TREE file, not a bzl file",
    );
}
