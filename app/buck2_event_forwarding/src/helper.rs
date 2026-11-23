/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Starting helper programs.

use std::io;
use std::process::Stdio;

use buck2_fs::paths::abs_path::AbsPath;
use buck2_util::process::background_command;
use futures::future::BoxFuture;
use tokio::io::AsyncWrite;
use tokio::process::Child;

/// The environment variable that gives a helper its command's event forwarding keys, separated
/// by commas.
pub(crate) const KEY_VARIABLE: &str = "BUCK2_EVENT_FORWARDING_KEY";

/// Starts helpers: real processes in the daemon, fakes in tests.
pub(crate) trait LaunchHelper: Send + Sync {
    /// Starts `program`, with no arguments, in `working_dir`, with [`KEY_VARIABLE`] set to `keys`.
    fn launch(&self, program: &AbsPath, working_dir: &AbsPath, keys: &str) -> io::Result<Helper>;
}

/// A running helper.
pub(crate) struct Helper {
    /// Its standard input. Dropping it closes the helper's input.
    pub(crate) input: Box<dyn AsyncWrite + Send + Unpin>,
    pub(crate) process: Box<dyn HelperProcess>,
}

pub(crate) trait HelperProcess: Send {
    /// Resolves once the helper has exited. Safe to cancel and call again.
    fn exited(&mut self) -> BoxFuture<'_, ()>;

    /// Ends the helper, if it still runs, and on Unix every process left in its process group,
    /// even after the helper itself exited. Resolves once the helper has exited.
    fn kill(&mut self) -> BoxFuture<'_, ()>;
}

/// Starts each helper as a child process of the daemon: on Unix in a process group of its own,
/// with the daemon's environment plus [`KEY_VARIABLE`], its standard input a pipe, and its
/// standard output and error discarded. Dropping a helper kills it.
///
/// Starting a process can block, for example while the program's file system answers slowly, so
/// call [`LaunchHelper::launch`] from a thread that may block.
pub(crate) struct ProcessLauncher;

impl LaunchHelper for ProcessLauncher {
    fn launch(&self, program: &AbsPath, working_dir: &AbsPath, keys: &str) -> io::Result<Helper> {
        let mut command = background_command(program.as_path());
        command
            .current_dir(working_dir.as_path())
            .env(KEY_VARIABLE, keys)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = tokio::process::Command::from(command)
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().expect("the helper's input is piped");
        Ok(Helper {
            input: Box::new(input),
            process: Box::new(ChildProcess {
                #[cfg(unix)]
                group: child.id().and_then(|id| i32::try_from(id).ok()),
                child,
            }),
        })
    }
}

struct ChildProcess {
    child: Child,
    /// The helper's process group, whose ID is the helper's process ID.
    #[cfg(unix)]
    group: Option<i32>,
}

impl HelperProcess for ChildProcess {
    fn exited(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // An error means the process cannot be waited for, which never gets better.
            if self.child.wait().await.is_err() {
                std::future::pending::<()>().await;
            }
        })
    }

    fn kill(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            // POSIX does not reuse a process group's ID while any process is in the group, so
            // this reaches only processes the helper started, also after the helper was reaped.
            #[cfg(unix)]
            if let Some(group) = self.group {
                let _ignored = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(group),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            let _ignored = self.child.kill().await;
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::time::Duration;

    use buck2_fs::paths::abs_path::AbsPathBuf;
    #[cfg(unix)]
    use tokio::io::AsyncWriteExt;

    use super::*;

    const KEY: &str = "the key of the command";

    /// Runs `script` as a helper through `sh`, in a fresh directory.
    #[cfg(unix)]
    async fn launch_script(script: &str) -> (tempfile::TempDir, Helper) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("helper.sh");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(
            &program,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .unwrap();
        let program = AbsPathBuf::new(program).unwrap();
        let working_dir = AbsPathBuf::new(dir.path()).unwrap();
        // Under OSS `cargo test`, all tests share one process. A process another test starts
        // meanwhile can inherit the handle `std::fs::write` holds on the script, and starting the
        // script fails with "Text file busy" until that process runs its own program.
        let helper = within_a_minute(async {
            loop {
                match ProcessLauncher.launch(&program, &working_dir, KEY) {
                    Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    launched => return launched.unwrap(),
                }
            }
        })
        .await;
        (dir, helper)
    }

    /// Bounds a wait on a real process, so a broken test fails instead of hanging.
    #[cfg(unix)]
    async fn within_a_minute<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(60), future)
            .await
            .expect("the helper should get there within a minute")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_helper_reads_its_input_in_its_working_directory_with_its_key() {
        let (dir, mut helper) = launch_script(&format!(
            "pwd > where; printf %s \"${KEY_VARIABLE}\" > key; cat > input"
        ))
        .await;
        helper.input.write_all(b"one line\n").await.unwrap();
        drop(helper.input);
        within_a_minute(helper.process.exited()).await;

        let read = |name| std::fs::read_to_string(dir.path().join(name)).unwrap();
        assert_eq!(
            (
                std::fs::canonicalize(read("where").trim()).unwrap(),
                read("key"),
                read("input")
            ),
            (
                std::fs::canonicalize(dir.path()).unwrap(),
                KEY.to_owned(),
                "one line\n".to_owned()
            )
        );
    }

    /// Whether the process `pid` has ended: it is gone, or a zombie waiting to be reaped.
    #[cfg(target_os = "linux")]
    fn has_ended(pid: u32) -> bool {
        // `/proc/<pid>/stat` reads `<pid> (<comm>) <state> ...`, and `comm` may hold `)`.
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(_) => true,
            Ok(stat) => stat
                .rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
        }
    }

    /// The process ID that a helper script wrote to `file` in `dir`, once it has.
    #[cfg(target_os = "linux")]
    async fn pid_written_to(dir: &std::path::Path, file: &str) -> u32 {
        within_a_minute(async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(dir.join(file))
                    && let Ok(pid) = pid.trim().parse()
                {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    }

    /// Waits until the process `pid` has ended.
    #[cfg(target_os = "linux")]
    async fn ends(pid: u32) {
        within_a_minute(async {
            while !has_ended(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn killing_a_helper_ends_the_processes_it_started() {
        // Starts a process that outlives the helper unless something ends it, then reads its
        // input, which stays open, until it is killed.
        let (dir, mut helper) =
            launch_script("sleep 3600 & echo $! > started; exec cat > /dev/null").await;
        let started = pid_written_to(dir.path(), "started").await;

        within_a_minute(helper.process.kill()).await;
        ends(started).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn ending_a_helper_that_exited_ends_the_processes_it_left_behind() {
        let (dir, mut helper) = launch_script("sleep 3600 & echo $! > started").await;
        within_a_minute(helper.process.exited()).await;
        let started = pid_written_to(dir.path(), "started").await;
        assert!(
            !has_ended(started),
            "the helper's process should outlive the helper until it is ended"
        );

        within_a_minute(helper.process.kill()).await;
        ends(started).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn killing_a_helper_ends_it() {
        // Reads its input, which stays open, until it is killed.
        let (_dir, mut helper) = launch_script("exec cat > /dev/null").await;
        within_a_minute(helper.process.kill()).await;
        within_a_minute(helper.process.exited()).await;
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let working_dir = AbsPathBuf::new(dir.path()).unwrap();
        let program = working_dir.join("no-such-helper");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        assert!(ProcessLauncher.launch(&program, &working_dir, KEY).is_err());
    }
}
