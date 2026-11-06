/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A Language Server Protocol client for `buck2 lsp`.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::BufReader;
use tokio::process::ChildStdout;
use tokio::sync::mpsc;

use crate::Buck;
use crate::buck::BuckCommand;
use crate::piped::PipedProcess;

/// How long [`LspClient::read_message`] waits for a message.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long [`LspClient::close`] waits for buck2 to exit once killed. Process startup is slow
/// sometimes in dev builds, the box can be overloaded, etc. The average case is much lower.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(120);
/// How many messages [`LspClient::receive_notification`] and [`LspClient::receive_response`]
/// read before they give up.
const RETRIES: usize = 10;

impl Buck {
    /// Starts `buck2 lsp <args>`.
    ///
    /// ```ignore
    /// let mut lsp = buck.lsp(&[]);
    /// lsp.init_connection().await?;
    /// let diagnostics = lsp.open_file("defs.bzl", None).await?;
    /// lsp.close().await;
    /// ```
    pub fn lsp(&self, args: &[&str]) -> LspClient {
        let mut argv = vec!["lsp"];
        argv.extend(args);
        LspClient::start(self.command(&argv), self.cwd().to_owned())
    }
}

/// The error a request got from the server.
#[derive(Debug, Clone)]
pub struct LspResponseError(pub Value);

impl fmt::Display for LspResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error returned from LSP: `{}`", self.0)
    }
}

impl std::error::Error for LspResponseError {}

/// A running `buck2 lsp`. Messages are read as they come and kept until asked for: responses by
/// their request id, notifications in order. Dropping it kills the process; [`close`](Self::close)
/// also waits for it.
pub struct LspClient {
    process: PipedProcess,
    cwd: PathBuf,
    id: u64,
    messages: mpsc::UnboundedReceiver<anyhow::Result<Value>>,
    notifications: VecDeque<Value>,
    responses: HashMap<String, Value>,
}

impl LspClient {
    fn start(command: BuckCommand<'_>, cwd: PathBuf) -> Self {
        let (process, stdout) = command.spawn_piped();
        Self {
            process,
            cwd,
            id: 0,
            messages: read_messages(stdout),
            notifications: VecDeque::new(),
            responses: HashMap::new(),
        }
    }

    /// The directory the server runs in, which relative paths are relative to.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The process, to close its stdin or check whether it is running.
    pub fn process(&mut self) -> &mut PipedProcess {
        &mut self.process
    }

    /// Kills the server and waits up to 120 seconds for it to exit.
    pub async fn close(mut self) {
        self.process.kill(CLOSE_TIMEOUT).await;
    }

