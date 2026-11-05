/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Golden files: expected output checked in next to the test's data, and the sanitizers that make
//! output stable enough to compare.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::NoExpand;
use regex::Regex;

/// The directory of a test's data where golden files usually go.
pub const GOLDEN_DIRECTORY: &str = "fixtures/";

const UPDATE_HINT: &str =
    "Re-run test with `-- --env BUCK2_UPDATE_GOLDEN=1` appended to the test command to regenerate the files";

/// Panics unless `output` matches the golden file at `rel_path` in the target's test data source.
/// With `BUCK2_UPDATE_GOLDEN` set, writes `output` there instead.
#[track_caller]
pub fn golden(output: &str, rel_path: &str) {
    if let Err(message) = golden_at(&test_repo_data_src(), update_golden(), output, rel_path) {
        panic!("{message}");
    }
}

fn test_repo_data_src() -> PathBuf {
    let src = std::env::var_os("TEST_REPO_DATA")
        .expect("golden files live in the target's test data, but it has none");
    std::path::absolute(PathBuf::from(src)).expect("an absolute test data path")
}

fn update_golden() -> bool {
    std::env::var_os("BUCK2_UPDATE_GOLDEN").is_some()
}

/// [`golden`] for a directory: `output` maps paths relative to `rel_path` to their expected
/// contents, and the directory must hold no other files. With `BUCK2_UPDATE_GOLDEN` set, other
/// files are deleted.
#[track_caller]
pub fn golden_dir(output: &BTreeMap<String, String>, rel_path: &str) {
    assert!(
        rel_path.contains("golden"),
        "Golden path `{rel_path}` must contain `golden`"
    );
    for (file, contents) in output {
        golden(contents, &Path::new(rel_path).join(file).to_string_lossy());
    }
    let dir = test_repo_data_src().join(rel_path);
    let mut files = Vec::new();
    list_files(&dir, &mut files);
    for file in files {
        let rel_file = file
            .strip_prefix(&dir)
            .expect("listed under the directory")
            .to_string_lossy()
            .replace('\\', "/");
        if !output.contains_key(&rel_file) {
            if update_golden() {
                std::fs::remove_file(&file)
                    .unwrap_or_else(|e| panic!("Deleting `{}`: {e}", file.display()));
            } else {
                panic!("Extra golden file `{rel_file}` found, please remove it");
            }
        }
    }
}

fn list_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            list_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// [`golden`], with the temporary directory `tmp_path` in `output` replaced by `tmp-path`, and
/// backslashes by slashes.
#[track_caller]
pub fn golden_replace_temp_path(output: &str, rel_path: &str, tmp_path: &str) {
    // In JSON, a Windows path's backslashes are escaped.
    let tmp_path_escaped = tmp_path.replace('\\', "\\\\");
    golden(
        &output
            .replace(&tmp_path_escaped, "tmp-path")
            .replace("\\\\", "/"),
        rel_path,
    )
}

/// [`golden`], with configuration hashes replaced by a placeholder.
#[track_caller]
pub fn golden_replace_cfg_hash(output: &str, rel_path: &str) {
    static CFG_HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-f]{16}\b").unwrap());
    golden(&CFG_HASH.replace_all(output, NoExpand("<HASH>")), rel_path)
}

fn golden_at(src: &Path, update: bool, output: &str, rel_path: &str) -> Result<(), String> {
    if !rel_path.contains("golden") {
        return Err(format!("Golden path `{rel_path}` must contain `golden`"));
    }
    // "@generated" is split so that this file is not marked generated itself.
    let output = normalize_newlines(&format!(
        "# This file is {}generated, regenerate by re-running test with `-- --env BUCK2_UPDATE_GOLDEN=1` appended to the test command\n\n{output}",
        "@"
    ));
    let path = src.join(rel_path);

    if update {
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap_or(src))?;
            std::fs::write(&path, &output)
        };
        return write().map_err(|e| format!("Writing `{}`: {e}", path.display()));
    }

    let expected = std::fs::read_to_string(&path)
        .map_err(|e| format!("Golden path `{}` must exist: {e}\n\n{UPDATE_HINT}", path.display()))?;
    if remove_ci_labels(&expected) != remove_ci_labels(&output) {
        let file = path.display().to_string();
        let diff = similar::TextDiff::from_lines(&expected, &output)
            .unified_diff()
            .context_radius(3)
            .header(&file, &file)
            .to_string();
        return Err(format!(
            "Expected golden file to match actual\n\n\n{diff}\n\n{UPDATE_HINT}"
        ));
    }
    Ok(())
}

