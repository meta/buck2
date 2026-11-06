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
use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

const PACKAGE: &str = "fbcode//buck2/tests/targets/rules/worker_grpc";

const WORKER_ARGS: [&str; 3] = [
    "-c",
    "build.require_persistent_workers=True",
    "--no-remote-cache",
];

/// `args` followed by `target`.
fn build_args<'a>(args: &[&'a str], target: &'a str) -> Vec<&'a str> {
    args.iter().copied().chain([target]).collect()
}

async fn read_what_ran_for_executor(buck: &Buck, executor: &str) -> anyhow::Result<Vec<Value>> {
    let mut entries = Vec::new();
    for entry in read_what_ran(buck, &[]).await? {
        let entry_executor = entry
            .get("reproducer")
            .and_then(|r| r.get("executor"))
            .with_context(|| format!("No `reproducer.executor` in {entry}"))?;
        if entry_executor == executor {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// The paths `glob.glob("<dir>/<prefix>*/stderr")` finds.
fn worker_stderrs(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .map(|entry| entry.path().join("stderr"))
        .filter(|stderr| stderr.exists())
        .collect()
}

// disabled on mac due to Python GRPC issues
#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_worker(buck: &Buck) -> anyhow::Result<()> {
    let target = format!("{PACKAGE}:gen_worker_run_out");
    let res = buck.build(&build_args(&WORKER_ARGS, &target)).await?;
    let output = res.build_report()?.output_for_target(&target)?;
    assert_eq!(std::fs::read_to_string(output)?, "hello worker");
    assert_eq!(read_what_ran_for_executor(buck, "Worker").await?.len(), 1);
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_worker_disabled(buck: &Buck) -> anyhow::Result<()> {
    // Check non-worker exe runs if workers are disabled.
    let fallback_args = ["-c", "build.use_persistent_workers=false", "--local-only"];
    let target = format!("{PACKAGE}:gen_worker_run_fallback");
    let res = buck.build(&build_args(&fallback_args, &target)).await?;
    let output = res.build_report()?.output_for_target(&target)?;
    assert!(std::fs::read_to_string(output)?.contains("hello fallback"));
    assert_eq!(read_what_ran_for_executor(buck, "Worker").await?.len(), 0);
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_worker_shared(buck: &Buck) -> anyhow::Result<()> {
    // Check worker is shared between multiple actions.
    let target = format!("{PACKAGE}:gen_worker_deps");
    let res = buck.build(&build_args(&WORKER_ARGS, &target)).await?;
    let report: Value = serde_json::from_str(&res.stdout)?;
    let trace_id = report
        .get("trace_id")
        .and_then(Value::as_str)
        .context("No `trace_id` in the build report")?;
    let worker_dirs = worker_stderrs(Path::new("/tmp/buck2_worker"), trace_id);
    let num_workers = worker_dirs.len();
    assert_eq!(
        num_workers, 1,
        "expected 1 worker, found {worker_dirs:?} for {trace_id}"
    );
    assert_eq!(
        read_what_ran_for_executor(buck, "WorkerInit").await?.len(),
        1
    );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_error_thrown_during_execution(buck: &Buck) {
    // Check error thrown during worker command execution.
    let target = format!("{PACKAGE}:gen_worker_run_error");
    buck.build(&build_args(&WORKER_ARGS, &target))
        .await
        .expect_failure()
        .assert_stderr_matches("compile error");
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_error_thrown_during_startup(buck: &Buck) {
    // Check error thrown on worker start up.
    let target = format!("{PACKAGE}:gen_worker_init_fail");
    buck.build(&build_args(&WORKER_ARGS, &target))
        .await
        .expect_failure()
        .assert_stderr_matches("init error");
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_error_thrown_missing_worker(buck: &Buck) {
    // Check error thrown by missing worker executable.
    let target = format!("{PACKAGE}:gen_worker_missing");
    buck.build(&build_args(&WORKER_ARGS, &target))
        .await
        .expect_failure()
        .assert_stderr_matches("Worker failed to spawn");
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_connection_error(buck: &Buck) {
    // Check connection error if worker server can't be connected to.
    let target = format!("{PACKAGE}:gen_worker_init_deadlock");
    buck.build(&build_args(&WORKER_ARGS, &target))
        .await
        .expect_failure()
        .assert_stderr_matches("Worker failed to connect");
}

// Streaming is stuck in "initialize_worker" state
// @buck_test(inplace=True, skip_for_os=["darwin", "windows"])
// async def test_streaming(buck: Buck) -> None:
//     # Streaming execution demo
//     res = await buck.build(*worker_args, package + ":gen_worker_run_out_streaming")
//     output = res.get_build_report().output_for_target(
//         package + ":gen_worker_run_out_streaming"
//     )
//     assert output.read_text() == "hello worker"
//     assert len(await _read_what_ran_for_executor(buck, "Worker")) == 1

// TODO(ctolliday) re-enable once cancellation is in place
// def assert_executed(
//     what_ran: List[Dict[str, Any]], expected: List[Tuple[str, str]]
// ) -> None:
//     def target_name(identity: str) -> str:
//         return identity.split()[0].split(":")[1]
//     expected_target_names = [entry[1] for entry in expected]
//     what_ran_matching = [
//         (entry["reproducer"]["executor"], target_name(entry["identity"]))
//         for entry in what_ran
//         if target_name(entry["identity"]) in expected_target_names
//     ]
//     assert what_ran_matching == expected, what_ran
//
// assert_executed(
//     await read_what_ran(buck),
//     [
//         # TODO(ctolliday) ideally this should use --no-remote-cache and check for "Re" not "Cache".
//         # Will fail if not cached because RE execution will block on local execution.
//         # Making workers cancellable fixes this.
//         ("Cache", "gen_slow_worker_fast_fallback"),
//         ("WorkerInit", "gen_fast_worker_slow_fallback"),
//         ("Worker", "gen_fast_worker_slow_fallback"),
//     ],
// )

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_worker_exit_handled(buck: &Buck) {
    // Check connection error if worker server dies mid-request
    let target = format!("{PACKAGE}:gen_worker_init_self_destruct");
    buck.build(&build_args(&WORKER_ARGS, &target))
        .await
        .expect_failure()
        .assert_stderr_matches("Worker exited while running command");
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_hybrid_execution(buck: &Buck) -> anyhow::Result<()> {
    // 1. Check that building `:gen_slow_worker_fast_fallback` first (as dependency) causes remote to succeed and worker to be cancelled.
    // 2. Check that `:gen_fast_worker_slow_fallback` worker execution succeeds, using same worker initialized by 1.
    let hybrid_args = [
        "-c",
        "build.use_persistent_workers=True",
        "-c",
        "build.use_limited_hybrid=False",
    ];
    let target = format!("{PACKAGE}:gen_fast_worker_slow_fallback");
    buck.build(&build_args(&hybrid_args, &target)).await?;
    assert_eq!(
        read_what_ran_for_executor(buck, "WorkerInit").await?.len(),
        1
    );
    Ok(())
}

#[buck_test(inplace = true, skip_for_os = ["darwin", "windows"])]
async fn test_worker_thread_limit(buck: &Buck) -> anyhow::Result<()> {
    let worker_args = [
        "-c",
        "build.use_persistent_workers=True",
        "--local-only",
        "--no-remote-cache",
    ];
    let target = format!("{PACKAGE}:gen_worker_concurrent_fail");
    buck.build(&build_args(&worker_args, &target))
        .await
        .expect_failure()
        .assert_stderr_matches("Concurrency check failed");
    let target = format!("{PACKAGE}:gen_worker_concurrent_pass");
    buck.build(&build_args(&worker_args, &target)).await?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_dummy() {
    // None of the tests in this file pass on Windows or mac and that upsets CI.
}
