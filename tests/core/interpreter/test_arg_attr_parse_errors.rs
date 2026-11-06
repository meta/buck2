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

/// A malformed `attrs.arg()` macro is a coercion error that quotes the input, also when a
/// multibyte character sits where the quoted window is cut.
#[buck_test]
async fn test_malformed_macro_after_multibyte_char_is_an_error(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["//:"])
        .await
        .expect_failure()
        .assert_stderr_matches("Unfinished macro type.*\n.*V:\u{e9}aaaaaaaaaaaa");
    // The daemon is still there: the next command does not have to start a new one.
    let result = buck.targets(&["//ok:"]).await?;
    assert!(
        !result.stderr.contains("Starting new buck2 daemon"),
        "{}",
        result.stderr
    );
    Ok(())
}
