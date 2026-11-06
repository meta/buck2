/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::BuckCommand;
use buck2_e2e::InvocationRecord;
use buck2_e2e::golden::sanitize_daemon_stderr;
use buck2_e2e::golden::sanitize_stacktrace;
use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;
use serde_json::Value;

async fn wait_for_daemon_pid(buck: &Buck) -> anyhow::Result<u32> {
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let status = buck.status(&[]).await?;
        if status.stderr != "no buckd running" {
            let status: Value = serde_json::from_str(&status.stdout)?;
            let pid = status["process_info"]["pid"]
                .as_u64()
                .context("No `process_info.pid` in the status")?;
            return Ok(u32::try_from(pid)?);
        }
    }
    anyhow::bail!("Failed to find buckd pid")
}

fn read_daemon_cgroup(pid: u32) -> anyhow::Result<String> {
    fs::read_to_string(format!("/proc/{pid}/cgroup"))?
        .lines()
        .find(|line| line.starts_with("0::/"))
        .map(|line| line["0::".len()..].to_owned())
        .with_context(|| format!("Could not find cgroup v2 entry for daemon PID {pid}"))
}

/// A `dmesg` that prints `output`, first in the `PATH` of a command run with the returned
/// environment, and the file it creates when it runs.
fn fake_dmesg_env(
    tmp_path: &Path,
    output: &str,
) -> anyhow::Result<(Vec<(&'static str, OsString)>, PathBuf)> {
    let dmesg_output = tmp_path.join("dmesg-output");
    fs::write(&dmesg_output, output)?;
    let fake_bin = tmp_path.join("fake-bin");
    fs::create_dir(&fake_bin)?;
    let fake_dmesg = fake_bin.join("dmesg");
    fs::write(
        &fake_dmesg,
        "#!/bin/sh\n: > \"$BUCK2_TEST_DMESG_CALLED\"\nexec /bin/cat \"$BUCK2_TEST_DMESG_OUTPUT\"\n",
    )?;
    set_mode(&fake_dmesg, 0o755)?;
    let dmesg_called = tmp_path.join("dmesg-called");
    let path = std::env::join_paths(std::iter::once(fake_bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").context("`PATH` is not set")?,
    )))?;
    Ok((
        vec![
            (
                "BUCK2_TEST_DMESG_CALLED",
                dmesg_called.clone().into_os_string(),
            ),
            ("BUCK2_TEST_DMESG_OUTPUT", dmesg_output.into_os_string()),
            ("PATH", path),
        ],
        dmesg_called,
    ))
}

fn with_env<'a>(command: BuckCommand<'a>, env: &[(&str, OsString)]) -> BuckCommand<'a> {
    env.iter()
        .fold(command, |command, (key, value)| command.env(*key, value))
}

/// The kernel log's timestamp of `seconds_ago` seconds ago: `CLOCK_MONOTONIC`, as
/// `seconds.microseconds`.
#[cfg(unix)]
fn current_dmesg_timestamp(seconds_ago: u64) -> anyhow::Result<String> {
    let mut timestamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `timestamp` is valid, writable storage for a `timespec` for the whole call, and
    // `clock_gettime` keeps no pointer to it.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timestamp) };
    anyhow::ensure!(
        rc == 0,
        "clock_gettime(CLOCK_MONOTONIC) failed: {}",
        std::io::Error::last_os_error()
    );
    let monotonic = Duration::new(
        u64::try_from(timestamp.tv_sec)?,
        u32::try_from(timestamp.tv_nsec)?,
    )
    .checked_sub(Duration::from_secs(seconds_ago))
    .context("The system has not been up for that long")?;
    Ok(format!(
        "{}.{:06}",
        monotonic.as_secs(),
        monotonic.subsec_micros()
    ))
}

#[cfg(not(unix))]
fn current_dmesg_timestamp(_seconds_ago: u64) -> anyhow::Result<String> {
    unreachable!("the tests that read the kernel log do not run on Windows")
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    unreachable!("the tests that change file modes do not run on Windows")
}