/// Golden files compare text, so line endings are normalized to be independent of the platform.
fn normalize_newlines(content: &str) -> String {
    split_lines(content)
        .into_iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

/// The lines of `s`, split as Python's `str.splitlines` splits them.
fn split_lines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(
            c,
            '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}'
        ) {
            lines.push(&s[start..i]);
            start = i + c.len_utf8();
            if c == '\r' && chars.next_if(|&(_, next)| next == '\n').is_some() {
                start += 1;
            }
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// Drops the CI labels from target listings: CI jobs add them, local runs do not.
fn remove_ci_labels(content: &str) -> String {
    static CI_LABEL_LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^\s*"ci:[^"]*",?$"#).unwrap());
    split_lines(content)
        .into_iter()
        .filter(|line| !CI_LABEL_LINE.is_match(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replaces the hashes that vary between runs and platforms with placeholders.
pub fn sanitize_hashes(s: &str) -> String {
    static STRING_HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9]{16,}\b").unwrap());
    // Configuration hashes do not always print as 16 characters, and that's hard to fix because
    // action digests may not change.
    static CFG_HASH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b[0-9a-f]{12,16}\b").unwrap());
    static DIGEST: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b[0-9a-f]{40}:[0-9]{1,3}\b").unwrap());
    let s = STRING_HASH.replace_all(s, NoExpand("<STRING_HASH>"));
    let s = CFG_HASH.replace_all(&s, NoExpand("<HASH>"));
    DIGEST.replace_all(&s, NoExpand("<DIGEST>")).into_owned()
}

/// Drops the lines that the C++ libraries in buck2 (glog and folly logging) and the client's own
/// `tracing` write to stderr: their timestamps, PIDs and source locations vary between runs, and
/// which of them appear depends on timing. The rest of the text, newlines included, is unchanged.
pub fn strip_glog_lines(s: &str) -> String {
    // The glog marker may be indented, as when it is nested in an embedded daemon stderr block.
    static GLOG_LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r".*[VIWECF]\d{4} \d{2}:\d{2}:\d{2}\.\d{6}.*\n?").unwrap());
    static TRACING_LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"(?m)^(?:\x1b\[[0-9;]*m)*\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z(?:\x1b\[[0-9;]*m)*",
            r"\s+(?:\x1b\[[0-9;]*m)*\s*(?:TRACE|DEBUG|INFO|WARN|ERROR)(?:\x1b\[[0-9;]*m)*\b.*\n?",
        ))
        .unwrap()
    });
    let s = GLOG_LINE.replace_all(s, "");
    TRACING_LINE.replace_all(&s, "").into_owned()
}

/// Drops jemalloc's warning about an option that some fbcode binaries have baked into their
/// allocator configuration, where `MALLOC_CONF` cannot suppress it.
pub fn strip_jemalloc_invalid_conf(s: &str) -> String {
    static INVALID_CONF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^[ \t]*<jemalloc>: Invalid conf pair: experimental_infallible_new:true\n?",
        )
        .unwrap()
    });
    INVALID_CONF.replace_all(s, "").into_owned()
}

/// A submitted remote action reports whether remote execution ran it, served it from its cache, or
/// merged it with an identical execution in flight. The last two depend on what other builds are
/// doing with the same action, so all three read as executed.
pub fn sanitize_remote_action_origin(s: &str) -> String {
    static ORIGIN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"Remote action (?:served from the remote execution cache|deduplicated by remote execution),",
        )
        .unwrap()
    });
    ORIGIN
        .replace_all(s, NoExpand("Remote action,"))
        .into_owned()
}

