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
use common_io::file_watcher_tests::run_aba_test;

#[buck_test(setup_eden = true)]
async fn test_edenfs_aba_eden(buck: &Buck) -> anyhow::Result<()> {
    run_aba_test(buck).await
}
