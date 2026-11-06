/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! With `buck2.defer_write_actions` on, a write action's content stays off its output path until
//! something needs the file: the deferred core keeps the write as a declaration, the disk-state core
//! keeps the content in the BlobStore's memory, compressed. Under either, the write counts as
//! executed, a build that materializes nothing leaves nothing on disk, and the file is written when
//! a build asks for it or an action needs it, on disk here or in the CAS for a remote run.

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::get_last_execution_kind;
use buck2_e2e::utils::random_string;
use serde_json::Value;

const ACTION_EXECUTION_KIND_SIMPLE: i64 = 4;

/// A daemon counter at the end of the command, from its last snapshot.
fn counter(res: &BuckOutput, name: &str) -> anyhow::Result<u64> {
    let record = res.invocation_record()?;
    let snapshot = record
        .get("last_snapshot")
        .and_then(Value::as_object)
        .context("No `last_snapshot` in the invocation record")?;
    match snapshot.get(name) {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .with_context(|| format!("`{name}` is not a count: {value}")),
    }
}

/// `args` followed by `configs`.
fn args<'a>(args: &[&'a str], configs: &[&'a str]) -> Vec<&'a str> {
    [args, configs].concat()
}

#[buck_test(write_invocation_record = true)]
async fn test_a_deferred_write_is_written_when_something_needs_it(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    // The write ran, and nothing is on disk...
    let res = buck
        .build(&args(&["//:write", "--materializations=none"], &configs))
        .await?;
    let output = res.build_report()?.output_for_target("//:write")?;
    assert_eq!(
        get_last_execution_kind(buck, Some("write"), &[], None).await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    assert!(!output.exists());

    // ...until something needs the file.
    buck.build(&args(&["//:write"], &configs)).await?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["deferred"])]
async fn test_the_disk_state_core_keeps_the_content_in_memory(buck: &Buck) -> anyhow::Result<()> {
    // Repetitive, so that compressing it shows.
    let content = random_string()[..16].repeat(64);
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    let res = buck
        .build(&args(&["//:write", "--materializations=none"], &configs))
        .await?;
    let output = res.build_report()?.output_for_target("//:write")?;
    assert_eq!(counter(&res, "blob_store_memory_entries")?, 1);
    let memory_bytes = counter(&res, "blob_store_memory_bytes")?;
    assert!(
        0 < memory_bytes && memory_bytes < content.len() as u64,
        "{memory_bytes}"
    );
    assert_eq!(
        counter(&res, "blob_store_entries")?,
        0,
        "nothing entered the stash"
    );
    assert!(!output.exists());

    let res = buck.build(&args(&["//:write"], &configs)).await?;
    assert_eq!(counter(&res, "blob_store_memory_served_files")?, 1);
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_a_deferred_write_is_uploaded_for_a_remote_action(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    let res = buck
        .build(&args(
            &["//:consume", "--remote-only", "--no-remote-cache"],
            &configs,
        ))
        .await?;
    let output = res.build_report()?.output_for_target("//:consume")?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_a_deferred_write_is_on_disk_for_a_local_action(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    let res = buck
        .build(&args(&["//:consume", "--local-only"], &configs))
        .await?;
    let output = res.build_report()?.output_for_target("//:consume")?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}