/// Makes buck2's stderr comparable between runs: drops the log lines that vary, and replaces
/// timestamps, ids, daemon details, statistics and hashes with placeholders.
pub fn sanitize_stderr(s: &str) -> String {
    static REPLACEMENTS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"\[.{29}\]", "[<TIMESTAMP>]"),
            (
                r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
                "<UUID>",
            ),
            (
                r"(?m)^    Daemon process info from .+:?$",
                "    Daemon process info from <BUCKD_INFO>",
            ),
            (r"(?m)^    daemon dir: .+$", "    daemon dir: <DAEMON_DIR>"),
            (r"(?m)^    pid: \d+$", "    pid: <PID>"),
            (r"(?m)^    endpoint: tcp:\d+$", "    endpoint: tcp:<PORT>"),
            (r"(?m)^    version: \S+$", "    version: <VERSION>"),
            (r"Commands: .+", "Commands: <COMMAND_STATS>"),
            (r"Cache hits: .+", "Cache hits: <CACHE_STATS>"),
            (r"Network: .+", "Network: <NETWORK_STATS>"),
            (
                r"\([0-9]+\) panicked at .+",
                "(<THREAD_ID>) panicked at <PATH>",
            ),
        ]
        .into_iter()
        .map(|(pattern, replacement)| (Regex::new(pattern).unwrap(), replacement))
        .collect()
    });
    let mut s =
        sanitize_remote_action_origin(&strip_jemalloc_invalid_conf(&strip_glog_lines(s)));
    for (regex, replacement) in REPLACEMENTS.iter() {
        s = regex.replace_all(&s, NoExpand(replacement)).into_owned();
    }
    sanitize_hashes(&s)
}

/// [`sanitize_stderr`] for daemon output embedded in an error message: drops ANSI escapes and the
/// lines that vary between environments, and replaces the daemon's timestamps, PIDs, ports,
/// versions, scratch paths and timeouts with placeholders.
pub fn sanitize_daemon_stderr(s: &str) -> String {
    static REPLACEMENTS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"\x1b\[[0-9;]*m", ""),
            (
                r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z",
                "<DAEMON_TIMESTAMP>",
            ),
            (r"PID: \d+", "PID: <PID>"),
            (r"Endpoint: tcp:\d+", "Endpoint: tcp:<PORT>"),
            (r"Version: \S+", "Version: <VERSION>"),
            (r"/data/users/\S+", "<SCRATCH_PATH>"),
            (r"timed out after [\d.]+s", "timed out after <DURATION>"),
            // Env overrides vary by environment.
            (r"(?m)^.*Env override found.*\n", ""),
            // BPFJailer is only used on hosts where the jail is available.
            (r"(?m)^.*Exited BPFJailer jail.*\n", ""),
            (r"(?m) +$", ""),
        ]
        .into_iter()
        .map(|(pattern, replacement)| (Regex::new(pattern).unwrap(), replacement))
        .collect()
    });
    let mut s = s.to_owned();
    for (regex, replacement) in REPLACEMENTS.iter() {
        s = regex.replace_all(&s, NoExpand(replacement)).into_owned();
    }
    sanitize_stderr(&s)
}

