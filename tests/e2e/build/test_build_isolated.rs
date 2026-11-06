/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use regex::Regex;
use serde_json::Value;

fn truncate(x: &str, limit: usize) -> String {
    let chars: Vec<char> = x.chars().collect();
    if chars.len() <= limit {
        x.to_owned()
    } else {
        let head: String = chars[..limit / 2].iter().collect();
        let tail: String = chars[chars.len() - limit / 2..].iter().collect();
        format!("{head} <<TRUNCATED>> {tail}")
    }
}

#[track_caller]
fn print_occurrences_msg(needle: &str, haystack: &str, occurrences: usize, success: bool) {
    const OUTPUT_LIMIT: usize = 10000;
    // Hacky way to actually make sure we print the full output when a string
    // does not appear the correct number of times.
    assert!(
        success,
        "Expected to find {occurrences} occurrences of `{needle}` in `{}`",
        truncate(&format!("{haystack:?}"), OUTPUT_LIMIT)
    );
}

#[track_caller]
fn assert_occurrences(needle: &str, haystack: &str, occurrences: usize) {
    print_occurrences_msg(
        needle,
        haystack,
        occurrences,
        haystack.matches(needle).count() == occurrences,
    );
}

#[track_caller]
fn assert_occurrences_regex(needle: &str, haystack: &str, occurrences: usize) {
    let regex = Regex::new(&format!("(?m){needle}"))
        .unwrap_or_else(|e| panic!("Invalid regex `{needle}`: {e}"));
    print_occurrences_msg(
        needle,
        haystack,
        occurrences,
        regex.find_iter(haystack).count() == occurrences,
    );
}

/// Reads `path` as Python's `Path.read_text` does, with universal newlines.
fn read_text(path: &Path) -> anyhow::Result<String> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("Reading `{}`", path.display()))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

// ##########################################################################################
// ## NOTE(JakobDegen): Do not add new tests to this file. Instead:
// ##  1. Use or make a file in `tests/core` with a name that explains what you're testing.
// ##  2. Give that file its own data directory, not one shared with other tests.
// ##########################################################################################

