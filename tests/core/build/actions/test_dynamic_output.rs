/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;
use std::time::Duration;
use std::time::Instant;

use buck2_e2e::golden::sanitize_stderr;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

#[buck_test(data_dir = "everything")]
async fn test_dynamic_output(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:"]).await?;
    Ok(())
}

#[buck_test(data_dir = "everything")]
async fn test_nested_dynamic_action_rule_type_names(buck: &Buck) -> anyhow::Result<()> {
    // Both levels of dynamic holders must inherit the original rule metadata.
    buck.build(&["root//:nested"]).await?;
    let actions = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    assert!(!actions.is_empty());
    assert_eq!(
        actions
            .iter()
            .map(|action| action["target_rule_type_name"].as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([Some("dynamic_check")])
    );
    Ok(())
}

#[buck_test(data_dir = "everything_new")]
async fn test_dynamic_output_new(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:"]).await?;
    Ok(())
}

#[buck_test(data_dir = "empty_dynamic_list")]
async fn test_empty_dynamic_list(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:empty_test"]).await?;
    Ok(())
}

#[buck_test(data_dir = "artifact_eq_bug")]
async fn test_artifact_eq_bug(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:bug"]).await?;
    Ok(())
}

/// Every artifact re-bound through a dynamic_output gets its own action key
/// that redirects to the producing action and stores that action's *entire*
/// `ActionOutputs` as its dice value. On an incremental rebuild all of those
/// keys recompute, and DICE equality-compares each one's old and new value on
/// its serialized core state thread. If that comparison is not O(1), the
/// rebuild does O(N^2) work serialized on one thread (N per key, N keys); for
/// real dist-ThinLTO links (~64k outputs on the index action) that stalled
/// rebuilds for over ten minutes.
///
/// The rebuild below does strictly less work than the initial build, so we can
/// use the initial build to calibrate for host speed. With the O(N^2) behavior
/// the rebuild is one to two orders of magnitude slower than the initial build
/// instead.
#[buck_test(data_dir = "many_rebound_outputs", skip_for_os = ["windows"])]
async fn test_many_rebound_outputs_incremental_rebuild(buck: &Buck) -> anyhow::Result<()> {
    let start = Instant::now();
    buck.build(&["root//:check"]).await?;
    let first = start.elapsed().as_secs_f64();

    // Invalidate the analysis (and with it every action key). The seed changes
    // the content of exactly one of the dynamic action's outputs (see the
    // fixture for why that matters).
    let targets = buck.cwd().join("TARGETS.fixture");
    std::fs::write(
        &targets,
        std::fs::read_to_string(&targets)?.replace(r#"seed = "A""#, r#"seed = "B""#),
    )?;

    // Once the rebuild hits this threshold the test's outcome is decided, so
    // don't wait for it to finish: with the O(N^2) behavior that would take
    // many more minutes and hit the 10 minute test level timeout, which our
    // test infra handles poorly.
    let threshold = f64::max(60.0, 3.0 * first);
    let start = Instant::now();
    let timed_out = match tokio::time::timeout(
        Duration::from_secs_f64(threshold),
        buck.build(&["root//:check"]),
    )
    .await
    {
        Ok(result) => {
            result?;
            false
        }
        Err(_) => {
            buck.kill().await?;
            true
        }
    };
    let second = start.elapsed().as_secs_f64();

    assert!(
        !timed_out && second < threshold,
        "incremental rebuild took {second:.1}s vs {first:.1}s for the initial build; dice is likely doing deep ActionOutputs comparisons"
    );
    Ok(())
}

#[buck_test(data_dir = "analysis_failure")]
async fn test_dynamic_output_analysis_failure(buck: &Buck) {
    let result = buck
        .build(&["root//:analysis_failure"])
        .await
        .expect_failure();
    result.assert_stderr_matches("Analysis failed: this is a test failure message");
    golden(
        &sanitize_stderr(&result.stderr),
        "analysis_failure/golden/test_dynamic_output_analysis_failure.golden.txt",
    );
}
