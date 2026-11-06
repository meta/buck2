/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::json_get;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use regex::Regex;
use serde_json::Value;

/// The RE use case of the test's buck2: its `buck2_re_client.override_use_case`, or else the one in
/// the harness's extra external config, or else `buck2-default`.
async fn get_buck2_re_use_case(buck: &Buck) -> anyhow::Result<String> {
    let key = "buck2_re_client.override_use_case";
    let config = buck
        .audit_config(&["--reuse-current-config", "--style=json", key])
        .await?
        .audit_config_json()?;
    if let Some(use_case) = config.get(key).and_then(Value::as_str) {
        return Ok(use_case.to_owned());
    }

    // The test harness's extra external config is part of normal Buck config
    // parsing, but `override_use_case` is filtered out of DICE-backed config
    // reads, so `audit config --reuse-current-config` may not report it. We need
    // to manually check the test config here.
    if let Some(extra_config_path) = buck.env_var("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG")
        && let Some(use_case) = get_buck2_re_use_case_from_config_file(Path::new(extra_config_path))
    {
        return Ok(use_case);
    }

    Ok("buck2-default".to_owned())
}

/// The last `override_use_case` in a `[buck2_re_client]` section of the config file at `path`, as
/// Python's `ConfigParser(strict=False)` reads it.
fn get_buck2_re_use_case_from_config_file(path: &Path) -> Option<String> {
    let config = std::fs::read_to_string(path).ok()?;
    let mut section = "";
    let mut use_case = None;
    for line in config.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header;
        } else if section == "buck2_re_client"
            && let Some((key, value)) = line.split_once(['=', ':'])
            && key.trim().eq_ignore_ascii_case("override_use_case")
        {
            use_case = Some(value.trim().to_owned());
        }
    }
    use_case
}

/// Checks that the first action execution in the last command's event log lists the empty output
/// `a` first; whether there was an action execution.
async fn check_first_action_outputs(buck: &Buck) -> anyhow::Result<bool> {
    let log = buck.log(&["show"]).await?;

    for line in log.stdout.trim().lines() {
        let event: Value = serde_json::from_str(line)?;
        let Some(outputs) = json_get(
            &event,
            &[
                "Event",
                "data",
                "SpanEnd",
                "data",
                "ActionExecution",
                "outputs",
            ],
        ) else {
            continue;
        };
        // da39a3ee is a digest for empty directory.
        // We have 2 directories "a" and "z", where
        // "a" is empty and "z" is not.
        // "z" is a first output for action.
        let digests: Vec<&Value> = outputs
            .as_array()
            .context("Action outputs that are not a list")?
            .iter()
            .map(|o| &o["tiny_digest"])
            .collect();
        assert_eq!(digests.len(), 2);
        // Checking that "a" is first in action outputs
        assert_eq!(digests[0], "da39a3ee");
        return Ok(true);
    }

    Ok(false)
}

#[buck_test]
async fn test_local_action(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:foo",
        "--no-remote-cache",
        "--local-only",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;

    assert!(
        check_first_action_outputs(buck).await?,
        "Didn't find ActionExecution data"
    );
    Ok(())
}

#[buck_test]
async fn test_remote_action(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:foo",
        "--no-remote-cache",
        "--remote-only",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;

    check_first_action_outputs(buck).await?;

    let what_ran = read_what_ran(buck, &[]).await?;
    assert_eq!(what_ran.len(), 1);
    let digest = what_ran[0]["reproducer"]["details"]["digest"]
        .as_str()
        .context("No digest in what ran")?;
    let use_case = get_buck2_re_use_case(buck).await?;
    let recli = std::env::var("RECLI").context("`RECLI` is not set")?;
    let output = tokio::process::Command::new("dotslash")
        .args([
            recli.as_str(),
            "--use-case",
            &use_case,
            "cas",
            "download-action",
            digest,
        ])
        .output()
        .await
        .context("Running `recli`")?;
    anyhow::ensure!(
        output.status.success(),
        "`recli cas download-action` failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let action_definition = String::from_utf8(output.stdout)?;
    // Though RE action has "a" first and then "z"
    assert!(
        Regex::new(
            r#"Output\WDirectories.+\n\["buck-out/.+/__foo__/a",\W"buck-out/.+/__foo__/z""#
        )?
        .is_match(&action_definition),
        "{action_definition}"
    );
    Ok(())
}
