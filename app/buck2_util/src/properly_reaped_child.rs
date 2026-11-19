/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::mem;
use std::process::Output;
use std::process::Stdio;

use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use tokio::io::AsyncReadExt;
use tokio::process::Child;

use crate::process::async_background_command;

/// A wrapper over a child process that will reap the child process on drop.
/// On Unix platforms, a child process becomes a zombie until it is reaped by its parent.
pub struct ProperlyReapedChild {
    child: Option<Child>,
}

impl ProperlyReapedChild {
    pub async fn output(mut self) -> buck2_error::Result<Output> {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut child = mem::take(&mut self.child).internal_error("child field must be set")?;
        let mut stdout_pipe = child.stdout.take().internal_error("stdout is not piped")?;
        let mut stderr_pipe = child.stderr.take().internal_error("stderr is not piped")?;
        let (stdout_error, stderr_error, status) = tokio::join!(
            stdout_pipe.read_to_end(&mut stdout),
            stderr_pipe.read_to_end(&mut stderr),
            child.wait(),
        );

        let result = match stdout_error.is_ok() || stderr_error.is_ok() {
            true => Ok(Output {
                status: status?,
                stdout,
                stderr,
            }),
            false => Err(internal_error!("Failed to read stdout and stderr")),
        };
        reap_child(child);
        result
    }
}

impl Drop for ProperlyReapedChild {
    fn drop(&mut self) {
        if let Some(child) = mem::take(&mut self.child) {
            reap_child(child);
        }
    }
}

pub fn reap_on_drop_command(
    command: &str,
    args: &[&str],
    env: Option<&[(&str, &str)]>,
) -> buck2_error::Result<ProperlyReapedChild> {
    let mut background_command = async_background_command(command);
    let mut background_command = background_command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(env_var) = env {
        background_command = background_command.envs(env_var.to_owned())
    }

    background_command
        .spawn()
        .map(|child| ProperlyReapedChild { child: Some(child) })
        .map_err(|e| e.into())
}

fn reap_child(mut child: Child) {
    tokio::spawn(async move {
        if let Some(child_id) = child.id() {
            // If a child process has already exited, the child.id() is None.
            // TODO(rajneeshl): Promote the info! below to warn! when the health check CLI lifetime is better managed.
            tracing::info!("Killed child process: {:?}", child_id);
        }
        drop(child.kill().await);
    });
}
