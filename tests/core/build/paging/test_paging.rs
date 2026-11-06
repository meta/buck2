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
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::json_get;
use regex::Regex;
use serde_json::Value;
use serde_json::json;

// The fixture's `.bucksettings.toml` sets two `DaemonStartupConfig`s:
// `hydration.enable_paging` (pagable DICE storage on disk) and
// `hydration.page_out_on_idle` (page the graph out when the daemon goes idle).
// `hydration_settings` adds `page_out_policy = "once_per_key"` and
// `eviction_policy = "on_idle"` unless a test says otherwise.

// `buck2_data.PageOutStarted::*` — the invocation record serializes the
// `page_out_started` enum field as its integer value (see data.proto).
const PAGE_OUT_STARTED_STARTED: i64 = 1;
const PAGE_OUT_STARTED_DISABLED: i64 = 2;
const PAGE_OUT_STARTED_COMMAND_OPTED_OUT: i64 = 7;
const PAGE_OUT_STARTED_DISABLED_AFTER_ERROR: i64 = 9;

async fn build(buck: &Buck) -> anyhow::Result<BuckOutput> {
    Ok(buck.build(&["//:mysrcrule"]).await?)
}

fn output(result: &BuckOutput) -> anyhow::Result<String> {
    target_output(result, "mysrcrule")
}

/// A count in JSON.
fn count(value: &Value) -> anyhow::Result<u64> {
    value
        .as_u64()
        .with_context(|| format!("Not a count: {value}"))
}

/// A count that is absent, or `null`, when 0.
fn count_or_zero(value: &Value) -> anyhow::Result<u64> {
    if value.is_null() {
        return Ok(0);
    }
    count(value)
}

async fn page_out(buck: &Buck) -> anyhow::Result<Value> {
    // Page out, returning the `PageOutSummary` describing what moved.
    let out = buck.debug(&["hydration", "page-out"]).await?.stdout;
    serde_json::from_str(&out)
        .with_context(|| format!("page-out did not print a JSON summary:\n{out}"))
}

static PAGED_OUT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+) paged out").unwrap());

async fn paged_out_count(buck: &Buck) -> anyhow::Result<u64> {
    let out = buck.debug(&["hydration", "status"]).await?.stdout;
    let captures = PAGED_OUT
        .captures(&out)
        .with_context(|| format!("unexpected status output:\n{out}"))?;
    Ok(captures[1].parse()?)
}

fn paged_in_count(result: &BuckOutput) -> anyhow::Result<u64> {
    // Populated by `write_invocation_record = true` on the test; absent/null when 0.
    count_or_zero(&result.invocation_record()?["page_in_count"])
}

fn paged_in_count_for_key_type(result: &BuckOutput, key_type: &str) -> anyhow::Result<u64> {
    let record = result.invocation_record()?;
    let stats = &record["page_in_by_key_type"][key_type];
    if stats.is_null() {
        return Ok(0);
    }
    count(&stats["count"])
}

async fn page_out_summaries(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    // The last command's `PageOutSummary` events, the rows `buck2_page_outs` gets.
    filter_events(
        buck,
        &["Event", "data", "Instant", "data", "PageOutSummary"],
    )
    .await
}

async fn action_rule_type_names(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    let actions = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    Ok(actions
        .iter()
        .map(|action| action["target_rule_type_name"].clone())
        .collect())
}

async fn hydration_counts_for_key_type(buck: &Buck, key_type: &str) -> anyhow::Result<(u64, u64)> {
    // `(resident, paged out)`; the backed column is a subset of resident.
    let out = buck.debug(&["hydration", "status"]).await?.stdout;
    let regex = Regex::new(&format!(
        r"(?m)^\s*(\d+)\s+(\d+)\s+\d+\s+{}\s*$",
        regex::escape(key_type)
    ))?;
    let captures = regex
        .captures(&out)
        .with_context(|| format!("missing {key_type} in hydration status:\n{out}"))?;
    Ok((captures[1].parse()?, captures[2].parse()?))
}

async fn analysis_activity(buck: &Buck, result: &BuckOutput) -> anyhow::Result<(u64, u64)> {
    let record = result.invocation_record()?;
    let trace_id = record["trace_id"].as_str().context("No `trace_id`")?;
    let event_log = buck
        .log(&["show", &format!("--trace-id={trace_id}")])
        .await?
        .stdout;
    let mut check_deps_started = 0;
    let mut compute_started = 0;
    for line in event_log.lines() {
        let event: Value = serde_json::from_str(line)?;
        let Some(snapshot) = json_get(
            &event,
            &["Event", "data", "Instant", "data", "DiceStateSnapshot"],
        ) else {
            continue;
        };
        let analysis = &snapshot["key_states"]["AnalysisKey"];
        if !analysis.is_null() {
            check_deps_started = check_deps_started.max(
                analysis["check_deps_started"]
                    .as_u64()
                    .context("No `check_deps_started`")?,
            );
            compute_started = compute_started.max(
                analysis["compute_started"]
                    .as_u64()
                    .context("No `compute_started`")?,
            );
        }
    }

    Ok((check_deps_started, compute_started))
}

