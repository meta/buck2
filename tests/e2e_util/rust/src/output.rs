/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::LazyLock;

use anyhow::Context;
use regex::Regex;
use serde_json::Value;

/// The exit codes of buck2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    UnknownError = 1,
    InfraError = 2,
    UserError = 3,
    DaemonIsBusy = 4,
    DaemonPreempted = 5,
    Timeout = 6,
    ConnectError = 11,
    FatalOom = 12,
    BrokenPipe = 130,
    SignalInterrupt = 141,
}

/// What a finished buck2 command printed and how it exited.
#[derive(Debug, Clone)]
pub struct BuckOutput {
    pub stdout: String,
    pub stderr: String,
    pub(crate) status: ExitStatus,
    /// The whole command line, executable first.
    pub(crate) command: Vec<OsString>,
    /// The subcommand and its arguments, including the ones the harness added.
    pub(crate) args: Vec<String>,
    pub(crate) working_dir: PathBuf,
    pub(crate) invocation_record_path: Option<PathBuf>,
}

impl BuckOutput {
    /// The process exit code; `128 + n` if signal `n` ended it, as a shell reports it.
    pub fn exit_code(&self) -> Option<i32> {
        #[cfg(unix)]
        if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&self.status) {
            return Some(128 + signal);
        }
        self.status.code()
    }

    fn has_arg(&self, arg: &str) -> bool {
        self.args.iter().any(|a| a == arg)
    }

    /// Panics unless the command exited with `expected`.
    #[track_caller]
    pub fn assert_exit_code(&self, expected: ExitCode) -> &Self {
        assert_eq!(
            self.exit_code(),
            Some(expected as i32),
            "expected exit code {expected:?} ({})\n<stderr>\n{}</stderr>",
            expected as i32,
            indent(&self.stderr),
        );
        self
    }

    /// Panics unless `regex` matches somewhere in stderr, ignoring case. `.` matches newlines, and,
    /// as in Python, `$` matches before a final newline too. The syntax is the `regex` crate's: a
    /// literal `{` must be escaped.
    #[track_caller]
    pub fn assert_stderr_matches(&self, regex: &str) -> &Self {
        assert_matches(&format!("(?si){regex}"), regex, "stderr", &self.stderr);
        self
    }

    /// Panics unless `regex` matches somewhere in stdout. `.` matches newlines, and, as in Python,
    /// `$` matches before a final newline too. The syntax is the `regex` crate's: a literal `{` must
    /// be escaped.
    #[track_caller]
    pub fn assert_stdout_matches(&self, regex: &str) -> &Self {
        assert_matches(&format!("(?s){regex}"), regex, "stdout", &self.stdout);
        self
    }

    /// The build report of a `build`, which `Buck::build` asks for on stdout.
    pub fn build_report(&self) -> anyhow::Result<BuildReport> {
        let start = self
            .stdout
            .find('{')
            .with_context(|| format!("No build report on stdout:\n{}", self.stdout))?;
        let end = self.stdout[start..]
            .find('\n')
            .map_or(self.stdout.len(), |end| start + end);
        let json: Value = serde_json::from_str(&self.stdout[start..end])
            .with_context(|| format!("Parsing the build report on stdout:\n{}", self.stdout))?;
        BuildReport::new(json)
    }

    /// The `target output` pairs that `--show-output` or `--show-full-output` prints, by target;
    /// a target without an output maps to the empty string.
    pub fn target_to_build_output(&self) -> anyhow::Result<BTreeMap<String, String>> {
        anyhow::ensure!(
            self.has_arg("--show-output") || self.has_arg("--show-full-output"),
            "Pass `--show-output` or `--show-full-output` to get the outputs"
        );
        let mut lines = self.stdout.trim().lines();
        if self.has_arg("--build-report=-") {
            // The build report comes first.
            lines.next();
        }
        let mut outputs = BTreeMap::new();
        for line in lines {
            let mut parts = line.split_whitespace();
            let target = parts.next().context("Empty `--show-output` line")?;
            let output = parts.next().unwrap_or_default();
            anyhow::ensure!(
                parts.next().is_none(),
                "Expected `target output` but got `{line}`"
            );
            outputs.insert(target.to_owned(), output.to_owned());
        }
        Ok(outputs)
    }

    /// The sorted labels of the targets `targets --json-lines` printed.
    pub fn target_list(&self) -> anyhow::Result<Vec<String>> {
        anyhow::ensure!(
            self.has_arg("--json-lines"),
            "Pass `--json-lines` to get the targets"
        );
        let mut targets = self
            .stdout
            .lines()
            .map(|line| {
                let target: Value = serde_json::from_str(line)?;
                Ok(format!(
                    "{}:{}",
                    target["buck.package"]
                        .as_str()
                        .context("No `buck.package`")?,
                    target["name"].as_str().context("No `name`")?,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        targets.sort();
        Ok(targets)
    }

    /// The JSON object `audit config --style=json` printed.
    pub fn audit_config_json(&self) -> anyhow::Result<serde_json::Map<String, Value>> {
        anyhow::ensure!(
            self.has_arg("--style=json")
                || self
                    .args
                    .windows(2)
                    .any(|w| w[0] == "--style" && w[1] == "json"),
            "Pass `--style=json` to get JSON"
        );
        let start = self
            .stdout
            .find('{')
            .with_context(|| format!("No JSON on stdout:\n{}", self.stdout))?;
        serde_json::from_str(self.stdout[start..].trim())
            .with_context(|| format!("Parsing the JSON on stdout:\n{}", self.stdout))
    }

    /// The invocation record of a command run with `write_invocation_record` set on the test.
    pub fn invocation_record(&self) -> anyhow::Result<InvocationRecord> {
        let path = self.invocation_record_path.as_ref().context(
            "No invocation record: set `write_invocation_record` on the test to write one",
        )?;
        InvocationRecord::read(path)
    }

    /// The trace id buck2 printed on stderr for this command. It is not the `BUCK_WRAPPER_UUID`
    /// the harness sets: a launcher in front of the client (deployed buck2) replaces it, and a
    /// restarted command gets a new one.
    pub fn buck_build_id(&self) -> anyhow::Result<String> {
        parse_trace_id(&self.stderr)
            .context("No `Buck UI:` or `Build ID:` line on stderr; the command printed no trace id")
    }
}

/// The line buck2's simple console prints when a command starts. A restarted command prints one
/// per attempt, so the last match is the trace id of the attempt that produced the result.
fn parse_trace_id(stderr: &str) -> Option<String> {
    static TRACE_ID_LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?m)^(?:\[[^\]]*\] )?(?:Buck UI: https://www\.internalfb\.com/buck2/|Build ID: )([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\s*$",
        )
        .unwrap()
    });
    TRACE_ID_LINE
        .captures_iter(stderr)
        .last()
        .map(|c| c[1].to_owned())
}

