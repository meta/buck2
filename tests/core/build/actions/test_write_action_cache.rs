/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! With `buck2.write_action_cache` on, a write action asks the action cache for its content under a
//! key that is a hash of its command line. A hit declares the output as a CAS download without
//! rendering the content, and nothing is written until something needs the file. A miss renders the
//! content and produces it the way a run action's output comes to be: written to its path when the
//! machine has room for local work, uploaded to the CAS and declared from there when it does not. A
//! write whose key falls within `buck2.write_action_cache_populate`, under an executor that publishes
//! local results (`allow_cache_uploads`), goes through the cache whatever the mode says, and a miss
//! first uploads the content and records the entry for later builds. Under `verify` every write is
//! rendered and declared as without the cache, and a hit is only compared against it.
//!
//! The daemon is killed between recording and looking up, so that a hit says the key is the same
//! from one process to the next.

use std::collections::BTreeMap;

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::get_last_execution_kind;
use buck2_e2e::utils::random_string;
use serde_json::Value;

const ACTION_EXECUTION_KIND_SIMPLE: i64 = 4;
const ACTION_EXECUTION_KIND_DEFERRED: i64 = 6;

/// How many CAS presence-check RPCs the daemon had issued by the end of the command.
fn presence_check_rpcs(res: &BuckOutput) -> anyhow::Result<u64> {
    counter(res, "re_presence_check_rpcs")
}

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

/// How many soft errors of `category` the last command reported.
async fn soft_errors(buck: &Buck, category: &str) -> anyhow::Result<usize> {
    Ok(filter_events(
        buck,
        &["Event", "data", "Instant", "data", "StructuredError"],
    )
    .await?
    .iter()
    .filter(|e| e["soft_error_category"]["category"] == category)
    .count())
}

/// `args` followed by `configs`.
fn args<'a>(args: &[&'a str], configs: &[&'a str]) -> Vec<&'a str> {
    [args, configs].concat()
}

async fn last_write_execution_kind(buck: &Buck) -> anyhow::Result<Option<i64>> {
    get_last_execution_kind(buck, Some("write"), &[], None).await
}

fn read_output(res: &BuckOutput, target: &str) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(
        res.build_report()?.output_for_target(target)?,
    )?)
}

// The keys of two fixed writes: `//:write` with the content `golden`, and `//:write_tset`'s
// projection of it. A change here is a change to what the key covers or how it is hashed, after
// which nothing recorded before is found: `KEY_VERSION` in `write_cache.rs` or `HASH_VERSION` in
// `builder.rs` is due a bump, and these an update with it. The tset key also covers the set's
// node identities, which include the configuration, so this repo's execution platform changing
// moves it too; the plain key does not move with that. The platform is the repo's own, with a
// fixed path separator, so the keys are the same on every host.
const GOLDEN_KEYS: [(&str, &str); 2] = [
    (
        "prelude//:write",
        "f2503b3ff7cb537d034d6d042b05b046ebe8980c4a8a2841a3bbc66c268ff2bf",
    ),
    (
        "prelude//:write_tset",
        "6da1d4f6369bb634140fc022b3cf0f97534e2bd7b60a3bf844810ba921ad679f",
    ),
];

