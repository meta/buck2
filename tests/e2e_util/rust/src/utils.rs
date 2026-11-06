/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Helpers for what tests commonly do between commands.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context;
use regex::NoExpand;
use regex::Regex;
use serde_json::Value;

use crate::Buck;

/// The values at `path` in the events of the last command's event log (`buck2 log show`), for each
/// event that has one.
pub async fn filter_events(buck: &Buck, path: &[&str]) -> anyhow::Result<Vec<Value>> {
    events(buck, path, false).await
}

/// The events of the last command's event log that have a value at `path`, whole.
pub async fn filter_event_roots(buck: &Buck, path: &[&str]) -> anyhow::Result<Vec<Value>> {
    events(buck, path, true).await
}

async fn events(buck: &Buck, path: &[&str], whole: bool) -> anyhow::Result<Vec<Value>> {
    let log = buck.log(&["show"]).await?;
    let mut found = Vec::new();
    for line in log.stdout.trim().lines() {
        let event: Value =
            serde_json::from_str(line).with_context(|| format!("Parsing the event `{line}`"))?;
        if let Some(value) = json_get(&event, path) {
            found.push(if whole { event.clone() } else { value.clone() });
        }
    }
    Ok(found)
}

/// The value at `path` in `value`, through nested objects. A `null` counts as no value.
pub fn json_get<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |value, key| value.get(key))
        .filter(|value| !value.is_null())
}

/// The times, in milliseconds since the epoch, of the events of the last command's event log that
/// have a value at `path`.
pub async fn read_timestamps(buck: &Buck, path: &[&str]) -> anyhow::Result<Vec<u64>> {
    filter_event_roots(buck, path)
        .await?
        .iter()
        .map(|event| {
            let timestamp = &event["Event"]["timestamp"];
            let (s, ns) = (timestamp[0].as_u64(), timestamp[1].as_u64());
            match (s, ns) {
                (Some(s), Some(ns)) if ns < 1_000_000_000 => Ok(s * 1000 + ns / 1_000_000),
                _ => anyhow::bail!("An event without a timestamp: {event}"),
            }
        })
        .collect()
}

/// The `execution_kind` of the last action execution in the last command's event log that has one
/// that is not in `excluded`, and, if given, the `category` and target `name`.
pub async fn get_last_execution_kind(
    buck: &Buck,
    category: Option<&str>,
    excluded: &[i64],
    target_name: Option<&str>,
) -> anyhow::Result<Option<i64>> {
    let executions = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    Ok(executions.iter().rev().find_map(|execution| {
        let kind = execution.get("execution_kind")?.as_i64()?;
        if excluded.contains(&kind) {
            return None;
        }
        if category
            .is_some_and(|c| json_get(execution, &["name", "category"]) != Some(&Value::from(c)))
        {
            return None;
        }
        if target_name.is_some_and(|n| {
            json_get(execution, &["key", "owner", "TargetLabel", "label", "name"])
                != Some(&Value::from(n))
        }) {
            return None;
        }
        Some(kind)
    }))
}

/// The actions the last command ran, as `buck2 log what-ran --format json <args>` lists them.
pub async fn read_what_ran(buck: &Buck, args: &[&str]) -> anyhow::Result<Vec<Value>> {
    let mut log_args = vec!["what-ran", "--format", "json"];
    log_args.extend(args);
    let out = buck.log(&log_args).await?;
    out.stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).with_context(|| format!("Parsing `{line}`")))
        .collect()
}

/// Panics unless the last command ran `n` actions.
pub async fn expect_exec_count(buck: &Buck, n: usize) -> anyhow::Result<()> {
    let what_ran = read_what_ran(buck, &[]).await?;
    assert_eq!(what_ran.len(), n, "unexpected actions: {what_ran:#?}");
    Ok(())
}

/// The reports of a `--streaming-build-report` file, one per line. It may be empty.
pub fn read_streaming_report(path: &Path) -> anyhow::Result<Vec<Value>> {
    std::fs::read_to_string(path)
        .with_context(|| format!("Reading `{}`", path.display()))?
        .lines()
        .map(|line| serde_json::from_str(line).with_context(|| format!("Parsing `{line}`")))
        .collect()
}

/// The `results` of streaming reports merged, the newest entry for a key winning: it works whether
/// each report repeats every entry so far or carries only some of them.
pub fn replay_streaming_report(reports: &[Value]) -> BTreeMap<String, Value> {
    reports
        .iter()
        .filter_map(|report| report["results"].as_object())
        .flatten()
        .map(|(key, entry)| (key.clone(), entry.clone()))
        .collect()
}

/// Each key's successive top-level `success` values across streaming reports, repeats collapsed so
/// that the result does not depend on how many reports carry an entry.
pub fn streaming_status_history(reports: &[Value]) -> BTreeMap<String, Vec<Value>> {
    let mut history: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (key, entry) in reports
        .iter()
        .filter_map(|report| report["results"].as_object())
        .flatten()
    {
        let statuses = history.entry(key.clone()).or_default();
        if statuses.last() != Some(&entry["success"]) {
            statuses.push(entry["success"].clone());
        }
    }
    history
}

