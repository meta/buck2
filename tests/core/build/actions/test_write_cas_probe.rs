/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! With `buck2.write_cas_probe` on, a write action asks the CAS for its content before writing it.
//! When the CAS has the content with enough TTL left, the output is declared as a CAS download and
//! nothing is written until something needs the file; otherwise the write is made here while the
//! machine has room for local work and uploaded when it has not. Under the disk-state materializer
//! a copy of a source file, alone or into a `copied_dir`, asks the same way before copying, whatever
//! the knob says, and a miss is made or uploaded the same way; the deferred materializer copies
//! without asking. None of them asks under `--local-only` or `--no-remote-cache`.

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::get_last_execution_kind;
use buck2_e2e::utils::random_string;
use serde_json::Value;

const ACTION_EXECUTION_KIND_SIMPLE: i64 = 4;
const ACTION_EXECUTION_KIND_DEFERRED: i64 = 6;

/// A daemon counter at the end of the command, from its last snapshot.
fn snapshot_counter(res: &BuckOutput, name: &str) -> anyhow::Result<u64> {
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

/// How many presence-check RPCs the daemon had issued by the end of the command.
fn presence_check_rpcs(res: &BuckOutput) -> anyhow::Result<u64> {
    snapshot_counter(res, "re_presence_check_rpcs")
}

/// How many digests the presence check had answered by the end of the command, whether by
/// asking the CAS or from an expiration the digest already carried.
fn presence_check_answers(res: &BuckOutput) -> anyhow::Result<u64> {
    Ok(snapshot_counter(res, "re_presence_check_digests_queried")?
        + snapshot_counter(res, "re_presence_check_instance_answers")?)
}

/// How many bytes the daemon had uploaded to the CAS by the end of the command.
fn upload_bytes(res: &BuckOutput) -> anyhow::Result<u64> {
    snapshot_counter(res, "re_upload_bytes")
}

async fn last_execution_kind(buck: &Buck, category: &str) -> anyhow::Result<Option<i64>> {
    get_last_execution_kind(buck, Some(category), &[], None).await
}

/// `args` followed by `configs`.
fn args<'a>(args: &[&'a str], configs: &[&'a str]) -> Vec<&'a str> {
    [args, configs].concat()
}

fn len(content: &str) -> u64 {
    content.len() as u64
}

async fn written_without_a_probe(buck: &Buck, flags: &[&str]) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let res = buck
        .build(&args(&args(&["//:write"], flags), &["-c", &content_config]))
        .await?;
    let output = res.build_report()?.output_for_target("//:write")?;
    assert_eq!(presence_check_rpcs(&res)?, 0, "the CAS was not asked");
    assert_eq!(
        last_execution_kind(buck, "write").await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_write_whose_content_the_cas_lacks_is_written(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    let res = buck.build(&args(&["//:write"], &configs)).await?;
    let output = res.build_report()?.output_for_target("//:write")?;
    assert!(presence_check_rpcs(&res)? >= 1, "the probe asked the CAS");
    assert_eq!(
        last_execution_kind(buck, "write").await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_write_whose_content_the_cas_has_is_not_written(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    // Running an RE action on the write's output uploads the content to the CAS.
    buck.build(&args(
        &["//:consume", "--remote-only", "--no-remote-cache"],
        &configs,
    ))
    .await?;

    // A second write of the same content finds it there: the output is declared as a CAS
    // download, and neither memory nor disk holds the bytes...
    let target = "//:write_again";
    let res = buck
        .build(&args(&[target, "--materializations=none"], &configs))
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    // The upload that seeded the CAS may already have recorded the blob's expiration on the
    // digest, in which case the probe answers from it without an RPC.
    assert!(presence_check_answers(&res)? >= 1, "the probe ran");
    assert_eq!(
        last_execution_kind(buck, "write").await?,
        Some(ACTION_EXECUTION_KIND_DEFERRED)
    );
    assert!(!output.exists());

    // ...until something needs the file, which then comes out of the CAS.
    buck.build(&args(&[target], &configs)).await?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_write_whose_content_the_cas_lacks_is_uploaded_when_there_is_no_room(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];
    // The gate the hybrid executor runs local commands behind, with no room at all.
    let env = ("BUCK2_TEST_LOW_PASS_FILTER_CAPACITY", "0");

    // The miss is uploaded and declared as CAS content; neither memory nor disk holds the
    // bytes...
    let res = buck
        .build(&args(&["//:write", "--materializations=none"], &configs))
        .env(env.0, env.1)
        .await?;
    let output = res.build_report()?.output_for_target("//:write")?;
    assert!(presence_check_rpcs(&res)? >= 1, "the probe asked the CAS");
    assert!(
        upload_bytes(&res)? >= len(&content),
        "the content was uploaded"
    );
    assert_eq!(
        last_execution_kind(buck, "write").await?,
        Some(ACTION_EXECUTION_KIND_DEFERRED)
    );
    assert!(!output.exists());

    // ...until something needs the file, which then comes out of the CAS.
    buck.build(&args(&["//:write"], &configs))
        .env(env.0, env.1)
        .await?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["deferred"])]
async fn test_copy_of_a_source_file_the_cas_has_is_not_made(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    std::fs::write(buck.cwd().join("src.txt"), &content)?;
    // The write targets in the same file need their content configured.
    let content_config = format!("test.content={}", random_string());
    let configs = ["-c", content_config.as_str()];

    let res = buck
        .build(&args(&["//:copy_src", "--materializations=none"], &configs))
        .await?;
    assert!(presence_check_rpcs(&res)? >= 1, "the probe asked the CAS");
    assert_eq!(
        last_execution_kind(buck, "copy").await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );

    // An RE action reading the source file uploads its content to the CAS.
    buck.build(&args(
        &["//:consume_src", "--remote-only", "--no-remote-cache"],
        &configs,
    ))
    .await?;

    // A copy of it now finds the content there: the output is declared as a CAS download and
    // nothing is copied...
    let target = "//:copy_src_again";
    let res = buck
        .build(&args(&[target, "--materializations=none"], &configs))
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert!(presence_check_answers(&res)? >= 1, "the probe ran");
    assert_eq!(
        last_execution_kind(buck, "copy").await?,
        Some(ACTION_EXECUTION_KIND_DEFERRED)
    );
    assert!(!output.exists());

    // ...until something needs the file, which then comes out of the CAS.
    buck.build(&args(&[target], &configs)).await?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["deferred"])]
async fn test_copy_of_a_source_file_the_cas_lacks_is_uploaded_when_there_is_no_room(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = random_string();
    std::fs::write(buck.cwd().join("src.txt"), &content)?;
    let content_config = format!("test.content={}", random_string());
    let configs = ["-c", content_config.as_str()];
    let env = ("BUCK2_TEST_LOW_PASS_FILTER_CAPACITY", "0");

    // The miss is uploaded, as a write's would be, and the copy is declared as CAS content:
    // nothing is copied...
    let target = "//:copy_src";
    let res = buck
        .build(&args(&[target, "--materializations=none"], &configs))
        .env(env.0, env.1)
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert!(presence_check_rpcs(&res)? >= 1, "the probe asked the CAS");
    assert!(
        upload_bytes(&res)? >= len(&content),
        "the source was uploaded"
    );
    assert_eq!(
        last_execution_kind(buck, "copy").await?,
        Some(ACTION_EXECUTION_KIND_DEFERRED)
    );
    assert!(!output.exists());

    // ...until something needs the file, which then comes out of the CAS.
    buck.build(&args(&[target], &configs))
        .env(env.0, env.1)
        .await?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["deferred"])]
async fn test_copied_dir_of_a_source_file_the_cas_has_is_not_made(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = random_string();
    std::fs::write(buck.cwd().join("src.txt"), &content)?;
    let content_config = format!("test.content={}", random_string());
    let configs = ["-c", content_config.as_str()];
    // `copied_dir` reports the `symlinked_dir` category.
    let category = "symlinked_dir";

    let res = buck
        .build(&args(
            &["//:copied_dir_src", "--materializations=none"],
            &configs,
        ))
        .await?;
    assert!(presence_check_rpcs(&res)? >= 1, "the probe asked the CAS");
    assert_eq!(
        last_execution_kind(buck, category).await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );

    buck.build(&args(
        &["//:consume_src", "--remote-only", "--no-remote-cache"],
        &configs,
    ))
    .await?;

    // The directory's one file is in the CAS now: nothing is copied...
    let target = "//:copied_dir_src_again";
    let res = buck
        .build(&args(&[target, "--materializations=none"], &configs))
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert!(presence_check_answers(&res)? >= 1, "the probe ran");
    assert_eq!(
        last_execution_kind(buck, category).await?,
        Some(ACTION_EXECUTION_KIND_DEFERRED)
    );
    assert!(!output.exists());

    // ...until something needs it, which then comes out of the CAS.
    buck.build(&args(&[target], &configs)).await?;
    assert_eq!(std::fs::read_to_string(output.join("src.txt"))?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["deferred"])]
async fn test_copied_dir_stashes_only_the_files_the_cas_lacks(buck: &Buck) -> anyhow::Result<()> {
    // The probe asks about a copied directory's files one at a time: the directory itself is
    // in the CAS only if some action uploaded it in this exact shape, while its files are
    // inputs the CAS usually has. Here one file is uploaded and one is not, so only the second
    // is made here, under its own path, and enters the stash.
    let content_a = random_string().repeat(32);
    let content_b = random_string().repeat(8);
    std::fs::write(buck.cwd().join("mixed_a.txt"), &content_a)?;
    std::fs::write(buck.cwd().join("mixed_b.txt"), &content_b)?;
    let content_config = format!("test.content={}", random_string());
    let configs = ["-c", content_config.as_str()];

    buck.build(&args(
        &["//:consume_mixed_b", "--remote-only", "--no-remote-cache"],
        &configs,
    ))
    .await?;

    let target = "//:copied_dir_mixed";
    let res = buck
        .build(&args(&[target, "--materializations=none"], &configs))
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert!(
        presence_check_answers(&res)? >= 2,
        "the probe asked about both files"
    );
    assert_eq!(
        last_execution_kind(buck, "symlinked_dir").await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    assert_eq!(
        snapshot_counter(&res, "blob_store_entries")?,
        1,
        "one file entered the stash"
    );
    assert_eq!(snapshot_counter(&res, "blob_store_bytes")?, len(&content_a));
    assert!(
        !output.join("mixed_b.txt").exists(),
        "the file the CAS has is not copied"
    );

    buck.build(&args(&[target], &configs)).await?;
    assert_eq!(
        std::fs::read_to_string(output.join("mixed_a.txt"))?,
        content_a
    );
    assert_eq!(
        std::fs::read_to_string(output.join("mixed_b.txt"))?,
        content_b
    );
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["deferred"])]
async fn test_copied_dir_uploads_the_files_the_cas_lacks_when_there_is_no_room(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content_a = random_string().repeat(32);
    let content_b = random_string().repeat(8);
    std::fs::write(buck.cwd().join("mixed_a.txt"), &content_a)?;
    std::fs::write(buck.cwd().join("mixed_b.txt"), &content_b)?;
    let content_config = format!("test.content={}", random_string());
    let configs = ["-c", content_config.as_str()];
    let env = ("BUCK2_TEST_LOW_PASS_FILTER_CAPACITY", "0");

    buck.build(&args(
        &["//:consume_mixed_b", "--remote-only", "--no-remote-cache"],
        &configs,
    ))
    .env(env.0, env.1)
    .await?;

    // With no room for local work the file the CAS lacks is uploaded instead, and nothing
    // enters the stash or lands on disk.
    let target = "//:copied_dir_mixed";
    let res = buck
        .build(&args(&[target, "--materializations=none"], &configs))
        .env(env.0, env.1)
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert!(
        upload_bytes(&res)? >= len(&content_a),
        "the file the CAS lacked was uploaded"
    );
    assert_eq!(
        last_execution_kind(buck, "symlinked_dir").await?,
        Some(ACTION_EXECUTION_KIND_DEFERRED)
    );
    assert_eq!(
        snapshot_counter(&res, "blob_store_entries")?,
        0,
        "nothing entered the stash"
    );
    assert!(!output.exists());

    buck.build(&args(&[target], &configs))
        .env(env.0, env.1)
        .await?;
    assert_eq!(
        std::fs::read_to_string(output.join("mixed_a.txt"))?,
        content_a
    );
    assert_eq!(
        std::fs::read_to_string(output.join("mixed_b.txt"))?,
        content_b
    );
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["deferred"])]
async fn test_copies_probe_whatever_the_write_knob_says(buck: &Buck) -> anyhow::Result<()> {
    // The knob is the write's: with it off, a write is made without asking...
    let knob_off = ["-c", "buck2.write_cas_probe=false"];
    written_without_a_probe(buck, &knob_off).await?;

    // ...while a copy of a source file still asks, since what a hit spares under the
    // disk-state core is a copy made now.
    std::fs::write(buck.cwd().join("src.txt"), random_string())?;
    let content_config = format!("test.content={}", random_string());
    let configs = args(&knob_off, &["-c", content_config.as_str()]);
    let res = buck.build(&args(&["//:copy_src"], &configs)).await?;
    let rpcs = presence_check_rpcs(&res)?;
    assert!(rpcs >= 1, "the probe asked the CAS");
    assert_eq!(
        last_execution_kind(buck, "copy").await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );

    // A `copied_dir` asks about its files the same way.
    std::fs::write(buck.cwd().join("mixed_a.txt"), random_string())?;
    std::fs::write(buck.cwd().join("mixed_b.txt"), random_string())?;
    let res = buck
        .build(&args(&["//:copied_dir_mixed"], &configs))
        .await?;
    assert!(presence_check_rpcs(&res)? > rpcs, "the probe asked the CAS");
    assert_eq!(
        last_execution_kind(buck, "symlinked_dir").await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["v2"])]
