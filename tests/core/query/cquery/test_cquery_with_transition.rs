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
async fn test_attrfilter_preserves_forward_node_attributes(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "attrfilter(labels, transitioned-label, root//:buck)",
            "--target-platforms=root//:p",
        ])
        .await?;
    assert_eq!(
        replace_hash(&result.stdout).lines().collect::<Vec<_>>(),
        vec!["root//:buck (transitioned-to-reindeer#<HASH>)"]
    );

    let result = buck
        .cquery(&[
            "attrregexfilter(actual, 'root//:buck', root//:buck)",
            "--target-platforms=root//:p",
        ])
        .await?;
    assert_eq!(
        replace_hash(&result.stdout).lines().collect::<Vec<_>>(),
        vec!["root//:buck (root//:p#<HASH>)"]
    );
    Ok(())
}

/// Both configurations of `root//:buck`, then, if `with_moose`, of `root//:moose`.
fn assert_transitioned(stdout: &str, with_moose: bool) {
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(if with_moose { 4 } else { 2 }, lines.len());
    assert_eq!(replace_hash(lines[0]), "root//:buck (root//:p#<HASH>)");
    assert_eq!(
        replace_hash(lines[1]),
        "root//:buck (transitioned-to-reindeer#<HASH>)"
    );
    if with_moose {
        assert_eq!(replace_hash(lines[2]), "root//:moose (root//:p#<HASH>)");
        assert_eq!(
            replace_hash(lines[3]),
            "root//:moose (transitioned-to-reindeer#<HASH>)"
        );
    }
}

#[buck_test]
async fn test_cquery_transition_without_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&["root//:buck", "--target-platforms=root//:p"])
        .await?;

    // Both configurations for the target are returned: the default, and the transition
    assert_transitioned(&result.stdout, false);

    // Test cquery with "%s".
    let result = buck
        .cquery(&[
            "%s",
            "root//:buck",
            "root//:moose",
            "--target-platforms=root//:p",
        ])
        .await?;

    assert_transitioned(&result.stdout, true);

    // Test cquery with "%Ss"
    let result = buck
        .cquery(&[
            "%Ss",
            "root//:buck",
            "root//:moose",
            "--target-platforms=root//:p",
        ])
        .await?;

    assert_transitioned(&result.stdout, true);
    Ok(())
}

#[buck_test]
async fn test_cquery_transition_with_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//:buck",
            "--target-platforms=root//:p",
            "--target-universe",
            "root//:buck",
        ])
        .await?;

    assert_transitioned(&result.stdout, false);

    // Test cquery with "%s".
    let result = buck
        .cquery(&[
            "%s",
            "root//:buck",
            "root//:moose",
            "--target-platforms=root//:p",
            "--target-universe",
            "root//:buck,root//:moose",
        ])
        .await?;

    assert_transitioned(&result.stdout, true);

    // Test cquery with "%Ss".
    let result = buck
        .cquery(&[
            "%Ss",
            "root//:buck",
            "root//:moose",
            "--target-platforms=root//:p",
            "--target-universe",
            "root//:buck,root//:moose",
        ])
        .await?;

    assert_transitioned(&result.stdout, true);
    Ok(())
}