#[cfg(unix)]
fn kill(pid: u32) -> anyhow::Result<()> {
    let pid = libc::pid_t::try_from(pid)?;
    // SAFETY: `kill` takes no pointers.
    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn kill(_pid: u32) -> anyhow::Result<()> {
    unreachable!("test_daemon_killed does not run on Windows")
}

/// `value`, which must be a string.
#[track_caller]
fn string(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("Expected a string, got {value}"))
}

/// The `tags` of an error, which must be a list.
#[track_caller]
fn tags(error: &Value) -> &[Value] {
    error["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("No `tags` list in {error}"))
}

fn has_tag(error: &Value, tag: &str) -> bool {
    tags(error).iter().any(|t| t == tag)
}

/// Restores a file's mode when dropped.
struct RestoreMode<'a> {
    path: &'a Path,
    mode: u32,
}

impl Drop for RestoreMode<'_> {
    fn drop(&mut self) {
        let _ignored = set_mode(self.path, self.mode);
    }
}

#[buck_test(write_invocation_record = true)]
async fn test_action_error(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&["//:action_fail"]).await.expect_failure();
    res.assert_stderr_matches("Failed to build 'root//:action_fail");
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["category"], "USER");
    // This test is unfortunately liable to break as a result of refactorings, since this is not
    // stable. Feel free to delete it if it becomes a problem.
    assert!(
        string(&error["source_location"])
            .starts_with("buck2_build_api/src/actions/errors/action_error.rs::ActionError::"),
        "{}",
        error["source_location"]
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_missing_outputs(buck: &Buck) -> anyhow::Result<()> {
    // FIXME(JakobDegen): This doesn't work with non-local-only actions
    let res = buck
        .build(&["//:missing_outputs", "--local-only"])
        .await
        .expect_failure();
    res.assert_stderr_matches("Failed to build 'root//:missing_outputs");
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["category"], "USER");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_bad_url(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&["//:bad_url"]).await.expect_failure();
    res.assert_stderr_matches("Failed to build 'root//:bad_url");
    let record = res.invocation_record()?;
    let error = record.single_error();
    // Also liable to break as a result of refactorings, feel free to update
    // FIXME(minglunli): This is a regression from before, the commented line is better and we should fix this
    assert!(
        string(&error["source_location"]).contains("buck2_http/src/lib.rs"),
        "{}",
        error["source_location"]
    );
    // assert_eq!(error["source_location"], "buck2_http/src/lib.rs::HttpError::SendRequest");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_attr_coercion(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["//attr_coercion:int_rule"])
        .await
        .expect_failure();
    res.assert_stderr_matches("evaluating build file: `root//attr_coercion:TARGETS.fixture");
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(
        string(&error["source_location"]).contains("StarlarkError::RuntimeType::"),
        "{}",
        error["source_location"]
    );
    assert!(has_tag(error, "STARLARK_VALUE"), "{}", error["tags"]);
    assert!(
        has_tag(error, "STARLARK_RUNTIME_TYPE_ERROR"),
        "{}",
        error["tags"]
    );
    assert!(
        string(&error["category_key"]).ends_with("starlark_runtime_type_error=VALUE_UNPACK"),
        "{}",
        error["category_key"]
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_buck2_fail(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&["//buck2_fail:foobar"]).await.expect_failure();
    res.assert_stderr_matches("evaluating build file: `root//buck2_fail:TARGETS.fixture`");
    let record = res.invocation_record()?;
    let error = record.single_error();
    // Just make sure that despite there being no context on the error, we still report the right
    // metadata
    assert!(
        string(&error["source_location"]).starts_with(
            "buck2_interpreter_for_build/src/interpreter/functions/internals.rs::BuckFail::"
        ),
        "{}",
        error["source_location"]
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_starlark_fail_error_categorization(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["//starlark_fail:foobar"])
        .await
        .expect_failure();
    res.assert_stderr_matches("evaluating build file: `root//starlark_fail:TARGETS.fixture`");
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(
        string(&error["source_location"]).contains("StarlarkError::Fail::"),
        "{}",
        error["source_location"]
    );
    assert_eq!(error["source_area"], "BUCK2");
    assert_eq!(error["category"], "USER");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_starlark_parse_error_categorization(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["//starlark_parse_error:starlark_parse_error"])
        .await
        .expect_failure();
    res.assert_stderr_matches(".*Parse error:.*");
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(
        string(&error["source_location"]).contains("StarlarkError::Parser::"),
        "{}",
        error["source_location"]
    );
    assert_eq!(error["tags"], serde_json::json!(["STARLARK_PARSER"]));
    assert_eq!(error["source_area"], "BUCK2");
    assert_eq!(error["category"], "USER");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_starlark_scope_error_categorization(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["//starlark_scope_error:value_err"])
        .await
        .expect_failure();
    res.assert_stderr_matches("evaluating build file: .* not found");
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(
        string(&error["source_location"]).contains("StarlarkError::Scope::"),
        "{}",
        error["source_location"]
    );
    assert_eq!(error["tags"], serde_json::json!(["STARLARK_SCOPE"]));
    assert_eq!(error["source_area"], "BUCK2");
    assert_eq!(error["category"], "USER");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_targets_error_categorization(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .targets(&["//starlark_fail:foobar"])
        .await
        .expect_failure();
    res.assert_stderr_matches("evaluating build file: `root//starlark_fail:TARGETS.fixture`");
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["tags"], serde_json::json!(["INPUT", "STARLARK_FAIL"]));
    assert_eq!(error["category"], "USER");

    golden(
        &sanitize_stderr(&res.stderr),
        "fixtures/test_targets_error_categorization.golden.txt",
    );
    Ok(())
}

#[buck_test(write_invocation_record = true, allow_daemon_panics = true)]
async fn test_daemon_crash(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[]).await?;

    let res = buck.debug(&["crash", "panic"]).await.expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    let message = string(&error["message"]);
    if cfg!(windows) {
        assert!(message.contains("transport error"), "{message}");
    } else {
        assert!(
            message.contains("stream closed because of a broken pipe"),
            "{message}"
        );
    }

    assert_eq!(
        tags(error).iter().take(3).collect::<Vec<_>>(),
        ["CLIENT_GRPC", "CLIENT_GRPC_STREAM", "SERVER_PANICKED"]
    );
    assert!(message.contains("buckd stderr:\n"), "{message}");
    assert!(message.contains("panicked at"), "{message}");

    assert_eq!(error["best_tag"], "SERVER_PANICKED");
    let category_key = string(&error["category_key"]);
    assert!(
        category_key.starts_with("SERVER_PANICKED"),
        "{category_key}"
    );

    // A crash key is only attached when the panic output has stack frames. Only Linux is known
    // to print frames the classifier parses.
    if cfg!(target_os = "linux") {
        assert!(category_key.contains("crash("), "{category_key}");
        assert!(
            tags(error)
                .get(4)
                .and_then(Value::as_str)
                .is_some_and(|t| t.starts_with("crash(")),
            "{}",
            error["tags"]
        );
    }

    if !cfg!(windows) {
        golden(
            &sanitize_stacktrace(&res.stderr),
            "fixtures/test_daemon_crash.golden.txt",
        );
    }
    Ok(())
}

#[buck_test(
    write_invocation_record = true,
    env = [("BUCKD_STARTUP_TIMEOUT", "0"), ("BUCKD_STARTUP_INIT_TIMEOUT", "0")],
)]
async fn test_connection_timeout(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&[":"]).await.expect_failure();
    assert!(
        res.stderr
            .contains("timed out before establishing connection to Buck daemon")
    );

    let record = res.invocation_record()?;

    assert_eq!(record.get("command_end"), Some(&Value::Null));
    assert_eq!(record["has_command_result"], false);
    assert_eq!(record["has_end_of_stream"], false);
    assert_eq!(record["daemon_connection_failure"], true);
    assert_eq!(record.get("daemon_was_started"), Some(&Value::Null));

    assert_eq!(
        record.single_error()["category_key"],
        "CLIENT_STARTUP_TIMEOUT"
    );

    if !cfg!(windows) {
        golden(
            &sanitize_stderr(&res.stderr),
            "fixtures/test_connection_timeout.golden.txt",
        );
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_daemon_abort(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[]).await?;

    let res = buck.debug(&["crash", "abort"]).await.expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    let category_key = string(&error["category_key"]);
    let message = string(&error["message"]);

    if cfg!(windows) {
        // TODO get windows to dump a stack trace / detect signals
        assert!(message.contains("buckd stderr is empty"), "{message}");
        assert_eq!(category_key, "DAEMON_DISCONNECT");
    } else {
        // Messages from folly's signal handler.
        assert!(message.contains("*** Aborted at"), "{message}");
        assert!(message.contains("*** Signal 6 (SIGABRT)"), "{message}");
        assert!(category_key.starts_with("SERVER_SIGABRT"), "{category_key}");
        assert_eq!(error["best_tag"], "SERVER_SIGABRT");
    }

    if cfg!(target_os = "linux") {
        assert!(message.contains("\n    @ "), "{message}");
        assert!(category_key.contains("crash("), "{category_key}");
    }
    if category_key.contains("crash(") {
        // test string tags are in error_tags
        assert!(
            tags(error)
                .last()
                .and_then(Value::as_str)
                .is_some_and(|t| t.starts_with("crash(")),
            "{}",
            error["tags"]
        );
    }
    Ok(())
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    write_invocation_record = true,
    allow_daemon_panics = true,
)]
async fn test_pre_daemon_oom_record_does_not_mask_daemon_crash(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let pre_daemon_timestamp = current_dmesg_timestamp(60)?;
    buck.build(&[]).await?;
    let daemon_cgroup = read_daemon_cgroup(wait_for_daemon_pid(buck).await?)?;

    let (dmesg_env, dmesg_called) = fake_dmesg_env(
        tmp_path,
        &format!(
            "<6>[{pre_daemon_timestamp}] oomd kill: 81.28 80.05 42.78 {daemon_cgroup} 1000 ruleset:[test]\n"
        ),
    )?;

    let res = with_env(buck.debug(&["crash", "panic"]), &dmesg_env)
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(dmesg_called.exists());
    assert!(!has_tag(error, "DAEMON_OOM_KILLED"), "{}", error["tags"]);
    assert_eq!(error["best_tag"], "SERVER_PANICKED");
    Ok(())
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    write_invocation_record = true,
    allow_daemon_panics = true,
)]
async fn test_current_oomd_record_marks_daemon_crash_as_oom(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    buck.build(&[]).await?;
    let daemon_cgroup = read_daemon_cgroup(wait_for_daemon_pid(buck).await?)?;
    let (dmesg_env, dmesg_called) = fake_dmesg_env(
        tmp_path,
        &format!(
            "<6>[{}] oomd kill: 81.28 80.05 42.78 {daemon_cgroup} 1000 ruleset:[test]\n",
            current_dmesg_timestamp(0)?
        ),
    )?;

    let res = with_env(buck.debug(&["crash", "panic"]), &dmesg_env)
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(dmesg_called.exists());
    assert!(
        has_tag(error, "DAEMON_OOM_KILLED"),
        "{daemon_cgroup:?} {error}"
    );
    assert_eq!(error["best_tag"], "DAEMON_OOM_KILLED");
    Ok(())
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    write_invocation_record = true,
    allow_daemon_panics = true,
)]
async fn test_oomd_kill_in_descendant_cgroup_does_not_mask_daemon_crash(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    buck.build(&[]).await?;
    let daemon_cgroup = read_daemon_cgroup(wait_for_daemon_pid(buck).await?)?;
    let (dmesg_env, dmesg_called) = fake_dmesg_env(
        tmp_path,
        &format!(
            "<6>[{}] oomd kill: 81.28 80.05 42.78 {daemon_cgroup}/child 1000 ruleset:[test]\n",
            current_dmesg_timestamp(0)?
        ),
    )?;

    let res = with_env(buck.debug(&["crash", "panic"]), &dmesg_env)
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(dmesg_called.exists());
    assert!(!has_tag(error, "DAEMON_OOM_KILLED"), "{}", error["tags"]);
    assert_eq!(error["best_tag"], "SERVER_PANICKED");
    Ok(())
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    write_invocation_record = true,
    allow_daemon_panics = true,
)]
async fn test_kernel_oom_victim_pid_marks_daemon_crash_as_oom(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    buck.build(&[]).await?;
    let daemon_pid = wait_for_daemon_pid(buck).await?;
    let (dmesg_env, dmesg_called) = fake_dmesg_env(
        tmp_path,
        &format!(
            "<6>[{}] Memory cgroup out of memory: Killed process {daemon_pid} (buck2) total-vm:1000kB\n",
            current_dmesg_timestamp(0)?
        ),
    )?;

    let res = with_env(buck.debug(&["crash", "panic"]), &dmesg_env)
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(dmesg_called.exists());
    assert!(has_tag(error, "DAEMON_OOM_KILLED"), "{}", error["tags"]);
    assert_eq!(error["best_tag"], "DAEMON_OOM_KILLED");
    Ok(())
}

