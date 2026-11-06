/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;
use regex::Regex;

fn sanitize_memory(s: &str) -> String {
    Regex::new(r"\b\d[\d.]*\s*(?:bytes|[KMGT]i?B)\b")
        .unwrap()
        .replace_all(s, "<USAGE>")
        .into_owned()
}

#[buck_test]
async fn test_peak_allocated_bytes_exceeds_limit(buck: &Buck) {
    let res = buck.uquery(&["//:EEE"]).await.expect_failure();
    golden(
        &sanitize_memory(&sanitize_stderr(&res.stderr)),
        "golden/peak_allocated_bytes_exceeds_limit.golden.stderr",
    );
}