#[buck_test(inplace = false, data_dir = "pass")]
async fn test_pass(buck: &Buck) -> anyhow::Result<()> {
    let results = buck.build(&["//:abc"]).await?;
    assert!(!results.stderr.contains("does not have any outputs"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "pass")]
async fn test_missing_target(buck: &Buck) {
    buck.build(&["//:not_a_target_name"]).await.expect_failure();
}

#[buck_test(inplace = false, data_dir = "pass")]
async fn test_success_message_printed(buck: &Buck) -> anyhow::Result<()> {
    let results = buck.build(&["//:abc", "--console=simplenotty"]).await?;

    assert!(results.stderr.contains("BUILD SUCCEEDED"));

    let results = buck.build(&["//:abc", "--console=simpletty"]).await?;

    assert_occurrences("\x1b[38;5;10mBUILD SUCCEEDED\x1b[39m", &results.stderr, 1);

    let results = buck.build(&["//:abc", "--console=super"]).await?;

    assert_occurrences("\x1b[38;5;10mBUILD SUCCEEDED\x1b[39m", &results.stderr, 1);
    Ok(())
}

#[buck_test(inplace = false, data_dir = "pass")]
async fn test_success_message_suppressed_at_v0(buck: &Buck) -> anyhow::Result<()> {
    let results = buck
        .build(&["//:abc", "-v0", "--console=simplenotty"])
        .await?;

    assert!(!results.stderr.contains("BUILD SUCCEEDED"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "failing")]
async fn test_multiple_errors_print_with_simple_console(buck: &Buck) {
    let e = buck
        .build(&["--console=simple", "//:foo", "//:bar", "//:using_dir"])
        .await
        .expect_failure();

    // Make sure that streamed events come back
    assert_occurrences_regex("(Build ID|Buck UI):", &e.stderr, 1);
    assert_occurrences("RE Session: ", &e.stderr, 1);
    assert_occurrences_regex("^BUILD FAILED", &e.stderr, 1);

    let execution_error =
        |target: &str| format!("Action failed: {target} (<unspecified>) (bin_false)");
    assert_occurrences(&execution_error("root//:foo"), &e.stderr, 2);
    assert_occurrences(&execution_error("root//:bar"), &e.stderr, 2);

    // TODO: Windows handle wrong binary path as internal error, maybe we should fix it.
    if !cfg!(windows) {
        let exit_code = "(Local|Remote) command returned non-zero exit code 1";
        assert_occurrences_regex(exit_code, &e.stderr, 6);
    }

    let build_error = |target: &str| format!("Failed to build '{target} (<unspecified>)'");
    assert_occurrences(&build_error("root//:foo"), &e.stderr, 1);
    assert_occurrences(&build_error("root//:bar"), &e.stderr, 1);
    // TODO(nmj): Remove this comment
    // assert_occurrences_regex("getting metadata for.*a_dir`", e.stderr, 1)

    let e = buck
        .build(&["--console=simple", "//:non_existent"])
        .await
        .expect_failure();

    assert_occurrences_regex("^BUILD FAILED", &e.stderr, 1);
    assert_occurrences(
        "Unknown target `non_existent` from package `root//`",
        &e.stderr,
        1,
    );
}

#[buck_test(inplace = false, data_dir = "failing")]
async fn test_multiple_errors_print_with_super_console(buck: &Buck) {
    let e = buck
        .build(&["--console=super", "//:foo", "//:bar", "//:using_dir"])
        .await
        .expect_failure();

    // See https://en.wikipedia.org/wiki/ANSI_escape_code#8-bit for how to decode this
    let dark_red = regex::escape("\x1b[38;5;1m");

    let execution_error = "Action failed: ";
    assert_occurrences_regex(execution_error, &e.stderr, 3);

    // TODO: Windows handle wrong binary path as internal error, maybe we should fix it.
    if !cfg!(windows) {
        let exit_code = "(Local|Remote) command returned non-zero exit code 1";
        assert_occurrences_regex(exit_code, &e.stderr, 3);
    }

    // These will eventually be red.
    let build_error = |target: &str| format!("Failed to build '{target} (<unspecified>)'");
    assert_occurrences(&build_error("root//:foo"), &e.stderr, 1);
    assert_occurrences(&build_error("root//:bar"), &e.stderr, 1);
    // TODO(nmj): Remove this comment
    // assert_occurrences_regex("getting metadata for.*a_dir`", e.stderr, 1)

    assert_occurrences("\x1b[38;5;1mBUILD FAILED\x1b[39m", &e.stderr, 1);

    let e = buck
        .build(&["--console=super", "//:non_existent"])
        .await
        .expect_failure();

    let target_error = format!("{dark_red}Unknown target `non_existent` from package `root//`");
    assert_occurrences("\x1b[38;5;1mBUILD FAILED\x1b[39m", &e.stderr, 1);
    assert_occurrences_regex(&target_error, &e.stderr, 1);
}

#[buck_test(inplace = false, data_dir = "pass")]
async fn test_stderr_is_printed_for_successful_actions(buck: &Buck) -> anyhow::Result<()> {
    let no_color_text = "warning on stderr no color";
    // Support '\r\n' which is printed on Windows.
    let simple_color_stripped = r"\] warning on stderr with color\r?$";
    // Make sure we reset the terminal after printing.
    let simple_color = "\x1b[33mwarning on stderr with color\x1b[0m";
    // Make sure we reset the terminal after printing. Color is represented
    // slightly differently (33m vs 38;5;3m) because of how parsed / sanitized
    // colors are stored. Those are the same color, though.
    let super_color = "\x1b[38;5;3mwarning on stderr with color\x1b[39m";

    let superconsole_stderr_line =
        |target: &str| format!("stderr for {target} (printer writing_stderr)");
    let simpleconsole_stderr_line = "stderr:";

    // By default stderr should not be relayed on success w/o the -v2 or higher flag.

    let res = buck
        .build(&["--console=simplenotty", "//:print_stderr_simple_notty"])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 0);
    assert_occurrences_regex(simple_color_stripped, &res.stderr, 0);
    assert_occurrences(simpleconsole_stderr_line, &res.stderr, 0);

    let res = buck
        .build(&["--console=simpletty", "//:print_stderr_simple_tty"])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 0);
    assert_occurrences(simple_color, &res.stderr, 0);
    assert_occurrences(simpleconsole_stderr_line, &res.stderr, 0);

    let res = buck
        .build(&["--console=super", "//:print_stderr_super"])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 0);
    assert_occurrences(super_color, &res.stderr, 0);
    assert_occurrences(
        &superconsole_stderr_line("root//:print_stderr_super"),
        &res.stderr,
        0,
    );

    let res = buck
        .build(&[
            "-v5",
            "--console=simplenotty",
            "//:v_print_stderr_simple_notty",
        ])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences_regex(simple_color_stripped, &res.stderr, 1);

    let res = buck
        .build(&["-v5", "--console=simpletty", "//:v_print_stderr_simple_tty"])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences(simple_color, &res.stderr, 1);
    assert_occurrences(simpleconsole_stderr_line, &res.stderr, 1);

    let res = buck
        .build(&["-v5", "--console=super", "//:v_print_stderr_super"])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences(super_color, &res.stderr, 1);
    assert_occurrences(
        &superconsole_stderr_line("root//:v_print_stderr_super"),
        &res.stderr,
        1,
    );

    // Things that print all the time should print w/o the verbosity flag, or with it

    let res = buck
        .build(&[
            "--console=simplenotty",
            "//:always_print_stderr_simple_notty",
        ])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences_regex(simple_color_stripped, &res.stderr, 1);
    assert_occurrences(simpleconsole_stderr_line, &res.stderr, 1);

    let res = buck
        .build(&["--console=simpletty", "//:always_print_stderr_simple_tty"])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences(simple_color, &res.stderr, 1);
    assert_occurrences(simpleconsole_stderr_line, &res.stderr, 1);

    let res = buck
        .build(&["--console=super", "//:always_print_stderr_super"])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences(super_color, &res.stderr, 1);
    assert_occurrences(
        &superconsole_stderr_line("root//:always_print_stderr_super"),
        &res.stderr,
        1,
    );

    let res = buck
        .build(&[
            "-v5",
            "--console=simplenotty",
            "//:v_always_print_stderr_simple_notty",
        ])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences_regex(simple_color_stripped, &res.stderr, 1);
    assert_occurrences(simpleconsole_stderr_line, &res.stderr, 1);

    let res = buck
        .build(&[
            "-v5",
            "--console=simpletty",
            "//:v_always_print_stderr_simple_tty",
        ])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences(simple_color, &res.stderr, 1);
    assert_occurrences(simpleconsole_stderr_line, &res.stderr, 1);

    let res = buck
        .build(&["-v5", "--console=super", "//:v_always_print_stderr_super"])
        .await?;

    assert_occurrences(no_color_text, &res.stderr, 1);
    assert_occurrences(super_color, &res.stderr, 1);
    assert_occurrences(
        &superconsole_stderr_line("root//:v_always_print_stderr_super"),
        &res.stderr,
        1,
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "flagfiles")]
async fn test_flagfiles_are_located_correctly(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .build(&["@//mode/dev", "cell//subdir:simple"])
        .rel_cwd("cell")
        .await?;

    let build_report = out.build_report()?;
    let output = build_report.output_for_target("cell//subdir:simple")?;
    assert_eq!(read_text(&output)?.trim_end(), "overridden");

    let out = buck
        .build(&["@//mode/dev", "cell//subdir:simple"])
        .rel_cwd("cell")
        .await?;

    let build_report = out.build_report()?;
    let output = build_report.output_for_target("cell//subdir:simple")?;
    assert_eq!(read_text(&output)?.trim_end(), "overridden");

    // Make sure that relative paths are resolved against the cell root
    // (determined from project root + cwd) if they're not found relative
    // to cwd
    let out = buck
        .build(&["@mode/dev", "cell//subdir:simple"])
        .rel_cwd(Path::new("cell").join("subdir"))
        .await?;

    let build_report = out.build_report()?;
    let output = build_report.output_for_target("cell//subdir:simple")?;
    assert_eq!(read_text(&output)?.trim_end(), "overridden");
    assert!(
        out.stderr
            .contains("`@mode/dev` was specified, but not found. Using file at `//mode/dev`.")
    );

    let out = buck
        .build(&["@cell//mode/dev", "cell//subdir:simple"])
        .await?;

    let build_report = out.build_report()?;
    let output = build_report.output_for_target("cell//subdir:simple")?;
    assert_eq!(read_text(&output)?.trim_end(), "overridden");

    buck.build(&["@cell/mode/missing", "cell//subdir:simple"])
        .await
        .expect_failure()
        .assert_stderr_matches("Unable to read flag file at `cell/mode/missing`");
    Ok(())
}