#[buck_test(
    skip_for_os = ["darwin", "windows"],
    write_invocation_record = true,
    allow_daemon_panics = true,
)]
async fn test_kernel_oom_for_other_pid_does_not_mask_daemon_crash(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    buck.build(&[]).await?;
    let other_pid = wait_for_daemon_pid(buck).await? + 1;
    let (dmesg_env, dmesg_called) = fake_dmesg_env(
        tmp_path,
        &format!(
            "<6>[{}] Memory cgroup out of memory: Killed process {other_pid} (buck2) total-vm:1000kB\n",
            current_dmesg_timestamp(0)?
        ),
    )?;

    let res = with_env(buck.debug(&["crash", "panic"]), &dmesg_env)
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(dmesg_called.exists());
    assert!(!has_tag(error, "DAEMON_OOM_KILLED"), "{}", error["tags"]);
    assert_eq!(error["best_tag"], "SERVER_PANICKED");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_daemon_killed(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let record = tmp_path.join("record.json");
    let build = buck
        .build(&[
            ":slow_action",
            "--unstable-write-invocation-record",
            &record.to_string_lossy(),
        ])
        .spawn();

    let pid = wait_for_daemon_pid(buck).await?;
    kill(pid)?;
    // Wait for the client to exit
    let _ignored = build.wait().await;

    let record = InvocationRecord::read(&record)?;
    let error = record.single_error();
    assert_eq!(error["category_key"], "DAEMON_DISCONNECT");
    assert_eq!(error["category"], "ENVIRONMENT");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_build_file_race(buck: &Buck) -> anyhow::Result<()> {
    let target = "//file_busy:file";
    // first build
    let file_path = buck
        .build(&[target])
        .await?
        .build_report()?
        .output_for_target(target)?;

    // Keep the file open: a handle is what makes it busy on Windows, and the output may be
    // read-only (`buck2.readonly_outputs`), so it is opened for reading.
    let mut open = fs::OpenOptions::new();
    open.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // `FILE_SHARE_READ | FILE_SHARE_WRITE`. The standard library also shares the handle for
        // deletion by default, and a handle that allows deletion does not make the file busy.
        open.share_mode(0x1 | 0x2);
    }
    let f = open.open(&file_path)?;
    // build again, source code has changed, binary must be rebuilt
    let build = buck.build(&[target, "--show-output", "-c", "test.cache_buster=2"]);

    if cfg!(windows) {
        let res = build.await.expect_failure();

        let record = res.invocation_record()?;
        let error = record.single_error();
        assert_eq!(error["best_tag"], "IO_MATERIALIZER_FILE_BUSY");
        assert_eq!(error["category"], "ENVIRONMENT");
    } else {
        build.await?;
    }

    drop(f);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_download_failure(buck: &Buck) -> anyhow::Result<()> {
    // Upload action if necessary
    buck.build(&["//:run_action", "--remote-only"]).await?;
    buck.clean(&[]).await?;
    let res = buck
        .build(&["//:run_action"])
        .env("BUCK2_TEST_FAIL_RE_DOWNLOADS", "true")
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["category"], "INFRA");
    assert_eq!(error["category_key"], "RE_NOT_FOUND:DIGEST_NOT_FOUND");
    assert!(
        res.stderr.contains(
            "Your build requires materializing an artifact that has expired in the RE CAS and Buck does not have it. This likely happened because your Buck daemon has been online for a long time. This error is currently unrecoverable. To proceed, you should restart Buck using `buck2 killall`."
        ),
        "{}",
        res.stderr
    );
    Ok(())
}

// The disk-state core downloads tracked values, whose misses are the CAS's failure; a digest the
// user got wrong is caught by `cas_artifact`'s presence check at action time instead.
#[buck_test(write_invocation_record = true, skip_for_materializer = ["v2"])]
async fn test_declared_artifact_download_failure(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["//:declared_file"])
        .env("BUCK2_TEST_FAIL_RE_DOWNLOADS", "true")
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["best_tag"], "DECLARED_ARTIFACT_NOT_FOUND");
    assert_eq!(error["category"], "USER");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_declared_tree_download_failure(buck: &Buck) -> anyhow::Result<()> {
    fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(b"[buck2]\ndigest_algorithms = BLAKE3-KEYED,SHA1\n")?;

    let res = buck
        .build(&["//:declared_tree"])
        .env("BUCK2_TEST_FAIL_RE_DOWNLOADS", "true")
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["best_tag"], "DECLARED_ARTIFACT_NOT_FOUND");
    assert_eq!(error["category"], "USER");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_re_execute_failure(buck: &Buck) -> anyhow::Result<()> {
    // Upload action if necessary
    buck.build(&["//:run_action", "--remote-only"]).await?;
    buck.clean(&[]).await?;
    let res = buck
        .build(&["//:run_action", "--no-remote-cache"])
        .env("BUCK2_TEST_FAIL_RE_EXECUTE", "true")
        .await
        .expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["category_key"], "RE_FAILED_PRECONDITION:UNKNOWN");
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_local_incompatible(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["//:local_run_action", "--remote-only", "--no-remote-cache"])
        .await
        .expect_failure();
    assert!(res.stderr.contains("Incompatible executor preferences"));

    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["category"], "USER");
    assert_eq!(
        error["category_key"],
        "IncompatibleExecutorPreferences:ANY_ACTION_EXECUTION"
    );
    Ok(())
}

