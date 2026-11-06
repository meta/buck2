/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A client for `buck2 subscribe`, which writes the daemon's messages to stdout as JSON lines and
//! reads length-prefixed requests on stdin.

use std::time::Duration;

use anyhow::Context;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::ChildStdout;
use tokio::sync::mpsc;

use crate::Buck;
use crate::buck::BuckCommand;
use crate::output::BuckError;
use crate::output::BuckOutput;
use crate::piped::PipedProcess;

/// How long [`SubscribeClient::read_message`] waits for a message.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long [`SubscribeClient::close`] waits for buck2 to exit.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(120);

impl Buck {
    /// Starts `buck2 subscribe --unstable-json <args>`.
    ///
    /// ```ignore
    /// let mut subscribe = buck.subscribe(&["--active-commands"]);
    /// let msg = subscribe.read_message().await?;
    /// subscribe.close().await?;
    /// ```
    pub fn subscribe(&self, args: &[&str]) -> SubscribeClient {
        let mut argv = vec!["subscribe", "--unstable-json"];
        argv.extend(args);
        SubscribeClient::start(self.command(&argv))
    }
}

/// A running `buck2 subscribe`. Dropping it kills the process; [`close`](Self::close) ends it the
/// way a client does.
pub struct SubscribeClient {
    process: PipedProcess,
    lines: mpsc::UnboundedReceiver<String>,
}

impl SubscribeClient {
    fn start(command: BuckCommand<'_>) -> Self {
        let (process, stdout) = command.spawn_piped();
        Self {
            process,
            lines: read_lines(stdout),
        }
    }

    /// The next message buck2 wrote, waiting up to 30 seconds for it.
    pub async fn read_message(&mut self) -> anyhow::Result<Value> {
        let line = tokio::time::timeout(READ_TIMEOUT, self.lines.recv())
            .await
            .with_context(|| format!("No message from `buck2 subscribe` within {READ_TIMEOUT:?}"))?
            .context("`buck2 subscribe` closed its stdout")?;
        serde_json::from_str(&line).with_context(|| format!("Parsing the message `{line}`"))
    }

    /// The process, to write requests to its stdin, close it, or check whether it is running.
    pub fn process(&mut self) -> &mut PipedProcess {
        &mut self.process
    }

    /// Closes stdin and waits up to 120 seconds for buck2 to exit, giving its output (stdout is
    /// what [`read_message`](Self::read_message) did not read) or, if it exited unsuccessfully, a
    /// [`BuckError`].
    pub async fn close(self) -> Result<BuckOutput, BuckError> {
        let Self { process, mut lines } = self;
        let rest = async move {
            let mut rest = String::new();
            while let Some(line) = lines.recv().await {
                rest.push_str(&line);
            }
            rest
        };
        process.finish(rest, CLOSE_TIMEOUT).await
    }
}

/// The lines of `stdout`, newlines included, as they come. A reader task keeps them, so that a
/// reader that gives up waiting loses none.
fn read_lines(stdout: ChildStdout) -> mpsc::UnboundedReceiver<String> {
    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match stdout.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if sender
                        .send(String::from_utf8_lossy(&line).into_owned())
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::ExpectFailure;

    /// A client of `script` run by the shell instead of `buck2 subscribe`.
    fn client(dir: &Path, script: &str) -> SubscribeClient {
        let buck = Buck::new(
            PathBuf::from("/bin/sh"),
            dir.to_owned(),
            BTreeMap::new(),
            false,
        );
        SubscribeClient::start(buck.command(&["-c", script]))
    }

    #[tokio::test]
    async fn test_messages_are_json_lines() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let mut subscribe = client(
            dir.path(),
            r#"echo '{"a": 1}'; read request; echo "{\"got\": \"$request\"}""#,
        );
        assert_eq!(subscribe.read_message().await?, json!({"a": 1}));
        subscribe.process().write_stdin(b"x\n").await?;
        assert_eq!(subscribe.read_message().await?, json!({"got": "x"}));
        subscribe.close().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_reading_past_the_end_is_an_error() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let mut subscribe = client(dir.path(), "true");
        let error = subscribe
            .read_message()
            .await
            .expect_err("there is no message");
        assert!(error.to_string().contains("closed its stdout"), "{error:#}");
        Ok(())
    }

    #[tokio::test]
    async fn test_closing_gives_the_unread_messages() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        // Exits unsuccessfully once stdin is closed.
        let subscribe = client(dir.path(), "echo one; echo two; cat >/dev/null; exit 2");
        let output = subscribe.close().await.expect_failure();
        assert_eq!(output.exit_code(), Some(2));
        assert_eq!(output.stdout, "one\ntwo\n");
        Ok(())
    }
}
