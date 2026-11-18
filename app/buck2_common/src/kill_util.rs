/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

#[allow(unused_variables)]
pub async fn try_terminate_process_gracefully(
    pid: i32,
    timeout: Duration,
) -> buck2_error::Result<()> {
    #[cfg(unix)]
    {
        unix::try_terminate_process_gracefully(pid, timeout).await
    }

    #[cfg(not(unix))]
    {
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Unimplemented,
            "Graceful process termination is not implemented for non-unix target family."
        ))
    }
}

#[cfg(unix)]
mod unix {
    use std::ops::ControlFlow;
    use std::time::Duration;
    use std::time::Instant;

    use nix::sys::signal::Signal;
    use tracing::info;

    pub(crate) async fn try_terminate_process_gracefully(
        pid: i32,
        timeout: Duration,
    ) -> buck2_error::Result<()> {
        let pid = nix::unistd::Pid::from_raw(pid);
        match send_signal_and_wait_for_shutdown(Signal::SIGTERM, pid, Some(timeout)).await {
            ControlFlow::Continue(_) => {}
            ControlFlow::Break(StoppedWaiting::Success) => {
                return Ok(());
            }
            ControlFlow::Break(StoppedWaiting::UnexpectedError(e)) => {
                return Err(e);
            }
        }
        match send_signal_and_wait_for_shutdown(Signal::SIGKILL, pid, None).await {
            ControlFlow::Continue(_) => {
                unreachable!("Not expected when there is no timeout specified.")
            }
            ControlFlow::Break(StoppedWaiting::Success) => Ok(()),
            ControlFlow::Break(StoppedWaiting::UnexpectedError(e)) => Err(e),
        }
    }

    enum StoppedWaiting {
        Success,
        UnexpectedError(buck2_error::Error),
    }

    fn unexpected(pid: nix::unistd::Pid, e: impl std::fmt::Display) -> ControlFlow<StoppedWaiting> {
        ControlFlow::Break(StoppedWaiting::UnexpectedError(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "Unexpected error while waiting for process `{}` to terminate (`{}`)",
            pid,
            e
        )))
    }

    /// `pid` is not a child of this process, so a zombie counts as gone: see
    /// [`buck2_process_kill::process_is_gone`].
    async fn send_signal_and_wait_for_shutdown(
        signal: nix::sys::signal::Signal,
        pid: nix::unistd::Pid,
        timeout: Option<Duration>,
    ) -> ControlFlow<StoppedWaiting> {
        let start = Instant::now();
        info!("Sending signal `{}` to process `{}`.", signal, pid);
        match nix::sys::signal::kill(pid, signal) {
            Ok(()) => {}
            // There is no such process, our desired outcome.
            Err(nix::errno::Errno::ESRCH) => return ControlFlow::Break(StoppedWaiting::Success),
            Err(e) => return unexpected(pid, e),
        }
        loop {
            match buck2_process_kill::process_is_gone(pid.as_raw()) {
                Ok(true) => return ControlFlow::Break(StoppedWaiting::Success),
                Ok(false) => {}
                Err(e) => return unexpected(pid, e),
            }
            if let Some(timeout) = timeout {
                if Instant::now() - start > timeout {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        ControlFlow::Continue(())
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use tokio::fs::File;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;

    use super::*;

    #[tokio::test]
    async fn test_graceful_termination() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;

        // We cannot start child script directly else it will enter a zombie state
        // when trying to kill it and termination would be stuck.
        let mut parent_file = File::create(dir.path().join("parent.sh")).await?;
        let parent_script = r#"
            sh child.sh &
            while [ ! -f trap_registered ]; do sleep 1; done # wait for a child script to register a trap
            echo $!
        "#;
        parent_file.write_all(parent_script.as_bytes()).await?;

        let mut child_file = File::create(dir.path().join("child.sh")).await?;
        let child_script = r#"
            sigterm()
            {
                touch gracefully_terminated
            }
            trap sigterm TERM
            touch trap_registered

            sleep 100 &
            wait
        "#;
        child_file.write_all(child_script.as_bytes()).await?;

        // ast-grep-ignore: rust/buck2-no-command-new
        let mut command = tokio::process::Command::new("sh");
        command
            .args(["parent.sh"])
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut parent = command.spawn()?;
        parent.wait().await?;

        let pid = {
            let stdout = parent.stdout.take().unwrap();
            let mut reader = BufReader::new(stdout).lines();
            let pid_str = reader.next_line().await?;
            pid_str.unwrap().parse().unwrap()
        };

        // Generous: on a loaded machine the trap can take more than a second,
        // and a SIGKILL mid-trap fails the test. The wait ends as soon as the
        // child is gone, so this costs nothing when the trap is quick.
        try_terminate_process_gracefully(pid, Duration::from_secs(60)).await?;

        assert!(tokio::fs::try_exists(dir.path().join("gracefully_terminated")).await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_sigkill_when_graceful_termination_fails() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;

        // We cannot start child script directly else it will enter a zombie state
        // when trying to kill it and termination would be stuck.
        let mut parent_file = File::create(dir.path().join("parent.sh")).await?;
        let parent_script = r#"
            sh child.sh &
            while [ ! -f trap_registered ]; do sleep 1; done # wait for a child script to register a trap
            echo $!
        "#;
        parent_file.write_all(parent_script.as_bytes()).await?;

        let mut child_file = File::create(dir.path().join("child.sh")).await?;
        let child_script = r#"
            sigterm()
            {
                sleep 3  # block termination
                touch gracefully_terminated
            }
            trap sigterm TERM
            touch trap_registered

            sleep 100 &
            wait
        "#;
        child_file.write_all(child_script.as_bytes()).await?;

        // ast-grep-ignore: rust/buck2-no-command-new
        let mut command = tokio::process::Command::new("sh");
        command
            .args(["parent.sh"])
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut parent = command.spawn()?;
        // Bounded: the parent script spins until the child has registered its
        // trap, and the SIGKILL leg waits for the child to be gone with no
        // timeout of its own. A hang here would otherwise be the suite's.
        tokio::time::timeout(Duration::from_secs(60), parent.wait())
            .await
            .expect("the parent script should finish within a minute")?;

        let pid = {
            let stdout = parent.stdout.take().unwrap();
            let mut reader = BufReader::new(stdout).lines();
            let pid_str = reader.next_line().await?;
            pid_str.unwrap().parse().unwrap()
        };

        tokio::time::timeout(
            Duration::from_secs(60),
            try_terminate_process_gracefully(pid, Duration::from_secs(1)),
        )
        .await
        .expect("the child should be gone well within a minute of the SIGKILL")?;

        assert!(!tokio::fs::try_exists(dir.path().join("gracefully_terminated")).await?);

        Ok(())
    }
}