fn target_output(result: &BuckOutput, target: &str) -> anyhow::Result<String> {
    let output = result
        .build_report()?
        .output_for_target(&format!("root//:{target}"))?;
    Ok(std::fs::read_to_string(output)?)
}

/// A `[hydration]` setting's value.
enum Setting {
    Bool(bool),
    Str(&'static str),
}

use Setting::Bool;
use Setting::Str;

fn hydration_settings(buck: &Buck, settings: &[(&str, Setting)]) -> anyhow::Result<()> {
    // Written to the home `.bucksettings.local.toml`, which outranks the
    // fixture's repo `.bucksettings.toml` (`enable_paging`, `page_out_on_idle`,
    // `page_out_min_free_disk_gb`) key by key.
    //
    // Under the default `eviction_policy`, `on_memory_pressure`, a page-out
    // evicts nothing, and a daemon with no memory tracker does not page out on
    // idle at all; the evictions these tests observe come from `on_idle`. The
    // page-out policy is pinned too, so each test states the one it is about.
    let mut lines = vec!["[hydration]".to_owned()];
    for (key, default) in [
        ("page_out_policy", "once_per_key"),
        ("eviction_policy", "on_idle"),
    ] {
        if !settings.iter().any(|(k, _)| *k == key) {
            lines.push(format!("{key} = \"{default}\""));
        }
    }
    for (key, value) in settings {
        lines.push(match value {
            Bool(value) => format!("{key} = {value}"),
            Str(value) => format!("{key} = \"{value}\""),
        });
    }
    std::fs::write(
        buck.settings_home_dir().join(".bucksettings.local.toml"),
        lines.join("\n") + "\n",
    )?;
    Ok(())
}

async fn wait_for_page_out_idle(buck: &Buck) -> anyhow::Result<u64> {
    // `status --wait` blocks until any in-progress idle page-out finishes, so tests
    // observe the settled state without polling. Returns the paged-out node count.
    static IN_PROGRESS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"page-out in progress: (yes|no)").unwrap());
    let out = buck.debug(&["hydration", "status", "--wait"]).await?.stdout;
    let paged_out = PAGED_OUT.captures(&out);
    let in_progress = IN_PROGRESS.captures(&out);
    match (paged_out, in_progress) {
        (Some(paged_out), Some(in_progress)) if &in_progress[1] == "no" => {
            Ok(paged_out[1].parse()?)
        }
        _ => panic!("unexpected status output after --wait:\n{out}"),
    }
}

async fn status_count(buck: &Buck, what: &str) -> anyhow::Result<u64> {
    let out = buck.debug(&["hydration", "status"]).await?.stdout;
    let regex = Regex::new(&format!(r"(\d+) {what}"))?;
    let captures = regex
        .captures(&out)
        .with_context(|| format!("unexpected status output:\n{out}"))?;
    Ok(captures[1].parse()?)
}

async fn backed_count(buck: &Buck) -> anyhow::Result<u64> {
    status_count(buck, "backed").await
}

async fn candidate_count(buck: &Buck) -> anyhow::Result<u64> {
    status_count(buck, "page-out candidates").await
}

fn data_key_io(result: &BuckOutput, prefix: &str) -> anyhow::Result<BTreeMap<&'static str, u64>> {
    // These tests configure pagable storage, so a missing field is a real failure.
    let record = result.invocation_record()?;
    [
        "data_keys_out",
        "data_key_bytes_out",
        "data_keys_in",
        "data_key_bytes_in",
    ]
    .into_iter()
    .map(|suffix| {
        let value = &record[format!("{prefix}{suffix}").as_str()];
        assert!(
            !value.is_null(),
            "`{prefix}{suffix}` missing from the invocation record"
        );
        Ok((suffix, count(value)?))
    })
    .collect()
}

fn page_out_started(result: &BuckOutput) -> anyhow::Result<Value> {
    Ok(result.invocation_record()?["page_out_started"].clone())
}

fn write_src(buck: &Buck, content: &str) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join("src.txt"), content)?;
    Ok(())
}

async fn incremental_build_after_page_out(
    buck: &Buck,
    defer_field_reads: bool,
) -> anyhow::Result<()> {
    hydration_settings(buck, &[("defer_field_reads", Bool(defer_field_reads))])?;
    // Incremental builds must stay correct after an explicit `buck2 debug
    // hydration page-out`, relying on on-demand page-in during the build. Page-in
    // is measured per command via `page_in_count` in the invocation record.
    //
    // Note: this does not check that stale paged-out values are reclaimed from
    // storage. Garbage-collecting them is future work (reference counting).
    //
    // Pagable storage is set up by `hydration.enable_paging = true` in the
    // fixture `.bucksettings.toml` (a `DaemonStartupConfig`).
    write_src(buck, "content-0\n")?;
    assert_eq!(output(&build(buck).await?)?, "content-0\n");

    // Page the whole graph out to disk; in-memory values are evicted.
    buck.debug(&["hydration", "page-out"]).await?;
    assert!(
        paged_out_count(buck).await? > 0,
        "expected node values to actually be paged out"
    );

    // Rebuild with no changes: served by on-demand page-in of the paged-out
    // values. The per-command page-in total is in the invocation record.
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-0\n");
    let paged_in = paged_in_count(&result)?;
    assert!(
        paged_in > 0,
        "expected values to be paged back in during rebuild, got {paged_in}"
    );

    // Invalidate: dependent nodes recompute, the rest hydrate on demand.
    write_src(buck, "content-1\n")?;
    assert_eq!(output(&build(buck).await?)?, "content-1\n");

    // Page out again (now including the recomputed values) and invalidate once
    // more, to confirm repeated page-out cycles keep producing correct results.
    buck.debug(&["hydration", "page-out"]).await?;
    assert!(
        paged_out_count(buck).await? > 0,
        "expected node values to be paged out again"
    );
    write_src(buck, "content-2\n")?;
    assert_eq!(output(&build(buck).await?)?, "content-2\n");
    Ok(())
}