#[track_caller]
fn assert_matches(pattern: &str, regex: &str, stream: &str, text: &str) {
    let compiled = Regex::new(pattern).unwrap_or_else(|e| panic!("Invalid regex `{regex}`: {e}"));
    // Python's `$` also matches before a final newline.
    let matches = compiled.is_match(text)
        || text
            .strip_suffix('\n')
            .is_some_and(|text| compiled.is_match(text));
    assert!(
        matches,
        "Did not find pattern `{regex}` in {stream}:\n{}",
        indent(text),
    );
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("        {line}\n"))
        .collect()
}

/// A buck2 command that exited unsuccessfully. It derefs to its [`BuckOutput`].
pub struct BuckError(pub(crate) Box<BuckOutput>);

impl BuckError {
    pub fn into_output(self) -> BuckOutput {
        *self.0
    }
}

impl Deref for BuckError {
    type Target = BuckOutput;

    fn deref(&self) -> &BuckOutput {
        &self.0
    }
}

impl fmt::Display for BuckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        render(f, "buck2 failed", &self.0)
    }
}

impl fmt::Debug for BuckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for BuckError {}

fn render(f: &mut fmt::Formatter<'_>, headline: &str, output: &BuckOutput) -> fmt::Result {
    let command = output
        .command
        .iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(f, "{headline} ({})", output.status)?;
    writeln!(f, "<cmd> {command}")?;
    writeln!(f, "<working_dir> {}", output.working_dir.display())?;
    if !output.stdout.is_empty() {
        write!(f, "<stdout>\n{}</stdout>\n", indent(&output.stdout))?;
    }
    write!(f, "<stderr>\n{}</stderr>", indent(&output.stderr))
}

