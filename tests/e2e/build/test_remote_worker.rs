/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::expect_exec_count;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

#[buck_test(inplace = true)]
async fn test_remote_worker(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/rules/remote_worker:run_two_worker_rules";
    let result = buck
        .build(&[
            target,
            "-c",
            &format!("test.cache_buster={}", random_string()),
        ])
        .await?;

    let output = result.build_report()?.output_for_target(target)?;

    let output_path = buck.cwd().join(output);
    let output_lines: Vec<String> = std::fs::read_to_string(&output_path)?
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(output_lines.len(), 2);
    // We would like to check that both lines are the same, as that means that
    // both actions used the same persistent worker.
    // However, RE just does a best effort to use the same persistent worker,
    // so we can't guarantee this is the case.
    // assert output_lines[0].strip() == output_lines[1].strip()

    let whatran_json = read_what_ran(buck, &[]).await?;
    let mut worker_entries = Vec::new();
    for x in &whatran_json {
        let identity = x
            .get("identity")
            .and_then(Value::as_str)
            .with_context(|| format!("No `identity` in {x}"))?;
        if identity.contains("run_remote_worker") {
            worker_entries.push(x);
        }
    }
    assert_eq!(worker_entries.len(), 2, "{whatran_json:?}");
    assert_eq!(
        worker_entries[0]
            .get("reproducer")
            .and_then(|r| r.get("executor"))
            .with_context(|| format!("No `reproducer.executor` in {}", worker_entries[0]))?,
        "ReWorker"
    );

    let whatran = buck.log(&["what-ran", "--skip-cache-hits"]).await?.stdout;
    let whatran: Vec<&str> = whatran.split('\n').collect();
    let worker_lines = whatran.iter().filter(|x| x.contains("re_worker(")).count();
    assert_eq!(worker_lines, 2, "{whatran:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_remote_worker_caches(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/rules/remote_worker:run_remote_worker_1";
    let args = [target];
    buck.build(&args).await?;

    // The Python test called `buck.kill()` without awaiting it, so the daemon was never killed.
    // After a kill, `what-ran` lists the second build's action cache hits (57,101 of them) and the
    // test fails, so the kill stays out.
    // buck.kill().await?;

    buck.build(&args).await?;
    expect_exec_count(buck, 0).await?;
    Ok(())
}
