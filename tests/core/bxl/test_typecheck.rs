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
async fn test_not_bxl(buck: &Buck) {
    let result = buck
        .bxl(&["//:test_typecheck.bxl:main"])
        .await
        .expect_failure();
    assert!(result.stderr.contains("Expected type `int` but got `str`"));
}

#[buck_test]
async fn test_target_set_element_type(buck: &Buck) {
    let result = buck
        .bxl(&["//:target_set_element.bxl:main"])
        .await
        .expect_failure();
    assert!(
        result.stderr.contains(
            "Expected type `bxl.UnconfiguredTargetNode` but got `bxl.ConfiguredTargetNode`"
        )
    );
}

#[buck_test]
async fn test_target_set_query_args_type(buck: &Buck) {
    let result = buck
        .bxl(&["//:target_set_query_args.bxl:main"])
        .await
        .expect_failure();
    assert!(
        result
            .stderr
            .contains("bxl.TargetSet[bxl.UnconfiguredTargetNode]")
    );
    assert!(
        result
            .stderr
            .contains("bxl.TargetSet[bxl.ConfiguredTargetNode]")
    );
}

#[buck_test]
async fn test_target_set_mixed_kinds(buck: &Buck) {
    let result = buck
        .bxl(&["//:target_set_mixed_kinds.bxl:main"])
        .await
        .expect_failure();
    assert!(result.stderr.contains(concat!(
        "Binary operator `+` is not available on the types ",
        "`bxl.TargetSet[bxl.ConfiguredTargetNode]` and `bxl.TargetSet[bxl.UnconfiguredTargetNode]`"
    )));
}
