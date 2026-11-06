/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Write;
use std::path::Path;

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_what_ran_incomplete(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.build(&["//:my_rule"]).await?;

    let log = buck.log(&["show"]).await?.stdout;
    let log_file = tmp_path.join("log.json-lines");

    // Truncate log
    let mut truncated = String::new();
    for line in log.trim().lines() {
        if line.contains("SpanEnd") && line.contains("ActionExecution") {
            break;
        }
        writeln!(truncated, "{line}")?;
    }
    std::fs::write(&log_file, truncated)?;
    let log_file = log_file.to_str().unwrap();

    let target = "build\tprelude//:my_rule (<unspecified>)";

    let what_ran = buck.log(&["what-ran", "--incomplete", log_file]).await?;
    assert!(what_ran.stderr.contains("Showing commands from:"));
    assert!(what_ran.stdout.contains(target));

    let what_failed = buck.log(&["what-failed", log_file]).await?;
    assert!(!what_failed.stdout.contains(target));

    let what_ran = buck.log(&["what-ran", "--show-std-err", log_file]).await?;
    assert!(
        what_ran
            .stdout
            .contains("<command did not finish executing>")
    );
    Ok(())
}