/// Asserting that a buck2 command failed.
pub trait ExpectFailure {
    /// The output of the failed command. Panics if the command succeeded.
    fn expect_failure(self) -> BuckOutput;
}

impl ExpectFailure for Result<BuckOutput, BuckError> {
    #[track_caller]
    fn expect_failure(self) -> BuckOutput {
        match self {
            Ok(output) => panic!("{}", Unexpected(&output)),
            Err(error) => error.into_output(),
        }
    }
}

struct Unexpected<'a>(&'a BuckOutput);

impl fmt::Display for Unexpected<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        render(f, "buck2 was expected to fail but succeeded", self.0)
    }
}

/// A build report, as `build --build-report` writes it.
#[derive(Debug, Clone)]
pub struct BuildReport {
    json: Value,
    root: PathBuf,
}

impl BuildReport {
    fn new(json: Value) -> anyhow::Result<Self> {
        let root = PathBuf::from(
            json["project_root"]
                .as_str()
                .context("No `project_root` in the build report")?,
        );
        anyhow::ensure!(
            json["results"].is_object(),
            "No `results` in the build report"
        );
        Ok(Self { json, root })
    }

    /// The whole report.
    pub fn json(&self) -> &Value {
        &self.json
    }

    /// The project root the report's paths are relative to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The report's results, by target.
    pub fn results(&self) -> &serde_json::Map<String, Value> {
        self.json["results"].as_object().unwrap()
    }

    /// The outputs of `target`'s `sub_target` (`DEFAULT` for the default outputs), relative to the
    /// project root. A `target` without a cell (`//foo:bar`) matches the result in any cell.
    pub fn outputs_for_target_rel(
        &self,
        target: &str,
        sub_target: &str,
    ) -> anyhow::Result<Vec<PathBuf>> {
        anyhow::ensure!(target.contains("//"), "`{target}` is not a target label");
        let entry = if target.starts_with("//") {
            let matched: Vec<_> = self
                .results()
                .iter()
                .filter(|(t, _)| t.ends_with(target))
                .map(|(_, entry)| entry)
                .collect();
            match matched.as_slice() {
                [entry] => *entry,
                [] => anyhow::bail!(
                    "No result for `{target}` in the build report:\n{}",
                    self.json
                ),
                _ => anyhow::bail!(
                    "Results for `{target}` in several cells in the build report:\n{}",
                    self.json
                ),
            }
        } else {
            self.results()
                .get(target)
                .with_context(|| format!("No result for `{target}` in the build report"))?
        };
        let outputs = entry["outputs"][sub_target].as_array().with_context(|| {
            format!("No `{sub_target}` outputs for `{target}` in the build report:\n{entry}")
        })?;
        outputs
            .iter()
            .map(|p| {
                Ok(PathBuf::from(
                    p.as_str().context("A non-string output path")?,
                ))
            })
            .collect()
    }

    /// The absolute paths of the outputs of `target`'s `sub_target`. See
    /// [`outputs_for_target_rel`](Self::outputs_for_target_rel).
    pub fn outputs_for_target(
        &self,
        target: &str,
        sub_target: &str,
    ) -> anyhow::Result<Vec<PathBuf>> {
        Ok(self
            .outputs_for_target_rel(target, sub_target)?
            .into_iter()
            // With the platform's separators, as Python's `pathlib` gives them.
            .map(|p| self.root.join(p.components().collect::<PathBuf>()))
            .collect())
    }

    /// The absolute path of the one default output of `target`.
    pub fn output_for_target(&self, target: &str) -> anyhow::Result<PathBuf> {
        self.output_for_sub_target(target, "DEFAULT")
    }

