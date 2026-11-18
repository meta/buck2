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
use std::process::Command;
use std::time::Duration;

use buck2_e2e::golden::sanitize_daemon_stderr;
use buck2_e2e::prelude::*;
use regex::Regex;

fn get_process_name(pid: u64) -> Option<String> {
    if cfg!(windows) {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .expect("running `tasklist`");
        return String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| {
                let row: Vec<&str> = line.trim().trim_matches('"').split("\",\"").collect();
                (row.len() >= 2 && row[1] == pid.to_string()).then(|| row[0].to_owned())
            });
    }

    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("ps");
        command.args(["-o", "comm=", &pid.to_string()]);
        command
    } else if cfg!(target_os = "linux") {
        let mut command = Command::new("ps");
        command.args(["-o", "cmd=", &pid.to_string()]);
        command
    } else {
        panic!("Unknown platform: {}", std::env::consts::OS);
    };

    let output = command.output().expect("running `ps`");
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// Removes the EdenFS checkout the harness mounted the project on.
fn eden_remove(eden_dir: &Path, project_dir: &Path) {
    let _ignored = Command::new("eden")
        .arg("--config-dir")
        .arg(eden_dir.join("config"))
        .arg("--home-dir")
        .arg(eden_dir.join("home"))
        .arg("--etc-eden-dir")
        .arg(eden_dir.join("etc"))
        .arg("remove")
        .arg(project_dir)
        .arg("-y")
        .stdout(std::io::stderr())
        .stderr(std::io::stderr())
        .status();
}

// The daemon noticing its project root is gone is the `daemon_project_root_unavailable`
// soft error.
#[buck_test(
    setup_eden = true,
    skip_final_kill = true,
    allow_soft_errors = true,
    env = [("BUCK2_TESTING_CHECKER_INTERVAL_SECONDS", "1")],
)]
async fn test_daemon_killed_on_checkout_removal(buck: &Buck) -> anyhow::Result<()> {
    // Start the daemon and capture its PID and daemon dir before removal.
    buck.server(&[]).await?;

    let status: serde_json::Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    let pid = status["process_info"]["pid"].as_u64().unwrap();
    let process_name = get_process_name(pid);
    assert!(
        process_name.is_some(),
        "Could not find daemon process {pid}"
    );
    let daemon_dir = buck.daemon_dir().await?;

    let project_dir = buck.cwd().to_owned();
    let eden_dir = project_dir.parent().unwrap().join("eden");
    eden_remove(&eden_dir, &project_dir);

    assert!(
        !project_dir.exists(),
        "Eden checkout was not removed: {}",
        project_dir.display()
    );

    // Wait for the daemon to detect the missing project root and shut down.
    tokio::time::sleep(Duration::from_secs(20)).await;
    if get_process_name(pid) == process_name {
        panic!("Server did not die in 20 seconds");
    }

    // Process is dead. Verify the shutdown reason in daemon stderr.
    let stderr = std::fs::read_to_string(daemon_dir.join("buckd.stderr"))?;
    // Replace the project root path before general sanitization.
    // sanitize_daemon_stderr only handles /data/users/ (Linux);
    // macOS scratch paths (e.g. /var/folders/…) need explicit replacement.
    let stderr = stderr.replace(&*project_dir.to_string_lossy(), "<SCRATCH_PATH>");
    let stderr = sanitize_daemon_stderr(&stderr);
    // Strip daemon_listener startup log lines — they can change between
    // versions and aren't what this test validates.
    let stderr = Regex::new(r"(?m)^.*daemon_listener:.*\n")
        .unwrap()
        .replace_all(&stderr, "")
        .into_owned();

    if cfg!(windows) {
        // Windows produces a different OS error (e.g. "The system cannot
        // find the path specified" instead of "No such file or directory"),
        // so skip the golden comparison and only check the key message so
        // that we don't have to update the golden file on 2 OSes every time
        // the error message changes.
        assert!(stderr.contains("is no longer accessible"));
    } else {
        golden(
            &stderr,
            "fixtures/test_daemon_killed_on_checkout_removal.golden.txt",
        );
    }
    Ok(())
}