    /// Sends `payload`, a JSON-RPC message.
    pub async fn write_message(&mut self, payload: &Value) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(payload)?;
        let mut message = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
        message.extend(payload);
        self.process
            .write_stdin(&message)
            .await
            .context("Writing to `buck2 lsp`")
    }

    /// The next message from the server, waiting up to 30 seconds for it. It is also kept, as a
    /// response if it has an id and as a notification otherwise.
    pub async fn read_message(&mut self) -> anyhow::Result<Value> {
        let message = tokio::time::timeout(READ_TIMEOUT, self.messages.recv())
            .await
            .with_context(|| format!("No message from `buck2 lsp` within {READ_TIMEOUT:?}"))?
            .context("`buck2 lsp` closed its stdout")??;
        match message.get("id") {
            Some(id) => {
                self.responses.insert(id.to_string(), message.clone());
            }
            None => self.notifications.push_back(message.clone()),
        }
        Ok(message)
    }

    /// Opens `relative_path` with `contents`, or what the file holds, and gives the parameters of
    /// the diagnostics the server publishes for it.
    pub async fn open_file(
        &mut self,
        relative_path: impl AsRef<Path>,
        contents: Option<&str>,
    ) -> anyhow::Result<Value> {
        let absolute_path = self.cwd.join(relative_path);
        let contents = match contents {
            Some(contents) => contents.to_owned(),
            None => std::fs::read_to_string(&absolute_path)
                .with_context(|| format!("Reading `{}`", absolute_path.display()))?,
        };
        let uri = file_uri(&absolute_path);

        let payload = json!({
            "textDocument": {
                "uri": uri,
                "languageId": "starlark",
                "version": 1,
                "text": contents,
            }
        });
        self.send_notification("textDocument/didOpen", payload)
            .await?;
        let notif = self
            .receive_notification(Some("textDocument/publishDiagnostics"))
            .await?
            .with_context(|| format!("No diagnostics for `{uri}`"))?;

        anyhow::ensure!(
            notif["params"]["uri"] == uri,
            "Diagnostics for another file than `{uri}`: {notif}"
        );
        Ok(notif["params"].clone())
    }

    /// Where the definition of what is at `line` and `col` (0-based) of `relative_path` is.
    pub async fn goto_definition(
        &mut self,
        relative_path: impl AsRef<Path>,
        line: u64,
        col: u64,
    ) -> anyhow::Result<Option<Value>> {
        let absolute_path = self.cwd.join(relative_path);
        let payload = json!({
            "textDocument": {
                "uri": file_uri(&absolute_path),
            },
            "position": {
                "line": line,
                "character": col,
            },
        });

        let req_id = self
            .send_request("textDocument/definition", payload)
            .await?;
        self.receive_response(req_id).await
    }

    /// The contents of the file at `uri`, as `starlark/fileContents` gives them.
    pub async fn file_contents(&mut self, uri: &str) -> anyhow::Result<Option<Value>> {
        let payload = json!({
            "uri": uri,
        });

        let req_id = self.send_request("starlark/fileContents", payload).await?;
        self.receive_response(req_id).await
    }

    /// Sends a notification.
    pub async fn send_notification(
        &mut self,
        method: &str,
        notification: Value,
    ) -> anyhow::Result<()> {
        let payload = json!({"jsonrpc": "2.0", "method": method, "params": notification});
        self.write_message(&payload).await
    }

    /// The first notification, of `method` if given, that came: reads a message, then looks among
    /// the notifications so far, up to 10 times. A notification it gives is no longer kept.
    pub async fn receive_notification(
        &mut self,
        method: Option<&str>,
    ) -> anyhow::Result<Option<Value>> {
        for _ in 0..RETRIES {
            self.read_message().await?;
            let found = self
                .notifications
                .iter()
                .position(|v| method.is_none_or(|m| v["method"] == m));
            if let Some(idx) = found {
                return Ok(self.notifications.remove(idx));
            }
        }
        Ok(None)
    }

    /// Sends a request, giving its id.
    pub async fn send_request(&mut self, method: &str, request: Value) -> anyhow::Result<u64> {
        self.id += 1;
        let id = self.id;
        let payload = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": request});
        self.write_message(&payload).await?;
        Ok(id)
    }

    /// The result of the request `request_id`: reads a message, then looks for its response, up to
    /// 10 times. A response with an error is an [`LspResponseError`].
    pub async fn receive_response(&mut self, request_id: u64) -> anyhow::Result<Option<Value>> {
        let key = Value::from(request_id).to_string();
        for _ in 0..RETRIES {
            self.read_message().await?;
            let Some(response) = self.responses.remove(&key) else {
                continue;
            };
            return match response.get("result") {
                Some(result) if !result.is_null() => Ok(Some(result.clone())),
                _ => Err(LspResponseError(response["error"].clone()).into()),
            };
        }
        Ok(None)
    }

    /// Initializes the connection as VS Code does, with the project as the workspace.
    pub async fn init_connection(&mut self) -> anyhow::Result<()> {
        let repo_uri = file_uri(&self.cwd);
        let mut request = init_request();
        request["workspaceFolders"][0]["uri"] = Value::from(repo_uri.clone());
        request["rootUri"] = Value::from(repo_uri);

        let req_id = self.send_request("initialize", request).await?;
        let response = self
            .receive_response(req_id)
            .await?
            .context("No response to `initialize`")?;

        let capabilities = &response["capabilities"];
        anyhow::ensure!(
            capabilities.get("definitionProvider").is_some()
                && capabilities.get("textDocumentSync").is_some(),
            "Missing capabilities in {response}"
        );
        self.send_notification("initialized", json!({})).await
    }
}

/// The messages on `stdout`, as they come. A reader task keeps them, so that a reader that gives up
/// waiting loses none. A malformed message is an error, and the last one.
fn read_messages(stdout: ChildStdout) -> mpsc::UnboundedReceiver<anyhow::Result<Value>> {
    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        loop {
            let message = match read_frame(&mut stdout).await {
                Ok(None) => break,
                Ok(Some(message)) => Ok(message),
                Err(e) => Err(e),
            };
            let failed = message.is_err();
            if sender.send(message).is_err() || failed {
                break;
            }
        }
    });
    receiver
}