#[buck_test(write_invocation_record = true)]
async fn test_miss_is_written_when_there_is_room_for_it(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", &content_config, "-c", "buck2.write_action_cache=use"];

    // One write in an otherwise idle build is made locally, and nothing asks the CAS about
    // it: `write_cas_probe` is on in this repo, and the miss does not probe.
    let res = buck.build(&args(&["//:write"], &configs)).await?;
    let output = res.build_report()?.output_for_target("//:write")?;
    assert_eq!(
        soft_errors(buck, "write_action_cache_lookup_failed").await?,
        0
    );
    assert!(counter(&res, "write_action_cache_misses")? >= 1);
    assert!(counter(&res, "write_action_cache_written")? >= 1);
    assert_eq!(counter(&res, "write_action_cache_uploaded")?, 0);
    assert_eq!(
        presence_check_rpcs(&res)?,
        0,
        "the miss did not probe the CAS"
    );
    assert_eq!(
        last_write_execution_kind(buck).await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_miss_is_uploaded_when_there_is_no_room_for_it(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", &content_config, "-c", "buck2.write_action_cache=use"];
    // The gate the hybrid executor runs local commands behind, with no room at all.
    let env = ("BUCK2_TEST_LOW_PASS_FILTER_CAPACITY", "0");

    // The miss is uploaded and declared as CAS content; neither memory nor disk holds the
    // bytes...
    let res = buck
        .build(&args(&["//:write", "--materializations=none"], &configs))
        .env(env.0, env.1)
        .await?;
    let output = res.build_report()?.output_for_target("//:write")?;
    assert!(counter(&res, "write_action_cache_misses")? >= 1);
    assert!(counter(&res, "write_action_cache_uploaded")? >= 1);
    assert_eq!(counter(&res, "write_action_cache_written")?, 0);
    assert_eq!(
        last_write_execution_kind(buck).await?,
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

// The populate failure where the cache cannot be written is a (quiet) soft error.
#[buck_test(write_invocation_record = true, allow_soft_errors = true)]
async fn test_recorded_content_is_not_written_again(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    let res = buck
        .build(&args(
            &["//:write", "-c", "buck2.write_action_cache_populate=1"],
            &configs,
        ))
        .await?;
    if soft_errors(buck, "write_action_cache_populate_failed").await? > 0 {
        // CI and devservers lack write access to the action cache (see `test_cache_uploads`),
        // and a skip is a failure under tpx: where the entry cannot be recorded, what
        // there is to check is that the output still came to be, as a miss does.
        let output = res.build_report()?.output_for_target("//:write")?;
        assert!(counter(&res, "write_action_cache_written")? >= 1);
        assert_eq!(
            last_write_execution_kind(buck).await?,
            Some(ACTION_EXECUTION_KIND_SIMPLE)
        );
        assert_eq!(std::fs::read_to_string(output)?, content);
        return Ok(());
    }

    // A second write of the same content, in another daemon, finds the entry: the output is
    // declared as a CAS download, and neither memory nor disk holds the bytes...
    buck.kill().await?;
    let target = "//:write_again";
    let res = buck
        .build(&args(
            &[
                target,
                "--materializations=none",
                "-c",
                "buck2.write_action_cache=use",
            ],
            &configs,
        ))
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert!(counter(&res, "write_action_cache_hits")? >= 1);
    assert_eq!(presence_check_rpcs(&res)?, 0, "the CAS was not probed");
    assert_eq!(
        last_write_execution_kind(buck).await?,
        Some(ACTION_EXECUTION_KIND_DEFERRED)
    );
    assert!(!output.exists());

    // ...until something needs the file, which then comes out of the CAS.
    buck.build(&args(
        &[target, "-c", "buck2.write_action_cache=use"],
        &configs,
    ))
    .await?;
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true, allow_soft_errors = true)]
async fn test_different_content_is_a_miss(buck: &Buck) -> anyhow::Result<()> {
    let recorded = random_string();
    buck.build(&[
        "//:write",
        "-c",
        &format!("test.content={recorded}"),
        "-c",
        "buck2.write_action_cache_populate=1",
    ])
    .await?;
    buck.kill().await?;

    // The key is of the content's command line, so other content is a miss, written here.
    let content = random_string();
    let target = "//:write_again";
    let res = buck
        .build(&[
            target,
            "-c",
            &format!("test.content={content}"),
            "-c",
            "buck2.write_action_cache=use",
        ])
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert_eq!(counter(&res, "write_action_cache_hits")?, 0);
    assert!(counter(&res, "write_action_cache_misses")? >= 1);
    assert!(counter(&res, "write_action_cache_written")? >= 1);
    assert_eq!(
        last_write_execution_kind(buck).await?,
        Some(ACTION_EXECUTION_KIND_SIMPLE)
    );
    assert_eq!(std::fs::read_to_string(output)?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true, allow_soft_errors = true)]
async fn test_verify_checks_a_hit_against_the_render(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];

    buck.build(&args(
        &["//:write", "-c", "buck2.write_action_cache_populate=1"],
        &configs,
    ))
    .await?;
    let recorded = soft_errors(buck, "write_action_cache_populate_failed").await? == 0;
    buck.kill().await?;

    // `verify` renders and declares the write as without the cache, and only compares.
    let target = "//:write_again";
    let res = buck
        .build(&args(
            &[target, "-c", "buck2.write_action_cache=verify"],
            &configs,
        ))
        .await?;
    assert_eq!(read_output(&res, target)?, content);
    assert_eq!(counter(&res, "write_action_cache_mismatches")?, 0);
    assert_eq!(soft_errors(buck, "write_action_cache_mismatch").await?, 0);
    if recorded {
        assert!(counter(&res, "write_action_cache_verified")? >= 1);
    }
    Ok(())
}

#[buck_test(write_invocation_record = true, allow_soft_errors = true)]
async fn test_transitive_set_projection_is_served(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = ["-c", content_config.as_str()];
    let expected = format!("{content}\nchild");

    let res = buck
        .build(&args(
            &["//:write_tset", "-c", "buck2.write_action_cache_populate=1"],
            &configs,
        ))
        .await?;
    if soft_errors(buck, "write_action_cache_populate_failed").await? > 0 {
        assert_eq!(read_output(&res, "//:write_tset")?, expected);
        return Ok(());
    }
    buck.kill().await?;

    // The projection's hash is memoized on the frozen set in each daemon; a hit across daemons
    // says it comes out the same.
    let target = "//:write_tset_again";
    let res = buck
        .build(&args(
            &[
                target,
                "--materializations=none",
                "-c",
                "buck2.write_action_cache=use",
            ],
            &configs,
        ))
        .await?;
    let output = res.build_report()?.output_for_target(target)?;
    assert!(counter(&res, "write_action_cache_hits")? >= 1);
    assert_eq!(
        last_write_execution_kind(buck).await?,
        Some(ACTION_EXECUTION_KIND_DEFERRED)
    );
    assert!(!output.exists());

    buck.build(&args(
        &[target, "-c", "buck2.write_action_cache=use"],
        &configs,
    ))
    .await?;
    assert_eq!(std::fs::read_to_string(output)?, expected);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_a_key_outside_the_share_is_a_plain_write(buck: &Buck) -> anyhow::Result<()> {
    let content = random_string();
    // A share this small holds none of the keys a test produces; a write outside it, with the
    // mode off, never asks the cache.
    let res = buck
        .build(&[
            "//:write",
            "-c",
            &format!("test.content={content}"),
            "-c",
            "buck2.write_action_cache=off",
            "-c",
            "buck2.write_action_cache_populate=0.000000001",
        ])
        .await?;
    assert_eq!(counter(&res, "write_action_cache_hits")?, 0);
    assert_eq!(counter(&res, "write_action_cache_misses")?, 0);
    assert_eq!(read_output(&res, "//:write")?, content);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_nothing_is_recorded_where_local_results_are_not_published(
    buck: &Buck,
) -> anyhow::Result<()> {
    let content = random_string();
    let content_config = format!("test.content={content}");
    let configs = [
        "-c",
        content_config.as_str(),
        "-c",
        "test.allow_cache_uploads=false",
        "-c",
        "buck2.write_action_cache_populate=1",
    ];

    // The share covers every key, but the executor publishes no local results: with the mode
    // off the cache is not asked...
    let res = buck
        .build(&args(
            &["//:write", "-c", "buck2.write_action_cache=off"],
            &configs,
        ))
        .await?;
    assert_eq!(counter(&res, "write_action_cache_misses")?, 0);
    assert_eq!(read_output(&res, "//:write")?, content);

    // ...and under `use` it is asked, and the miss is not recorded.
    let res = buck
        .build(&args(
            &["//:write_again", "-c", "buck2.write_action_cache=use"],
            &configs,
        ))
        .await?;
    assert!(counter(&res, "write_action_cache_misses")? >= 1);
    assert_eq!(counter(&res, "write_action_cache_populated")?, 0);
    assert_eq!(counter(&res, "write_action_cache_populate_failures")?, 0);
    assert_eq!(read_output(&res, "//:write_again")?, content);
    Ok(())
}

#[buck_test]
async fn test_keys_are_golden(buck: &Buck) -> anyhow::Result<()> {
    let dump = buck.cwd().join("keys.txt");
    buck.build(&[
        "//:write",
        "//:write_tset",
        "-c",
        "test.content=golden",
        "-c",
        "buck2.write_action_cache=verify",
    ])
    .env("BUCK2_TEST_WRITE_ACTION_CACHE_KEY_DUMP", &dump)
    .await?;
    let text = std::fs::read_to_string(&dump)?;
    let mut keys = BTreeMap::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        // `<owner>\t<output>\t<key>`; the owner displays as `prelude//:write (<configuration>)`.
        let owner = parts[0].split(" (").next().unwrap_or_default();
        keys.insert(owner, parts[parts.len() - 1]);
    }
    assert_eq!(keys, BTreeMap::from(GOLDEN_KEYS), "{text}");
    Ok(())
}
