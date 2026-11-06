/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::lsp::LspResponseError;
use buck2_e2e::lsp::file_uri;
use buck2_e2e::piped::PipedProcess;
use buck2_e2e::prelude::*;
use buck2_e2e::process;
use buck2_e2e::subscribe::SubscribeClient;
use regex::Regex;
use serde_json::Value;
use tokio::time::Instant;

/// A span within a template file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Span {
    start_line: u64,
    start_col: u64,
    end_line: u64,
    end_col: u64,
}

/// Takes a multiline string, and finds the line and column based on the string index.
struct Lines {
    lines: Vec<(usize, usize)>,
}

impl Lines {
    fn new(content: &str) -> Self {
        let mut lines = Vec::new();
        let mut idx = 0;
        for line in content.split_inclusive('\n') {
            let new_idx = idx + line.chars().count();
            lines.push((idx, new_idx));
            idx = new_idx;
        }
        lines.push((idx, content.chars().count()));
        Self { lines }
    }

    fn find_range(&self, start_pos: usize, end_pos: usize) -> anyhow::Result<Span> {
        fn pos_in_line(line_start: usize, line_end: usize, pos: usize) -> Option<usize> {
            if pos < line_start || pos >= line_end {
                None
            } else {
                Some(pos - line_start)
            }
        }

        let mut start = None;
        let mut end = None;
        for (lineno, &(line_start, line_end)) in self.lines.iter().enumerate() {
            if let Some(s_col) = pos_in_line(line_start, line_end, start_pos) {
                start = Some((lineno, s_col));
            }
            if let Some(e_col) = pos_in_line(line_start, line_end, end_pos) {
                end = Some((lineno, e_col));
            }
        }

        match (start, end) {
            (Some((start_line, start_col)), Some((end_line, end_col))) => Ok(Span {
                start_line: start_line as u64,
                start_col: start_col as u64,
                end_line: end_line as u64,
                end_col: end_col as u64,
            }),
            _ => anyhow::bail!("Could not find range for position `{start_pos}` and `{end_pos}`"),
        }
    }
}

/// A template file with named spans, to make things like clicking and getting a range back easier.
///
/// The template
/// ```text
/// def <foo>f<f>o</f>o</foo>(x: "string"):
///     pass
/// ```
/// has the content
/// ```text
/// def foo(x: "string"):
///     pass
/// ```
/// and the spans `foo` (0, 4, 0, 7) and `f` (0, 5, 0, 6).
struct Fixture {
    content: String,
    spans: HashMap<String, Span>,
}

impl Fixture {
    fn new(content: &str) -> anyhow::Result<Self> {
        static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(/)?(\w[-\w_]*)>").unwrap());

        // Positions count characters, as the template's string indices did in Python.
        let mut raw_positions: Vec<(String, usize, Option<usize>)> = Vec::new();
        let mut new_content = String::new();
        let mut idx = 0;
        while idx < content.len() {
            let Some(m) = TAG.captures_at(content, idx) else {
                break;
            };
            let whole = m.get(0).expect("the whole match");
            new_content.push_str(&content[idx..whole.start()]);
            idx = whole.end();

            let identifier = &m[2];
            let position = new_content.chars().count();
            let existing = raw_positions.iter_mut().find(|(id, _, _)| id == identifier);
            if m.get(1).is_some() {
                // Closing tag. Must have seen an open tag
                let Some(existing) = existing else {
                    anyhow::bail!("Found closing, but not starting tag, for `{identifier}`");
                };
                anyhow::ensure!(
                    existing.2.is_none(),
                    "Found duplicate closing tags for `{identifier}`"
                );
                existing.2 = Some(position);
            } else {
                // Open tag. Duplicates are not allowed.
                anyhow::ensure!(existing.is_none(), "Duplicate span `{identifier}`");
                raw_positions.push((identifier.to_owned(), position, None));
            }
        }
        new_content.push_str(&content[idx..]);

        let lines = Lines::new(&new_content);
        let spans = raw_positions
            .into_iter()
            .map(|(identifier, start, end)| {
                let end = end.with_context(|| format!("No closing tag for `{identifier}`"))?;
                Ok((identifier, lines.find_range(start, end)?))
            })
            .collect::<anyhow::Result<_>>()?;

        Ok(Self {
            content: new_content,
            spans,
        })
    }

    fn span(&self, id: &str) -> Span {
        self.spans[id]
    }

    fn start_line(&self, id: &str) -> u64 {
        self.spans[id].start_line
    }