#[buck_test(inplace = false, data_dir = "early_action_cutoff")]
async fn test_early_action_cutoff(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let sentinel = tmp_path.join("sentinel");
    std::fs::File::create(&sentinel)?;

    // This action is going to check that the file pointed at by "sentinel"
    // exists. Point it at a valid file.

    std::fs::write(
        buck.cwd().join("sentinel"),
        sentinel.to_string_lossy().as_bytes(),
    )?;

    buck.build(&["//:check"]).await?;

    // Now, invalidate the action, and remove the underlying sentinel file. If
    // :check executes now, it'll fail.

    std::fs::write(buck.cwd().join("src"), "TEXT2")?;

    std::fs::remove_file(&sentinel)?;

    // Run it to find out

    buck.build(&["//:check"]).await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "toolchain_deps")]
async fn test_toolchain_deps(buck: &Buck) -> anyhow::Result<()> {
    // This test builds two targets, both with the same `default_target_platform` platform
    // but which should resolve to different execution platforms because of toolchain deps.
    // Both targets still get configured with the `default_target_platform` of release.
    //
    // The Python toolchain works on Windows/Linux, but we prefer Linux as an exec platform.
    // The ASIC toolchain only works on Windows, so `python_and_asic` (which does both) must
    // pick Windows for Python as well.
    let result = buck
        .build(&["root//tests:python_and_asic", "root//tests:python_only"])
        .await?;
    let python_and_asic = read_text(
        &result
            .build_report()?
            .output_for_target("root//tests:python_and_asic")?,
    )?;
    let python_only = read_text(
        &result
            .build_report()?
            .output_for_target("root//tests:python_only")?,
    )?;

    // If any of the selects get resolved incorrectly, the toolchain binaries below will change.
    assert_eq!(python_and_asic, "python_release_windows\nasic\n");
    assert_eq!(python_only, "python_release_linux\n");

    // Test foo_binary: a target that consumes a toolchain exposing multiple exec_deps
    // (compiler + linter). This validates the pattern documented in
    // docs/rule_authors/writing_toolchains.md "Exposing execution dependencies".
    let foo_result = buck.build(&["root//tests:foo_hello"]).await?;
    let foo_output = read_text(
        &foo_result
            .build_report()?
            .output_for_target("root//tests:foo_hello")?,
    )?;
    assert!(foo_output.contains("compiled[-O2 -Wall]:"));
    assert!(foo_output.contains("hello world"));

    buck.build(&[
        "root//...",
        "--target-platforms=root//config:platform_windows",
    ])
    .await?;

    // Check we get the toolchain dependencies in uquery and cquery
    let result = buck.uquery(&["deps(//toolchains:python)"]).await?;
    assert!(
        result
            .stdout
            .contains("//toolchains:compile_python_release_linux\n")
    );
    assert!(result.stdout.contains("//toolchains:python_debug\n"));
    let result = buck
        .cquery(&[
            "deps(//toolchains:python)",
            "--target-platforms=root//config:platform_linux",
        ])
        .await?;
    assert!(
        result
            .stdout
            .contains("//toolchains:compile_python_release_linux ")
    );
    assert!(!result.stdout.contains("//toolchains:python_debug "));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "no_output")]
