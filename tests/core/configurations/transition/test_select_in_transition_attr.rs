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
async fn test_transition_success_if_attr_value_has_not_changed(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:target_where_transition_does_not_change_attr"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_transition_dep_success_if_attr_value_has_not_changed(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&["root//:target_with_transition_dep"]).await?;
    Ok(())
}

#[buck_test]
async fn test_transition_failed_if_attr_value_has_changed(buck: &Buck) {
    let err_msg = concat!(
        r"Target root//:target_where_transition_changes_attr configuration transitioned\n",
        r"\s+old: root//:iphone#.*\n",
        r"\s+new: <transitioned-from-watch>#.*\n",
        r"\s+but attribute: extra\n",
        r#"\s+resolved with old configuration to: "phone"\n"#,
        r#"\s+resolved with new configuration to: "watch""#,
    );

    buck.build(&["root//:target_where_transition_changes_attr"])
        .await
        .expect_failure()
        .assert_stderr_matches(err_msg);
}

#[buck_test]
async fn test_transition_failed_if_attr_value_cycle(buck: &Buck) {
    let err_msg = concat!(
        r#"Dependency cycle detected \(`->` means "depends on"\):\n"#,
        r"\s+root//:target_where_transition_cycles_via_changed_attrs \(<transitioned-from-.*>#.*\) ->.*\n",
        r"\s+root//:target_where_transition_cycles_via_changed_attrs \(<transitioned-from-.*>#.*\) ->.*\n",
    );

    buck.build(&["root//:target_where_transition_cycles_via_changed_attrs"])
        .await
        .expect_failure()
        .assert_stderr_matches(err_msg);
}