    fn start_col(&self, id: &str) -> u64 {
        self.spans[id].start_col
    }
}

/// Assert that this Span is equal to an LSP range dict
#[track_caller]
fn assert_range(range: &Value, expected: Option<Span>) {
    let expected = expected.unwrap_or_default();
    assert_eq!(range["start"]["line"], expected.start_line, "{range}");
    assert_eq!(range["start"]["character"], expected.start_col, "{range}");
    assert_eq!(range["end"]["line"], expected.end_line, "{range}");
    assert_eq!(range["end"]["character"], expected.end_col, "{range}");
}

#[track_caller]
fn assert_uris(actual: &Value, expected: &str) {
    let actual = actual.as_str().expect("a URI");
    if cfg!(windows) {
        // Windows file paths are case-insensitive, and the LSP returns the drive identifier in upper-case.
        // Windows also allows paths to use forward and backward slashes interchangeably.
        // Normalize the paths only on Windows to avoid flakiness.
        assert_eq!(
            actual.replace('\\', "/").replace("%3A", ":").to_lowercase(),
            expected.to_lowercase()
        );
    } else {
        assert_eq!(actual, expected);
    }
}

#[track_caller]
fn assert_goto_result(
    res: &Value,
    expected_src: Span,
    expected_dest_path: &Path,
    expected_dest: Option<Span>,
) {
    let res = res.as_array().expect("a list of locations");
    assert_eq!(res.len(), 1, "{res:?}");
    assert_range(&res[0]["originSelectionRange"], Some(expected_src));
    assert_range(&res[0]["targetRange"], expected_dest);
    assert_range(&res[0]["targetSelectionRange"], expected_dest);
    assert_uris(&res[0]["targetUri"], &file_uri(expected_dest_path));
}

fn fixture(buck: &Buck, path: &str) -> anyhow::Result<Fixture> {
    let abs_path = buck.cwd().join(path);
    let fixture = Fixture::new(&std::fs::read_to_string(&abs_path)?)?;
    std::fs::write(&abs_path, &fixture.content)?;
    Ok(fixture)
}

async fn wait_for_exit(process: &mut PipedProcess, timeout: Duration) -> bool {
    process.wait_timeout(timeout).await.is_some()
}

async fn kill_if_alive(process: &mut PipedProcess) {
    process.kill(Duration::from_secs(30)).await;
}

async fn wait_for_file_to_contain(path: &Path, substring: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() && std::fs::read_to_string(path).is_ok_and(|text| text.contains(substring))
        {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    false
}

fn active_commands_snapshot_has_command(msg: &Value, command_name: &str) -> bool {
    let Some(snapshot) = msg
        .get("response")
        .and_then(|r| r.get("ActiveCommandsSnapshot"))
    else {
        return false;
    };

    snapshot["active_commands"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|command| {
            command["argv"]
                .as_array()
                .is_some_and(|argv| argv.iter().any(|a| a == command_name))
        })
}

async fn wait_for_active_command_state(
    subscribe: &mut SubscribeClient,
    command_name: &str,
    present: bool,
    timeout: Duration,
) -> anyhow::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }

        let msg = match tokio::time::timeout(remaining, subscribe.read_message()).await {
            Err(_) => return Ok(false),
            Ok(msg) => msg?,
        };

        if active_commands_snapshot_has_command(&msg, command_name) == present {
            return Ok(true);
        }
    }
}

/// Whether the process `pid`, a JSON number, exists.
fn daemon_is_alive(pid: &Value) -> bool {
    process::daemon_is_alive(
        pid.as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .expect("a process id"),
    )
}

async fn status_pid(buck: &Buck) -> anyhow::Result<Value> {
    let status = buck.status(&[]).await?;
    let status: Value = serde_json::from_str(&status.stdout)?;
    status
        .pointer("/process_info/pid")
        .filter(|pid| pid.is_u64())
        .cloned()
        .context("No `process_info.pid` in `buck2 status`")
}

/// The daemon's pid, as its `buckd.info` file has it.
fn info_pid(daemon_info: &Path) -> anyhow::Result<Value> {
    let info: Value = serde_json::from_str(&std::fs::read_to_string(daemon_info)?)?;
    info.get("pid")
        .filter(|pid| pid.is_u64())
        .cloned()
        .with_context(|| format!("No `pid` in {info}"))
}

#[buck_test]
async fn test_lsp_starts(buck: &Buck) -> anyhow::Result<()> {
    let mut lsp = buck.lsp(&[]);
    // Will fail if the initialize response is not received
    lsp.init_connection().await?;
    lsp.close().await;
    Ok(())
}