    /// The absolute path of the one output of `target`'s `sub_target`.
    pub fn output_for_sub_target(&self, target: &str, sub_target: &str) -> anyhow::Result<PathBuf> {
        let outputs = self.outputs_for_target(target, sub_target)?;
        match <[PathBuf; 1]>::try_from(outputs) {
            Ok([output]) => Ok(output),
            Err(outputs) => {
                anyhow::bail!(
                    "`{target}[{sub_target}]` has {} outputs: {outputs:?}",
                    outputs.len()
                )
            }
        }
    }
}

/// The invocation record of one command. It derefs to the record's JSON object, with the command's
/// `trace_id` added.
#[derive(Debug, Clone)]
pub struct InvocationRecord(Value);

impl InvocationRecord {
    /// Reads the invocation record at `path`.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Reading the invocation record `{}`", path.display()))?;
        Self::parse(&text)
            .with_context(|| format!("Parsing the invocation record `{}`", path.display()))
    }

    fn parse(text: &str) -> anyhow::Result<Self> {
        let mut json: Value = serde_json::from_str(text)?;
        let trace_id = json["trace_id"].take();
        let mut record = json["data"]["Record"]["data"]["InvocationRecord"].take();
        let object = record
            .as_object_mut()
            .context("No `data.Record.data.InvocationRecord` object")?;
        object.insert("trace_id".to_owned(), trace_id);
        Ok(Self(record))
    }

    /// The one error the record holds. Panics unless there is exactly one.
    #[track_caller]
    pub fn single_error(&self) -> &Value {
        match self.0["errors"].as_array().map(Vec::as_slice) {
            Some([error]) => error,
            errors => panic!("Expected exactly one error in the invocation record, got {errors:?}"),
        }
    }
}