async fn test_the_deferred_materializer_copies_without_a_probe(buck: &Buck) -> anyhow::Result<()> {
    // A copy under the deferred materializer is made when something needs the file, so there
    // is nothing for a hit to spare: neither a copy nor a `copied_dir` asks, with the knob
    // on...
    let content = random_string();
    std::fs::write(buck.cwd().join("src.txt"), &content)?;
    let content_config = format!("test.content={}", random_string());
    let configs = ["-c", content_config.as_str()];

    let res = buck.build(&args(&["//:copy_src"], &configs)).await?;
    let output = res.build_report()?.output_for_target("//:copy_src")?;
    assert_eq!(presence_check_rpcs(&res)?, 0, "the CAS was not asked");
    assert_eq!(
        last_execution_kind(buck, "copy").await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    assert_eq!(std::fs::read_to_string(output)?, content);

    let res = buck.build(&args(&["//:copied_dir_src"], &configs)).await?;
    let output = res.build_report()?.output_for_target("//:copied_dir_src")?;
    assert_eq!(presence_check_rpcs(&res)?, 0, "the CAS was not asked");
    assert_eq!(
        last_execution_kind(buck, "symlinked_dir").await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    assert_eq!(std::fs::read_to_string(output.join("src.txt"))?, content);

    // ...while a write does.
    let res = buck.build(&args(&["//:write"], &configs)).await?;
    assert!(presence_check_rpcs(&res)? >= 1, "the probe asked the CAS");
    Ok(())
}

#[buck_test(write_invocation_record = true, skip_for_materializer = ["deferred"])]
async fn test_content_already_on_disk_is_copied_rather_than_downloaded(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    // An RE action on the write's output puts the content in the CAS, and the upload puts the
    // output on disk; the stash lets go of content the CAS holds.
    buck.build(&args(
        &["//:consume", "--remote-only", "--no-remote-cache"],
        &configs,
    ))
    .await?;

    // The second write of the same content is a probe hit, so nothing is written, and its output
    // is a copy of the first's rather than a download.
    let target = "//:write_again";
    let res = buck.build(&args(&[target], &configs)).await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    assert!(snapshot_counter(&res, "disk_state_materializer_copied_from_disk")? >= 1);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_a_local_only_build_does_not_probe(buck: &Buck) -> anyhow::Result<()> {
    written_without_a_probe(buck, &["--local-only"]).await
}

#[buck_test(write_invocation_record = true)]
async fn test_a_build_without_remote_cache_does_not_probe(buck: &Buck) -> anyhow::Result<()> {
    written_without_a_probe(buck, &["--no-remote-cache"]).await
}