async fn test_no_output(buck: &Buck) -> anyhow::Result<()> {
    let results = buck.build(&["//:none"]).await?;

    assert!(results.stderr.contains("BUILD SUCCEEDED"));
    assert!(results.stderr.contains("does not have any outputs"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "no_output")]
async fn test_no_output_wildcard(buck: &Buck) -> anyhow::Result<()> {
    let results = buck.build(&["//..."]).await?;

    assert!(results.stderr.contains("BUILD SUCCEEDED"));
    assert!(!results.stderr.contains("does not have any outputs"));
    Ok(())
}

async fn read_executors(buck: &Buck) -> anyhow::Result<Vec<String>> {
    read_what_ran(buck, &[])
        .await?
        .iter()
        .map(|line| {
            line.get("reproducer")
                .and_then(|r| r.get("executor"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .with_context(|| format!("No `reproducer.executor` in {line}"))
        })
        .collect()
}

#[buck_test(inplace = false, data_dir = "executor_caching")]
async fn test_executor_caching_disabled(buck: &Buck) -> anyhow::Result<()> {
    let seed = random_string();
    // Run on RE with cache lookup and writes disabled
    buck.build(&[
        ":test",
        "-c",
        &format!("test.seed={seed}"),
        "-c",
        "test.remote_enabled=true",
        "--no-remote-cache",
    ])
    .await?;
    assert_eq!(read_executors(buck).await?, ["Re"]);

    buck.kill().await?;

    // Run on RE, should not get any cache hit as the cache writes were disabled
    buck.build(&[
        ":test",
        "-c",
        &format!("test.seed={seed}"),
        "-c",
        "test.remote_enabled=true",
    ])
    .await?;

    let executors = read_executors(buck).await?;
    assert!(executors == ["Re"] || executors.is_empty(), "{executors:?}");
    Ok(())
}

#[buck_test(inplace = false, data_dir = "executor_caching")]
async fn test_executor_cache_writes_enabled(buck: &Buck) -> anyhow::Result<()> {
    let seed = random_string();
    // Run on RE with cache lookup disabled and cache writes enabled
    buck.build(&[
        ":test",
        "-c",
        &format!("test.seed={seed}"),
        "-c",
        "test.remote_enabled=true",
        "--no-remote-cache",
        "--write-to-cache-anyway",
    ])
    .await?;
    assert_eq!(read_executors(buck).await?, ["Re"]);

    buck.kill().await?;

    // Run on RE, should not get cache hits as writes were enabled
    buck.build(&[
        ":test",
        "-c",
        &format!("test.seed={seed}"),
        "-c",
        "test.remote_enabled=true",
    ])
    .await?;
    assert_eq!(read_executors(buck).await?, ["Cache"]);
    Ok(())
}

#[buck_test(inplace = false, data_dir = "executor_caching")]
async fn test_executor_caching(buck: &Buck) -> anyhow::Result<()> {
    let seed = random_string();
    let seed = format!("test.seed={seed}");

    // Run on RE
    buck.build(&[":test", "-c", &seed, "-c", "test.remote_enabled=true"])
        .await?;
    assert_eq!(read_executors(buck).await?, ["Re"]);

    // Run on RE, with caching (the default)
    buck.kill().await?;
    buck.build(&[":test", "-c", &seed, "-c", "test.remote_enabled=true"])
        .await?;
    assert_eq!(read_executors(buck).await?, ["Cache"]);

    // Kill, run locally, no caching.
    buck.kill().await?;
    buck.build(&[
        ":test",
        "-c",
        &seed,
        "-c",
        "test.local_enabled=true",
        "-c",
        "test.remote_enabled=false",
        "-c",
        "test.remote_cache_enabled=false",
    ])
    .await?;
    assert_eq!(read_executors(buck).await?, ["Local"]);

    // Kill, run locally, with caching (explicitly).
    buck.kill().await?;
    buck.build(&[
        ":test",
        "-c",
        &seed,
        "-c",
        "test.remote_enabled=false",
        "-c",
        "test.local_enabled=true",
        "-c",
        "test.remote_cache_enabled=true",
    ])
    .await?;
    assert_eq!(read_executors(buck).await?, ["Cache"]);

    // Kill, run locally, with caching (the default).
    buck.kill().await?;
    buck.build(&[
        ":test",
        "-c",
        &seed,
        "-c",
        "test.remote_enabled=false",
        "-c",
        "test.local_enabled=true",
    ])
    .await?;
    assert_eq!(read_executors(buck).await?, ["Cache"]);
    Ok(())
}

#[buck_test(inplace = false, data_dir = "pass")]
async fn test_sandcastle_id_check(buck: &Buck) -> anyhow::Result<()> {
    async fn pid(buck: &Buck) -> anyhow::Result<u64> {
        let res = buck.status(&[]).await?;
        let status: Value = serde_json::from_str(&res.stdout)?;
        status
            .get("process_info")
            .and_then(|i| i.get("pid"))
            .and_then(Value::as_u64)
            .with_context(|| format!("No `process_info.pid` in {status}"))
    }

    buck.build(&[]).await?;
    let pid1 = pid(buck).await?;
    buck.build(&[]).env("SANDCASTLE_ID", "foo").await?;
    let pid2 = pid(buck).await?;
    buck.build(&[]).env("SANDCASTLE_ID", "foo").await?;
    let pid3 = pid(buck).await?;

    assert_ne!(pid1, pid2);
    assert_eq!(pid2, pid3);
    Ok(())
}

#[buck_test(inplace = false, data_dir = "execution_platforms")]
async fn test_enforce_unique_inodes(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//executor_unique_inode_tests/...",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "execution_platforms", skip_for_os = ["windows"])]
async fn test_executable_bit(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//executor_exec_bit_tests/...",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "execution_platforms")]
async fn test_symlink_output(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2_re_client]\nrespect_file_symlinks = false\n",
    )?;
    buck.build(&[
        "root//executor_symlink_tests:check_not_symlink",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    buck.kill().await?;
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2_re_client]\nrespect_file_symlinks = true\n",
    )?;
    buck.build(&[
        "root//executor_symlink_tests:check_symlink",
        "-c",
        &format!("test.cache_buster={}", random_string()),
    ])
    .await?;
    Ok(())
}

// ##########################################################################################
// ## NOTE(JakobDegen): Do not add new tests to this file. Instead:
// ##  1. Use or make a different test file with a name that explains what you're testing.
// ##  2. Give that file its own data directory, not one shared with other tests.
// ##########################################################################################
