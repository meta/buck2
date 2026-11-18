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
use serde_json::json;

#[buck_test(write_invocation_record = true)]
async fn test_representative_config_flags_disregards_run_args(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .run(&[
            "//:my_rule",
            "--config",
            "foo.bar=baz",
            "--",
            "--config",
            "should.not=include",
        ])
        .await?;

    assert_eq!(
        res.invocation_record()?["representative_config_flags"],
        json!(["-c foo.bar=baz"])
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_representative_config_flags_includes_build_args(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&[
            "--config",
            "foo.bar=baz",
            // For `build` commands, anything after `--` is a positional arg.
            "--",
            "//:my_rule",
        ])
        .await?;

    assert_eq!(
        res.invocation_record()?["representative_config_flags"],
        json!(["-c foo.bar=baz"])
    );
    Ok(())
}
