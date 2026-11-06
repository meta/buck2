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
use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::http::sha1_hex;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::expect_exec_count;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::get_last_execution_kind;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

// Taken from data.proto
const ACTION_EXECUTION_KIND_LOCAL: i64 = 1;
const ACTION_EXECUTION_KIND_ACTION_CACHE: i64 = 3;
const ACTION_EXECUTION_KIND_SIMPLE: i64 = 4;
// A write whose content the CAS already holds, which the disk-state core probes for.
const ACTION_EXECUTION_KIND_DEFERRED: i64 = 6;
const ACTION_EXECUTION_KIND_LOCAL_DEP_FILE: i64 = 7;
const ACTION_EXECUTION_KIND_REMOTE_DEP_FILE_CACHE: i64 = 9;
const ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE: i64 = 10;

// The kinds a write action reports, which these tests look past.
const WRITE_KINDS: [i64; 2] = [ACTION_EXECUTION_KIND_SIMPLE, ACTION_EXECUTION_KIND_DEFERRED];

const UPLOAD_RESULT_ACTION_UPLOAD_NOT_ALLOWED: i64 = 6;
const UPLOAD_RESULT_DEP_FILE_UPLOAD_NOT_ALLOWED: i64 = 18;

fn as_strs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// Reads `path` as Python's `Path.read_text` does, with universal newlines: `\r\n` and `\r` read as
/// `\n`. An output served from the remote cache may have been produced on Windows.
fn read_text(path: impl AsRef<Path>) -> anyhow::Result<String> {
    let path = path.as_ref();
    let text =
        std::fs::read_to_string(path).with_context(|| format!("Reading `{}`", path.display()))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

async fn expect_only_dep_file_hit(buck: &Buck) -> anyhow::Result<()> {
    let what_ran = read_what_ran(buck, &[]).await?;
    assert_eq!(
        what_ran
            .iter()
            .filter(|x| x["reproducer"]["executor"] == "LocalDepFileCache")
            .count(),
        1,
        "{what_ran:#?}"
    );
    assert_eq!(what_ran.len(), 1, "{what_ran:#?}");
    Ok(())
}

async fn execution_kinds(buck: &Buck) -> anyhow::Result<Vec<i64>> {
    filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "ActionExecution",
            "execution_kind",
        ],
    )
    .await?
    .iter()
    .map(|kind| {
        kind.as_i64()
            .with_context(|| format!("A non-integer `execution_kind`: {kind}"))
    })
    .collect()
}

async fn check_execution_kind(
    buck: &Buck,
    expecteds: &[i64],
    ignored: &[i64],
) -> anyhow::Result<()> {
    let execution_kinds: Vec<i64> = execution_kinds(buck)
        .await?
        .into_iter()
        .filter(|kind| !ignored.contains(kind))
        .collect();
    assert_eq!(execution_kinds, expecteds);
    Ok(())
}

#[derive(Debug, PartialEq)]
struct MatchDepFilesEvent {
    remote_cache: bool,
    checking_filtered_inputs: bool,
}

async fn check_match_dep_files_events(
    buck: &Buck,
    expected_events: &[MatchDepFilesEvent],
) -> anyhow::Result<()> {
    let flag = |event: &Value, key: &str| {
        event[key]
            .as_bool()
            .with_context(|| format!("No boolean `{key}` in `MatchDepFiles`: {event}"))
    };
    let match_dep_files = filter_events(
        buck,
        &["Event", "data", "SpanStart", "data", "MatchDepFiles"],
    )
    .await?
    .iter()
    .map(|event| {
        Ok(MatchDepFilesEvent {
            remote_cache: flag(event, "remote_cache")?,
            checking_filtered_inputs: flag(event, "checking_filtered_inputs")?,
        })
    })
    .collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(match_dep_files, expected_events);
    Ok(())
}

async fn get_execution_kind(buck: &Buck) -> anyhow::Result<i64> {
    execution_kinds(buck)
        .await?
        .first()
        .copied()
        .context("No action execution in the event log")
}

/// Append a random string to the marker in the file
fn touch(buck: &Buck, name: &str) -> std::io::Result<()> {
    let path = buck.cwd().join(name);
    let text = std::fs::read_to_string(&path)?;
    std::fs::write(
        &path,
        text.replace("__MARKER__", &format!("__MARKER__{}", random_string())),
    )
}

/// Common implementation for dep files tests.
async fn test_dep_files_impl(buck: &Buck, use_content_based_paths: bool) -> anyhow::Result<()> {
    // We query cache before we query dep file. Disable remote cache to make
    // sure that for the last build what-ran doesn't return cached entry.
    let content_based_paths = format!("test.use_content_based_paths={use_content_based_paths}");
    let args = ["app:app", "--no-remote-cache", "-c", &content_based_paths];
    buck.build(&args).await?;
    expect_exec_count(buck, 1).await?;

    touch(buck, "app/app.h")?;
    buck.build(&args).await?;
    expect_exec_count(buck, 1).await?;

    touch(buck, "app/app.c")?;
    buck.build(&args).await?;
    expect_exec_count(buck, 1).await?;

    // //app:app doesn't use other.h and
    // using dep file this should build nothing.
    touch(buck, "app/other.h")?;
    buck.build(&args).await?;
    expect_only_dep_file_hit(buck).await?;
    check_execution_kind(
        buck,
        &[ACTION_EXECUTION_KIND_LOCAL_DEP_FILE],
        // A symlinked_dir command was re-run because app/other.h was changed
        &WRITE_KINDS,
    )
    .await?;

    // Changing the command line itself should cause a rebuild.
    touch(buck, "app/other.h")?;
    let unused_command_line_param = format!("test.unused_command_line_param={}", random_string());
    buck.build(&[&args[..], &["-c", &unused_command_line_param]].concat())
        .await?;
    expect_exec_count(buck, 1).await?;
    Ok(())
}

async fn test_dep_files_in_same_package_impl(
    buck: &Buck,
    use_content_based_paths: bool,
) -> anyhow::Result<()> {
    let make_args = |used_input1_contents: &str,
                     used_input2_contents: &str,
                     unused_input1_contents: &str,
                     unused_input2_contents: &str|
     -> Vec<String> {
        vec![
            "app:simple_dep_file".into(),
            "--no-remote-cache".into(),
            "-c".into(),
            format!("test.used_input1_contents={used_input1_contents}"),
            "-c".into(),
            format!("test.used_input2_contents={used_input2_contents}"),
            "-c".into(),
            format!("test.unused_input1_contents={unused_input1_contents}"),
            "-c".into(),
            format!("test.unused_input2_contents={unused_input2_contents}"),
            "-c".into(),
            format!("test.use_content_based_paths={use_content_based_paths}"),
        ]
    };

    let mut used_input1_contents = random_string();
    let mut used_input2_contents = random_string();
    let mut unused_input1_contents = random_string();
    let mut unused_input2_contents = random_string();

    buck.build(&as_strs(&make_args(
        &used_input1_contents,
        &used_input2_contents,
        &unused_input1_contents,
        &unused_input2_contents,
    )))
    .await?;
    expect_exec_count(buck, 1).await?;

    used_input1_contents = random_string();
    buck.build(&as_strs(&make_args(
        &used_input1_contents,
        &used_input2_contents,
        &unused_input1_contents,
        &unused_input2_contents,
    )))
    .await?;
    expect_exec_count(buck, 1).await?;

    used_input2_contents = random_string();
    buck.build(&as_strs(&make_args(
        &used_input1_contents,
        &used_input2_contents,
        &unused_input1_contents,
        &unused_input2_contents,
    )))
    .await?;
    expect_exec_count(buck, 1).await?;

    unused_input1_contents = random_string();
    buck.build(&as_strs(&make_args(
        &used_input1_contents,
        &used_input2_contents,
        &unused_input1_contents,
        &unused_input2_contents,
    )))
    .await?;
    expect_only_dep_file_hit(buck).await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL_DEP_FILE], &WRITE_KINDS).await?;

    unused_input2_contents = random_string();
    buck.build(&as_strs(&make_args(
        &used_input1_contents,
        &used_input2_contents,
        &unused_input1_contents,
        &unused_input2_contents,
    )))
    .await?;
    expect_only_dep_file_hit(buck).await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL_DEP_FILE], &WRITE_KINDS).await?;
    Ok(())
}

async fn test_dep_files_in_same_dir_impl(
    buck: &Buck,
    use_content_based_paths: bool,
) -> anyhow::Result<()> {
    let make_args = |used_input_contents: &str, unused_input_contents: &str| -> Vec<String> {
        vec![
            "app:shared_dir_dep_file".into(),
            "--no-remote-cache".into(),
            "-c".into(),
            format!("test.used_input_contents={used_input_contents}"),
            "-c".into(),
            format!("test.unused_input_contents={unused_input_contents}"),
            "-c".into(),
            format!("test.use_content_based_paths={use_content_based_paths}"),
        ]
    };

    let mut used_input_contents = random_string();
    let mut unused_input_contents = random_string();

    buck.build(&as_strs(&make_args(
        &used_input_contents,
        &unused_input_contents,
    )))
    .await?;
    expect_exec_count(buck, 1).await?;

    used_input_contents = random_string();
    buck.build(&as_strs(&make_args(
        &used_input_contents,
        &unused_input_contents,
    )))
    .await?;
    expect_exec_count(buck, 1).await?;

    unused_input_contents = random_string();
    buck.build(&as_strs(&make_args(
        &used_input_contents,
        &unused_input_contents,
    )))
    .await?;
    expect_only_dep_file_hit(buck).await?;

    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL_DEP_FILE], &WRITE_KINDS).await?;
    Ok(())
}

async fn get_cache_queries(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanStart",
            "data",
            "ExecutorStage",
            "stage",
            "CacheQuery",
        ],
    )
    .await
}

async fn check_no_cache_query(buck: &Buck) -> anyhow::Result<()> {
    let cache_queries = get_cache_queries(buck).await?;
    assert_eq!(cache_queries.len(), 0, "{cache_queries:#?}");
    Ok(())
}

async fn check_cache_query(buck: &Buck) -> anyhow::Result<()> {
    let cache_queries = get_cache_queries(buck).await?;
    assert_eq!(cache_queries.len(), 1, "{cache_queries:#?}");
    Ok(())
}

