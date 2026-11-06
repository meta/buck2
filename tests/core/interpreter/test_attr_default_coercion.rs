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
async fn test_default_not_a_label(buck: &Buck) {
    let res = buck
        .uquery(&["root//:", "--console=none", "-v0"])
        .await
        .expect_failure();
    golden(&res.stderr, "golden/default_not_a_label.golden.stderr");
}
