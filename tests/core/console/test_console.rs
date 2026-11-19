/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::json_get;
use regex::Regex;
use serde_json::Value;

fn fixture(name: &str) -> String {
    let p = Path::new(&std::env::var_os("FIXTURES").expect("the test's `BUCK` sets `FIXTURES`"))
        .join(format!("{name}.proto"));
    std::path::absolute(p)
        .expect("an absolute fixture path")
        .to_string_lossy()
        .into_owned()
}

/// The filesystem root, relative to `dir`, as `os.path.relpath("/", dir)` gives it.
fn root_relative_to(dir: &Path) -> PathBuf {
    std::iter::repeat_n("..", dir.ancestors().count() - 1).collect()
}

fn has_network_stats(stderr: &str) -> bool {
    Regex::new("Network: .*([0-9.]+)([KMG]?)B")
        .unwrap()
        .is_match(stderr)
}

#[buck_test]
async fn test_console_facts(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.log(&["replay", &fixture("my_genrule0")]).await?;
    assert!(has_network_stats(&res.stderr));
    assert!(res.stderr.contains("Cache hits: 100%"));
    assert!(
        res.stderr
            .contains("Commands: 1 (cached: 1, remote: 0, local: 0)")
    );
    Ok(())
}

#[buck_test]
async fn test_console_facts_no_repo(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .log(&["replay", &fixture("my_genrule0")])
        .rel_cwd(root_relative_to(buck.cwd()))
        .await?;
    assert!(has_network_stats(&res.stderr));
    assert!(res.stderr.contains("Cache hits: 100%"));
    assert!(
        res.stderr
            .contains("Commands: 1 (cached: 1, remote: 0, local: 0)")
    );
    Ok(())
}

#[buck_test]
async fn test_super_console_facts(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.log(&["replay", &fixture("my_genrule0")]).await?;
    assert!(has_network_stats(&res.stderr));
    assert!(res.stderr.contains("Cache hits: 100%"));
    assert!(res.stderr.contains("Commands: 1"));
    Ok(())
}

#[buck_test]
async fn test_whatran(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.log(&["what-ran", &fixture("my_genrule0")]).await?;
    assert!(res.stdout.contains("cache"));
    assert!(
        res.stdout
            .contains("87eeee88c133dfa39711399a81f500147275cfeeb1f06b6b4805f2c0588615d1:145")
    );
    Ok(())
}

#[buck_test]
async fn test_whatran_no_repo(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .log(&["what-ran", &fixture("my_genrule0")])
        .rel_cwd(root_relative_to(buck.cwd()))
        .await?;
    assert!(res.stdout.contains("cache"));
    assert!(
        res.stdout
            .contains("87eeee88c133dfa39711399a81f500147275cfeeb1f06b6b4805f2c0588615d1:145")
    );
    Ok(())
}

/// Whether Python would find `value` true.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// We show spans up to depth 2 in the console. We should make sure that spans
/// whose runtime depends on external tools (i.e. the file watcher) are
/// displayed.
#[buck_test]
async fn test_file_watcher_span_depth(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[]).await?;
    let log = buck.log(&["show"]).await?;

    let mut depths: HashMap<String, i64> = HashMap::new();
    let mut file_watcher_span = None;

    for line in log.stdout.lines() {
        let line: Value = serde_json::from_str(line)?;
        let Some(event) = json_get(&line, &["Event"]) else {
            continue;
        };

        let Some(span) = json_get(event, &["data", "SpanStart", "data"]) else {
            continue;
        };

        let id = |key: &str| {
            event
                .get(key)
                .unwrap_or_else(|| panic!("No `{key}` in {event}"))
                .to_string()
        };

        // This event is relevant to us, but it's also not shown, so it means
        // its children are roots.
        let depth = if span.get("DiceCriticalSection").is_some() {
            -1
        } else {
            depths.get(&id("parent_id")).copied().unwrap_or_default() + 1
        };

        depths.insert(id("span_id"), depth);

        if json_get(event, &["data", "SpanStart", "data", "FileWatcher"]).is_some_and(truthy) {
            file_watcher_span = Some(event.clone());
        }
    }

    let file_watcher_span = file_watcher_span.expect("Did not find FileWatcher span");
    assert!(
        depths
            .get(
                &file_watcher_span
                    .get("span_id")
                    .expect("A `span_id` in the FileWatcher span")
                    .to_string()
            )
            .copied()
            .unwrap_or_default()
            <= 2
    );
    Ok(())
}

#[buck_test]
async fn test_stale_snapshot(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let original = fixture("my_genrule0");
    let log = buck.log(&["show", &original]).await?.stdout;

    // Now we're going to make a new log where we just delay the last event by
    // some amount of time.
    let mut lines: Vec<String> = log.lines().map(str::to_owned).collect();

    // Last event (last line is command result).
    let last_index = lines.len() - 2;
    let mut last: Value = serde_json::from_str(&lines[last_index])?;
    let timestamp = &mut last["Event"]["timestamp"][0];
    *timestamp = Value::from(timestamp.as_i64().expect("a timestamp") + 20);
    lines[last_index] = serde_json::to_string(&last)?;

    let logfile = tmp_path.join("test.json-lines");

    std::fs::write(&logfile, lines.join("\n"))?;
    let logfile = logfile.to_string_lossy();

    let stale_message = "Resource usage: <snapshot is stale>";

    // Check it's there.
    let res = buck.log(&["replay", &logfile]).await?;
    assert!(res.stderr.contains(stale_message));

    // AI agents get "Waiting on ..." at most once a minute; resource reports keep
    // the 7s cadence.
    let res = buck
        .log(&["replay", &logfile])
        .env("CODING_AGENT_METADATA", "id=e2e_test_agent")
        .await?;
    assert!(res.stderr.contains(stale_message));
    assert!(!res.stderr.contains("Waiting on buck2 daemon"));

    // Check it's not in the original one.
    let res = buck.log(&["replay", &original]).await?;
    assert!(!res.stderr.contains(stale_message));
    Ok(())
}

#[buck_test]
async fn test_super_console_changes(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.log(&["replay", &fixture("my_genrule1")]).await?;
    assert!(
        res.stderr
            .contains("File changed: fbcode//buck2/dir1/file1")
    );
    assert!(res.stderr.contains("Directory changed: fbcode//buck2/dir1"));
    Ok(())
}

#[buck_test(
    extra_buck_config = [("buck2_system_warning", "memory_pressure_threshold_percent", "1")],
    env = [("BUCK2_TEST_FAKE_SYSTEM_TOTAL_MEMORY", "1000")],
)]
async fn test_system_memory_exceeded_warning(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&["//:slow", "--console=simple"]).await?;
    assert!(res.stderr.contains("High memory pressure"));
    Ok(())
}