fn action_digests(value: &Value) -> Vec<&Value> {
    match value {
        Value::Object(map) => map
            .iter()
            .filter(|(k, _)| *k == "action_digest")
            .map(|(_, v)| v)
            .chain(map.values().flat_map(action_digests))
            .collect(),
        Value::Array(items) => items.iter().flat_map(action_digests).collect(),
        _ => Vec::new(),
    }
}

async fn executed_action_digests(buck: &Buck) -> anyhow::Result<Vec<String>> {
    let commands = Value::Array(
        filter_events(
            buck,
            &[
                "Event",
                "data",
                "SpanEnd",
                "data",
                "ActionExecution",
                "commands",
            ],
        )
        .await?,
    );
    action_digests(&commands)
        .into_iter()
        .map(|digest| {
            digest
                .as_str()
                .map(str::to_owned)
                .with_context(|| format!("A non-string `action_digest`: {digest}"))
        })
        .collect()
}

async fn prepare_persisted_dep_file_input_after_clean(
    buck: &Buck,
) -> anyhow::Result<(&'static str, Vec<&'static str>)> {
    let target = "root//app:consume_persisted_dep_file_output";
    let args = vec![target, "--local-only", "--no-remote-cache"];
    let first = buck.build(&args).await?;
    let output = first.build_report()?.output_for_target(target)?;
    assert_eq!(read_text(output)?, "output");

    buck.kill().await?;
    // Reload both actions from the persisted cache without final-output materialization. The
    // producer is accepted by `declare_match` and must be protected from stale cleanup.
    buck.build(&[&args[..], &["--materializations=none"]].concat())
        .await?;
    let kinds = execution_kinds(buck).await?;
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE)
            .count(),
        2,
        "{kinds:?}"
    );

    let entries = buck.audit(&["deferred-materializer", "list"]).await?.stdout;
    fn path_of(entry: &str) -> &str {
        entry.split_once('\t').map_or(entry, |(path, _)| path)
    }
    let args_entries: Vec<&str> = entries
        .lines()
        .filter(|entry| path_of(entry).ends_with("/__persisted_dep_file_output_args__/args"))
        .collect();
    assert_eq!(args_entries.len(), 1, "{args_entries:?}");
    let args_file = buck.cwd().join(path_of(args_entries[0]));
    let producer_output = buck.cwd().join(read_text(&args_file)?.trim());
    assert!(
        !producer_output
            .to_string_lossy()
            .contains("/output_artifacts/"),
        "{}",
        producer_output.display()
    );
    assert!(producer_output.exists(), "{}", producer_output.display());

    let clean = buck.clean(&["--stale=0s"]).await?;
    assert!(producer_output.exists(), "{}", clean.stderr);
    // Invalidate only the consumer. Its retained args file still names the producer output.
    touch(buck, "app/other.h")?;

    Ok((target, args))
}

async fn dep_file_uploads(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    filter_events(buck, &["Event", "data", "SpanEnd", "data", "DepFileUpload"]).await
}

async fn action_executions(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await
}

async fn dep_file_key_from_executions(buck: &Buck) -> anyhow::Result<Value> {
    let execs = action_executions(buck).await?;
    assert_eq!(execs.len(), 1, "{execs:#?}");
    execs[0]
        .get("dep_file_key")
        .filter(|key| key.is_string())
        .cloned()
        .with_context(|| format!("No `dep_file_key` in {}", execs[0]))
}

async fn check_uploaded_dep_file_key(buck: &Buck, dep_file_key: &Value) -> anyhow::Result<()> {
    // BUCK2_TEST_SKIP_ACTION_CACHE_WRITE causes action result writes for dep files to always pass.
    // This is to allow testing without action cache write permission.
    let dep_file_uploads: Vec<Value> = dep_file_uploads(buck)
        .await?
        .into_iter()
        .filter(|upload| upload["success"].as_bool() == Some(true))
        .collect();
    assert_eq!(dep_file_uploads.len(), 1, "{dep_file_uploads:#?}");
    let uploaded_key = dep_file_uploads[0]
        .get("remote_dep_file_key")
        .filter(|key| key.is_string())
        .with_context(|| format!("No `remote_dep_file_key` in {}", dep_file_uploads[0]))?;
    assert_eq!(dep_file_key, uploaded_key);
    Ok(())
}

async fn check_remote_dep_file_cache_query_took_place(buck: &Buck) -> anyhow::Result<Value> {
    let what_ran = read_what_ran(buck, &["--emit-cache-queries"]).await?;
    assert_eq!(
        what_ran[0]["reproducer"]["executor"], "CacheQuery",
        "{what_ran:#?}"
    );
    Ok(what_ran[0]["reproducer"]["details"]["digest"].clone())
}

async fn run_test_input_cannot_be_normalized(
    buck: &Buck,
    allow_soft_errors: bool,
) -> anyhow::Result<()> {
    let target = "root//:input_cannot_be_normalized";
    let tagged_unused = buck.cwd().join("unused.1");
    assert!(tagged_unused.exists());

    // We query cache before we query dep file. Disable remote cache to make
    // sure that for the last build what-ran doesn't return cached entry.
    let args = [target, "--no-remote-cache"];
    buck.build(&args).await?;
    expect_exec_count(buck, 1).await?;

    // We should get a dep file cache hit, but we don't because the input cannot be normalized.
    std::fs::write(&tagged_unused, random_string())?;
    if allow_soft_errors {
        buck.build(&args).await?;
        expect_exec_count(buck, 1).await?;
    } else {
        buck.build(&args).await.expect_failure().assert_stderr_matches(
            "Path.*cannot be normalized for dep-files because it has two path segments that look like a content-based hash!",
        );
    }
    Ok(())
}

/// The `test.<key>` buckconfig values `root//app:canonical_json` reads, in the order they were
/// first set.
struct Options(Vec<(&'static str, String)>);

impl Options {
    fn new(options: &[(&'static str, &str)]) -> Self {
        Self(
            options
                .iter()
                .map(|(key, value)| (*key, (*value).to_owned()))
                .collect(),
        )
    }

    fn set(&mut self, key: &'static str, value: impl Into<String>) {
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = value,
            None => self.0.push((key, value)),
        }
    }

    fn get(&self, key: &str) -> &str {
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or_else(|| panic!("No option `{key}`"))
    }

    fn flags(&self) -> Vec<String> {
        self.0
            .iter()
            .flat_map(|(key, value)| ["-c".to_owned(), format!("test.{key}={value}")])
            .collect()
    }
}

async fn build_canonical_input(
    buck: &Buck,
    options: &Options,
    expected_kind: i64,
) -> anyhow::Result<BuckOutput> {
    let flags = options.flags();
    let result = buck
        .build(
            &[
                &[
                    "root//app:canonical_json",
                    "root//app:canonical_json[json]",
                    "--local-only",
                    "--no-remote-cache",
                ][..],
                &as_strs(&flags),
            ]
            .concat(),
        )
        .await?;
    assert_eq!(
        get_last_execution_kind(
            buck,
            Some("canonical_json_consumer"),
            &[],
            Some("canonical_json")
        )
        .await?,
        Some(expected_kind)
    );
    Ok(result)
}

fn canonical_input_output(result: &BuckOutput) -> anyhow::Result<(String, String)> {
    let report = result.build_report()?;
    let [json] =
        <[PathBuf; 1]>::try_from(report.outputs_for_target("root//app:canonical_json", "json")?)
            .map_err(|outputs| {
                anyhow::anyhow!("Found {} outputs for `json`: {outputs:?}", outputs.len())
            })?;
    Ok((
        read_text(report.output_for_target("root//app:canonical_json")?)?,
        read_text(json)?,
    ))
}

// Flaky because of watchman on mac (and maybe windows)
// Skipping on windows due to gcc dependency
#[buck_test(data_dir = "dep_files", skip_for_os = ["darwin", "windows"])]
async fn test_dep_files_with_content_based_paths(buck: &Buck) -> anyhow::Result<()> {
    test_dep_files_impl(buck, true).await
}

// Flaky because of watchman on mac (and maybe windows)
// Skipping on windows due to gcc dependency
#[buck_test(data_dir = "dep_files", skip_for_os = ["darwin", "windows"])]
async fn test_dep_files_without_content_based_paths(buck: &Buck) -> anyhow::Result<()> {
    test_dep_files_impl(buck, false).await
}

// Flaky because of watchman on mac (and maybe windows)
// Skipping on windows due to gcc dependency
#[buck_test(data_dir = "dep_files", skip_for_os = ["darwin", "windows"])]
async fn test_dep_files_in_same_package_with_content_based(buck: &Buck) -> anyhow::Result<()> {
    test_dep_files_in_same_package_impl(buck, true).await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["darwin", "windows"])]
async fn test_dep_files_in_same_package_without_content_based(buck: &Buck) -> anyhow::Result<()> {
    test_dep_files_in_same_package_impl(buck, false).await
}

// Flaky because of watchman on mac (and maybe windows)
// Skipping on windows due to gcc dependency
#[buck_test(data_dir = "dep_files", skip_for_os = ["darwin", "windows"])]
async fn test_dep_files_in_same_dir_with_content_based(buck: &Buck) -> anyhow::Result<()> {
    test_dep_files_in_same_dir_impl(buck, true).await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["darwin", "windows"])]
async fn test_dep_files_in_same_dir_without_content_based(buck: &Buck) -> anyhow::Result<()> {
    test_dep_files_in_same_dir_impl(buck, false).await
}

// Skipping on windows due to gcc dependency
#[buck_test(
    // test uses symlinks that mess up with eden symlink redirection on MacOS
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
)]
async fn test_dep_file_hit_identical_action(buck: &Buck) -> anyhow::Result<()> {
    // For actions that have dep files, buck will query the local dep file cache to see
    // if an identical action is stored there. Otherwise, it will fall back to an action cache
    // look up (if enabled) and then to the full dep file query.
    // This test builds a target to build up a dep file cache, then builds the target again
    // with a no-op configuration change so that we hit the initial dep file lookup hit case.
    let dummy1 = "dummy1";
    buck.build(&[
        "app:app_with_dummy_config",
        "--local-only",
        "--no-remote-cache", // Turn off remote cache query so we execute locally
        "-c",
        &format!("test.dummy_config={dummy1}"),
    ])
    .await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL], &WRITE_KINDS).await?;

    let dummy2 = "dummy2";
    buck.build(&[
        "app:app_with_dummy_config",
        "--local-only",
        "-c",
        &format!("test.dummy_config={dummy2}"),
    ])
    .await?;
    // The result should be served by the local dep file cache BEFORE an action cache lookup
    check_no_cache_query(buck).await?;
    // Ignoring any simple actions because there can be either one or two symlink dir actions,
    // with the same dice key,
    // Not sure why but this feels like a DICE bug triggered by the buckconfig change.
    check_execution_kind(
        buck,
        &[ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE],
        &WRITE_KINDS,
    )
    .await?;
    // The MatchDepFilesStart span should indicate we only checked the depfile cache once
    check_match_dep_files_events(
        buck,
        &[MatchDepFilesEvent {
            remote_cache: false,
            checking_filtered_inputs: false,
        }],
    )
    .await?;
    Ok(())
}