/// The next message on `reader`: a `Content-Length: <n>` line, a blank line and `n` bytes of JSON.
/// `None` at the end.
async fn read_frame(reader: &mut (impl AsyncBufRead + Unpin)) -> anyhow::Result<Option<Value>> {
    let mut header = Vec::new();
    if reader.read_until(b'\n', &mut header).await? == 0 {
        return Ok(None);
    }
    let mut blank = Vec::new();
    reader.read_until(b'\n', &mut blank).await?;

    let header = String::from_utf8_lossy(&header);
    let length = header
        .strip_prefix("Content-Length: ")
        .map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|digits| digits.parse::<usize>().ok())
        .with_context(|| format!("Did not get content length header. Got `{header}`"))?;
    let mut content = vec![0; length];
    reader.read_exact(&mut content).await?;
    let message = serde_json::from_slice(&content).with_context(|| {
        format!(
            "Parsing the message `{}`",
            String::from_utf8_lossy(&content)
        )
    })?;
    Ok(Some(message))
}

/// The `file:` URI of the absolute `path`, as Python's `Path.as_uri` writes it: bytes other than
/// ASCII letters, digits, `_.-~` and the separators are percent-encoded.
pub fn file_uri(path: &Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        let path = path.replace('\\', "/");
        let bytes = path.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' {
            // A path on a local drive: `file:///c:/a/b`.
            return format!("file:///{}{}", &path[..2], quote(&path[2..]));
        }
        // A path on a network drive: `file://host/share/a/b`.
        return format!("file:{}", quote(&path));
    }
    format!("file://{}", quote(&path))
}