// Recomputing paged-out nodes gives their critical path entries overlapping spans, which is
// the `critical_path_entry_overlap` soft error.
#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true
)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_incremental_build_after_page_out_False(buck: &Buck) -> anyhow::Result<()> {
    incremental_build_after_page_out(buck, false).await
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true
)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_incremental_build_after_page_out_True(buck: &Buck) -> anyhow::Result<()> {
    incremental_build_after_page_out(buck, true).await
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
async fn test_eviction_on_memory_pressure_keeps_paged_out_values_resident(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Under the default `on_memory_pressure` a page-out writes the values but
    // evicts none; `buck2 debug hydration evict`, what memory pressure triggers,
    // drops them without another write. A daemon with no memory tracker could
    // never see pressure, so it does not page out on idle at all: the test host
    // decides which of the two this daemon is.
    hydration_settings(buck, &[("eviction_policy", Str("on_memory_pressure"))])?;
    write_src(buck, "content-0\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-0\n");
    let started = page_out_started(&result)?;
    if started == PAGE_OUT_STARTED_STARTED {
        wait_for_page_out_idle(buck).await?;
    } else {
        assert_eq!(
            started, PAGE_OUT_STARTED_DISABLED,
            "without a memory tracker, idle page-out must be disabled rather than write rows nothing evicts; got {started}"
        );
        page_out(buck).await?;
    }
    assert_eq!(
        paged_out_count(buck).await?,
        0,
        "a page-out must evict nothing"
    );
    let backed = backed_count(buck).await?;
    assert!(
        backed > 0,
        "the written values stay resident, backed by their rows"
    );

    let out = buck.debug(&["hydration", "evict"]).await?.stdout;
    assert!(
        out.contains(&format!("evicted {backed} backed DICE values")),
        "{out}"
    );
    assert_eq!(paged_out_count(buck).await?, backed);
    assert_eq!(backed_count(buck).await?, 0);
    Ok(())
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
async fn test_evict_drops_values_rows_already_hold(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(buck, &[])?;
    // Values read back from storage stay resident, backed by their rows, and
    // `buck2 debug hydration evict`, what memory pressure triggers, drops them
    // without a write.
    // Read-back values are backed only under `once_per_value`; idle page-out is
    // off so the page-outs below are the explicit ones.
    hydration_settings(
        buck,
        &[
            ("page_out_on_idle", Bool(false)),
            ("page_out_policy", Str("once_per_value")),
        ],
    )?;
    write_src(buck, "content-0\n")?;
    assert_eq!(output(&build(buck).await?)?, "content-0\n");
    buck.debug(&["hydration", "page-out"]).await?;
    let paged_out = paged_out_count(buck).await?;
    assert!(paged_out > 0);

    let result = build(buck).await?;
    let read_back = paged_in_count(&result)?;
    assert!(read_back > 0);
    assert_eq!(backed_count(buck).await?, read_back);
    assert_eq!(paged_out_count(buck).await?, paged_out - read_back);

    let out = buck.debug(&["hydration", "evict"]).await?.stdout;
    assert!(
        out.contains(&format!("evicted {read_back} backed DICE values")),
        "{out}"
    );
    // A command that evicts gets a `buck2_page_outs` row even though it starts
    // no idle page-out.
    let rows = page_out_summaries(buck).await?;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0]["page_out_started"],
        PAGE_OUT_STARTED_COMMAND_OPTED_OUT
    );
    assert_eq!(count(&rows[0]["command_evictions"])?, 1);
    assert_eq!(count(&rows[0]["command_evicted_count"])?, read_back);
    assert_eq!(backed_count(buck).await?, 0);
    assert_eq!(paged_out_count(buck).await?, paged_out);
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-0\n");
    assert_eq!(
        paged_in_count(&result)?,
        read_back,
        "evicted values are read back"
    );
    assert_eq!(
        page_out_summaries(buck).await?,
        Vec::<Value>::new(),
        "a command that neither evicts nor pages out logs no row"
    );
    Ok(())
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true,
    extra_buck_config = [("buck2", "restarter", "true")],
)]
async fn test_failed_demand_page_in_recomputes_without_restart(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(buck, &[("page_out_on_idle", Bool(false))])?;
    let storage_dir = buck.cwd().join("paging-storage");
    write_src(buck, "content-0\n")?;
    let result = buck
        .build(&["//:mysrcrule"])
        .env("BUCK2_DICE_DB_PATH", &storage_dir)
        .await?;
    assert_eq!(output(&result)?, "content-0\n");
    page_out(buck).await?;
    assert!(paged_out_count(buck).await? > 0);

    // Remove only this test daemon's stored values, leaving its in-memory graph
    // pointing at missing rows. Demanding these values must recompute them in
    // this daemon, without restarting the command.
    let databases = std::fs::read_dir(&storage_dir)?
        .map(|entry| Ok(entry?.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| is_pagable_db(path))
        .collect::<Vec<_>>();
    assert!(
        !databases.is_empty(),
        "expected the test daemon's SQLite paging storage"
    );
    let mut removed = 0;
    for path in &databases {
        let connection = rusqlite::Connection::open_with_flags(
            format!("file:{}?mode=rw", path.display()),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        removed += connection.execute("DELETE FROM pagable_data", [])?;
    }
    assert!(removed > 0, "the test must remove stored values");

    let result = buck
        .build(&["//:mysrcrule"])
        .env("BUCK2_DICE_DB_PATH", &storage_dir)
        .await?;
    assert!(!result.stderr.contains("Your command will now restart"));
    assert!(result.invocation_record()?["restarted_trace_id"].is_null());
    assert_eq!(output(&result)?, "content-0\n");
    Ok(())
}

/// Whether `path` matches `pagable.*.db`.
fn is_pagable_db(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("pagable."))
        .and_then(|rest| rest.strip_suffix(".db"))
        .is_some()
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    env = [("BUCK2_DICE_SNAPSHOT_INTERVAL_MS", "1")],
)]
async fn test_config_change_after_page_out_analysis_validation(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(buck, &[("page_out_on_idle", Bool(false))])?;
    buck.build(&["//:analysis_root"]).await?;
    buck.debug(&["hydration", "page-out"]).await?;
    assert!(
        paged_out_count(buck).await? > 0,
        "expected the analysis graph to page out"
    );
    let (analysis_resident, analysis_paged_out) =
        hydration_counts_for_key_type(buck, "AnalysisKey").await?;
    assert_eq!(
        analysis_resident, 0,
        "all AnalysisKey values should have been evicted"
    );
    assert!(
        analysis_paged_out >= 3,
        "the three fixture AnalysisKeys should be paged out"
    );

    let result = buck
        .build(&["//:analysis_root", "-c", "fbcode.unused=0"])
        .await?;
    let (analysis_checks, analysis_computes) = analysis_activity(buck, &result).await?;
    let analysis_page_ins = paged_in_count_for_key_type(&result, "AnalysisKey")?;
    assert!(
        analysis_checks > 0,
        "expected the paged-out analysis graph to be revalidated after an unrelated command-line configuration change"
    );
    assert_eq!(
        analysis_page_ins, 0,
        "dependency validation should not page in paged-out AnalysisKey values; checks={analysis_checks}, computes={analysis_computes}, page_ins={analysis_page_ins}"
    );
    assert_eq!(
        analysis_computes, 0,
        "the unrelated configuration change should be cut off without rerunning analysis; checks={analysis_checks}, computes={analysis_computes}"
    );
    Ok(())
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
async fn test_page_out_frozen_value_into_already_paged_out_heap(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(buck, &[])?;
    // Regression test for a page-out serialization panic:
    //   FrozenValue pointer ... not found in any registered heap's chunk index
    //   (starlark-rust/.../pagable/starlark_serialize_context.rs)
    //
    // `module_const_a` and `module_const_b` each hold, in their analysis result, a
    // `FrozenValue` that lives on `rules.bzl`'s shared module heap. The panic only
    // surfaces across *two* page-outs: the first evicts the shared module heap, and
    // the second must serialize the other target's `FrozenValue` into that
    // now-paged-out (unregistered) heap. A single page-out of a fresh graph never
    // trips it, because every reachable heap is still registered.
    buck.build(&["//:module_const_a"]).await?;
    buck.debug(&["hydration", "page-out"]).await?;
    assert!(
        paged_out_count(buck).await? > 0,
        "expected the first target's values (and the shared module heap) to page out"
    );

    // Analyze the second target, then page out again. Serializing its provider's
    // `FrozenValue` into the already-paged-out module heap must not crash.
    buck.build(&["//:module_const_b"]).await?;
    buck.debug(&["hydration", "page-out"]).await?;

    // The second page-out must leave module_const_b hydratable.
    buck.build(&["//:module_const_b"]).await?;
    Ok(())
}

async fn page_out_bxl_dynamic_callback_after_page_in(
    buck: &Buck,
    defer_field_reads: bool,
) -> anyhow::Result<()> {
    hydration_settings(buck, &[("defer_field_reads", Bool(defer_field_reads))])?;
    // `page_out.bxl` creates a nested dynamic-output callback. The callback is
    // allocated on the BXL evaluation heap, but its compiler metadata lives on
    // the loaded `.bxl` module heap. Page-out must retain and serialize that
    // owning module heap instead of following an unowned `FrozenValue` pointer.
    buck.bxl(&["//page_out.bxl:main", "--", "--value", "first"])
        .await?;
    assert_eq!(
        action_rule_type_names(buck).await?,
        [json!(null), json!(null)]
    );
    buck.debug(&["hydration", "page-out"]).await?;

    // A distinct BXL key pages the module back in while computing a new root.
    // The restored module must be retained by that root's evaluation heap too.
    buck.bxl(&["//page_out.bxl:main", "--", "--value", "second"])
        .await?;
    assert_eq!(
        action_rule_type_names(buck).await?,
        [json!(null), json!(null)]
    );
    buck.debug(&["hydration", "page-out"]).await?;
    Ok(())
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_page_out_bxl_dynamic_callback_after_page_in_False(buck: &Buck) -> anyhow::Result<()> {
    page_out_bxl_dynamic_callback_after_page_in(buck, false).await
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_page_out_bxl_dynamic_callback_after_page_in_True(buck: &Buck) -> anyhow::Result<()> {
    page_out_bxl_dynamic_callback_after_page_in(buck, true).await
}

async fn page_in_shared_anon_target(buck: &Buck, defer_field_reads: bool) -> anyhow::Result<()> {
    hydration_settings(buck, &[("defer_field_reads", Bool(defer_field_reads))])?;
    // Bound post-page-out commands because the old typetag mismatch hung hydration.
    let command_timeout = Duration::from_secs(60);
    let result = buck.build(&["//:uses_anon_a"]).await?;
    assert_eq!(target_output(&result, "uses_anon_a")?, "anonymous target\n");

    tokio::time::timeout(command_timeout, buck.debug(&["hydration", "page-out"])).await??;
    let paged_out_count = tokio::time::timeout(command_timeout, paged_out_count(buck)).await??;
    assert!(
        paged_out_count > 0,
        "expected the anonymous target analysis to be paged out"
    );

    // Analyze a new parent that requests the same anonymous target. Unlike a
    // no-op rebuild of the first parent, this has to load the paged-out anonymous
    // target analysis result before it can resolve the promise.
    let result = tokio::time::timeout(command_timeout, buck.build(&["//:uses_anon_b"])).await??;
    assert_eq!(target_output(&result, "uses_anon_b")?, "anonymous target\n");
    assert!(
        paged_in_count(&result)? > 0,
        "expected the anonymous target analysis to be paged back in"
    );
    Ok(())
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_page_in_shared_anon_target_False(buck: &Buck) -> anyhow::Result<()> {
    page_in_shared_anon_target(buck, false).await
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_page_in_shared_anon_target_True(buck: &Buck) -> anyhow::Result<()> {
    page_in_shared_anon_target(buck, true).await
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true
)]
async fn test_page_out_on_idle(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(buck, &[])?;
    // With `hydration.page_out_on_idle`, the daemon pages the DICE graph out to
    // disk in a background task once it goes idle after a command. Subsequent
    // builds stay correct by paging values back in on demand.
    write_src(buck, "content-0\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-0\n");
    // The sole active command triggers the idle page-out and records it.
    assert_eq!(
        page_out_started(&result)?,
        PAGE_OUT_STARTED_STARTED,
        "expected the sole active command to trigger an idle page-out"
    );

    // Page-out runs in a detached background task once the daemon is idle. Wait
    // for it to finish, then confirm the graph was actually paged out.
    assert!(
        wait_for_page_out_idle(buck).await? > 0,
        "expected idle page-out to actually page values out"
    );

    // The paged-out values must now be paged back in on the next build.
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-0\n");
    let paged_in = paged_in_count(&result)?;
    assert!(
        paged_in > 0,
        "expected paged-out values to be paged back in, got {paged_in}"
    );

    // Incremental correctness across invalidations (each also schedules an idle
    // page-out): dependent nodes recompute, the rest hydrate on demand.
    write_src(buck, "content-1\n")?;
    assert_eq!(output(&build(buck).await?)?, "content-1\n");

    write_src(buck, "content-2\n")?;
    assert_eq!(output(&build(buck).await?)?, "content-2\n");
    Ok(())
}

// The injected failure is reported with `soft_error_or_panic!`, which panics the
// daemon under the harness's default `BUCK2_HARD_ERROR=true`; the test is about
// what the daemon does after the failure, so soft errors are allowed.
#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true,
    env = [("BUCK2_TEST_FAIL_PAGE_OUT", "true")],
)]
async fn test_idle_page_out_disabled_after_error(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(buck, &[])?;
    // A failed idle page-out disables idle page-out for the rest of the daemon's
    // lifetime: the failure is likely persistent, so retrying at the end of every
    // command would repeat the work and re-log the error. `BUCK2_TEST_FAIL_PAGE_OUT`
    // injects the failure on the idle path only.
    write_src(buck, "content-0\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-0\n");
    // The cold build computes values, so it schedules an idle page-out...
    assert_eq!(
        page_out_started(&result)?,
        PAGE_OUT_STARTED_STARTED,
        "the cold build should schedule an idle page-out"
    );

    // ...which then fails (injected). Wait for the background task to settle; it
    // sets the "disabled" flag before releasing the single-flight guard that
    // `status --wait` blocks on, so the flag is observable once this returns.
    wait_for_page_out_idle(buck).await?;

    // A rebuild that recomputes values would normally schedule another page-out,
    // but the prior failure has disabled idle page-out, so it must not.
    write_src(buck, "content-1\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-1\n");
    assert_eq!(
        page_out_started(&result)?,
        PAGE_OUT_STARTED_DISABLED_AFTER_ERROR,
        "a prior idle page-out failure must disable idle page-out for future commands"
    );
    Ok(())
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
async fn test_page_out_triggered_only_when_values_computed(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(buck, &[])?;
    // A command triggers an idle page-out only when it computed values worth
    // paging out. The cold build does; a following no-op rebuild does not.
    write_src(buck, "content-0\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-0\n");
    assert_eq!(
        page_out_started(&result)?,
        PAGE_OUT_STARTED_STARTED,
        "the cold build computes values, so it triggers a page-out"
    );
    wait_for_page_out_idle(buck).await?;

    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-0\n");
    assert_ne!(
        page_out_started(&result)?,
        PAGE_OUT_STARTED_STARTED,
        "a no-op rebuild computes nothing new, so it should not trigger a page-out"
    );
    Ok(())
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true
)]
async fn test_page_out_at_most_once(buck: &Buck) -> anyhow::Result<()> {
    // Under `once_per_key` a value is paged out at most once: once an
    // incremental build pages a value back in (or recomputes it), it stays
    // resident rather than being paged out again.
    hydration_settings(buck, &[("page_out_policy", Str("once_per_key"))])?;
    write_src(buck, "content-0\n")?;
    assert_eq!(output(&build(buck).await?)?, "content-0\n");
    wait_for_page_out_idle(buck).await?;

    // This build pages its working set back in and recomputes the affected nodes.
    // Those values were already paged out once, so they stay resident: nothing new
    // to page out, so the build does not trigger a page-out.
    write_src(buck, "content-1\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-1\n");
    // Precondition: this build actually paged values back in (otherwise the
    // assertion below would hold vacuously).
    assert!(
        paged_in_count(&result)? > 0,
        "expected the incremental build to page its working set back in"
    );
    assert_ne!(
        page_out_started(&result)?,
        PAGE_OUT_STARTED_STARTED,
        "values paged in by an incremental build must stay resident, leaving nothing new to page out"
    );

    // The working set stayed resident, so the next incremental build pages nothing
    // back in.
    write_src(buck, "content-2\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-2\n");
    assert_eq!(
        paged_in_count(&result)?,
        0,
        "expected the working set to stay resident, but {} values were paged in",
        paged_in_count(&result)?
    );
    Ok(())
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true
)]
async fn test_values_paged_out_once_stay_resident(buck: &Buck) -> anyhow::Result<()> {
    // Under `once_per_value`: once an incremental build pages a value back in, or
    // recomputes it, the value stays resident: the idle page-out writes the
    // recomputed ones but evicts neither.
    hydration_settings(buck, &[("page_out_policy", Str("once_per_value"))])?;
    write_src(buck, "content-0\n")?;
    assert_eq!(output(&build(buck).await?)?, "content-0\n");
    wait_for_page_out_idle(buck).await?;

    // This build pages its working set back in and recomputes the affected nodes.
    // The recomputed values have no row yet, so the build triggers a page-out,
    // which writes them and leaves them resident.
    write_src(buck, "content-1\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-1\n");
    // Precondition: this build actually paged values back in (otherwise the
    // assertion below would hold vacuously).
    assert!(
        paged_in_count(&result)? > 0,
        "expected the incremental build to page its working set back in"
    );
    assert_eq!(
        page_out_started(&result)?,
        PAGE_OUT_STARTED_STARTED,
        "recomputed values should be written by an idle page-out"
    );
    let paged_out_before = paged_out_count(buck).await?;
    wait_for_page_out_idle(buck).await?;
    assert_eq!(
        paged_out_count(buck).await?,
        paged_out_before,
        "writing recomputed values must not evict anything"
    );
    assert_eq!(candidate_count(buck).await?, 0);
    assert!(backed_count(buck).await? > 0);

    // The working set stayed resident, so the next incremental build pages nothing
    // back in.
    write_src(buck, "content-2\n")?;
    let result = build(buck).await?;
    assert_eq!(output(&result)?, "content-2\n");
    assert_eq!(
        paged_in_count(&result)?,
        0,
        "expected the working set to stay resident, but {} values were paged in",
        paged_in_count(&result)?
    );
    Ok(())
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
async fn test_idle_page_out_allows_multiple_runs(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(buck, &[])?;
    let first = buck.build(&["//:module_const_a"]).await?;
    assert_eq!(
        page_out_started(&first)?,
        PAGE_OUT_STARTED_STARTED,
        "expected the first eligible command to start an idle page-out"
    );
    wait_for_page_out_idle(buck).await?;

    let second = buck.build(&["//:module_const_b"]).await?;
    assert_eq!(
        page_out_started(&second)?,
        PAGE_OUT_STARTED_STARTED,
        "expected newly computed values to trigger a second idle page-out"
    );
    wait_for_page_out_idle(buck).await?;
    Ok(())
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
async fn test_data_key_io_in_invocation_record(buck: &Buck) -> anyhow::Result<()> {
    // Two views reach the record: `paging_data_key_*` is this command's own work,
    // `paging_daemon_data_key_*` the daemon's running total.
    //
    // A value is paged out at most once per daemon, so an idle page-out would
    // leave nothing for the explicit one below. With it off, this test's own
    // page-out is the only one, and every counter it asserts on is its doing.
    hydration_settings(buck, &[("page_out_on_idle", Bool(false))])?;
    write_src(buck, "content-0\n")?;
    build(buck).await?;
    let page_out = page_out(buck).await?;
    assert!(
        count(&page_out["data_key_bytes_out"])? > 0,
        "expected the page-out to write DataKeys, got {page_out}"
    );
    assert!(
        paged_out_count(buck).await? > 0,
        "expected values to be paged out"
    );

    // The rebuild pages those values back in. Page-out happened in earlier commands,
    // so only the running totals see bytes going out.
    let rebuild = build(buck).await?;
    let delta = data_key_io(&rebuild, "paging_")?;
    let cumulative = data_key_io(&rebuild, "paging_daemon_")?;

    assert!(
        delta["data_keys_in"] > 0 && delta["data_key_bytes_in"] > 0,
        "the rebuild should have read DataKeys back, got {delta:?}"
    );
    assert!(
        cumulative["data_key_bytes_out"] > 0,
        "the earlier page-out should show in the running totals, got {cumulative:?}"
    );
    // Both read the same daemon counters, so the record cannot lag the page-out.
    let page_out_daemon_bytes_out = count(&page_out["daemon_data_key_bytes_out"])?;
    assert!(
        cumulative["data_key_bytes_out"] >= page_out_daemon_bytes_out,
        "record running total {} is behind the page-out's {page_out_daemon_bytes_out}",
        cumulative["data_key_bytes_out"]
    );
    // DataKeys are content-addressed, so nothing can be read that was not written
    // first: net offloaded bytes are never negative.
    assert!(
        cumulative["data_key_bytes_out"] >= cumulative["data_key_bytes_in"],
        "paged-in bytes exceed paged-out bytes in the running totals: {cumulative:?}"
    );
    // DataKey accounting is a superset of the per-DICE-value page-in bytes, which
    // miss nested PagableArc sub-values. Both are deltas for this same command.
    let page_in_bytes = count_or_zero(&rebuild.invocation_record()?["page_in_bytes"])?;
    assert!(
        delta["data_key_bytes_in"] >= page_in_bytes,
        "DataKey page-in bytes {} should cover per-value {page_in_bytes}",
        delta["data_key_bytes_in"]
    );

    // Everything is resident again, so a no-op rebuild moves nothing. Its delta
    // going to zero while the running totals hold steady is what distinguishes the
    // two views — a cumulative value reported in the delta fields would not.
    let settled = build(buck).await?;
    assert_eq!(
        data_key_io(&settled, "paging_")?,
        BTreeMap::from([
            ("data_keys_out", 0),
            ("data_key_bytes_out", 0),
            ("data_keys_in", 0),
            ("data_key_bytes_in", 0),
        ]),
        "a build that pages nothing should report no DataKey I/O of its own"
    );
    let settled_cumulative = data_key_io(&settled, "paging_daemon_")?;
    assert!(
        cumulative
            .iter()
            .all(|(field, value)| settled_cumulative[field] >= *value),
        "the running totals must carry the earlier commands' work forward, got {settled_cumulative:?} after {cumulative:?}"
    );
    Ok(())
}

#[buck_test(data_dir = "paging", write_invocation_record = true)]
async fn test_page_out_command_reports_summary(buck: &Buck) -> anyhow::Result<()> {
    // The manual page-out reports the same `PageOutSummary` the idle one logs.
    // Idle page-out is off: a value is paged out at most once per daemon, so an
    // idle one would leave nothing for the manual one this test is about.
    hydration_settings(buck, &[("page_out_on_idle", Bool(false))])?;
    write_src(buck, "content-0\n")?;
    build(buck).await?;

    let summary = page_out(buck).await?;
    let field = |name: &str| -> anyhow::Result<u64> {
        summary[name]
            .as_u64()
            .with_context(|| format!("No `{name}` in the summary: {summary}"))
    };

    assert!(
        summary["error"].is_null(),
        "page-out reported an error: {summary}"
    );
    assert!(
        !summary["cancelled"]
            .as_bool()
            .with_context(|| format!("No `cancelled` in the summary: {summary}"))?,
        "a manual page-out holds the exclusive command lock, so nothing can cancel it"
    );
    assert!(
        field("data_keys_out")? > 0 && field("data_key_bytes_out")? > 0,
        "expected the page-out to write DataKeys, got {summary}"
    );
    // A page-out only writes, and the command lock keeps a page-in from overlapping.
    assert!(
        field("data_keys_in")? == 0 && field("data_key_bytes_in")? == 0,
        "nothing should have been paged in during the page-out, got {summary}"
    );
    // Running totals are read after the delta, so they include it.
    assert!(
        field("daemon_data_key_bytes_out")? >= field("data_key_bytes_out")?,
        "running totals should include this page-out's own bytes, got {summary}"
    );
    // The store is append-only.
    assert!(
        field("db_size_bytes_after")? >= field("db_size_bytes_before")?,
        "the pagable store shrank across a page-out: {summary}"
    );
    Ok(())
}

async fn provider_and_action_reads_skip_unrelated_actions(
    buck: &Buck,
    defer_field_reads: bool,
) -> anyhow::Result<()> {
    hydration_settings(
        buck,
        &[
            ("page_out_on_idle", Bool(false)),
            ("page_out_policy", Str("once_per_value")),
            ("defer_field_reads", Bool(defer_field_reads)),
        ],
    )?;
    write_src(buck, "before eviction\n")?;
    let warm = buck
        .audit(&["providers", "-p", "InstallInfo", "//:many_actions"])
        .await?;
    assert_eq!(
        target_output(&buck.build(&["//:many_actions"]).await?, "many_actions")?,
        "before eviction\n"
    );
    assert!(count(&page_out(buck).await?["paged_out_count"])? > 0);

    let providers = buck
        .audit(&["providers", "-p", "InstallInfo", "//:many_actions"])
        .await?;
    assert_eq!(providers.stdout, warm.stdout);
    assert_eq!(providers.invocation_record()?["analysis_count"], 0);
    if defer_field_reads {
        // There are 2,000 irrelevant actions. Root/provider metadata should need
        // only a bounded set of reads, independently of those action payloads.
        assert!(count(&providers.invocation_record()?["paging_data_keys_in"])? < 500);
    }

    // A source-content edit reuses analysis but needs the copy action. This must
    // resolve that action without restoring the 2,000 unrelated write actions.
    write_src(buck, "after eviction\n")?;
    let changed = buck.build(&["//:many_actions"]).await?;
    assert_eq!(target_output(&changed, "many_actions")?, "after eviction\n");
    assert_eq!(changed.invocation_record()?["analysis_count"], 0);
    if defer_field_reads {
        assert!(count(&changed.invocation_record()?["paging_data_keys_in"])? < 500);
    }
    Ok(())
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true
)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_provider_and_action_reads_skip_unrelated_actions_False(
    buck: &Buck,
) -> anyhow::Result<()> {
    provider_and_action_reads_skip_unrelated_actions(buck, false).await
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true
)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_provider_and_action_reads_skip_unrelated_actions_True(
    buck: &Buck,
) -> anyhow::Result<()> {
    provider_and_action_reads_skip_unrelated_actions(buck, true).await
}

#[buck_test(
    data_dir = "paging",
    write_invocation_record = true,
    allow_soft_errors = true
)]
async fn test_cached_build_metadata_skips_configured_graph(buck: &Buck) -> anyhow::Result<()> {
    hydration_settings(
        buck,
        &[
            ("page_out_on_idle", Bool(false)),
            ("page_out_policy", Str("once_per_value")),
        ],
    )?;
    let graph = buck.cwd().join("graph");
    std::fs::create_dir(&graph)?;
    // A separate package keeps the top-level pattern lookup small. Reading its
    // cached outputs and rule name must not restore these dependency nodes.
    let leaves: Vec<String> = (0..2000)
        .map(|i| format!("analysis_node(name=\"leaf_{i}\")"))
        .collect();
    std::fs::write(
        graph.join("TARGETS.fixture"),
        "load(\"//:rules.bzl\", \"analysis_node\")\n".to_owned()
            + &leaves.join("\n")
            + "\nanalysis_node(name=\"all\", deps=[\":leaf_{}\".format(i) for i in range(2000)], visibility=[\"PUBLIC\"])\n",
    )?;
    let targets = buck.cwd().join("TARGETS.fixture");
    std::fs::write(
        &targets,
        "load(\":rules.bzl\", \"copy_file\")\ncopy_file(name=\"graph_root\", src=\"src.txt\", out=\"out.txt\", deps=[\"//graph:all\"])\n",
    )?;
    write_src(buck, "before eviction\n")?;
    assert_eq!(
        target_output(&buck.build(&["//:graph_root"]).await?, "graph_root")?,
        "before eviction\n"
    );
    assert!(count(&page_out(buck).await?["paged_out_count"])? > 0);

    for content in ["before eviction\n", "after eviction\n"] {
        write_src(buck, content)?;
        let result = buck.build(&["//:graph_root"]).await?;
        assert_eq!(target_output(&result, "graph_root")?, content);
        assert_eq!(result.invocation_record()?["analysis_count"], 0);
        assert!(count(&result.invocation_record()?["paging_data_keys_in"])? < 500);
        assert_eq!(
            paged_in_count_for_key_type(&result, "ConfiguredTargetNodeKey")?,
            0
        );

        let responses = filter_events(
            buck,
            &["Result", "result", "build_response", "build_targets"],
        )
        .await?;
        assert_eq!(responses[0][0]["target_rule_type_name"], "copy_file");
        if content == "after eviction\n" {
            assert_eq!(action_rule_type_names(buck).await?, [json!("copy_file")]);
        }
    }

    // A rule-type change must update both analysis metadata and cached outputs.
    std::fs::write(
        &targets,
        "load(\":rules.bzl\", \"analysis_node\")\nanalysis_node(name=\"graph_root\", deps=[\"//graph:all\"])\n",
    )?;
    let changed = buck.build(&["//:graph_root"]).await?;
    assert!(count(&changed.invocation_record()?["analysis_count"])? > 0);
    let responses = filter_events(
        buck,
        &["Result", "result", "build_response", "build_targets"],
    )
    .await?;
    assert_eq!(responses[0][0]["target_rule_type_name"], "analysis_node");
    Ok(())
}
