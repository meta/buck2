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
use std::time::Duration;

use buck2_e2e::BuckCommand;
use buck2_e2e::BuckOutput;
use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;
use regex::Regex;

// It's better to fail a test than to hit our test timeout. When cycle detection is not working, buck will just hang. So wrap these in a timeout.
async fn expect_cycle(command: BuckCommand<'_>) -> anyhow::Result<BuckOutput> {
    Ok(tokio::time::timeout(Duration::from_secs(200), command)
        .await?
        .expect_failure())
}

// A cycle is only reported once the command has stalled on it, at times for long enough that the
// client starts reporting on the wait. Those lines come and go with the timing, and carry resource
// usage figures besides.
static WAIT_REPORT_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\[<TIMESTAMP>\] (Waiting on buck2 daemon|Resource usage:|IO:).*\n").unwrap()
});

fn check_cycle_error(failure: &BuckOutput, name: &str) {
    golden(
        &WAIT_REPORT_LINE.replace_all(&sanitize_stderr(&failure.stderr), ""),
        &format!("golden/{name}.golden.txt"),
    );
}

#[buck_test]
async fn test_detect_load_cycle(buck: &Buck) -> anyhow::Result<()> {
    let failure = expect_cycle(buck.cquery(&["//:top", "-c", "cycles.load=yes"])).await?;
    check_cycle_error(&failure, "load_cycle");
    Ok(())
}

#[buck_test]
async fn test_detect_configured_graph_cycles(buck: &Buck) -> anyhow::Result<()> {
    let failure = expect_cycle(buck.cquery(&["//:top", "-c", "cycles.cfg_graph=yes"])).await?;
    check_cycle_error(&failure, "configured_graph_cycle");
    Ok(())
}

#[buck_test]
async fn test_detect_configured_graph_cycles_on_recompute(buck: &Buck) -> anyhow::Result<()> {
    buck.cquery(&["//:top"]).await?;

    let failure = expect_cycle(buck.cquery(&["//:top", "-c", "cycles.cfg_graph=yes"])).await?;

    check_cycle_error(&failure, "configured_graph_cycle_on_recompute");
    Ok(())
}

#[buck_test]
async fn test_detect_configured_graph_cycles_2(buck: &Buck) -> anyhow::Result<()> {
    let failure = expect_cycle(buck.cquery(&["//:top", "-c", "cycles.cfg_toolchain=yes"])).await?;
    check_cycle_error(&failure, "configured_toolchain_cycle");
    Ok(())
}

#[buck_test]
async fn test_more_recompute_cases(buck: &Buck) -> anyhow::Result<()> {
    buck.cquery(&["//:top"]).await?;

    let failure = expect_cycle(buck.cquery(&["//:top", "-c", "cycles.load=yes"])).await?;
    check_cycle_error(&failure, "more_recompute_cases_load");

    let failure = expect_cycle(buck.cquery(&["//:top", "-c", "cycles.cfg_graph=yes"])).await?;
    check_cycle_error(&failure, "more_recompute_cases_cfg_graph");
    Ok(())
}

#[buck_test]
async fn test_detect_dynamic_action_self_cycle(buck: &Buck) -> anyhow::Result<()> {
    let failure = expect_cycle(buck.build(&["//dynamic_cycle:self"])).await?;
    check_cycle_error(&failure, "dynamic_action_self_cycle");
    Ok(())
}

#[buck_test]
async fn test_detect_dynamic_action_cycle_through_lambda(buck: &Buck) -> anyhow::Result<()> {
    let failure = expect_cycle(buck.build(&["//dynamic_cycle:through_lambda"])).await?;
    check_cycle_error(&failure, "dynamic_action_cycle_through_lambda");
    Ok(())
}