// The persisted local dep-file cache reloads across daemon restarts. After a restart the in-memory
// cache is gone, but the entry is reloaded from the sqlite db and, because the outputs are still
// materialized on disk, the identical action is served from the LOCAL_ACTION_CACHE without
// re-executing. The `_disabled` control proves this only happens with the feature enabled.
#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    // The persisted dep-file cache is gated on a daemon-startup buckconfig (read once when the daemon
    // boots, like the materializer/incremental state dbs), so it must be set here rather than via `-c`.
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_dep_file_hit_persisted_across_restart(buck: &Buck) -> anyhow::Result<()> {
    let args = [
        "app:app_with_dummy_config",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.dummy_config=dummy1",
    ];
    // First build populates both buck-out and the persisted dep-file cache.
    buck.build(&args).await?;
    // Killing the daemon drops the in-memory dep-file cache; the sqlite db and outputs persist.
    buck.kill().await?;
    // The rebuild reloads the entry from disk and serves the identical action from the local cache.
    buck.build(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    // Served by the reloaded local dep-file cache before any action-cache lookup.
    check_no_cache_query(buck).await?;
    Ok(())
}

// An action listed in `BUCK2_INDUCED_CACHE_MISSES` must re-execute even when the local dep-file
// cache could serve it as an identical action.
#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_induced_cache_miss_skips_local_dep_file_cache(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let args = [
        "app:app_with_dummy_config",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.dummy_config=dummy1",
    ];
    buck.build(&args).await?;
    let induced_cache_misses = tmp_path.join("induced_cache_misses");
    std::fs::write(
        &induced_cache_misses,
        executed_action_digests(buck)
            .await?
            .iter()
            .map(|d| format!("{d}\n"))
            .collect::<String>(),
    )?;
    // The list is read once at daemon startup.
    buck.kill().await?;
    buck.build(&args)
        .env("BUCK2_INDUCED_CACHE_MISSES", &induced_cache_misses)
        .await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        !kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(
        !kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_DEP_FILE),
        "{kinds:?}"
    );
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    Ok(())
}

#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [
        ("buck2", "defer_write_actions", "true"),
        ("buck2", "restarter", "false"),
        ("buck2", "sqlite_dep_file_state", "true"),
    ],
)]
async fn test_persisted_dep_file_hit_survives_clean_stale(buck: &Buck) -> anyhow::Result<()> {
    let (target, args) = prepare_persisted_dep_file_input_after_clean(buck).await?;

    let result = buck.build(&args).await?;
    golden(
        &sanitize_stderr(&result.stderr),
        "dep_files/golden/persisted_dep_file_hit_survives_clean_stale.stderr",
    );
    assert_eq!(
        read_text(result.build_report()?.output_for_target(target)?)?,
        "output"
    );
    Ok(())
}

#[buck_test(setup_eden = false, data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_dep_file_not_persisted_across_restart_when_disabled(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Control for `test_dep_file_hit_persisted_across_restart`: with the feature disabled (the
    // default), a restart loses the cache and the identical action re-executes locally.
    let args = [
        "app:app_with_dummy_config",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.dummy_config=dummy1",
    ];
    buck.build(&args).await?;
    buck.kill().await?;
    buck.build(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        !kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    Ok(())
}

// An anon target's actions are keyed by the anon target's identity hash rather than a configured
// label. They persist and reload like any other locally-produced action. Without content-based paths
// the identity hash is also part of the output path, so that case is the one where its stability
// across sessions is observable.
async fn test_anon_target_action_persisted_across_restart(
    buck: &Buck,
    content_paths: &str,
) -> anyhow::Result<()> {
    let target = "root//app:anon_owned_run";
    let content_based_paths = format!("test.use_content_based_paths={content_paths}");
    let args = [
        target,
        "--local-only",
        "--no-remote-cache",
        "-c",
        &content_based_paths,
    ];
    buck.build(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    buck.kill().await?;

    let result = buck.build(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(!kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    assert_eq!(
        read_text(result.build_report()?.output_for_target(target)?)?,
        "anon"
    );
    check_no_cache_query(buck).await?;
    Ok(())
}

#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_anon_target_action_persisted_across_restart_true(buck: &Buck) -> anyhow::Result<()> {
    test_anon_target_action_persisted_across_restart(buck, "true").await
}

#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_anon_target_action_persisted_across_restart_false(buck: &Buck) -> anyhow::Result<()> {
    test_anon_target_action_persisted_across_restart(buck, "false").await
}

#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_changed_anon_target_action_is_not_served_from_persisted_cache(
    buck: &Buck,
) -> anyhow::Result<()> {
    // The marker is passed to the anon target by artifact, so this changes the anon action's inputs
    // but not the anon target's identity: the persisted row from the previous session is found under
    // the same key, and must be rejected on its digests.
    let target = "root//app:anon_owned_run";

    let args = |marker_content: &str| -> Vec<String> {
        vec![
            target.into(),
            "--local-only".into(),
            "--no-remote-cache".into(),
            "-c".into(),
            format!("test.anon_marker_content={marker_content}"),
        ]
    };

    buck.build(&as_strs(&args("anon1"))).await?;
    buck.kill().await?;

    let result = buck.build(&as_strs(&args("anon2"))).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        !kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    assert_eq!(
        read_text(result.build_report()?.output_for_target(target)?)?,
        "anon2"
    );
    Ok(())
}

