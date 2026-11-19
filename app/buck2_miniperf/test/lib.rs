/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(test)]
#![cfg(target_os = "linux")]
#![feature(exit_status_error)]

use std::process::Command;

use anyhow::Context;
use buck2_miniperf_proto::MiniperfOutput;

#[test]
fn test_miniperf() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("out");

    // ast-grep-ignore: rust/buck2-no-command-new
    let mut cmd = Command::new(std::env::var_os("MINIPERF").context("$MINIPERF is not set")?);
    cmd.arg(&path);
    cmd.arg(
        std::env::var_os("THREE_BILLION_INSTRUCTIONS")
            .context("$THREE_BILLION_INSTRUCTIONS is not set")?,
    );
    cmd.status()?.exit_ok()?;

    let out = std::fs::read(&path).context("Failed to read")?;
    let (out, _) =
        bincode::serde::decode_from_slice::<MiniperfOutput, _>(&out, bincode::config::legacy())
            .context("Failed to deserialize")?;

    // Check that we're within 5%
    assert!(
        out.counters
            .as_ref()
            .unwrap()
            .user_instructions
            .adjusted_count()
            > 2850000000
    );
    assert!(
        out.counters
            .as_ref()
            .unwrap()
            .user_instructions
            .adjusted_count()
            < 3150000000
    );

    Ok(())
}

#[test]
fn test_miniperf_forwards_sigterm() -> anyhow::Result<()> {
    use std::io::BufRead;
    use std::io::BufReader;
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("out");
    let marker = temp_dir.path().join("got_sigterm");

    // ast-grep-ignore: rust/buck2-no-command-new
    let mut cmd = Command::new(std::env::var_os("MINIPERF").context("$MINIPERF is not set")?);
    cmd.arg(&path)
        .args(["sh", "-c"])
        .arg("trap 'touch \"$MARKER\"; kill $!; exit 3' TERM; echo ready; sleep 1000 & wait")
        .env("MARKER", &marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut miniperf = cmd.spawn()?;
    let pid = miniperf.id() as i32;
    // Kills what is left of the group if an assertion fails, so that it does not hold the test's
    // output open. Dropped while miniperf is unreaped, so the group id is still its own.
    let cleanup = buck2_process_kill::KillGroupOnDrop::new(pid);

    let mut ready = String::new();
    BufReader::new(miniperf.stdout.take().context("no stdout")?).read_line(&mut ready)?;
    assert_eq!(ready.trim(), "ready");

    buck2_process_kill::terminate(pid)?;

    let start = Instant::now();
    while buck2_process_kill::child_state(pid) == buck2_process_kill::ChildState::Running {
        assert!(
            Instant::now() - start < Duration::from_secs(30),
            "miniperf should exit once the command it forwarded SIGTERM to exits"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(cleanup);
    miniperf.wait()?.exit_ok()?;
    assert!(
        marker.exists(),
        "the wrapped command should receive SIGTERM"
    );

    let out = std::fs::read(&path).context("Failed to read")?;
    let (out, _) =
        bincode::serde::decode_from_slice::<MiniperfOutput, _>(&out, bincode::config::legacy())
            .context("Failed to deserialize")?;
    let raw = out.raw_exit_code.map_err(anyhow::Error::msg)?;
    assert_eq!(
        std::process::ExitStatus::from_raw(raw).code(),
        Some(3),
        "miniperf should report the exit status of the wrapped command"
    );

    Ok(())
}
