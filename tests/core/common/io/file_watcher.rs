/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The file watcher events of a command, from its event log.

use std::path::Path;

use anyhow::Context;
use buck2_e2e::Buck;
use buck2_e2e::utils::json_get;
use serde_json::Value;

/// The file watcher implementations, as `buck2.file_watcher` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileWatcherProvider {
    Watchman,
    RustNotify,
    FsHashCrawler,
    EdenFs,
}

/// What happened to a path, in the order of the event log's numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileWatcherEventType {
    Create,
    Modify,
    Delete,
}

/// What a path is, in the order of the event log's numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileWatcherKind {
    File,
    Directory,
    Symlink,
}

/// One change the file watcher reported. Events order by type, then kind, then path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileWatcherEvent {
    pub event: FileWatcherEventType,
    pub kind: FileWatcherKind,
    pub path: String,
}

impl FileWatcherEvent {
    pub fn new(event: FileWatcherEventType, kind: FileWatcherKind, path: &str) -> Self {
        Self {
            event,
            kind,
            path: path.to_owned(),
        }
    }

    /// The event as the event log writes it: `{"event": 0, "kind": 1, "path": "..."}`.
    fn from_json(event: &Value) -> anyhow::Result<Self> {
        let event_type = match event["event"].as_i64() {
            Some(0) => FileWatcherEventType::Create,
            Some(1) => FileWatcherEventType::Modify,
            Some(2) => FileWatcherEventType::Delete,
            _ => anyhow::bail!("Unknown file watcher event type in {event}"),
        };
        let kind = match event["kind"].as_i64() {
            Some(0) => FileWatcherKind::File,
            Some(1) => FileWatcherKind::Directory,
            Some(2) => FileWatcherKind::Symlink,
            _ => anyhow::bail!("Unknown file watcher kind in {event}"),
        };
        let path = event["path"]
            .as_str()
            .with_context(|| format!("No path in {event}"))?;
        Ok(Self::new(event_type, kind, path))
    }
}

/// The values at `path` in the events of the last command's event log, for each event that has
/// one, reading the log in `rel_cwd` if given.
async fn filter_events(
    buck: &Buck,
    path: &[&str],
    rel_cwd: Option<&Path>,
) -> anyhow::Result<Vec<Value>> {
    let log = match rel_cwd {
        Some(rel_cwd) => buck.log(&["show"]).rel_cwd(rel_cwd).await?,
        None => buck.log(&["show"]).await?,
    };
    log.stdout
        .trim()
        .lines()
        .filter_map(|line| match serde_json::from_str::<Value>(line) {
            Ok(event) => json_get(&event, path).cloned().map(Ok),
            Err(e) => Some(Err(
                anyhow::Error::new(e).context(format!("Parsing `{line}`"))
            )),
        })
        .collect()
}

/// Runs `targets root//:` and gives whether the file watcher started over (a fresh instance) and
/// the events it reported.
pub async fn get_file_watcher_events(buck: &Buck) -> anyhow::Result<(bool, Vec<FileWatcherEvent>)> {
    get_file_watcher_events_for(buck, "root//:", None).await
}

/// Runs `targets target_pattern` in `rel_cwd`, if given, and gives whether the file watcher
/// started over (a fresh instance) and the events it reported.
pub async fn get_file_watcher_events_for(
    buck: &Buck,
    target_pattern: &str,
    rel_cwd: Option<&Path>,
) -> anyhow::Result<(bool, Vec<FileWatcherEvent>)> {
    match rel_cwd {
        Some(rel_cwd) => buck.targets(&[target_pattern]).rel_cwd(rel_cwd).await?,
        None => buck.targets(&[target_pattern]).await?,
    };
    let filtered_events = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "FileWatcher",
            "stats",
            "events",
        ],
        rel_cwd,
    )
    .await?;
    let file_watcher_events = parse_events(&filtered_events)?;

    let fresh_instance = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "FileWatcher",
            "stats",
            "fresh_instance",
        ],
        rel_cwd,
    )
    .await?;
    let fresh_instance = fresh_instance
        .first()
        .and_then(Value::as_bool)
        .with_context(|| {
            format!("No `fresh_instance` in the file watcher stats: {fresh_instance:?}")
        })?;

    Ok((fresh_instance, file_watcher_events))
}

fn parse_events(filtered_events: &[Value]) -> anyhow::Result<Vec<FileWatcherEvent>> {
    filtered_events
        .iter()
        .flat_map(|events| events.as_array().map(Vec::as_slice).unwrap_or_default())
        .map(FileWatcherEvent::from_json)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_events_are_read_from_every_span() {
        let events = parse_events(&[
            json!([{"event": 0, "kind": 0, "path": "a"}]),
            json!([{"event": 2, "kind": 1, "path": "b"}, {"event": 1, "kind": 2, "path": "c"}]),
        ])
        .unwrap();
        assert_eq!(
            events,
            [
                FileWatcherEvent::new(FileWatcherEventType::Create, FileWatcherKind::File, "a"),
                FileWatcherEvent::new(
                    FileWatcherEventType::Delete,
                    FileWatcherKind::Directory,
                    "b"
                ),
                FileWatcherEvent::new(FileWatcherEventType::Modify, FileWatcherKind::Symlink, "c"),
            ]
        );
    }

    #[test]
    fn test_an_unknown_event_type_is_an_error() {
        assert!(parse_events(&[json!([{"event": 3, "kind": 0, "path": "a"}])]).is_err());
    }

    #[test]
    fn test_events_order_by_type_then_kind_then_path() {
        let event = |event, kind, path| FileWatcherEvent::new(event, kind, path);
        let mut events = vec![
            event(FileWatcherEventType::Delete, FileWatcherKind::File, "a"),
            event(
                FileWatcherEventType::Create,
                FileWatcherKind::Directory,
                "a",
            ),
            event(FileWatcherEventType::Create, FileWatcherKind::File, "b"),
            event(FileWatcherEventType::Create, FileWatcherKind::File, "a"),
        ];
        events.sort();
        assert_eq!(
            events,
            [
                event(FileWatcherEventType::Create, FileWatcherKind::File, "a"),
                event(FileWatcherEventType::Create, FileWatcherKind::File, "b"),
                event(
                    FileWatcherEventType::Create,
                    FileWatcherKind::Directory,
                    "a"
                ),
                event(FileWatcherEventType::Delete, FileWatcherKind::File, "a"),
            ]
        );
    }
}