/// Replaces configuration hashes with `<HASH>`.
pub fn replace_hash(s: &str) -> String {
    static HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-f]{16}\b").unwrap());
    HASH.replace_all(s, NoExpand("<HASH>")).into_owned()
}

/// Replaces action digests with `<DIGEST>`.
pub fn replace_digest(s: &str) -> String {
    static DIGEST: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b[0-9a-f]{40}:[0-9]{1,3}\b").unwrap());
    DIGEST.replace_all(s, NoExpand("<DIGEST>")).into_owned()
}

/// Deletes a build output, which is read-only (`buck2.readonly_outputs`): Windows refuses until the
/// attribute is cleared.
pub fn remove_output(path: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    #[expect(
        clippy::permissions_set_readonly_false,
        reason = "The output is being deleted"
    )]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions)?;
    std::fs::remove_file(path)
}

/// The identity of `path`'s file on its filesystem, as Python's `os.stat(path).st_ino` gives it:
/// its inode number, or on Windows its file index, which the standard library does not expose yet.
pub fn file_id(path: &Path) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(path)?.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION;
        use windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES;
        use windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle;

        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .open(path)?;
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle is open for the call, and `information` is the struct it fills.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok((u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow))
    }
}

/// 256 random lowercase letters.
pub fn random_string() -> String {
    std::iter::repeat_with(|| uuid::Uuid::new_v4().into_bytes())
        .flatten()
        .take(256)
        .map(|b| char::from(b'a' + b % 26))
        .collect()
}

/// Replaces every occurrence of `old` in `file`, which must have one.
#[track_caller]
pub fn replace_in_file(old: &str, new: &str, file: &Path) {
    let content = std::fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("Reading `{}`: {e}", file.display()));
    assert!(
        content.contains(old),
        "`{old}` not found in `{}`",
        file.display()
    );
    std::fs::write(file, content.replace(old, new))
        .unwrap_or_else(|e| panic!("Writing `{}`: {e}", file.display()));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_json_get_walks_objects() {
        let event = json!({"Event": {"data": {"SpanEnd": {"name": "x"}}}});
        assert_eq!(
            json_get(&event, &["Event", "data", "SpanEnd", "name"]),
            Some(&json!("x"))
        );
        assert_eq!(json_get(&event, &["Event", "data", "SpanStart"]), None);
    }

    #[test]
    fn test_json_get_treats_null_as_missing() {
        assert_eq!(json_get(&json!({"a": null}), &["a"]), None);
    }

    #[test]
    fn test_streaming_reports_replay_to_the_newest_entries() {
        let reports = [
            json!({"results": {"a": {"success": "PENDING"}, "b": {"success": "SUCCESS"}}}),
            json!({"results": {"a": {"success": "PENDING"}}}),
            json!({"results": {"a": {"success": "FAIL"}}}),
        ];
        assert_eq!(
            replay_streaming_report(&reports),
            BTreeMap::from([
                ("a".to_owned(), json!({"success": "FAIL"})),
                ("b".to_owned(), json!({"success": "SUCCESS"})),
            ])
        );
        assert_eq!(
            streaming_status_history(&reports),
            BTreeMap::from([
                ("a".to_owned(), vec![json!("PENDING"), json!("FAIL")]),
                ("b".to_owned(), vec![json!("SUCCESS")]),
            ])
        );
    }

    #[test]
    fn test_hashes_and_digests_get_placeholders() {
        assert_eq!(replace_hash("cfg#0123456789abcdef"), "cfg#<HASH>");
        assert_eq!(
            replace_digest("0123456789abcdef0123456789abcdef01234567:42"),
            "<DIGEST>"
        );
    }

    #[test]
    fn test_random_strings_are_lowercase_and_differ() {
        let (a, b) = (random_string(), random_string());
        assert_eq!(a.len(), 256);
        assert!(a.bytes().all(|c| c.is_ascii_lowercase()));
        assert_ne!(a, b);
    }

    #[test]
    fn test_replace_in_file_replaces_every_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "a b a").unwrap();
        replace_in_file("a", "c", &file);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "c b c");
    }

    #[test]
    fn test_file_id_follows_the_file_not_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let id = file_id(&a).unwrap();
        assert_eq!(file_id(&a).unwrap(), id);
        assert_ne!(file_id(&b).unwrap(), id);
        std::fs::rename(&a, &b).unwrap();
        assert_eq!(file_id(&b).unwrap(), id);
    }

    #[test]
    #[should_panic(expected = "not found")]
    fn test_replace_in_file_requires_an_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, "b").unwrap();
        replace_in_file("a", "c", &file);
    }
}
