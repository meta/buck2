/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::LazyLock;

use buck2_e2e::prelude::*;
use regex::Regex;

// One target per character a `superconsole::Span` refuses, each reporting a single
// test case under a name that holds it (see `TARGETS.fixture`).
const TARGETS: [&str; 2] = ["line_break", "tab"];

// `line_break` reports a failing test and `tab` a skipped one.
fn expected_exit_code(target: &str) -> i32 {
    match target {
        "line_break" => 32,
        "tab" => 0,
        _ => unreachable!("no expected exit code for `{target}`"),
    }
}

static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[[^\]]+\] ?").unwrap());
static ANSI_ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
static RESULT_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(✗ Fail|↷ Skip): ").unwrap());
const SPAN_ERROR: &str = "contains non-space whitespace";

/// Runs `buck2 test` and returns its exit code and stderr, without failing.
async fn run_test(buck: &Buck, console: &str, target: &str) -> (i32, String) {
    match buck
        .test(&["--console", console, &format!(":{target}")])
        .await
    {
        Ok(result) => (0, result.stderr),
        Err(e) => {
            let exit_code = e.exit_code().expect("a failed buck2 process has exited");
            (exit_code, e.into_output().stderr)
        }
    }
}

/// The exit code, the test result lines, and any command failure, as shown.
fn simple_console_output(exit_code: i32, stderr: &str) -> String {
    let mut lines = vec![format!("exit code: {exit_code}")];
    let mut in_failure = false;
    for line in stderr.lines() {
        let line = TIMESTAMP.replace(line, "");
        in_failure = in_failure || line.starts_with("Command failed:");
        if !line.is_empty() && (in_failure || RESULT_LINE.is_match(&line)) {
            lines.push(line.into_owned());
        }
    }
    lines.join("\n")
}

#[buck_test]
async fn test_simple_console(buck: &Buck) {
    for target in TARGETS {
        let (exit_code, stderr) = run_test(buck, "simple", target).await;
        golden(
            &simple_console_output(exit_code, &stderr),
            &format!("fixtures/{target}.simple.golden.txt"),
        );
    }
}

#[buck_test]
async fn test_superconsole(buck: &Buck) {
    // The superconsole redraws its output, so the test checks the outcome
    // rather than comparing it with a golden file.
    for target in TARGETS {
        let (exit_code, stderr) = run_test(buck, "super", target).await;
        let stderr = ANSI_ESCAPE.replace_all(&stderr, "");
        assert!(!stderr.contains(SPAN_ERROR), "{target}: {stderr}");
        assert!(
            stderr.lines().any(|line| RESULT_LINE.is_match(line)),
            "{target}: no result line in {stderr}"
        );
        assert_eq!(
            exit_code,
            expected_exit_code(target),
            "{target}: exit code {exit_code}"
        );
    }
}