/// [`sanitize_stderr`] for a panic: also drops the frames of the backtrace, which vary with what
/// was running when buck2 crashed.
pub fn sanitize_stacktrace(s: &str) -> String {
    static BACKTRACE_START: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\[<TIMESTAMP>\].*stack backtrace:\s*$").unwrap());
    static BACKTRACE_END: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\[<TIMESTAMP>\]\s+note:").unwrap());
    static FRAME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\[<TIMESTAMP>\]((\s+\d+:)|(\s+at )).*").unwrap());
    let s = sanitize_stderr(s);
    let mut lines = split_lines(&s);
    // A long frame symbol wraps over several lines, of which only the first is numbered, so the
    // whole block of frames goes.
    if let Some(start) = lines.iter().position(|l| BACKTRACE_START.is_match(l)) {
        if let Some(end) = (start + 1..lines.len()).find(|&i| BACKTRACE_END.is_match(lines[i])) {
            lines.drain(start + 1..end);
        }
    }
    lines
        .into_iter()
        .filter(|l| !FRAME.is_match(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keeps only what matters of an error in a build report: analysis errors and unknown target
/// errors mention line numbers and target lists that change with any edit to the fixture.
pub fn sanitize_build_report_error(s: &str) -> String {
    static ANALYSIS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"Error running analysis for.*""#).unwrap());
    static UNKNOWN_TARGET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"Unknown target `.*` from package .*""#).unwrap());
    let s = ANALYSIS.replace_all(s, NoExpand(r#"Error running analysis for <IRRELEVANT>""#));
    let s = UNKNOWN_TARGET.replace_all(
        &s,
        NoExpand(r#"Unknown target `<TARGET>` from package <IRRELEVANT>""#),
    );
    sanitize_hashes(&s)
}

/// Makes a build report comparable between runs: sanitizes its strings, drops its trace id and
/// project root, and renumbers its string cache, whose keys vary with platform hashes.
pub fn sanitize_build_report(report: &mut serde_json::Value) {
    fn sanitize(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) => {
                *s = sanitize_remote_action_origin(&strip_jemalloc_invalid_conf(s));
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(sanitize),
            serde_json::Value::Object(map) => map.values_mut().for_each(sanitize),
            _ => {}
        }
    }
    sanitize(report);
    let Some(report) = report.as_object_mut() else {
        return;
    };
    report.remove("trace_id");
    report.remove("project_root");
    if let Some(serde_json::Value::Object(strings)) = report.get_mut("strings") {
        let mut values: Vec<serde_json::Value> =
            std::mem::take(strings).into_iter().map(|(_, v)| v).collect();
        values.sort_by_cached_key(|v| sanitize_hashes(v.as_str().unwrap_or_default()));
        // Still shaped like string hashes.
        for (i, value) in values.into_iter().enumerate() {
            strings.insert((10000000000000000 + i).to_string(), value);
        }
    }
}

/// Makes Python output from an action comparable: strips the project directory and the
/// differences between Python versions' syntax error formats.
pub fn sanitize_python(s: &str, project_dir: &Path) -> String {
    static CARETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" *\^+").unwrap());
    static NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\\n)+").unwrap());
    let s = s.replace(&format!("{}/", project_dir.display()), "");
    let s = CARETS.replace_all(&s, "");
    let s = s
        .replace(
            "SyntaxError: invalid syntax",
            "IndentationError: unexpected indent",
        )
        .replace("[syntax] Syntax error!", "[indentation] Indentation error!");
    NEWLINES.replace_all(&s, NoExpand(r"\n")).into_owned()
}

/// Drops the `Waiting on` lines of the console.
pub fn strip_waiting_on(s: &str) -> String {
    split_lines(s)
        .into_iter()
        .filter(|l| !l.contains("Waiting on"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "golden/out.golden.json";

    /// Writes `expected` as the golden file, then checks `actual` against it.
    fn check(expected: &str, actual: &str) -> Result<(), String> {
        let src = tempfile::tempdir().unwrap();
        golden_at(src.path(), true, expected, PATH)?;
        golden_at(src.path(), false, actual, PATH)
    }

    #[test]
    fn test_an_update_writes_the_header_and_normalized_newlines() {
        let src = tempfile::tempdir().unwrap();
        golden_at(src.path(), true, "a\r\nb", PATH).unwrap();
        let written = std::fs::read_to_string(src.path().join(PATH)).unwrap();
        assert!(written.starts_with("# This file is "), "{written}");
        assert!(written.ends_with("\n\na\nb\n"), "{written}");
    }

    #[test]
    fn test_lines_split_as_in_python() {
        assert_eq!(
            split_lines("a\r\nb\rc\x0cd\n\ne\n"),
            ["a", "b", "c", "d", "", "e"]
        );
        assert_eq!(split_lines(""), Vec::<&str>::new());
    }

    #[test]
    fn test_a_plain_change_fails() {
        let error = check("[\n  \"root//tools:lint\"\n]", "[\n  \"root//tools:fmt\"\n]")
            .unwrap_err();
        assert!(error.contains("-  \"root//tools:lint\""), "{error}");
        assert!(error.contains("+  \"root//tools:fmt\""), "{error}");
    }

    #[test]
    fn test_ci_labels_are_ignored() {
        check(
            "[\n  \"ci:overwrite\",\n  \"ci:diff:linux\",\n  \"foo\"\n]",
            "[\n  \"foo\"\n]",
        )
        .unwrap();
    }

    #[test]
    fn test_a_lost_dependency_in_a_ci_package_fails() {
        assert!(check("deps:\n  root//a:a\n  root//ci:lint\n", "deps:\n  root//a:a\n").is_err());
    }

    #[test]
    fn test_a_golden_path_names_itself_golden() {
        let src = tempfile::tempdir().unwrap();
        assert!(golden_at(src.path(), true, "x", "out.json").is_err());
    }

    #[test]
    fn test_a_missing_golden_file_fails() {
        let src = tempfile::tempdir().unwrap();
        assert!(golden_at(src.path(), false, "x", PATH).is_err());
    }

    #[test]
    fn test_hashes_get_placeholders() {
        assert_eq!(
            sanitize_hashes(
                "cfg:<linux>#0123456789abcdef id 12345678901234567 digest 0123456789abcdef0123456789abcdef01234567:12"
            ),
            "cfg:<linux>#<HASH> id <STRING_HASH> digest <DIGEST>"
        );
    }

    #[test]
    fn test_log_lines_are_dropped_and_the_rest_kept() {
        assert_eq!(
            strip_glog_lines(
                "keep\n  I0623 15:40:41.926481 128942 Hash.cpp:327] seed\n\x1b[2m2026-10-15T02:15:11.181159Z\x1b[0m  \x1b[33mWARN\x1b[0m client: slow\n\nend"
            ),
            "keep\n\nend"
        );
    }

    #[test]
    fn test_remote_execution_cache_hits_read_as_executed() {
        for origin in [
            "served from the remote execution cache",
            "deduplicated by remote execution",
        ] {
            assert_eq!(
                sanitize_remote_action_origin(&format!(
                    "Remote action {origin}, reproduce with: `frecli cas download-action <DIGEST>`"
                )),
                "Remote action, reproduce with: `frecli cas download-action <DIGEST>`"
            );
        }
    }

    #[test]
    fn test_daemon_details_get_placeholders() {
        assert_eq!(
            sanitize_daemon_stderr(
                "\x1b[2m2026-03-11T21:36:35.686733Z\x1b[0m PID: 12 Endpoint: tcp:345  \nEnv override found: X\nkept\n"
            ),
            "<DAEMON_TIMESTAMP> PID: <PID> Endpoint: tcp:<PORT>\nkept\n"
        );
    }

    #[test]
    fn test_backtrace_frames_are_dropped() {
        let stderr = concat!(
            "[2024-01-01T00:00:00.000-00:00] thread panicked\n",
            "[2024-01-01T00:00:00.000-00:00] stack backtrace:\n",
            "[2024-01-01T00:00:00.000-00:00]    0: frame\n",
            "a wrapped symbol\n",
            "[2024-01-01T00:00:00.000-00:00] note: run with backtraces\n",
        );
        assert_eq!(
            sanitize_stacktrace(stderr),
            concat!(
                "[<TIMESTAMP>] thread panicked\n",
                "[<TIMESTAMP>] stack backtrace:\n",
                "[<TIMESTAMP>] note: run with backtraces",
            )
        );
    }

    #[test]
    fn test_build_report_strings_are_renumbered_in_sanitized_order() {
        let mut report = serde_json::json!({
            "trace_id": "x",
            "project_root": "/p",
            "strings": {"222": "b", "111": "a"},
        });
        sanitize_build_report(&mut report);
        assert_eq!(
            report,
            serde_json::json!({"strings": {"10000000000000000": "a", "10000000000000001": "b"}})
        );
    }

    #[test]
    fn test_waiting_on_lines_are_dropped() {
        assert_eq!(strip_waiting_on("a\nWaiting on x\nb\n"), "a\nb");
    }

    #[test]
    fn test_stderr_details_get_placeholders() {
        assert_eq!(
            sanitize_stderr(concat!(
                "[2024-01-01T00:00:00.000-00:00] Build ID: 01234567-89ab-cdef-0123-456789abcdef\n",
                "    pid: 1234\n",
                "Commands: 4 (cached: 1)\n",
                "thread 'main' (42) panicked at src/main.rs:1:1\n",
            )),
            concat!(
                "[<TIMESTAMP>] Build ID: <UUID>\n",
                "    pid: <PID>\n",
                "Commands: <COMMAND_STATS>\n",
                "thread 'main' (<THREAD_ID>) panicked at <PATH>\n",
            )
        );
    }
}
