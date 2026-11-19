/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! What TestX recorded for the test runs of `buck2 test` under tpx.

use std::sync::LazyLock;

use anyhow::Context;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

/// The id of the test run, from the `Test session: .../<id>` line a `buck2 test` prints on stderr.
pub fn extract_test_run_id(stderr: &str) -> anyhow::Result<u64> {
    static TEST_SESSION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^.*Test session:.*/(\d+).*$").unwrap());
    let session = TEST_SESSION
        .captures(stderr)
        .with_context(|| format!("Test run ID not found in {stderr}"))?;
    session[1]
        .parse()
        .with_context(|| format!("`{}` is not a test run ID", &session[1]))
}

/// What TestX recorded for one test of a test run.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TestArtifacts {
    #[serde(default)]
    pub test_id: i64,
    /// The test's artifacts, as TestX prints `TestResultArtifact`s: each has a `name`, a `handle`
    /// (an `everstore_handle` or a `cas_digest`) and `annotations`.
    #[serde(default)]
    pub artifacts: Vec<Value>,
}

/// The artifacts of the test run `run_id`, by test, as `testx --as-json artifacts list` prints
/// them. The test target gives the TestX executable in `TESTX_BIN`.
pub async fn artifacts_list(run_id: u64) -> anyhow::Result<Vec<TestArtifacts>> {
    let testx = std::env::var_os("TESTX_BIN")
        .context("`TESTX_BIN` is not set; the test target must set it to the TestX executable")?;
    let output = tokio::process::Command::new(&testx)
        .args(["--caller", "buck-e2e", "--as-json", "artifacts", "list"])
        .arg(run_id.to_string())
        .output()
        .await
        .with_context(|| format!("Running `{}`", testx.to_string_lossy()))?;
    anyhow::ensure!(
        output.status.success(),
        "`testx artifacts list {run_id}` failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    parse_artifacts_list(&String::from_utf8_lossy(&output.stdout))
}

fn parse_artifacts_list(stdout: &str) -> anyhow::Result<Vec<TestArtifacts>> {
    serde_json::from_str(stdout)
        .with_context(|| format!("Parsing the artifacts TestX listed:\n{stdout}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_the_run_id_comes_from_the_test_session_line() {
        let stderr = "[2026-11-03T09:23:36.591-08:00] RE Session: reSessionID-8e06\n\
            [2026-11-03T09:23:37.055-08:00] Test session: https://www.internalfb.com/intern/testinfra/testrun/6473924860949146\n\
            Tests finished: Pass 1.\n";
        assert_eq!(extract_test_run_id(stderr).unwrap(), 6473924860949146);
    }

    #[test]
    fn test_no_test_session_line_is_an_error() {
        assert!(extract_test_run_id("Tests finished: Pass 1.\n").is_err());
    }

    #[test]
    fn test_the_artifacts_list_is_a_list_of_tests() {
        let artifact =
            json!({"name": "a", "handle": {"cas_digest": {"hash": "h", "size_in_bytes": 1}}});
        assert_eq!(
            parse_artifacts_list(&json!([{"test_id": 1, "artifacts": [artifact]}]).to_string())
                .unwrap(),
            vec![TestArtifacts {
                test_id: 1,
                artifacts: vec![artifact],
            }]
        );
        assert!(parse_artifacts_list(r#"{"test_id": 1}"#).is_err());
        assert!(parse_artifacts_list("not json").is_err());
    }

    #[test]
    fn test_a_test_without_artifacts_has_none() {
        assert_eq!(
            parse_artifacts_list(r#"[{"test_id": 2}]"#).unwrap(),
            vec![TestArtifacts {
                test_id: 2,
                artifacts: Vec::new(),
            }]
        );
    }
}