// A BXL action's key hashes the whole BXL invocation (script, CLI args, global configuration, and
// execution platform resolution), so the same invocation after a restart finds its persisted row.
#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_bxl_action_persisted_across_restart(buck: &Buck) -> anyhow::Result<()> {
    let args = [
        "root//app/run.bxl:bxl_run",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "build.execution_platforms=root//app:canonical_platforms",
    ];
    let first = buck.bxl(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    buck.kill().await?;

    let second = buck.bxl(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(!kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    assert_eq!(second.stdout, first.stdout);
    assert_eq!(read_text(buck.cwd().join(second.stdout.trim()))?, "bxl");

    // A different CLI arg is a different BXL key as well as a different input; it re-executes.
    let third = buck
        .bxl(&[&args[..], &["--", "--marker_content", "bxl2"]].concat())
        .await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        !kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    assert_eq!(read_text(buck.cwd().join(third.stdout.trim()))?, "bxl2");
    Ok(())
}

#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_changed_action_is_not_served_from_persisted_cache(buck: &Buck) -> anyhow::Result<()> {
    // The risk the persisted cache carries is not missing a hit, it is serving a stale output. A
    // reloaded entry is only usable for an action that is genuinely identical, so changing an input
    // across the restart must re-execute and produce the new content.
    let args = |used_input_contents: &str| -> Vec<String> {
        vec![
            "app:dir_output_dep_file".into(),
            "--local-only".into(),
            "--no-remote-cache".into(),
            "-c".into(),
            format!("test.used_input_contents={used_input_contents}"),
            "--show-output".into(),
        ]
    };

    buck.build(&as_strs(&args("used1"))).await?;
    buck.kill().await?;

    let result = buck.build(&as_strs(&args("used2"))).await?;
    let kinds = execution_kinds(buck).await?;
    // The persisted entry exists for this action, but its digests no longer match.
    assert!(
        !kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");

    // The output on disk must be the newly produced one, not the reloaded entry's.
    let outputs = result.target_to_build_output()?;
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    let output = outputs.values().next().context("No output")?;
    let out_dir = std::fs::canonicalize(buck.cwd().join(output))?;
    // The action echoes its used input, so this distinguishes the new tree from a reloaded one.
    assert_eq!(read_text(out_dir.join("f"))?, "used2");
    Ok(())
}

// A directory output's tree is not serialized into the dep-file db; only its fingerprint is. Across a
// restart the tree is rehydrated from the materializer (which persists+reloads it) and verified
// against that fingerprint, so an action with a directory output still hits the LOCAL_ACTION_CACHE.
#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_dir_output_dep_file_hit_persisted_across_restart(buck: &Buck) -> anyhow::Result<()> {
    let args = [
        "app:dir_output_dep_file",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.used_input_contents=used1",
    ];
    buck.build(&args).await?;
    buck.kill().await?;
    buck.build(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    check_no_cache_query(buck).await?;
    Ok(())
}

#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_dir_output_dep_file_hit_persisted_without_content_based_paths(
    buck: &Buck,
) -> anyhow::Result<()> {
    // As above, but with a configuration-based output path, so the reload resolves the directory
    // without a content hash.
    let args = [
        "app:dir_output_dep_file",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.used_input_contents=used1",
        "-c",
        "test.use_content_based_paths=false",
    ];
    buck.build(&args).await?;
    buck.kill().await?;
    buck.build(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    Ok(())
}

#[buck_test(
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_flush_dep_files_clears_persisted_cache(buck: &Buck) -> anyhow::Result<()> {
    // `flush-dep-files` clears the in-memory cache synchronously, so the persisted rows must be gone
    // by the time it returns as well -- a row that outlives it would be reloaded after a restart and
    // serve an entry the user explicitly invalidated.
    let args = [
        "app:app_with_dummy_config",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.dummy_config=dummy1",
    ];
    buck.build(&args).await?;
    buck.debug(&["flush-dep-files"]).await?;
    // Only a restart can distinguish a cleared db from a still-populated one: without the kill the
    // empty in-memory cache would produce a miss either way.
    buck.kill().await?;
    buck.build(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        !kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    Ok(())
}

#[buck_test(
    // Setting `sqlite_dep_file_state` without `sqlite_materializer_state` is reported as
    // a soft error, which is what this test sets up.
    allow_soft_errors = true,
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    // The persisted cache re-validates reloaded outputs against the materializer's own state db, so
    // it refuses to start without it. Requesting it here should warn and stay disabled, not fail.
    extra_buck_config = [
        ("buck2", "sqlite_dep_file_state", "true"),
        ("buck2", "sqlite_materializer_state", "false"),
    ],
)]
async fn test_dep_file_persistence_disabled_without_materializer_state(
    buck: &Buck,
) -> anyhow::Result<()> {
    let args = [
        "app:app_with_dummy_config",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.dummy_config=dummy1",
    ];
    buck.build(&args).await?;
    buck.kill().await?;
    buck.build(&args).await?;
    let kinds = execution_kinds(buck).await?;
    assert!(
        !kinds.contains(&ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE),
        "{kinds:?}"
    );
    assert!(kinds.contains(&ACTION_EXECUTION_KIND_LOCAL), "{kinds:?}");
    Ok(())
}

// Skipping on windows: simple_dep_file's action uses symlinks, which aren't supported there.
#[buck_test(
    // test uses symlinks that mess up with eden symlink redirection on MacOS
    setup_eden = false,
    data_dir = "dep_files",
    skip_for_os = ["windows"],
)]
async fn test_dep_file_hit_across_configurations(buck: &Buck) -> anyhow::Result<()> {
    // An action whose outputs are all content-based and whose inputs are all eligible for dedupe has
    // a configuration-independent input directory, command line and output paths.
    //
    // This builds an identical, dedupe-eligible action under two different
    // target configurations (platform_a and platform_b).
    let result_a = buck
        .build(&[
            "app:simple_dep_file",
            "--target-platforms",
            "root//platforms:platform_a",
            "--local-only",
            "--no-remote-cache", // Turn off remote cache query so we execute locally
            "--show-output",
        ])
        .await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL], &WRITE_KINDS).await?;

    let result_b = buck
        .build(&[
            "app:simple_dep_file",
            "--target-platforms",
            "root//platforms:platform_b",
            "--local-only",
            "--show-output",
        ])
        .await?;
    check_no_cache_query(buck).await?;
    check_execution_kind(
        buck,
        &[ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE],
        &WRITE_KINDS,
    )
    .await?;
    check_match_dep_files_events(
        buck,
        &[MatchDepFilesEvent {
            remote_cache: false,
            checking_filtered_inputs: false,
        }],
    )
    .await?;

    fn single_output(result: &BuckOutput) -> anyhow::Result<String> {
        let outputs = result.target_to_build_output()?;
        assert_eq!(outputs.len(), 1, "{outputs:?}");
        outputs.into_values().next().context("No output")
    }

    async fn platform_config_hash(buck: &Buck, platform: &str) -> anyhow::Result<String> {
        // cquery keys its json output by the configured target label, which ends in the
        // configuration hash, e.g.
        // "root//app:simple_dep_file (root//platforms:platform_a#5baf920a753b3a79)".
        let out = buck
            .cquery(&[
                "app:simple_dep_file",
                "--target-platforms",
                platform,
                "--output-attribute=name",
            ])
            .await?
            .stdout;
        let json: serde_json::Map<String, Value> = serde_json::from_str(&out)?;
        let keys: Vec<&String> = json.keys().collect();
        assert_eq!(keys.len(), 1, "{keys:?}");
        let label = keys[0];
        let (_, hash) = label
            .split_once('#')
            .with_context(|| format!("no configuration hash in cquery key: {label}"))?;
        Ok(hash
            .split_once(')')
            .map_or(hash, |(hash, _)| hash)
            .to_owned())
    }

    let reported_a = single_output(&result_a)?;
    let reported_b = single_output(&result_b)?;

    let hash_a = platform_config_hash(buck, "root//platforms:platform_a").await?;
    let hash_b = platform_config_hash(buck, "root//platforms:platform_b").await?;
    assert_ne!(
        hash_a, hash_b,
        "platforms should have distinct config hashes: {hash_a}"
    );
    assert!(reported_a.contains(&hash_a), "{hash_a} not in {reported_a}");
    assert!(reported_b.contains(&hash_b), "{hash_b} not in {reported_b}");
    assert_eq!(
        reported_a.replace(&hash_a, &hash_b),
        reported_b,
        "outputs paths should only differ in config hashes {reported_a} vs {reported_b}"
    );

    // Each configuration-hash path is a symlink into the deduplicated, configuration-independent
    // content-based location, so both resolve to the exact same file.
    let path_a = std::fs::canonicalize(buck.cwd().join(&reported_a))?;
    let path_b = std::fs::canonicalize(buck.cwd().join(&reported_b))?;
    assert_eq!(
        path_a,
        path_b,
        "content-based output should resolve to one file: {} vs {}",
        path_a.display(),
        path_b.display()
    );
    // ...and the file the cache hit points at must exist with the correct content.
    assert!(
        path_b.exists(),
        "cross-config cache hit output is not materialized: {}",
        path_b.display()
    );
    assert_eq!(read_text(&path_b)?, "output");
    Ok(())
}

#[buck_test(setup_eden = false, data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_no_cross_config_hit_without_content_based_paths(buck: &Buck) -> anyhow::Result<()> {
    // The mirror of `test_dep_file_hit_across_configurations`: with non-content-based paths the
    // action is not configuration-independent, so building it under a second configuration must
    // execute rather than reuse the first configuration's entry.
    //
    // This pins the precondition that `hit_outputs_if_present` relies on. It verifies the outputs of
    // the *live* configuration are materialized rather than the candidate's, and those resolve to
    // the same path on disk only when the outputs are content-based. A non-content-based action
    // never reaches that check cross-configuration today, because its output paths are part of the
    // command line digest and so differ per configuration -- this test is what keeps that true.
    for platform in ["platform_a", "platform_b"] {
        buck.build(&[
            "app:simple_dep_file",
            "--target-platforms",
            &format!("root//platforms:{platform}"),
            "-c",
            "test.use_content_based_paths=false",
            "--local-only",
            "--no-remote-cache",
            "--show-output",
        ])
        .await?;
        check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL], &WRITE_KINDS).await?;
    }
    Ok(())
}

#[buck_test(setup_eden = false, data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_select_divergent_actions_do_not_thrash_across_configurations(
    buck: &Buck,
) -> anyhow::Result<()> {
    // `app:select_divergent`'s action command line differs per configuration via select(), so its two
    // configurations are genuinely different actions. The local action cache
    // keeps a per-configuration entry, so building one configuration must not evict
    // another's entry: each configuration still gets its own incremental local action cache hit.

    // Build under platform_a -> executes and caches platform_a's entry.
    buck.build(&[
        "app:select_divergent",
        "--target-platforms",
        "root//platforms:platform_a",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.dummy_config=d1",
    ])
    .await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL], &WRITE_KINDS).await?;

    // Build under platform_b -> a different action (different command line). It must execute (no
    // cross-config hit, since the identity differs) and must not evict platform_a's entry.
    buck.build(&[
        "app:select_divergent",
        "--target-platforms",
        "root//platforms:platform_b",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.dummy_config=d1",
    ])
    .await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL], &WRITE_KINDS).await?;

    // Rebuild under platform_a with a no-op config change to force a DICE recompute. platform_a's
    // entry survived the platform_b build, so this is served by the local action cache.
    buck.build(&[
        "app:select_divergent",
        "--target-platforms",
        "root//platforms:platform_a",
        "--local-only",
        "-c",
        "test.dummy_config=d2",
    ])
    .await?;
    check_execution_kind(
        buck,
        &[ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE],
        &WRITE_KINDS,
    )
    .await?;
    Ok(())
}

// Changing ActionKey (by registering additional actions before the dep-file action
// during analysis) does not cause a dep-file cache miss when the dep-file action itself is
// identical -- the dep-file/output comparison is configuration- and ActionKey-independent.
#[buck_test(setup_eden = false, data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_dep_file_hit_with_action_key_change(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "app:dep_file_with_preceding_actions",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.num_preceding_actions=0",
    ])
    .await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL], &WRITE_KINDS).await?;

    // Add a preceding action, shifting the dep-file action's ActionKey index.
    // The dep-file action itself (command, inputs, outputs) is identical.
    buck.build(&[
        "app:dep_file_with_preceding_actions",
        "--local-only",
        "--no-remote-cache",
        "-c",
        "test.num_preceding_actions=1",
    ])
    .await?;
    check_execution_kind(
        buck,
        &[ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE],
        &WRITE_KINDS,
    )
    .await?;
    Ok(())
}

// Flaky because of watchman on mac (and maybe windows)
// Skipping on windows due to gcc dependency
// This test tombstones the hash of the dep file produced by this action.
#[buck_test(
    data_dir = "dep_files",
    skip_for_os = ["darwin", "windows"],
    env = [(
        "BUCK2_TEST_TOMBSTONED_DIGESTS",
        "e537c6611d7e2ba1c9b71248f7a0ca506e5a0f9a:78",
    )],
)]
async fn test_dep_files_ignore_missing_digests(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.build(&["app:app"]).await?;

    let dep_file_path = tmp_path.join("dep_file");
    let materialized = buck
        .build(&[
            "app:app[dep_file]",
            &format!("--out={}", dep_file_path.display()),
        ])
        .await;
    if materialized.is_ok() {
        // If we get here, that means materialization did not fail.
        let dep_file = std::fs::read(&dep_file_path)?;
        anyhow::bail!(
            "Misconfigured test, BUCK2_TEST_TOMBSTONED_DIGESTS to {}:{}",
            sha1_hex(&dep_file),
            dep_file.len()
        );
    }

    touch(buck, "app/other.h")?;
    buck.build(&["app:app"]).await?;

    expect_exec_count(buck, 1).await?;
    Ok(())
}

