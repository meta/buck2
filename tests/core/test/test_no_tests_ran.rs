/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::BuckOutput;
use buck2_e2e::ansi::remove_ansi_escape_sequences;
use buck2_e2e::prelude::*;

// The simple console's counters and warning are stable; progress, timing and
// invocation links are not. Keep the full streams in the test output too.
fn console_result(output: &BuckOutput) -> String {
    let stderr = remove_ansi_escape_sequences(&output.stderr);
    let lines = stderr
        .lines()
        .filter_map(|line| {
            line.find("Tests finished:")
                .or_else(|| line.find("NO TESTS RAN"))
                .map(|start| &line[start..])
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "exit code: {}\n{lines}",
        output.exit_code().expect("command exited")
    )
}

#[buck_test(meta_only = "tpx")]
async fn test_console_when_no_tests_ran(buck: &Buck) -> anyhow::Result<()> {
    let cases: [(&str, &[&str], bool); 6] = [
        ("all_omitted", &["//:omitted"], false),
        ("no_tests", &["//:no_tests"], false),
        ("list_only", &["//:passes", "--", "--list-only"], false),
        ("passed", &["//:passes"], true),
        ("mixed", &["//:passes", "//:omitted"], true),
        ("skipped", &["//:skips"], true),
    ];
    for (name, args, executed) in cases {
        let sentinel = buck.cwd().join(format!("{name}.executed"));
        let config = format!("test.body_sentinel={}", sentinel.display());
        let mut args = args.to_vec();
        let separator = args
            .iter()
            .position(|arg| *arg == "--")
            .unwrap_or(args.len());
        args.splice(separator..separator, ["--config", &config]);
        if !args.contains(&"--") {
            args.push("--");
        }
        args.push("--no-record-results");
        let output = buck.test(&args).await?;
        println!(
            "{}",
            serde_json::json!({
                "case": name,
                "client": buck.executable(),
                "project": buck.cwd(),
                "executor": buck.env_var("BUCK2_TPX").map(|value| value.to_string_lossy()),
                "external_config": buck.env_var("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG").map(|value| value.to_string_lossy()),
                "caller_arguments": args,
                "buck_trace_id": output.buck_build_id()?,
                "exit_code": output.exit_code(),
                "stdout": output.stdout,
                "stderr": output.stderr,
            })
        );
        assert!(output.stdout.is_empty(), "{name}: unexpected stdout");
        assert_eq!(
            sentinel.exists(),
            executed,
            "{name}: the fixture body must only run for executed cases"
        );
        golden(&console_result(&output), &format!("fixtures/{name}.golden"));
    }
    Ok(())
}
