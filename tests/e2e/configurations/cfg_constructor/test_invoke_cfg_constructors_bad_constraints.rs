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

#[buck_test(inplace = false)]
async fn test_invoke_cfg_constructors_bad_constraints(buck: &Buck) {
    let result = buck.cquery(&["root//:test"]).await.expect_failure();
    assert!(
        result
            .stderr
            .contains("root//:not_a_constraint is not a configuration rule.")
    );
}