#[buck_test(data_dir = "invalid_dep_files")]
async fn test_invalid_dep_files(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:lazy"]).await?;
    // Disable remote cache lookup so we actually check for local dep files
    buck.build(&["//:lazy", "-c", "test.seed=123", "--no-remote-cache"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid line encountered in dep file");

    buck.debug(&["flush-dep-files"]).await?;
    buck.build(&["//:lazy"]).await?;

    // Disable remote cache lookup so we actually check for local dep files
    buck.build(&["//:eager", "--eager-dep-files", "--no-remote-cache"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid line encountered in dep file");
    Ok(())
}

#[buck_test(data_dir = "mismatched_outputs_dep_files")]
async fn test_mismatched_outputs_dep_files(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:test", "-c", "test.prefix=foo", "-c", "test.suffix=bar"])
        .await?;
    // Different output now, even though the command has not changed.
    buck.build(&["//:test", "-c", "test.prefix=foo/bar", "-c", "test.suffix="])
        .await?;
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    env = [
        ("BUCK_LOG", "buck2_execute_impl::executors::caching=debug"),
        ("BUCK2_TEST_SKIP_ACTION_CACHE_WRITE", "true"),
    ],
)]
async fn test_re_dep_file_uploads_same_key(buck: &Buck) -> anyhow::Result<()> {
    // Test all the cases where the remote dep file key should stay the same
    let target = "root//:dep_files";
    let tagged_used_file1 = buck.cwd().join("used.1"); // Used for depfile 0
    let tagged_used_file3 = buck.cwd().join("used.3"); // Used for depfile 1
    assert!(tagged_used_file1.exists());
    assert!(tagged_used_file3.exists());

    let cache_buster = format!("test.cache_buster={}", random_string());
    let target = [
        target,
        "-c",
        "test.allow_dep_file_cache_upload=true",
        "-c",
        "test.allow_cache_upload=true",
        "-c",
        &cache_buster,
        "--local-only",
    ];

    // Check that building this target results in a dep file cache upload
    buck.build(&target).await?;

    let key = dep_file_key_from_executions(buck).await?;
    check_uploaded_dep_file_key(buck, &key).await?;

    // Changing a tagged (associated with a dep file) input should not change the key
    // The remote dep file key only tracks the untagged inputs. The dep file cache is for checking whether
    // the output is the same despite a tagged file changing.
    std::fs::write(&tagged_used_file1, "CHANGE")?;
    std::fs::write(&tagged_used_file3, "CHANGE")?;
    buck.build(&target).await?;
    let key_tagged_input_change = dep_file_key_from_executions(buck).await?;
    check_uploaded_dep_file_key(buck, &key_tagged_input_change).await?;
    assert_eq!(key, key_tagged_input_change);
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    env = [
        ("BUCK_LOG", "buck2_execute_impl::executors::caching=debug"),
        ("BUCK2_TEST_SKIP_ACTION_CACHE_WRITE", "true"),
    ],
)]
async fn test_re_dep_file_uploads_different_key(buck: &Buck) -> anyhow::Result<()> {
    // TODO: Mergebase is currently not set in this test.
    // Include it so we can test for the case where the mergebase differs

    let mut keys_seen = Vec::new();
    let target = "root//:dep_files";
    let untagged_file1 = buck.cwd().join("untagged.1");
    assert!(untagged_file1.exists());
    let targets_file = buck.cwd().join("TARGETS.fixture");
    assert!(targets_file.exists());

    let cache_buster = format!("test.cache_buster={}", random_string());
    let target = [
        target,
        "-c",
        "test.allow_dep_file_cache_upload=true",
        "-c",
        "test.allow_cache_upload=true",
        "-c",
        &cache_buster,
        "--local-only",
    ];

    // Check that building this target results in a dep file cache upload
    buck.build(&target).await?;
    let key = dep_file_key_from_executions(buck).await?;
    check_uploaded_dep_file_key(buck, &key).await?;
    keys_seen.push(key);

    // Modify the depfile name and check the new key is different
    std::fs::write(
        &targets_file,
        read_text(&targets_file)?.replace("\"dep_file_name1\",", "\"dep_file_name1_modified\","),
    )?;
    buck.build(&target).await?;

    let key_different_depfile_name = dep_file_key_from_executions(buck).await?;
    check_uploaded_dep_file_key(buck, &key_different_depfile_name).await?;
    assert!(!keys_seen.contains(&key_different_depfile_name));
    keys_seen.push(key_different_depfile_name);

    // Modify the output name and check the new key is different
    std::fs::write(
        &targets_file,
        read_text(&targets_file)?.replace(
            "out_name = \"dep_files_out\"",
            "out_name = \"dep_files_out_changed\"",
        ),
    )?;
    buck.build(&target).await?;
    let key_different_out_name = dep_file_key_from_executions(buck).await?;
    check_uploaded_dep_file_key(buck, &key_different_out_name).await?;
    assert!(!keys_seen.contains(&key_different_out_name));
    keys_seen.push(key_different_out_name);

    // Modify an untagged input and check the new key is different
    std::fs::write(&untagged_file1, "CHANGE")?;
    buck.build(&target).await?;
    let key_untagged_input_change = dep_file_key_from_executions(buck).await?;
    check_uploaded_dep_file_key(buck, &key_untagged_input_change).await?;
    assert!(!keys_seen.contains(&key_untagged_input_change));
    keys_seen.push(key_untagged_input_change);
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    env = [
        ("BUCK_LOG", "buck2_execute_impl::executors::caching=debug"),
        ("BUCK2_TEST_SKIP_ACTION_CACHE_WRITE", "true"),
    ],
)]
async fn test_local_dep_file_upload_follows_cache_upload_permission(
    buck: &Buck,
) -> anyhow::Result<()> {
    async fn build(buck: &Buck, configs: &[&str]) -> anyhow::Result<Value> {
        let cache_buster = format!("test.cache_buster={}", random_string());
        let mut args = vec!["root//:dep_files", "--local-only", "-c", &cache_buster];
        for config in configs {
            args.extend(["-c", config]);
        }
        buck.build(&args).await?;
        let mut execs = action_executions(buck).await?;
        assert_eq!(execs.len(), 1, "{execs:#?}");
        Ok(execs.remove(0))
    }

    // A dep-file entry over locally produced results would publish them, so it
    // needs the action's own upload permission.
    let execution = build(buck, &["test.allow_dep_file_cache_upload=true"]).await?;
    assert_eq!(dep_file_uploads(buck).await?.len(), 0);
    assert_eq!(
        execution["cache_upload_result"],
        UPLOAD_RESULT_ACTION_UPLOAD_NOT_ALLOWED
    );
    assert_eq!(
        execution["dep_file_cache_upload_result"],
        UPLOAD_RESULT_ACTION_UPLOAD_NOT_ALLOWED
    );

    // The two permissions are reported apart.
    let execution = build(buck, &["test.allow_cache_upload=true"]).await?;
    assert_eq!(
        execution["dep_file_cache_upload_result"],
        UPLOAD_RESULT_DEP_FILE_UPLOAD_NOT_ALLOWED
    );

    let execution = build(buck, &[]).await?;
    assert_eq!(
        execution["cache_upload_result"],
        UPLOAD_RESULT_ACTION_UPLOAD_NOT_ALLOWED
    );
    assert_eq!(
        execution["dep_file_cache_upload_result"],
        UPLOAD_RESULT_DEP_FILE_UPLOAD_NOT_ALLOWED
    );
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    env = [
        ("BUCK_LOG", "buck2_execute_impl::executors::caching=debug"),
        ("BUCK2_TEST_SKIP_ACTION_CACHE_WRITE", "true"),
    ],
)]
async fn test_dep_file_does_not_upload_when_allow_cache_upload_is_true(
    buck: &Buck,
) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let target = [
        "root//:dep_files",
        "-c",
        "test.allow_dep_file_cache_upload=false",
        "-c",
        "test.allow_cache_upload=true",
        "-c",
        &cache_buster,
        "--remote-only",
    ];

    // Check that we don't do a dep file cache upload when allow_dep_file_cache_upload is false,
    // even though allow_cache_upload is true
    buck.build(&target).await?;
    let uploads = dep_file_uploads(buck).await?;
    assert_eq!(uploads.len(), 0, "{uploads:#?}");
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    env = [
        ("BUCK_LOG", "buck2_execute_impl::executors::caching=debug"),
        ("BUCK2_TEST_SKIP_ACTION_CACHE_WRITE", "true"),
        ("BUCK2_TEST_ONLY_REMOTE_DEP_FILE_CACHE", "true"),
    ],
)]
async fn test_only_do_cache_lookup_when_dep_file_upload_is_enabled(
    buck: &Buck,
) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let target = [
        "root//:dep_files",
        "-c",
        "test.allow_dep_file_cache_upload=false",
        "-c",
        "test.allow_cache_upload=true",
        "-c",
        &cache_buster,
        "--remote-only",
    ];

    // Check that we don't do a dep file cache lookup when allow_dep_file_cache_upload is false
    buck.build(&target).await?;
    check_no_cache_query(buck).await?;

    let cache_buster = format!("test.cache_buster={}", random_string());
    let target = [
        "root//:dep_files",
        "-c",
        "test.allow_dep_file_cache_upload=true",
        "-c",
        "test.allow_cache_upload=true",
        "-c",
        &cache_buster,
        "--remote-only",
    ];

    // Check that we do a dep file cache lookup when allow_dep_file_cache_upload is true
    buck.build(&target).await?;
    check_cache_query(buck).await?;
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    env = [
        ("BUCK_LOG", "buck2_execute_impl::executors::caching=debug"),
        ("BUCK2_TEST_SKIP_ACTION_CACHE_WRITE", "true"),
    ],
)]
async fn test_re_dep_file_remote_upload(buck: &Buck) -> anyhow::Result<()> {
    let cache_buster = format!("test.cache_buster={}", random_string());
    let target = [
        "root//:dep_files",
        "-c",
        "test.allow_dep_file_cache_upload=true",
        "-c",
        &cache_buster,
        "--remote-only",
    ];

    // Check that building on RE results in a dep file cache upload
    buck.build(&target).await?;
    let key = dep_file_key_from_executions(buck).await?;
    check_uploaded_dep_file_key(buck, &key).await?;
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    write_invocation_record = true,
    env = [
        (
            "BUCK_LOG",
            "buck2_action_impl=debug,buck2_execute_impl::executors::caching=debug",
        ),
        ("BUCK2_TEST_SKIP_ACTION_CACHE_WRITE", "true"),
    ],
)]
async fn test_re_dep_file_cache_hit_upload(buck: &Buck) -> anyhow::Result<()> {
    // The upload is skipped when the dep file entry already exists, so the key must be fresh.
    let cache_buster = format!("test.cache_buster={}", random_string());
    let target = [
        "root//:dep_files",
        "--remote-only",
        "-c",
        &cache_buster,
        "-c",
        // Ensure we don't get a dep file cache hit
        "test.remote_dep_file_cache_enabled=false",
    ];

    // Build on RE to make sure action cache is populated
    buck.build(&target).await?;
    buck.kill().await?;

    // Check for action cache hit and dep file cache upload
    let res = buck
        .build(
            &[
                &target[..],
                &["-c", "test.allow_dep_file_cache_upload=true"],
            ]
            .concat(),
        )
        .await?;
    let what_ran = read_what_ran(buck, &[]).await?;
    assert_eq!(
        what_ran[0]["reproducer"]["executor"], "Cache",
        "{what_ran:#?}"
    );
    assert_eq!(what_ran.len(), 1, "{what_ran:#?}");
    let key = dep_file_key_from_executions(buck).await?;
    check_uploaded_dep_file_key(buck, &key).await?;

    let invocation_record = res.invocation_record()?;

    assert_eq!(invocation_record["dep_file_upload_count"], 1);
    assert_eq!(
        invocation_record["dep_file_upload_count"],
        invocation_record["dep_file_upload_attempt_count"]
    );

    // Simulate 'user' build, with action cache hit from previous build and dep file cache checking enabled.
    buck.clean(&[]).await?;
    buck.build(&[
        "root//:dep_files",
        "--remote-only",
        "-c",
        &cache_buster,
        "-c",
        "test.remote_dep_file_cache_enabled=true",
        "-c",
        "test.allow_dep_file_cache_upload=false",
    ])
    .await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_ACTION_CACHE], &[]).await?;
    let uploads = dep_file_uploads(buck).await?;
    // Ensure no dep file uploads are attempted for cache hits with dep file cache checking enabled, but dep file uploads disabled.
    assert_eq!(uploads.len(), 0, "{uploads:#?}");
    Ok(())
}