#[buck_test(
    write_invocation_record = true,
    env = [("BUCK2_TEST_INIT_DAEMON_ERROR", "true")],
)]
async fn test_daemon_startup_error(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&[":"]).await.expect_failure();
    assert!(res.stderr.contains("Injected init daemon error"));
    assert!(res.stderr.contains("Error initializing DaemonStateData"));

    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(has_tag(error, "DAEMON_CONNECT"), "{}", error["tags"]);
    assert!(
        has_tag(error, "DAEMON_STATE_INIT_FAILED"),
        "{}",
        error["tags"]
    );

    golden(
        &sanitize_stderr(&res.stderr),
        "fixtures/test_daemon_startup_error.golden.txt",
    );
    Ok(())
}

#[buck_test(
    skip_for_os = ["windows"],
    write_invocation_record = true,
    env = [("BUCK2_TEST_DAEMON_STARTUP_SIGNAL", "true")],
)]
async fn test_daemon_startup_signal(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&[":"]).await.expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["category_key"], "DAEMON_STARTUP_FAILED:SIGTERM");

    golden(
        &sanitize_daemon_stderr(&res.stderr),
        "fixtures/test_daemon_startup_signal.golden.txt",
    );
    Ok(())
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [("buck2", "allow_eden_io", "false")],
    skip_for_os = ["windows"],
    write_invocation_record = true,
)]
async fn test_eden_io_error_tagging(buck: &Buck) -> anyhow::Result<()> {
    let targets_file = buck.cwd().join("TARGETS.fixture");

    // remove file read permissions during test execution, test setup will fail if permissions are set on any fixture earlier
    set_mode(&targets_file, 0o000)?;

    // triggers file read IO error
    let res = buck.targets(&[":"]).await.expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(has_tag(error, "IO_EDEN"), "{}", error["tags"]);
    assert_eq!(error["category_key"], "IO_EDEN:IO_PERMISSION_DENIED");
    Ok(())
}