fn quote(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' | b'/' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A sample initialization request sent from VS Code.
fn init_request() -> Value {
    json!({
        "capabilities": {
            "textDocument": {
                "completion": {
                    "completionItem": {
                        "commitCharactersSupport": true,
                        "deprecatedSupport": true,
                        "documentationFormat": ["markdown", "plaintext"],
                        "preselectSupport": true,
                        "snippetSupport": true,
                        "tagSupport": {"valueSet": [1]},
                    },
                    "completionItemKind": {"valueSet": (1..=25).collect::<Vec<u32>>()},
                    "contextSupport": true,
                    "dynamicRegistration": true,
                },
                "declaration": {"dynamicRegistration": true, "linkSupport": true},
                "definition": {"dynamicRegistration": true, "linkSupport": true},
                "documentHighlight": {"dynamicRegistration": true},
                "documentLink": {"dynamicRegistration": true, "tooltipSupport": true},
                "documentSymbol": {
                    "dynamicRegistration": true,
                    "hierarchicalDocumentSymbolSupport": true,
                    "symbolKind": {"valueSet": (1..=26).collect::<Vec<u32>>()},
                },
                "foldingRange": {
                    "dynamicRegistration": true,
                    "lineFoldingOnly": true,
                    "rangeLimit": 5000,
                },
                "formatting": {"dynamicRegistration": true},
                "hover": {
                    "contentFormat": ["markdown", "plaintext"],
                    "dynamicRegistration": true,
                },
                "implementation": {"dynamicRegistration": true, "linkSupport": true},
                "onTypeFormatting": {"dynamicRegistration": true},
                "publishDiagnostics": {
                    "relatedInformation": true,
                    "tagSupport": {"valueSet": [1, 2]},
                    "versionSupport": false,
                },
                "rangeFormatting": {"dynamicRegistration": true},
                "references": {"dynamicRegistration": true},
                "rename": {"dynamicRegistration": true, "prepareSupport": true},
                "selectionRange": {"dynamicRegistration": true},
                "signatureHelp": {
                    "contextSupport": true,
                    "dynamicRegistration": true,
                    "signatureInformation": {
                        "documentationFormat": ["markdown", "plaintext"],
                        "parameterInformation": {"labelOffsetSupport": true},
                    },
                },
                "synchronization": {
                    "didSave": true,
                    "dynamicRegistration": true,
                    "willSave": true,
                    "willSaveWaitUntil": true,
                },
                "typeDefinition": {"dynamicRegistration": true, "linkSupport": true},
            },
            "window": {"workDoneProgress": true},
        },
        "processId": null,
        "rootUri": "file:///INVALID",
        "trace": "off",
        "workspaceFolders": [{"name": "buck2", "uri": "file:///INVALID"}],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    #[tokio::test]
    async fn test_frames_are_read_in_order_until_the_end() -> anyhow::Result<()> {
        let input = frame(r#"{"id": 1}"#) + &frame(r#"{"method": "m"}"#);
        let mut reader = input.as_bytes();
        assert_eq!(read_frame(&mut reader).await?, Some(json!({"id": 1})));
        assert_eq!(read_frame(&mut reader).await?, Some(json!({"method": "m"})));
        assert_eq!(read_frame(&mut reader).await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_a_frame_without_a_length_is_an_error() {
        let mut reader = "{}\r\n\r\n{}".as_bytes();
        let error = read_frame(&mut reader)
            .await
            .expect_err("there is no length");
        assert!(
            error
                .to_string()
                .contains("Did not get content length header"),
            "{error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_file_uris_are_percent_encoded_like_python() {
        assert_eq!(file_uri(Path::new("/a/b_c.d-e~")), "file:///a/b_c.d-e~");
        assert_eq!(
            file_uri(Path::new("/a b/c:d%é")),
            "file:///a%20b/c%3Ad%25%C3%A9"
        );
    }

    #[test]
    fn test_the_init_request_names_every_kind() {
        let request = init_request();
        let kinds = &request["capabilities"]["textDocument"]["completion"]["completionItemKind"]["valueSet"];
        assert_eq!(kinds.as_array().map(Vec::len), Some(25));
        assert_eq!(kinds[0], 1);
        assert_eq!(kinds[24], 25);
        let symbols =
            &request["capabilities"]["textDocument"]["documentSymbol"]["symbolKind"]["valueSet"];
        assert_eq!(symbols.as_array().map(Vec::len), Some(26));
    }

    #[cfg(unix)]
    mod client {
        use std::collections::BTreeMap;

        use super::*;

        /// A client of `script` run by the shell instead of `buck2 lsp`.
        fn client(dir: &Path, script: &str) -> LspClient {
            let buck = Buck::new(
                PathBuf::from("/bin/sh"),
                dir.to_owned(),
                BTreeMap::new(),
                false,
            );
            LspClient::start(buck.command(&["-c", script]), dir.to_owned())
        }

        /// A script that writes `messages` as LSP frames, then waits for stdin to close.
        fn script(messages: &[&str]) -> String {
            let frames: String = messages
                .iter()
                .map(|m| format!("printf 'Content-Length: {}\\r\\n\\r\\n%s' '{m}'; ", m.len()))
                .collect();
            format!("{frames}cat >/dev/null")
        }

        #[tokio::test]
        async fn test_responses_are_matched_to_requests() -> anyhow::Result<()> {
            let dir = tempfile::tempdir()?;
            let mut lsp = client(
                dir.path(),
                &script(&[
                    r#"{"method": "first"}"#,
                    r#"{"id": 2, "result": "two"}"#,
                    r#"{"id": 1, "result": "one"}"#,
                ]),
            );
            assert_eq!(lsp.send_request("a", json!({})).await?, 1);
            assert_eq!(lsp.send_request("b", json!({})).await?, 2);
            assert_eq!(lsp.receive_response(1).await?, Some(json!("one")));
            assert_eq!(
                lsp.responses.get("2"),
                Some(&json!({"id": 2, "result": "two"})),
                "the response that came first is kept"
            );
            assert_eq!(
                lsp.notifications.pop_front(),
                Some(json!({"method": "first"}))
            );
            lsp.close().await;
            Ok(())
        }

        #[tokio::test]
        async fn test_an_error_response_is_an_lsp_response_error() -> anyhow::Result<()> {
            let dir = tempfile::tempdir()?;
            let mut lsp = client(
                dir.path(),
                &script(&[r#"{"id": 1, "result": null, "error": {"code": 1}}"#]),
            );
            let id = lsp.send_request("a", json!({})).await?;
            let error = lsp.receive_response(id).await.expect_err("an error");
            assert_eq!(
                error.downcast_ref::<LspResponseError>().map(|e| &e.0),
                Some(&json!({"code": 1}))
            );
            lsp.close().await;
            Ok(())
        }

        #[tokio::test]
        async fn test_notifications_are_found_by_method() -> anyhow::Result<()> {
            let dir = tempfile::tempdir()?;
            let mut lsp = client(
                dir.path(),
                &script(&[r#"{"method": "a"}"#, r#"{"method": "b"}"#]),
            );
            assert_eq!(
                lsp.receive_notification(Some("b")).await?,
                Some(json!({"method": "b"}))
            );
            assert_eq!(lsp.notifications, [json!({"method": "a"})]);
            lsp.close().await;
            Ok(())
        }
    }
}
