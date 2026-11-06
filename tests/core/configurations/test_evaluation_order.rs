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

const PASS_TARGETS: &[&str] = &[
    "select_resolution_error_after_compatibility",
    "select_resolution_error_in_dep_after_self_compatibility",
    "select_resolution_error_in_dep_after_dep_compatibility",
    "select_resolution_error_in_exec_dep_after_exec_dep_compatibility_1",
    "select_resolution_error_in_exec_dep_after_exec_dep_compatibility_2",
    "configured_dep_platform_error_after_compatibility",
    "default_target_platform_no_error_in_dep",
    "default_target_platform_no_error_if_global_override",
];

const FAIL_TARGETS: &[&str] = &[
    "select_keys_error_in_dead_branch",
    "select_keys_error_before_compatibility",
    "configured_dep_platform_error_in_dead_branch",
    "configured_dep_platform_error_before_compatibility_when_incoming_edge_transition",
    "configured_dep_platform_error_before_compatibility_when_toolchain",
    "compatibility_attrs_error_in_dead_branch",
    "compatibility_attrs_exec_compatible_errors_before_compatibility",
];

#[buck_test(allow_soft_errors = true)]
async fn test_evaluation_order(buck: &Buck) -> anyhow::Result<()> {
    for t in PASS_TARGETS {
        let target = format!(":{t}");
        let mut args = vec![target.as_str()];
        if *t == "default_target_platform_no_error_if_global_override" {
            args.extend(["--target-platforms", "root//:p-cat"]);
        }
        buck.ctargets(&args).await?;
    }
    for t in FAIL_TARGETS {
        let res = buck
            .ctargets(&[&format!(":{t}"), "-v0", "--console=none"])
            .await
            .expect_failure();
        golden(&res.stderr, &format!("golden/{t}.golden.stderr"));
    }
    Ok(())
}