#[buck_test]
async fn test_lsp_stdin_eof_clears_server_command(buck: &Buck) -> anyhow::Result<()> {
    let mut subscribe = buck.subscribe(&["--active-commands"]);
    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    assert!(
        wait_for_active_command_state(&mut subscribe, "lsp", true, Duration::from_secs(10)).await?
    );

    lsp.process().close_stdin();

    let exited = wait_for_exit(lsp.process(), Duration::from_secs(10)).await;
    assert!(exited);
    assert!(lsp.process().try_wait().is_some());

    assert!(
        wait_for_active_command_state(&mut subscribe, "lsp", false, Duration::from_secs(10))
            .await?
    );
    kill_if_alive(lsp.process()).await;
    subscribe.close().await?;
    buck.kill().await?;
    Ok(())
}

#[buck_test(env = [("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")])]
async fn test_lsp_exits_when_daemon_times_out(buck: &Buck) -> anyhow::Result<()> {
    // The inactivity timer only resets when a command *starts*, so it fires while the
    // lsp is still connected. The daemon cannot drain that stream, so it exits on its
    // shutdown deadline instead of waiting, and the lsp follows its daemon down.
    buck.server(&[]).await?;
    let pid = status_pid(buck).await?;
    let daemon_dir = buck.daemon_dir().await?;
    let daemon_stderr = daemon_dir.join("buckd.stderr");

    let mut lsp = buck.lsp(&[]);
    let saw_inactivity_timeout = wait_for_file_to_contain(
        &daemon_stderr,
        "inactivity timeout elapsed",
        Duration::from_secs(20),
    )
    .await;
    assert!(saw_inactivity_timeout);

    let exited = wait_for_exit(lsp.process(), Duration::from_secs(20)).await;
    assert!(exited);
    assert!(!daemon_is_alive(&pid));
    kill_if_alive(lsp.process()).await;
    Ok(())
}

#[buck_test(
    skip_for_os = ["windows"],
    env = [("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true"), ("BUCKD_STARTUP_TIMEOUT", "90")],
)]
async fn test_lsp_daemon_inactivity_shutdown_recovers_with_different_version(
    buck: &Buck,
) -> anyhow::Result<()> {
    // `buckd.info` outlives the daemon, and a version mismatch in it is what sends the
    // client down the connect-to-the-existing-daemon path. That used to cost the full
    // `BUCKD_STARTUP_TIMEOUT`, because the daemon accepted the connection and then
    // never answered it. Now there is nothing listening and the client replaces it.
    buck.server(&[]).await?;
    let original_pid = status_pid(buck).await?;
    let daemon_dir = buck.daemon_dir().await?;
    let daemon_stderr = daemon_dir.join("buckd.stderr");
    let daemon_info = daemon_dir.join("buckd.info");

    let mut lsp = buck.lsp(&[]);
    let saw_inactivity_timeout = wait_for_file_to_contain(
        &daemon_stderr,
        "inactivity timeout elapsed",
        Duration::from_secs(20),
    )
    .await;
    assert!(saw_inactivity_timeout);

    let exited = wait_for_exit(lsp.process(), Duration::from_secs(20)).await;
    assert!(exited);
    assert!(!daemon_is_alive(&original_pid));

    let mut info: Value = serde_json::from_str(&std::fs::read_to_string(&daemon_info)?)?;
    info["version"] = Value::from("different-version");
    std::fs::write(&daemon_info, serde_json::to_string(&info)?)?;

    let start = Instant::now();
    buck.server(&[]).await?;
    let elapsed = start.elapsed();

    // Well inside the 90s budget this used to consume in full.
    assert!(
        elapsed < Duration::from_secs(30),
        "took {:.2}s to recover",
        elapsed.as_secs_f64()
    );
    assert_ne!(info_pid(&daemon_info)?, original_pid);
    kill_if_alive(lsp.process()).await;
    Ok(())
}

#[buck_test]
async fn test_lsp_exits_when_daemon_disappears(buck: &Buck) -> anyhow::Result<()> {
    buck.server(&[]).await?;

    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    buck.kill().await?;

    let exited = wait_for_exit(lsp.process(), Duration::from_secs(10)).await;
    assert!(exited);
    assert!(lsp.process().try_wait().is_some());
    kill_if_alive(lsp.process()).await;
    Ok(())
}