#[buck_test(data_dir = "upload_dep_files")]
async fn test_re_dep_file_uploads_failed_action(buck: &Buck) -> anyhow::Result<()> {
    // If the action failed, we should not attempt to upload to cache even if it's configured to
    let target = [
        "root//:dep_files_fail",
        "-c",
        "test.allow_dep_file_cache_upload=true",
    ];
    buck.build(&[&target[..], &["--no-remote-cache", "--local-only"]].concat())
        .await
        .expect_failure()
        .assert_stderr_matches("Failing on purpose");
    // Assert cache upload was not attempted
    let what_ran = read_what_ran(buck, &["--emit-cache-queries"]).await?;
    for what in &what_ran {
        assert_ne!(
            what.pointer("/reproducer/executor")
                .unwrap_or_else(|| panic!("No `reproducer.executor` in {what:#?}")),
            "CacheQuery",
            "{what:#?}"
        );
    }
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    env = [
        (
            "BUCK_LOG",
            "buck2_execute_impl::executors::caching=debug,buck2_execute_impl::executors::action_cache=debug,buck2_action_impl=debug",
        ),
        // Disable the regular action cache query so that we actually hit the remote dep file cache query.
        ("BUCK2_TEST_ONLY_REMOTE_DEP_FILE_CACHE", "true"),
    ],
)]
async fn test_re_dep_file_query_change_tagged_unused_file(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:dep_files";
    // Tagged for depfile0, and exists in depfile0
    let tagged_used_file1 = buck.cwd().join("used.1");
    // Tagged for depfile0, but does NOT exist in depfile0
    let tagged_unused = buck.cwd().join("unused.1");
    assert!(tagged_used_file1.exists());
    assert!(tagged_unused.exists());

    let target_upload_enabled = [
        target,
        "-c",
        "test.allow_dep_file_cache_upload=true",
        "-c",
        "test.cache_buster=tagged_unused_file_test",
        "--local-only",
    ];

    let target_upload_enabled_with_action_definition_change = [
        &target_upload_enabled[..],
        &["-c", "test.allow_cache_upload=true"],
    ]
    .concat();

    // Build it once with cache upload (cache upload will fail locally)
    let result = buck
        .build(&[&target_upload_enabled[..], &["--no-remote-cache"]].concat())
        .await?;
    let output = read_text(result.build_report()?.output_for_target(target)?)?;
    assert_eq!(output, "used1\nused2\nused3\n");

    // Build the target again. This will either result in one of
    // 1. A remote dep file cache hit and a subsequent dep file validation
    // 2. A remote dep file cache miss, fall back to local execution (local dep file cache is
    //    flushed) This can occur if the action definition changes, because the new remote dep file
    //    can only be uploaded by a job with the correct permissions, so it will run locally until
    //    that takes place.
    buck.debug(&["flush-dep-files"]).await?;
    let result = buck
        .build(&target_upload_enabled_with_action_definition_change)
        .await?;
    let output = read_text(result.build_report()?.output_for_target(target)?)?;
    assert_eq!(output, "used1\nused2\nused3\n");

    check_remote_dep_file_cache_query_took_place(buck).await?;
    let execution_kind = get_execution_kind(buck).await?;
    let was_cache_hit = result.stderr.contains("Cache hits: 100%");
    assert!(
        (was_cache_hit && execution_kind == ACTION_EXECUTION_KIND_REMOTE_DEP_FILE_CACHE)
            || (!was_cache_hit && execution_kind == ACTION_EXECUTION_KIND_LOCAL),
        "was_cache_hit: {was_cache_hit}, execution_kind: {execution_kind}"
    );
    let mut expected_dep_file_match_events = vec![
        // Initial local dep file cache lookup for an identical action
        MatchDepFilesEvent {
            remote_cache: false,
            checking_filtered_inputs: false,
        },
    ];

    if execution_kind == ACTION_EXECUTION_KIND_REMOTE_DEP_FILE_CACHE {
        // Remote dep file cache hit verification
        expected_dep_file_match_events.push(MatchDepFilesEvent {
            remote_cache: true,
            checking_filtered_inputs: true,
        });
    }

    // Check the MatchDepFiles events
    check_match_dep_files_events(buck, &expected_dep_file_match_events).await?;

    // # Change a file that is tracked by a dep file but shows up as unused, we get a local dep file cache hit
    // # as that is checked first.
    std::fs::write(&tagged_unused, random_string())?;
    let result = buck.build(&target_upload_enabled).await?;
    let output = read_text(result.build_report()?.output_for_target(target)?)?;
    assert_eq!(output, "used1\nused2\nused3\n");

    let execution_kind = get_execution_kind(buck).await?;
    assert_eq!(execution_kind, ACTION_EXECUTION_KIND_LOCAL_DEP_FILE);

    // Change a file that is tracked by a dep file but shows up as unused, this will again result in one of
    // 1. A remote dep file cache hit and a subsequent dep file validation
    // 2. A remote dep file cache miss, fall back to local execution (local dep file cache is flushed)
    buck.debug(&["flush-dep-files"]).await?;
    std::fs::write(&tagged_unused, random_string())?;
    let result = buck.build(&target_upload_enabled).await?;
    let output = read_text(result.build_report()?.output_for_target(target)?)?;
    assert_eq!(output, "used1\nused2\nused3\n");

    check_remote_dep_file_cache_query_took_place(buck).await?;
    let execution_kind = get_execution_kind(buck).await?;
    let was_cache_hit = result.stderr.contains("Cache hits: 100%");
    assert!(
        (was_cache_hit && execution_kind == ACTION_EXECUTION_KIND_REMOTE_DEP_FILE_CACHE)
            || (!was_cache_hit && execution_kind == ACTION_EXECUTION_KIND_LOCAL),
        "was_cache_hit: {was_cache_hit}, execution_kind: {execution_kind}"
    );

    // Check the MatchDepFiles events
    check_match_dep_files_events(buck, &expected_dep_file_match_events).await?;
    Ok(())
}

#[buck_test(
    data_dir = "upload_dep_files",
    env = [
        (
            "BUCK_LOG",
            "buck2_execute_impl::executors::caching=debug,buck2_execute_impl::executors::action_cache=debug,buck2_action_impl=debug",
        ),
        // Disable the regular action cache query so that we actually hit the remote dep file cache query.
        ("BUCK2_TEST_ONLY_REMOTE_DEP_FILE_CACHE", "true"),
    ],
)]
async fn test_re_dep_file_query_change_tagged_used_file(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:dep_files";
    // Tagged for depfile0, and exists in depfile0
    let tagged_used_file1 = buck.cwd().join("used.1");
    // Tagged for depfile0, but does NOT exist in depfile0
    let tagged_unused = buck.cwd().join("unused.1");
    assert!(tagged_used_file1.exists());
    assert!(tagged_unused.exists());

    let target_upload_enabled = [
        target,
        "-c",
        "test.allow_dep_file_cache_upload=true",
        "--local-only",
    ];

    // Build it once with cache upload (cache upload will fail locally)
    let result = buck
        .build(&[&target_upload_enabled[..], &["--no-remote-cache"]].concat())
        .await?;
    let output = read_text(result.build_report()?.output_for_target(target)?)?;
    assert_eq!(output, "used1\nused2\nused3\n");

    // Change a file that is tracked by a dep file and shows up as used (ends up listed in the dep file).
    // Build the target again. This will either result in one of
    // 1. A remote dep file cache hit and a subsequent dep file validation (which fails)
    // 2. A remote dep file cache miss, fall back to local execution (local dep file cache is flushed)
    // Either way, it should be executed locally
    buck.debug(&["flush-dep-files"]).await?;
    let used1_modified_str = format!("used1({})", random_string());
    std::fs::write(&tagged_used_file1, format!("{used1_modified_str}\n"))?;
    let result = buck.build(&target_upload_enabled).await?;
    check_remote_dep_file_cache_query_took_place(buck).await?;
    check_execution_kind(buck, &[ACTION_EXECUTION_KIND_LOCAL], &[]).await?;
    let output = read_text(result.build_report()?.output_for_target(target)?)?;
    assert_eq!(output, format!("{used1_modified_str}\nused2\nused3\n"));
    Ok(())
}

