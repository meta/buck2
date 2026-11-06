/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_what_ran;

// The test binaries assert on the shape of the command they were given, so a
// passing run means Buck expanded the argsfile to `@<path>` and wrote a file
// holding the case, and a failing one means it did not. Nothing else covers the
// Buck side of this: TPX's own suites stop at the command it hands over.

async fn argsfile_test(buck: &Buck, targets: &[&str], remote: bool) -> anyhow::Result<()> {
    // One invocation for all targets: a `#[buck_test]` builds its own workspace, so splitting them
    // up would pay for that repeatedly to assert parts of the same behaviour. `argsfile` and
    // `argsfile_absolute` differ only in whether the suite takes project-relative paths, which is
    // what decides how the `@<path>` is rendered.
    let local_enabled = format!("test.local_enabled={}", !remote);
    let remote_enabled = format!("test.remote_enabled={remote}");
    let mut args = vec!["-c", &local_enabled, "-c", &remote_enabled];
    args.extend(targets);
    buck.test(&args).await?;

    // Only a run that really happened remotely shows the argsfile reaching the executor as an
    // action input, so a silent fallback to local would make the remote variant meaningless.
    let executors: BTreeMap<String, serde_json::Value> = read_what_ran(buck, &[])
        .await?
        .into_iter()
        .filter(|entry| entry["reason"] == "test.run")
        .map(|entry| {
            (
                entry["identity"].as_str().unwrap_or_default().to_owned(),
                entry["reproducer"]["executor"].clone(),
            )
        })
        .collect();
    let expected = if remote { "Re" } else { "Local" };
    assert_eq!(
        executors,
        targets
            .iter()
            .map(|t| (format!("root{t}"), serde_json::json!(expected)))
            .collect::<BTreeMap<_, _>>()
    );
    Ok(())
}

#[buck_test]
async fn test_runner_argsfile_reaches_the_test_locally(buck: &Buck) -> anyhow::Result<()> {
    argsfile_test(
        buck,
        &["//:argsfile", "//:argsfile_absolute", "//:inline"],
        false,
    )
    .await
}

#[buck_test]
async fn test_runner_argsfile_reaches_the_test_remotely(buck: &Buck) -> anyhow::Result<()> {
    // No `argsfile_absolute`: a test that takes absolute paths can only run locally.
    argsfile_test(buck, &["//:argsfile", "//:inline"], true).await
}