#[buck_test(env = [("BUCK2_TESTING_INACTIVITY_TIMEOUT", "true")])]
async fn test_lsp_requests_keep_daemon_alive(buck: &Buck) -> anyhow::Result<()> {
    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    let daemon_info = buck.daemon_dir().await?.join("buckd.info");
    let pid = info_pid(&daemon_info)?;

    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        lsp.open_file("clean_lint.bzl", None).await?;
    }

    assert_eq!(info_pid(&daemon_info)?, pid);
    assert!(lsp.process().try_wait().is_none());
    lsp.close().await;
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_lsp_exits_when_daemon_is_killed(buck: &Buck) -> anyhow::Result<()> {
    buck.server(&[]).await?;
    let pid = status_pid(buck).await?;

    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    let killed = std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()?;
    assert!(killed.success());

    let exited = wait_for_exit(lsp.process(), Duration::from_secs(8)).await;
    assert!(exited);
    assert!(lsp.process().try_wait().is_some());
    kill_if_alive(lsp.process()).await;
    Ok(())
}

#[buck_test]
async fn test_lints_on_open(buck: &Buck) -> anyhow::Result<()> {
    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    let diags = lsp.open_file("clean_lint.bzl", None).await?;
    assert_eq!(
        diags["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "{diags}"
    );

    let diags = lsp.open_file("bad_syntax.bzl", None).await?;
    assert_eq!(
        diags["diagnostics"].as_array().map(Vec::len),
        Some(1),
        "{diags}"
    );
    lsp.close().await;
    Ok(())
}

#[buck_test]
async fn test_goto_definition(buck: &Buck) -> anyhow::Result<()> {
    let src_targets_path = "dir/TARGETS.fixture";
    let dest_targets_path = "cell/sub/TARGETS.fixture";
    let dest_bzl_path = "cell/sub/defs.bzl";

    let src_targets = fixture(buck, src_targets_path)?;
    let dest_targets = fixture(buck, dest_targets_path)?;
    let dest_bzl = fixture(buck, dest_bzl_path)?;

    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    let diags = lsp.open_file(src_targets_path, None).await?;
    assert_eq!(
        diags["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "{diags}"
    );

    let res = lsp
        .goto_definition(
            src_targets_path,
            src_targets.start_line("load_click"),
            src_targets.start_col("load_click"),
        )
        .await?
        .expect("a result");
    assert_goto_result(
        &res,
        src_targets.span("load"),
        &buck.cwd().join(dest_bzl_path),
        None,
    );

    let res = lsp
        .goto_definition(
            src_targets_path,
            src_targets.start_line("dummy_click"),
            src_targets.start_col("dummy_click"),
        )
        .await?
        .expect("a result");
    assert_goto_result(
        &res,
        src_targets.span("dummy"),
        &buck.cwd().join(dest_bzl_path),
        Some(dest_bzl.span("rule")),
    );

    let res = lsp
        .goto_definition(
            src_targets_path,
            src_targets.start_line("missing_click"),
            src_targets.start_col("missing_click"),
        )
        .await?
        .expect("a result");
    assert_eq!(res.as_array().map(Vec::len), Some(0), "{res}");

    let res = lsp
        .goto_definition(
            src_targets_path,
            src_targets.start_line("missing_foo_click"),
            src_targets.start_col("missing_foo_click"),
        )
        .await?
        .expect("a result");
    assert_goto_result(
        &res,
        src_targets.span("missing_foo"),
        &buck.cwd().join(dest_targets_path),
        None,
    );

    let res = lsp
        .goto_definition(
            src_targets_path,
            src_targets.start_line("rule_click"),
            src_targets.start_col("rule_click"),
        )
        .await?
        .expect("a result");
    assert_goto_result(
        &res,
        src_targets.span("rule"),
        &buck.cwd().join(dest_bzl_path),
        Some(dest_bzl.span("rule")),
    );

    let res = lsp
        .goto_definition(
            src_targets_path,
            src_targets.start_line("baz_click"),
            src_targets.start_col("baz_click"),
        )
        .await?
        .expect("a result");
    assert_goto_result(
        &res,
        src_targets.span("baz"),
        &buck.cwd().join(dest_targets_path),
        Some(dest_targets.span("baz")),
    );
    lsp.close().await;
    Ok(())
}

#[buck_test]
async fn test_returns_file_contents_for_starlark_types(buck: &Buck) -> anyhow::Result<()> {
    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;

    let res = lsp
        .file_contents("starlark:/native/DefaultInfo.bzl")
        .await?
        .expect("a result");
    assert!(!res["contents"].is_null(), "{res}");

    let res = lsp
        .file_contents("starlark:/native/NonExistent.bzl")
        .await?
        .expect("a result");
    assert_eq!(res.get("contents"), Some(&Value::Null), "{res}");

    let buckconfig = file_uri(&lsp.cwd().join(".buckconfig"));
    let error = lsp
        .file_contents(&buckconfig)
        .await
        .expect_err("`.buckconfig` is not a Starlark file");
    assert!(
        error.downcast_ref::<LspResponseError>().is_some(),
        "{error:#}"
    );
    lsp.close().await;
    Ok(())
}

#[buck_test]
async fn test_goto_definition_for_globals(buck: &Buck) -> anyhow::Result<()> {
    let globals_bzl_path = "globals.bzl";

    let globals_bzl = fixture(buck, globals_bzl_path)?;
    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    let diags = lsp.open_file(globals_bzl_path, None).await?;
    assert_eq!(
        diags["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "{diags}"
    );

    let res = lsp
        .goto_definition(
            globals_bzl_path,
            globals_bzl.start_line("func2_click"),
            globals_bzl.start_col("func2_click"),
        )
        .await?
        .expect("a result");

    assert_eq!(res.as_array().map(Vec::len), Some(1), "{res}");
    assert_range(
        &res[0]["originSelectionRange"],
        Some(globals_bzl.span("func2")),
    );
    for range in ["targetRange", "targetSelectionRange"] {
        let line = res[0].pointer(&format!("/{range}/start/line"));
        assert!(
            line.and_then(Value::as_u64).is_some_and(|l| l != 0),
            "{res}"
        );
    }
    assert_uris(
        &res[0]["targetUri"],
        &file_uri(&buck.cwd().join("prelude").join("prelude.bzl")),
    );

    let res = lsp
        .goto_definition(
            globals_bzl_path,
            globals_bzl.start_line("info_click"),
            globals_bzl.start_col("info_click"),
        )
        .await?
        .expect("a result");

    assert_eq!(res.as_array().map(Vec::len), Some(1), "{res}");
    assert_range(
        &res[0]["originSelectionRange"],
        Some(globals_bzl.span("info")),
    );
    assert_uris(&res[0]["targetUri"], "starlark:/native/DefaultInfo.bzl");

    let res = lsp
        .goto_definition(
            globals_bzl_path,
            globals_bzl.start_line("invalid_click"),
            globals_bzl.start_col("invalid_click"),
        )
        .await?
        .expect("a result");
    assert_eq!(res.as_array().map(Vec::len), Some(0), "{res}");
    lsp.close().await;
    Ok(())
}

#[buck_test]
async fn test_supports_bxl_files(buck: &Buck) -> anyhow::Result<()> {
    let src_bxl_path = "query.bxl";

    let src_bxl = fixture(buck, src_bxl_path)?;

    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    let diags = lsp.open_file(src_bxl_path, None).await?;
    assert_eq!(
        diags["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "{diags}"
    );

    let res = lsp
        .goto_definition(
            src_bxl_path,
            src_bxl.start_line("foo_click"),
            src_bxl.start_col("foo_click"),
        )
        .await?
        .expect("a result");
    assert_goto_result(
        &res,
        src_bxl.span("foo"),
        &buck.cwd().join(src_bxl_path),
        Some(src_bxl.span("dest_foo")),
    );

    let res = lsp
        .goto_definition(
            src_bxl_path,
            src_bxl.start_line("f_click"),
            src_bxl.start_col("f_click"),
        )
        .await?
        .expect("a result");
    assert_goto_result(
        &res,
        src_bxl.span("f"),
        &buck.cwd().join(src_bxl_path),
        Some(src_bxl.span("dest_f")),
    );
    lsp.close().await;
    Ok(())
}

#[buck_test]
async fn test_goto_definition_after_non_ascii_text(buck: &Buck) -> anyhow::Result<()> {
    // The LSP column counts UTF-16 units; `x` is at column 12 after a three-character
    // CJK string, at byte 18.
    let path = "utf16_column.bzl";
    let mut lsp = buck.lsp(&[]);
    lsp.init_connection().await?;
    lsp.open_file(path, None).await?;
    let res = lsp.goto_definition(path, 9, 12).await?.expect("a result");
    assert_eq!(
        res.pointer("/0/targetRange"),
        Some(&serde_json::json!({
            "start": {"line": 8, "character": 0},
            "end": {"line": 8, "character": 1},
        })),
        "{res}"
    );
    lsp.close().await;
    Ok(())
}
