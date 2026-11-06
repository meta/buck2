/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_root(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:root.bxl:root_test"]).await?;

    assert!(result.stdout.contains(&*buck.cwd().to_string_lossy()));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_cell_root(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["fbcode//cell_root.bxl:cell_root_test"]).await?;

    assert!(
        result
            .stdout
            .contains(&*buck.cwd().join("fbcode").to_string_lossy())
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_instant_event(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/event.bxl:good"]).await?;

    // Get event log
    let log = buck.log(&["show"]).await?.stdout;
    // try to find starlark instant event
    let line = log
        .trim()
        .lines()
        .find(|line| line.contains("StarlarkUser"))
        .context("Failed to find starlark instant event.")?;
    assert!(line.contains("foo"));
    assert!(line.contains("bool_value"));
    assert!(line.contains("string_value"));
    assert!(line.contains("int_value"));

    // Shouldn't fail
    buck.bxl(&["//bxl/event.bxl:metadata_with_duration"])
        .await?;

    buck.bxl(&["//bxl/event.bxl:bad_metadata"])
        .await
        .expect_failure()
        .assert_stderr_matches("Metadata should be a dict where keys are strings, and values are strings, ints, bools, or dicts/lists of the mentioned types. Got type: `list`");

    buck.bxl(&["//bxl/event.bxl:bad_metadata_key"])
        .await
        .expect_failure()
        .assert_stderr_matches("Metadata keys should be strings. Got type: `int`");

    buck.bxl(&["//bxl/event.bxl:bad_metadata_value"])
        .await
        .expect_failure()
        .assert_stderr_matches("Metadata values should be strings, ints, bools, or dicts/lists of the mentioned types. Key `key` had value type `tuple`");

    let result = buck.bxl(&["//bxl/event.bxl:ensured_artifact"]).await?;

    let artifact_path = result.stdout.trim();

    // Get event log
    let log = buck.log(&["show-user"]).await?.stdout;
    let line = log
        .trim()
        .lines()
        .find(|line| line.contains("StarlarkUserEvent"))
        .context("Failed to find starlark instant event.")?;
    let event: serde_json::Value = serde_json::from_str(line)?;
    let metadata = event
        .get("StarlarkUserEvent")
        .and_then(|event| event.get("metadata"))
        .context("No `StarlarkUserEvent.metadata`")?;
    assert_eq!(metadata["rel_path"], artifact_path);
    // As `pathlib` joins it, with the platform's separators.
    let abs_path = artifact_path
        .split('/')
        .fold(buck.cwd().to_path_buf(), |path, part| path.join(part));
    assert_eq!(
        metadata["abs_path"],
        abs_path.to_string_lossy().into_owned()
    );
    assert_eq!(metadata["nested"]["nested_artifact"], artifact_path);
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_read_config(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "-c",
            "key.section=foo",
            "//bxl/read_config.bxl:read_config_test",
        ])
        .await?;

    assert!(result.stdout.contains("foo"));
    assert!(result.stdout.contains("True"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_load_file(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:load_file.bxl:load_test"]).await?;

    assert!(result.stdout.contains(&*buck.cwd().to_string_lossy()));
    Ok(())
}