// Flaky because of watchman on mac (and maybe windows)
// Skipping on windows due to gcc dependency
#[buck_test(data_dir = "dep_files", skip_for_os = ["darwin", "windows"])]
async fn test_flush_dep_files(buck: &Buck) -> anyhow::Result<()> {
    // Make sure that we build locally
    let args = ["app:app", "--no-remote-cache", "--local-only"];
    buck.build(&args).await?;
    expect_exec_count(buck, 1).await?;

    buck.debug(&["flush-dep-files", "--retain-local"]).await?;

    // //app:app doesn't use other.h and
    // dep file should still be present
    // since we retained local dep files
    touch(buck, "app/other.h")?;
    buck.build(&args).await?;
    expect_only_dep_file_hit(buck).await?;

    buck.debug(&["flush-dep-files"]).await?;

    // all dep files are gone, so we have
    // to rebuild.
    touch(buck, "app/other.h")?;
    buck.build(&args).await?;
    expect_exec_count(buck, 1).await?;
    Ok(())
}

#[buck_test(data_dir = "upload_dep_files", allow_soft_errors = false)]
async fn test_input_cannot_be_normalized_and_hard_error(buck: &Buck) -> anyhow::Result<()> {
    run_test_input_cannot_be_normalized(buck, false).await
}

#[buck_test(data_dir = "upload_dep_files", allow_soft_errors = true)]
async fn test_input_cannot_be_normalized(buck: &Buck) -> anyhow::Result<()> {
    run_test_input_cannot_be_normalized(buck, true).await
}

