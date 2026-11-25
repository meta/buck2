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
use std::time::Duration;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::json_get;
use serde_json::Value;

const WRITTEN_BY_DAEMON: &str = "--setting=event_log.written_by_daemon=true";

/// The events of the last command's event log, or none while there is no log to read.
async fn events(buck: &Buck) -> Vec<Value> {
    let Ok(shown) = buck.log(&["show"]).await else {
        return Vec::new();
    };
    shown
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The last command's event log, once the daemon has finished it: it does so after it has
/// sent the command's result.
async fn finished_log(buck: &Buck) -> Vec<Value> {
    for _ in 0..100 {
        let log = events(buck).await;
        if log.last().is_some_and(|e| e.get("Result").is_some()) {
            return log;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("The daemon did not finish the event log");
}

/// A command that asks the daemon for the event log has one log, where the client's would be,
/// and it holds the command from its start to its result.
#[buck_test(write_invocation_record = true)]
async fn test_daemon_writes_the_event_log(buck: &Buck) -> anyhow::Result<()> {
    let built = buck.build(&[WRITTEN_BY_DAEMON]).await?;

    let log = finished_log(buck).await;
    let has = |path: &[&str]| log.iter().any(|e| json_get(e, path).is_some());
    assert!(has(&["Event", "data", "SpanStart", "data", "Command"]));
    assert!(has(&["Event", "data", "SpanEnd", "data", "Command"]));
    // What the client sends the daemon about itself is in the log too.
    assert!(has(&["Event", "data", "Instant", "data", "ClientSnapshot"]));

    let path = log
        .iter()
        .find_map(|e| {
            let path = ["Event", "data", "Instant", "data", "DaemonEventLog", "path"];
            json_get(e, &path)?.as_str()
        })
        .expect("the log says where the daemon writes it");
    let logs: Vec<_> = std::fs::read_dir(Path::new(path).parent().unwrap())?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.to_string_lossy().ends_with("_events.pb.zst"))
        .collect();
    assert_eq!(logs, [Path::new(path)]);

    // The daemon says how much of the log it had written when it sent the result, which is
    // not all of it, and the client records that.
    let said = log.last().unwrap()["Result"]["event_log_size_bytes"].as_u64();
    let file_bytes = std::fs::metadata(path)?.len();
    assert!(said.is_some_and(|said| 0 < said && said <= file_bytes));
    let recorded = &built.invocation_record()?["compressed_event_log_size_bytes"];
    assert_eq!(recorded.as_u64(), said);
    Ok(())
}

#[buck_test]
async fn test_client_writes_the_event_log_unasked(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[]).await?;

    let daemon_event_log = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "DaemonEventLog"],
    )
    .await?;
    assert!(daemon_event_log.is_empty());
    assert!(!events(buck).await.is_empty());
    Ok(())
}

/// A command that is told to write no event log does not ask the daemon for one either.
#[buck_test]
async fn test_no_event_log(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["--no-event-log", WRITTEN_BY_DAEMON]).await?;

    let log_dir = buck.cwd().join("buck-out").join("v2").join("log");
    let logs = std::fs::read_dir(log_dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with("_events.pb.zst"))
        .count();
    assert_eq!(logs, 0);
    Ok(())
}

/// A daemon that cannot open the log reports that to the command the log is of, under that
/// command's soft error policy, and the command goes on without an event log. The daemon here
/// was started by a command with another policy.
#[buck_test(allow_soft_errors = true)]
async fn test_daemon_reports_a_log_it_cannot_open(buck: &Buck) -> anyhow::Result<()> {
    // A file where the log directory goes.
    let buck_out = buck.cwd().join("buck-out").join("v2");
    std::fs::create_dir_all(&buck_out)?;
    std::fs::write(buck_out.join("log"), "")?;
    buck.build(&["--no-event-log"]).await?;

    let built = buck
        .build(&[WRITTEN_BY_DAEMON])
        .env("BUCK2_SHOW_SOFT_ERRORS", "true")
        .await?;

    assert!(
        built.stderr.contains("Error creating event log directory"),
        "{}",
        built.stderr
    );
    assert!(buck_out.join("log").is_file());
    Ok(())
}

/// `debug allocative` is not run by the daemon like other commands. The daemon writes its log
/// too.
#[buck_test]
async fn test_daemon_writes_the_event_log_of_debug_allocative(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    std::fs::write(
        buck.settings_home_dir().join(".bucksettings.local.toml"),
        "[event_log]\nwritten_by_daemon = true\n",
    )?;
    // The command needs a daemon that is running.
    buck.build(&[]).await?;
    finished_log(buck).await;
    let output = tmp_path.join("allocative");
    buck.debug(&["allocative", "--output", output.to_str().unwrap()])
        .await?;

    let log = finished_log(buck).await;
    let args = log[0]["command_line_args"].as_array().unwrap();
    assert!(args.iter().any(|arg| arg == "allocative"), "{args:?}");
    Ok(())
}

/// The header of the daemon's log has the command line, as it was given and with its
/// argfiles expanded, and a start time that is no later than the command's first event.
#[buck_test]
async fn test_daemon_event_log_header(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join("args"), "--config=foo.bar=1\n")?;
    buck.build(&["@args", WRITTEN_BY_DAEMON]).await?;

    let log = finished_log(buck).await;
    let args = |key: &str| -> Vec<&str> {
        let args = log[0][key].as_array().unwrap();
        args.iter().filter_map(Value::as_str).collect()
    };
    assert!(args("command_line_args").contains(&"@args"));
    assert!(args("expanded_command_line_args").contains(&"--config=foo.bar=1"));

    let time = |value: &Value| (value[0].as_u64().unwrap(), value[1].as_u64().unwrap());
    assert!(time(&log[0]["start_time"]) <= time(&log[1]["Event"]["timestamp"]));
    Ok(())
}
