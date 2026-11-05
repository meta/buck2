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

#[buck_test(data_dir = "identifier")]
async fn test_analysis_action_ids_unique_identifier_within_category(buck: &Buck) {
    buck.audit(&["providers", "//:yyy"])
        .await
        .expect_failure()
        .assert_stderr_matches("Action category `foo` contains duplicate identifier `x`");
}

#[buck_test(data_dir = "category")]
async fn test_analysis_action_ids_unique_singleton_category(buck: &Buck) {
    buck.audit(&["providers", "//:zzz"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Analysis produced multiple actions with category `foo` and at least one of them had no identifier",
        );
}