#[buck_test(data_dir = "invalid_dep_files")]
async fn test_two_outputs_tagged_as_dep_file(buck: &Buck) {
    buck.build(&["root//:two_outputs_tagged_as_dep_file"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "`dep_files` value with key `deps` has an invalid count of associated outputs. Expected 1, got 2",
        );
}

#[buck_test(data_dir = "invalid_dep_files")]
async fn test_no_outputs_tagged_as_dep_file(buck: &Buck) {
    buck.build(&["root//:no_outputs_tagged_as_dep_file"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "`dep_files` value with key `deps` has an invalid count of associated outputs. Expected 1, got 0",
        );
}

#[buck_test(data_dir = "invalid_dep_files")]
async fn test_same_tag_for_multiple_labels(buck: &Buck) {
    buck.build(&["root//:same_tag_for_multiple_labels"])
        .await
        .expect_failure()
        .assert_stderr_matches("`dep_files` with keys `deps` and `deps2` are using the same tag");
}

#[buck_test(data_dir = "invalid_dep_files")]
async fn test_input_tagged_multiple_times(buck: &Buck) {
    buck.build(&["root//:input_tagged_multiple_times"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Dep-files input.*input_tagged_multiple_times.txt.*is tagged with multiple tags relevant for dep-files: `deps1` and `deps2`",
        );
}

async fn test_canonical_input_dep_file(
    buck: &Buck,
    content_paths: &str,
    input_format: &str,
    placement: &str,
) -> anyhow::Result<()> {
    let mut options = Options::new(&[
        ("use_content_based_paths", content_paths),
        ("canonical_placement", placement),
        ("canonical_format", input_format),
    ]);
    let (output, original_json) = canonical_input_output(
        &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?,
    )?;
    assert_eq!(output, "optionusedextra");

    options.set("canonical_unused", "unused-changed");
    let (output, changed_json) = canonical_input_output(
        &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL_DEP_FILE).await?,
    )?;
    assert_eq!(output, "optionusedextra");
    assert_eq!(original_json != changed_json, content_paths == "true");

    for (key, value, expected) in [
        ("canonical_used", "used-changed", "optionused-changedextra"),
        (
            "canonical_untagged",
            "extra-changed",
            "optionused-changedextra-changed",
        ),
        (
            "canonical_option",
            "new-option",
            "new-optionused-changedextra-changed",
        ),
        (
            "canonical_reverse",
            "true",
            "new-optionunused-changedextra-changed",
        ),
        (
            "canonical_repeat",
            "true",
            "new-optionunused-changedextra-changed",
        ),
    ] {
        options.set(key, value);
        let (output, _) = canonical_input_output(
            &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?,
        )?;
        assert_eq!(output, expected);
    }
    Ok(())
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_json_direct_true(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "true", "json", "direct").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_json_direct_false(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "false", "json", "direct").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_json_without_inputs_true(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "true", "json", "without_inputs").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_json_without_inputs_false(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "false", "json", "without_inputs").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_args_direct_true(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "true", "args", "direct").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_args_direct_false(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "false", "args", "direct").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_args_no_allow_args_true(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "true", "args", "no_allow_args").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_args_no_allow_args_false(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "false", "args", "no_allow_args").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_args_nested_true(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "true", "args", "nested").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_args_nested_false(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "false", "args", "nested").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_args_without_inputs_true(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "true", "args", "without_inputs").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_dep_file_args_without_inputs_false(
    buck: &Buck,
) -> anyhow::Result<()> {
    test_canonical_input_dep_file(buck, "false", "args", "without_inputs").await
}

async fn test_canonical_args_executable(buck: &Buck, content_paths: &str) -> anyhow::Result<()> {
    let mut options = Options::new(&[
        ("canonical_format", "args"),
        ("canonical_option", "executable-bit"),
        ("use_content_based_paths", content_paths),
    ]);
    for executable in ["false", "true", "false"] {
        options.set("canonical_executable", executable);
        let (output, _) = canonical_input_output(
            &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?,
        )?;
        // Python's `str` of a bool.
        assert_eq!(
            output,
            if executable == "true" {
                "True"
            } else {
                "False"
            }
        );
    }
    Ok(())
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_args_executable_true(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_args_executable(buck, "true").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_args_executable_false(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_args_executable(buck, "false").await
}

async fn test_canonical_input_absolute_pretty(
    buck: &Buck,
    input_format: &str,
) -> anyhow::Result<()> {
    let mut options = Options::new(&[
        ("canonical_absolute", "true"),
        ("canonical_pretty", "true"),
        ("canonical_format", input_format),
    ]);
    let (output, original_json) = canonical_input_output(
        &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?,
    )?;
    assert_eq!(output, "optionusedextra");
    if input_format == "json" {
        let json: Value = serde_json::from_str(&original_json)?;
        let input = json["inputs"][0]
            .as_str()
            .with_context(|| format!("No string `inputs[0]` in {original_json}"))?;
        assert!(Path::new(input).is_absolute(), "{original_json}");
        assert!(original_json.contains('\n'), "{original_json}");
    } else {
        let args = shlex::split(&original_json)
            .with_context(|| format!("Invalid shell quoting: {original_json}"))?;
        assert!(Path::new(&args[4]).is_absolute(), "{original_json}");
        assert!(!original_json.contains('\n'), "{original_json}");
    }
    options.set("canonical_unused", "changed");
    let (output, changed_json) = canonical_input_output(
        &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL_DEP_FILE).await?,
    )?;
    assert_eq!(output, "optionusedextra");
    assert_ne!(original_json, changed_json);
    Ok(())
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_absolute_pretty_json(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_absolute_pretty(buck, "json").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_absolute_pretty_args(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_absolute_pretty(buck, "args").await
}

async fn test_canonical_input_ordinary_occurrence(
    buck: &Buck,
    input_format: &str,
) -> anyhow::Result<()> {
    let mut options = Options::new(&[
        ("canonical_placement", "ordinary"),
        ("canonical_format", input_format),
    ]);
    build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?;
    options.set("canonical_unused", "changed");
    // An ordinary occurrence of the same file still requires its physical digest to match.
    let (output, _) = canonical_input_output(
        &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?,
    )?;
    assert_eq!(output, "optionusedextra");
    Ok(())
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_ordinary_occurrence_json(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_ordinary_occurrence(buck, "json").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_ordinary_occurrence_args(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_ordinary_occurrence(buck, "args").await
}

const WITH_INPUTS_ERROR: &str = "`with_inputs = True` cannot be combined with `dep_files_fingerprint_using_canonical_paths = True`: the fingerprint already forwards inputs with their original tags";

async fn test_canonical_input_invalid_placement(
    buck: &Buck,
    placement: &str,
    error: &str,
    input_format: &str,
) {
    buck.build(&[
        "root//app:canonical_json",
        "-c",
        &format!("test.canonical_placement={placement}"),
        "-c",
        &format!("test.canonical_format={input_format}"),
        "-c",
        "test.use_content_based_paths=false",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(error);
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_canonical_input_invalid_placement_json_with_inputs_with_inputs_True_cannot_be_combined_with_dep_files_fingerprint_using_canonical_paths_True_the_fingerprint_already_forwards_inputs_with_their_original_tags(
    buck: &Buck,
) {
    test_canonical_input_invalid_placement(buck, "with_inputs", WITH_INPUTS_ERROR, "json").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_invalid_placement_json_placeholder_cannot_be_combined_with_placeholder_output(
    buck: &Buck,
) {
    test_canonical_input_invalid_placement(
        buck,
        "placeholder",
        "cannot be combined with placeholder output",
        "json",
    )
    .await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_canonical_input_invalid_placement_json_command_line_DepFileFingerprint(buck: &Buck) {
    test_canonical_input_invalid_placement(buck, "command_line", "DepFileFingerprint", "json").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_canonical_input_invalid_placement_json_invalid_descriptor_DepFileFingerprint(
    buck: &Buck,
) {
    test_canonical_input_invalid_placement(buck, "invalid_descriptor", "DepFileFingerprint", "json")
        .await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_canonical_input_invalid_placement_args_with_inputs_with_inputs_True_cannot_be_combined_with_dep_files_fingerprint_using_canonical_paths_True_the_fingerprint_already_forwards_inputs_with_their_original_tags(
    buck: &Buck,
) {
    test_canonical_input_invalid_placement(buck, "with_inputs", WITH_INPUTS_ERROR, "args").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_input_invalid_placement_args_placeholder_cannot_be_combined_with_placeholder_output(
    buck: &Buck,
) {
    test_canonical_input_invalid_placement(
        buck,
        "placeholder",
        "cannot be combined with placeholder output",
        "args",
    )
    .await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_canonical_input_invalid_placement_args_command_line_DepFileFingerprint(buck: &Buck) {
    test_canonical_input_invalid_placement(buck, "command_line", "DepFileFingerprint", "args").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
#[expect(
    non_snake_case,
    reason = "named like the Python harness's parametrized case"
)]
async fn test_canonical_input_invalid_placement_args_invalid_descriptor_DepFileFingerprint(
    buck: &Buck,
) {
    test_canonical_input_invalid_placement(buck, "invalid_descriptor", "DepFileFingerprint", "args")
        .await
}

async fn test_canonical_input_persisted_dep_file(
    buck: &Buck,
    input_format: &str,
) -> anyhow::Result<()> {
    build_canonical_input(
        buck,
        &Options::new(&[("canonical_format", input_format)]),
        ACTION_EXECUTION_KIND_LOCAL,
    )
    .await?;
    buck.kill().await?;
    build_canonical_input(
        buck,
        &Options::new(&[("canonical_format", input_format)]),
        ACTION_EXECUTION_KIND_LOCAL_ACTION_CACHE,
    )
    .await?;
    // Reloaded entries have no live filtered-input signatures. The first changed build
    // executes; subsequent unused changes can use the new live dep-file entry.
    build_canonical_input(
        buck,
        &Options::new(&[
            ("canonical_format", input_format),
            ("canonical_unused", "changed"),
        ]),
        ACTION_EXECUTION_KIND_LOCAL,
    )
    .await?;
    let (output, _) = canonical_input_output(
        &build_canonical_input(
            buck,
            &Options::new(&[
                ("canonical_format", input_format),
                ("canonical_unused", "changed-again"),
            ]),
            ACTION_EXECUTION_KIND_LOCAL_DEP_FILE,
        )
        .await?,
    )?;
    assert_eq!(output, "optionusedextra");
    let (output, _) = canonical_input_output(
        &build_canonical_input(
            buck,
            &Options::new(&[
                ("canonical_format", input_format),
                ("canonical_option", "new-option"),
            ]),
            ACTION_EXECUTION_KIND_LOCAL,
        )
        .await?,
    )?;
    assert_eq!(output, "new-optionusedextra");
    Ok(())
}

#[buck_test(
    data_dir = "dep_files",
    setup_eden = false,
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_canonical_input_persisted_dep_file_json(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_persisted_dep_file(buck, "json").await
}

#[buck_test(
    data_dir = "dep_files",
    setup_eden = false,
    skip_for_os = ["windows"],
    extra_buck_config = [("buck2", "sqlite_dep_file_state", "true")],
)]
async fn test_canonical_input_persisted_dep_file_args(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_persisted_dep_file(buck, "args").await
}

async fn test_canonical_input_paged_analysis(
    buck: &Buck,
    input_format: &str,
) -> anyhow::Result<()> {
    // The page-in below needs the page-out to evict, which the default
    // `eviction_policy`, `on_memory_pressure`, does not.
    std::fs::write(
        buck.settings_home_dir().join(".bucksettings.local.toml"),
        concat!(
            "[hydration]\n",
            "enable_paging = true\n",
            "page_out_on_idle = true\n",
            "page_out_min_free_disk_gb = 0\n",
            "eviction_policy = \"on_idle\"\n",
        ),
    )?;
    let options = Options::new(&[
        ("canonical_unused_src", "true"),
        ("canonical_format", input_format),
    ]);
    build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?;
    buck.debug(&["hydration", "page-out"]).await?;
    std::fs::write(buck.cwd().join("app/other.h"), "changed unused source")?;
    let result =
        build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL_DEP_FILE).await?;
    let invocation_record = result.invocation_record()?;
    let page_in_count = &invocation_record["page_in_count"];
    assert!(
        page_in_count.as_i64().is_some_and(|count| count > 0),
        "page_in_count: {page_in_count}"
    );
    let (output, _) = canonical_input_output(&result)?;
    assert_eq!(output, "optionusedextra");
    Ok(())
}

#[buck_test(
    data_dir = "dep_files",
    write_invocation_record = true,
    skip_for_os = ["windows"],
    // Recomputing paged-out nodes gives their critical path entries overlapping spans, which
    // is the `critical_path_entry_overlap` soft error.
    allow_soft_errors = true,
)]
async fn test_canonical_input_paged_analysis_json(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_paged_analysis(buck, "json").await
}

#[buck_test(
    data_dir = "dep_files",
    write_invocation_record = true,
    skip_for_os = ["windows"],
    // Recomputing paged-out nodes gives their critical path entries overlapping spans, which
    // is the `critical_path_entry_overlap` soft error.
    allow_soft_errors = true,
)]
async fn test_canonical_input_paged_analysis_args(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_paged_analysis(buck, "args").await
}

/// Builds `root//app:canonical_json` with `options`, giving the digest of its one remote cache
/// query, its output and its execution kind.
async fn build_canonical_input_remotely(
    buck: &Buck,
    options: &Options,
) -> anyhow::Result<(Value, String, Option<i64>)> {
    let flags = options.flags();
    let result = buck
        .build(
            &[
                &["root//app:canonical_json", "--local-only"][..],
                &as_strs(&flags),
            ]
            .concat(),
        )
        .await?;
    let mut queries: Vec<Value> = read_what_ran(
        buck,
        &[
            "--emit-cache-queries",
            "--filter-category",
            "canonical_json_consumer",
        ],
    )
    .await?
    .into_iter()
    .filter(|entry| entry["reproducer"]["executor"] == "CacheQuery")
    .map(|entry| entry["reproducer"]["details"]["digest"].clone())
    .collect();
    assert_eq!(queries.len(), 1, "{queries:?}");
    let output = read_text(
        result
            .build_report()?
            .output_for_target("root//app:canonical_json")?,
    )?;
    let kind = get_last_execution_kind(buck, Some("canonical_json_consumer"), &[], None).await?;
    Ok((queries.remove(0), output, kind))
}

async fn test_canonical_input_remote_dep_file_key(
    buck: &Buck,
    input_format: &str,
) -> anyhow::Result<()> {
    let mut options = Options::new(&[
        ("canonical_option", &random_string()),
        ("canonical_format", input_format),
    ]);

    let (initial_key, output, _) = build_canonical_input_remotely(buck, &options).await?;
    assert_eq!(
        output,
        format!("{}usedextra", options.get("canonical_option"))
    );
    buck.debug(&["flush-dep-files"]).await?;
    options.set("canonical_unused", "changed");
    let (unused_key, output, kind) = build_canonical_input_remotely(buck, &options).await?;
    assert_eq!(unused_key, initial_key);
    assert_eq!(
        output,
        format!("{}usedextra", options.get("canonical_option"))
    );
    // Developer machines may not have permission to populate the remote cache.
    assert!(
        [
            Some(ACTION_EXECUTION_KIND_REMOTE_DEP_FILE_CACHE),
            Some(ACTION_EXECUTION_KIND_LOCAL),
        ]
        .contains(&kind),
        "{kind:?}"
    );

    buck.debug(&["flush-dep-files"]).await?;
    options.set("canonical_used", "used-changed");
    let (used_key, output, kind) = build_canonical_input_remotely(buck, &options).await?;
    assert_eq!(used_key, initial_key);
    assert_eq!(
        output,
        format!("{}used-changedextra", options.get("canonical_option"))
    );
    assert_eq!(kind, Some(ACTION_EXECUTION_KIND_LOCAL));

    buck.debug(&["flush-dep-files"]).await?;
    let changed_option = format!("{}-changed", options.get("canonical_option"));
    options.set("canonical_option", changed_option);
    let (changed_key, output, kind) = build_canonical_input_remotely(buck, &options).await?;
    assert_ne!(changed_key, initial_key);
    assert_eq!(
        output,
        format!("{}used-changedextra", options.get("canonical_option"))
    );
    assert_eq!(kind, Some(ACTION_EXECUTION_KIND_LOCAL));
    Ok(())
}

#[buck_test(
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("build", "execution_platforms", "root//app:canonical_platforms")],
    env = [("BUCK2_TEST_ONLY_REMOTE_DEP_FILE_CACHE", "true")],
)]
async fn test_canonical_input_remote_dep_file_key_json(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_remote_dep_file_key(buck, "json").await
}

#[buck_test(
    data_dir = "dep_files",
    skip_for_os = ["windows"],
    extra_buck_config = [("build", "execution_platforms", "root//app:canonical_platforms")],
    env = [("BUCK2_TEST_ONLY_REMOTE_DEP_FILE_CACHE", "true")],
)]
async fn test_canonical_input_remote_dep_file_key_args(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_input_remote_dep_file_key(buck, "args").await
}

async fn test_canonical_args_quoting(buck: &Buck, delimiter: &str) -> anyhow::Result<()> {
    let option = "option with 'quotes' and\na newline";
    let mut options = Options::new(&[
        ("canonical_format", "args"),
        ("canonical_pretty", delimiter),
        ("canonical_option", option),
    ]);
    let (output, args) = canonical_input_output(
        &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?,
    )?;
    assert_eq!(output, format!("{option}usedextra"));
    let split = shlex::split(&args).with_context(|| format!("Invalid shell quoting: {args}"))?;
    assert_eq!(split[0], option);
    options.set("canonical_unused", "changed");
    build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL_DEP_FILE).await?;
    options.set("canonical_empty_option", "true");
    let (output, args) = canonical_input_output(
        &build_canonical_input(buck, &options, ACTION_EXECUTION_KIND_LOCAL).await?,
    )?;
    assert_eq!(output, "usedextra");
    let split = shlex::split(&args).with_context(|| format!("Invalid shell quoting: {args}"))?;
    assert_eq!(split[0], "");
    Ok(())
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_args_quoting_true(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_args_quoting(buck, "true").await
}

#[buck_test(data_dir = "dep_files", skip_for_os = ["windows"])]
async fn test_canonical_args_quoting_false(buck: &Buck) -> anyhow::Result<()> {
    test_canonical_args_quoting(buck, "false").await
}

#[buck_test(data_dir = "invalid_dep_files")]
async fn test_invalid_dep_file_second_lookup_without_flush(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:lazy"]).await?;
    buck.build(&["//:lazy", "-c", "test.seed=123", "--no-remote-cache"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid line encountered in dep file");
    // A second change to the inputs without `flush-dep-files` reports the same error again.
    buck.build(&["//:lazy", "-c", "test.seed=456", "--no-remote-cache"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid line encountered in dep file");
    Ok(())
}