impl Deref for InvocationRecord {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[cfg(unix)]
    fn status(code: i32) -> ExitStatus {
        std::os::unix::process::ExitStatusExt::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn status(code: i32) -> ExitStatus {
        std::os::windows::process::ExitStatusExt::from_raw(code as u32)
    }

    fn output(stdout: &str, args: &[&str], code: i32) -> BuckOutput {
        BuckOutput {
            stdout: stdout.to_owned(),
            stderr: "some stderr\n".to_owned(),
            status: status(code),
            command: vec!["buck2".into(), "build".into()],
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            working_dir: PathBuf::from("/project"),
            invocation_record_path: None,
        }
    }

    fn report() -> BuildReport {
        BuildReport::new(json!({
            "project_root": "/project",
            "results": {
                "root//:one": {"outputs": {"DEFAULT": ["buck-out/one"], "sub": ["buck-out/a", "buck-out/b"]}},
                "root//other:one": {"outputs": {"DEFAULT": ["buck-out/other"]}},
                "cell//:two": {"outputs": {"DEFAULT": []}},
            },
        }))
        .unwrap()
    }

    #[test]
    fn test_a_cellless_target_matches_any_cell() {
        assert_eq!(
            report().output_for_target("//other:one").unwrap(),
            PathBuf::from("/project/buck-out/other")
        );
    }

    #[test]
    fn test_a_cellless_target_matching_no_result_is_an_error() {
        assert!(report().output_for_target("//:three").is_err());
    }

    #[test]
    fn test_outputs_of_a_sub_target_are_relative_to_the_root() {
        assert_eq!(
            report()
                .outputs_for_target_rel("root//:one", "sub")
                .unwrap(),
            vec![PathBuf::from("buck-out/a"), PathBuf::from("buck-out/b")]
        );
    }

    #[test]
    fn test_a_target_without_one_output_is_an_error() {
        assert!(report().output_for_target("cell//:two").is_err());
    }

    #[test]
    fn test_a_sub_target_without_one_output_is_an_error() {
        let error = report()
            .output_for_sub_target("root//:one", "sub")
            .unwrap_err();
        assert!(error.to_string().contains("has 2 outputs"), "{error}");
    }

    #[test]
    fn test_the_build_report_is_the_first_json_line() {
        let out = output(
            "{\"project_root\": \"/p\", \"results\": {}}\nroot//:x buck-out/x\n",
            &["--build-report=-"],
            0,
        );
        assert_eq!(out.build_report().unwrap().root(), Path::new("/p"));
    }

    #[test]
    fn test_show_output_skips_the_build_report() {
        let out = output(
            "{\"project_root\": \"/p\", \"results\": {}}\nroot//:x buck-out/x\nroot//:y\n",
            &["//:x", "--show-output", "--build-report=-"],
            0,
        );
        assert_eq!(
            out.target_to_build_output().unwrap(),
            BTreeMap::from([
                ("root//:x".to_owned(), "buck-out/x".to_owned()),
                ("root//:y".to_owned(), String::new()),
            ])
        );
    }

    #[test]
    fn test_show_output_needs_the_flag() {
        assert!(output("", &[], 0).target_to_build_output().is_err());
    }

    #[test]
    fn test_target_list_is_sorted() {
        let out = output(
            "{\"buck.package\": \"root//b\", \"name\": \"x\"}\n{\"buck.package\": \"root//a\", \"name\": \"y\"}\n",
            &["--json-lines"],
            0,
        );
        assert_eq!(out.target_list().unwrap(), ["root//a:y", "root//b:x"]);
    }

    #[test]
    fn test_the_last_trace_id_wins() {
        let mut out = output("", &[], 0);
        out.stderr = "Build ID: 11111111-2222-3333-4444-555555555555\nretrying\n[2024] Build ID: aaaaaaaa-2222-3333-4444-555555555555\n".to_owned();
        assert_eq!(
            out.buck_build_id().unwrap(),
            "aaaaaaaa-2222-3333-4444-555555555555"
        );
    }

    #[test]
    fn test_an_invocation_record_carries_its_trace_id() {
        let record = InvocationRecord::parse(
            r#"{"trace_id": "t", "data": {"Record": {"data": {"InvocationRecord": {"errors": [{"category": "USER"}]}}}}}"#,
        )
        .unwrap();
        assert_eq!(record["trace_id"], "t");
        assert_eq!(record.single_error()["category"], "USER");
    }

    #[test]
    fn test_stderr_matching_ignores_case_and_spans_lines() {
        let mut out = output("", &[], 3);
        out.stderr = "First line\nSECOND line\n".to_owned();
        out.assert_stderr_matches("first.*second");
    }

    #[test]
    #[should_panic(expected = "Did not find pattern `missing` in stderr")]
    fn test_a_stderr_mismatch_panics() {
        output("", &[], 3).assert_stderr_matches("missing");
    }

    #[test]
    fn test_dollar_matches_before_a_final_newline() {
        let mut out = output("", &[], 3);
        out.stderr = "Error: foo bar\n".to_owned();
        out.assert_stderr_matches("foo bar$");
    }

    #[cfg(unix)]
    #[test]
    fn test_a_signal_exits_with_128_plus_the_signal() {
        let mut out = output("", &[], 0);
        out.status = std::os::unix::process::ExitStatusExt::from_raw(2);
        assert_eq!(out.exit_code(), Some(130));
    }

    #[test]
    fn test_exit_codes_compare_by_value() {
        output("", &[], 3).assert_exit_code(ExitCode::UserError);
    }

    #[test]
    #[should_panic(expected = "buck2 was expected to fail but succeeded")]
    fn test_expecting_failure_of_a_success_panics() {
        Ok::<_, BuckError>(output("", &[], 0)).expect_failure();
    }

    #[test]
    fn test_a_failure_renders_the_command_and_its_output() {
        let error = BuckError(Box::new(output("out\n", &[], 3)));
        let rendered = error.to_string();
        assert!(rendered.contains("<cmd> buck2 build"), "{rendered}");
        assert!(rendered.contains("<working_dir> /project"), "{rendered}");
        assert!(
            rendered.contains("<stdout>\n        out\n</stdout>"),
            "{rendered}"
        );
        assert!(
            rendered.contains("<stderr>\n        some stderr\n</stderr>"),
            "{rendered}"
        );
    }
}