#[buck_test(
    write_invocation_record = true,
    env = [("BUCK2_TEST_FAIL_STREAMING", "true")],
)]
async fn test_client_streaming_error(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&[":"]).await.expect_failure();
    assert!(res.stderr.contains("Injected client streaming error"));

    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(string(&error["message"]).contains("Injected client streaming error"));

    golden(
        &sanitize_stderr(&res.stderr),
        "fixtures/test_client_streaming_error.golden.txt",
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_action_error_has_categorization(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["//fail_action:error_handler_produced_error_categories"])
        .await
        .expect_failure();
    res.assert_stderr_matches("Action sub-errors produced by error handlers");

    let record = res.invocation_record()?;
    let error = record.single_error();
    assert!(
        has_tag(error, "ACTION_COMMAND_FAILURE"),
        "{}",
        error["tags"]
    );
    assert_eq!(error["category_key"], "ACTION_COMMAND_FAILURE:FirstError");
    Ok(())
}

#[buck_test(
    write_invocation_record = true,
    skip_for_os = ["windows"],
    env = [("BUCK2_TEST_INIT_DATA_SLEEP_SECS", "120"), ("BUCKD_STARTUP_INIT_TIMEOUT", "5")],
)]
async fn test_init_data_timeout(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&[":"]).await.expect_failure();
    let record = res.invocation_record()?;
    let error = record.single_error();

    assert_eq!(error["category_key"], "CLIENT_STARTUP_TIMEOUT");
    golden(
        &sanitize_daemon_stderr(&res.stderr),
        "fixtures/test_init_timeout.golden.txt",
    );
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"], write_invocation_record = true)]
async fn test_nix_errno(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[":run_action", "--show-output"]).await?;
    fs::remove_dir_all(buck.cwd().join("buck-out/v2"))?;

    let res = buck.targets(&[":"]).await.expect_failure();
    res.assert_stderr_matches("Failed to stat.*ENOENT: No such file or directory");
    let record = res.invocation_record()?;
    let error = record.single_error();
    assert_eq!(error["category_key"], "NIX:ENOENT");
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"], write_invocation_record = true)]
async fn test_re_logs_permission_denied(buck: &Buck) -> anyhow::Result<()> {
    // Start daemon to create buck-out/v2 with correct permissions
    buck.targets(&[":"]).await?;

    // Kill daemon so next command starts a fresh one that creates a new RE client
    buck.kill().await?;

    // Create re_logs/lock with no permissions so RE client gets a permission error
    let re_logs = buck.cwd().join("buck-out").join("v2").join("re_logs");
    fs::create_dir_all(&re_logs)?;
    let lock_file = re_logs.join("lock");
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lock_file)?;
    set_mode(&lock_file, 0o000)?;

    // Restore permissions so test cleanup can proceed
    let _restore = RestoreMode {
        path: &lock_file,
        mode: 0o644,
    };
    let res = buck
        .build(&["//:run_action", "--remote-only"])
        .await
        .expect_failure();
    res.assert_stderr_matches("Unable to lock file in log dir");
    let record = res.invocation_record()?;
    let error = record.single_error();
    // Check that TCode/TCodeReasonGroup are propagated correctly from RE
    assert!(
        string(&error["telemetry_message"]).contains("Permission denied"),
        "{}",
        error["telemetry_message"]
    );
    assert_eq!(error["category_key"], "RE_PERMISSION_DENIED:RE_CLIENT");
    assert_eq!(error["category"], "ENVIRONMENT");
    Ok(())
}
