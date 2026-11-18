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

use buck2_e2e::approx::approx;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use serde_json::Value;

async fn get_detailed_metrics(buck: &Buck) -> anyhow::Result<Option<Value>> {
    Ok(filter_events(
        buck,
        &[
            "Event",
            "data",
            "Instant",
            "data",
            "DetailedAggregatedMetrics",
        ],
    )
    .await?
    .into_iter()
    .next())
}

fn parse_metrics(metrics: &Value) -> (&Value, BTreeMap<String, &Value>) {
    let all_targets = field(metrics, "all_targets_build_metrics");

    let stringify = |t: &Value| {
        let t = &t["label"];
        format!(
            "{}:{}",
            t["package"].as_str().expect("a package"),
            t["name"].as_str().expect("a name")
        )
    };

    let per_targets = metrics["top_level_target_metrics"]
        .as_array()
        .expect("`top_level_target_metrics` is a list")
        .iter()
        .map(|v| (stringify(&v["target"]), v))
        .collect();
    (all_targets, per_targets)
}

/// The value of `key` in `object`, which must have one, `null` or not.
#[track_caller]
fn field<'a>(object: &'a Value, key: &str) -> &'a Value {
    object
        .get(key)
        .unwrap_or_else(|| panic!("No `{key}` in {object}"))
}

#[buck_test]
async fn test_disabled(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:foo4", "-c", "buck2.detailed_aggregated_metrics=false"])
        .await?;
    let message = get_detailed_metrics(buck).await?;
    assert!(message.is_none());
    Ok(())
}

#[buck_test]
async fn test_enabled(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:foo4", "-c", "buck2.detailed_aggregated_metrics=true"])
        .await?;
    let message = get_detailed_metrics(buck).await?.expect("metrics");
    let (all_targets_metrics, per_target_metrics) = parse_metrics(&message);
    let foo4 = per_target_metrics["root//:foo4"];
    assert_eq!(field(all_targets_metrics, "action_graph_size"), 7);
    assert_eq!(field(foo4, "action_graph_size"), 7);
    assert_eq!(field(&foo4["metrics"], "declared_actions"), &approx(12.0));
    Ok(())
}

#[buck_test]
async fn test_incomplete_graph(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:foo4",
        "-c",
        "buck2.detailed_aggregated_metrics=true",
        "-c",
        "user.dyn_input_good=0",
    ])
    .await
    .expect_failure();
    let message = get_detailed_metrics(buck).await?.expect("metrics");
    let (all_targets_metrics, per_target_metrics) = parse_metrics(&message);
    let foo4 = per_target_metrics["root//:foo4"];
    assert_eq!(
        field(all_targets_metrics, "action_graph_size"),
        &Value::Null
    );
    assert_eq!(field(foo4, "action_graph_size"), &Value::Null);
    assert_eq!(field(&foo4["metrics"], "declared_actions"), &approx(12.0));
    Ok(())
}

#[buck_test]
async fn test_wall_clock_completion(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:foo4", "-c", "buck2.detailed_aggregated_metrics=true"])
        .await?;
    let message = get_detailed_metrics(buck).await?.expect("metrics");
    let (_all_targets_metrics, per_target_metrics) = parse_metrics(&message);
    let wall_clock = field(
        per_target_metrics["root//:foo4"],
        "wall_clock_completion_ms",
    );
    assert!(!wall_clock.is_null());
    assert!(wall_clock.as_f64().is_some_and(|w| w > 0.0), "{wall_clock}");
    Ok(())
}

#[buck_test]
async fn test_wall_clock_completion_on_timeout(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:slow",
        "-c",
        "buck2.detailed_aggregated_metrics=true",
        "--overall-timeout",
        "1s",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Build timed out");
    let message = get_detailed_metrics(buck).await?.expect("metrics");
    let (_all_targets_metrics, per_target_metrics) = parse_metrics(&message);
    let wall_clock = field(
        per_target_metrics["root//:slow"],
        "wall_clock_completion_ms",
    );
    assert!(!wall_clock.is_null());
    assert!(wall_clock.as_f64().is_some_and(|w| w > 0.0), "{wall_clock}");
    Ok(())
}

#[buck_test]
async fn test_wall_clock_completion_on_failure(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:foo4",
        "-c",
        "buck2.detailed_aggregated_metrics=true",
        "-c",
        "user.dyn_input_good=0",
    ])
    .await
    .expect_failure();
    let message = get_detailed_metrics(buck).await?.expect("metrics");
    let (_all_targets_metrics, per_target_metrics) = parse_metrics(&message);
    let wall_clock = field(
        per_target_metrics["root//:foo4"],
        "wall_clock_completion_ms",
    );
    assert!(!wall_clock.is_null());
    assert!(wall_clock.as_f64().is_some_and(|w| w > 0.0), "{wall_clock}");
    Ok(())
}

#[buck_test]
async fn test_amortization(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:foo4",
        "//:foo5",
        "-c",
        "buck2.detailed_aggregated_metrics=true",
    ])
    .await?;
    let message = get_detailed_metrics(buck).await?.expect("metrics");
    let (all_targets_metrics, per_target_metrics) = parse_metrics(&message);
    let (foo4, foo5) = (
        per_target_metrics["root//:foo4"],
        per_target_metrics["root//:foo5"],
    );
    assert_eq!(field(all_targets_metrics, "action_graph_size"), 11);
    assert_eq!(
        field(&all_targets_metrics["metrics"], "declared_actions"),
        &approx(15.0)
    );
    assert_eq!(field(foo4, "action_graph_size"), 7);
    assert_eq!(field(&foo4["metrics"], "declared_actions"), &approx(12.0));
    assert_eq!(
        field(&foo4["amortized_metrics"], "declared_actions"),
        &approx(7.5)
    );
    assert_eq!(field(foo5, "action_graph_size"), 7);
    assert_eq!(field(&foo5["metrics"], "declared_actions"), &approx(12.0));
    assert_eq!(
        field(&foo5["amortized_metrics"], "declared_actions"),
        &approx(7.5)
    );
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_enabled_after_analysis_soft_errors(buck: &Buck) -> anyhow::Result<()> {
    // First command runs analysis with collection off; enabling it on a later
    // command can't produce complete metrics, so we expect a soft error and empty
    // metrics rather than partial ones.
    buck.build(&["//:foo4"]).await?;
    buck.build(&["//:foo4", "-c", "buck2.detailed_aggregated_metrics=true"])
        .await?;
    let log = buck.log(&["show"]).await?.stdout;
    assert!(log.contains("detailed_aggregated_metrics_enabled_after_analysis"));
    let message = get_detailed_metrics(buck).await?.expect("metrics");
    let (all_targets_metrics, per_target_metrics) = parse_metrics(&message);
    assert!(per_target_metrics.is_empty(), "{per_target_metrics:?}");
    assert!(all_targets_metrics["action_graph_size"].is_null());
    Ok(())
}
