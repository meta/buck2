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
use std::io::Write;

use anyhow::Context;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use serde_json::Value;

/// A line of `buck2 log critical-path --format=tabulated`, with the name up to its first space.
struct CriticalPathLog {
    kind: String,
    name: String,
    execution_kind: String,
}

impl CriticalPathLog {
    fn new(fields: &[&str]) -> anyhow::Result<Self> {
        let [
            kind,
            name,
            _category,
            _identifier,
            execution_kind,
            _total_duration,
            _user_duration,
            _potential_improvement_duration,
            ..,
        ] = fields
        else {
            anyhow::bail!("Expected 8 fields in the critical path entry {fields:?}");
        };
        Ok(Self {
            kind: (*kind).to_owned(),
            name: name.split(' ').next().unwrap_or_default().to_owned(),
            execution_kind: (*execution_kind).to_owned(),
        })
    }
}

async fn do_critical_path(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:step_3", "--no-remote-cache"]).await?;

    let critical_path = buck.log(&["critical-path", "--format=tabulated"]).await?;
    let trimmed_critical_path = critical_path
        .stdout
        .trim()
        .lines()
        .map(|e| e.split('\t').collect::<Vec<_>>())
        .filter(|e| e[0] != "waiting")
        .map(|e| CriticalPathLog::new(&e))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let expected = [
        ("buckd_command_init", ""),
        ("file-watcher-wait", ""),
        ("other-command-start-overhead", ""),
        ("listing", "root//"),
        ("load", "root//"),
        ("configure_target", ""),
        ("analysis", "root//:step_0"),
        ("analysis", "root//:step_1"),
        ("analysis", "root//:step_2"),
        ("analysis", "root//:step_3"),
        ("action", "root//:step_0"),
        ("action", "root//:step_1"),
        ("action", "root//:step_2"),
        ("build_key", ""),
        ("action", "root//:step_3"),
        ("materialization", "root//:step_3"),
        ("compute-critical-path", ""),
    ];

    // List of n.kind gives better failure messages
    assert_eq!(
        trimmed_critical_path
            .iter()
            .map(|n| n.kind.as_str())
            .collect::<Vec<_>>(),
        expected.iter().map(|e| e.0).collect::<Vec<_>>()
    );

    for (s, e) in trimmed_critical_path
        .iter()
        .rev()
        .zip(expected.iter().rev())
    {
        if s.kind == "action" {
            assert_ne!(s.execution_kind, "");
            assert_ne!(s.execution_kind, "ACTION_EXECUTION_NOTSET");
        } else {
            assert_eq!(s.execution_kind, "");
        }

        assert_eq!(s.kind, e.0);
        assert_eq!(s.name, e.1);
    }
    Ok(())
}

async fn critical_path_helper(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    let critical_path_actions = filter_events(
        buck,
        &[
            "Event",
            "data",
            "Instant",
            "data",
            "BuildGraphInfo",
            "critical_path2",
        ],
    )
    .await?;

    assert_eq!(critical_path_actions.len(), 1);
    match critical_path_actions.into_iter().next() {
        Some(Value::Array(actions)) => Ok(actions),
        other => anyhow::bail!("`critical_path2` is not a list: {other:?}"),
    }
}

fn set_critical_path_backend(buck: &Buck, backend: &str) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .append(true)
        .open(buck.cwd().join(".buckconfig"))?
        .write_all(format!("[buck2]\ncritical_path_backend2 = {backend}\n").as_bytes())
}

fn use_logging_backend(buck: &Buck) -> std::io::Result<()> {
    set_critical_path_backend(buck, "logging")
}

/// The `UnstableE2eData` events of the last command.
async fn unstable_e2e_data(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    filter_events(
        buck,
        &["Event", "data", "Instant", "data", "UnstableE2eData"],
    )
    .await
}

/// The parsed `data` of the `events` with `key`.
fn data_with_key(events: &[Value], key: &str) -> anyhow::Result<Vec<Value>> {
    events
        .iter()
        .filter(|ev| ev["key"] == key)
        .map(|ev| {
            let data = ev["data"].as_str().context("A non-string `data`")?;
            Ok(serde_json::from_str(data)?)
        })
        .collect()
}

fn string(value: &Value) -> anyhow::Result<String> {
    value
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("Expected a string, got {value}"))
}

/// The keys of the critical path's logged nodes.
fn node_keys(events: &[Value]) -> anyhow::Result<BTreeSet<String>> {
    data_with_key(events, "critical_path_logging_node")?
        .iter()
        .map(|data| string(&data["key"]))
        .collect()
}

/// The critical path's logged discovery edges, as `(key, dep)`.
fn edges(events: &[Value]) -> anyhow::Result<BTreeSet<(String, String)>> {
    data_with_key(events, "critical_path_logging_edge")?
        .iter()
        .map(|data| Ok((string(&data["key"])?, string(&data["dep"])?)))
        .collect()
}

async fn discovery_edges(buck: &Buck) -> anyhow::Result<BTreeSet<(String, String)>> {
    edges(&unstable_e2e_data(buck).await?)
}

fn edge(key: &str, dep: &str) -> (String, String) {
    (key.to_owned(), dep.to_owned())
}

const ROOT_LOAD: &str = "InterpreterResultsKey(root//)";
const SUB_LOAD: &str = "InterpreterResultsKey(root//sub)";

fn assert_no_load_cycle(edges: &BTreeSet<(String, String)>) {
    assert!(
        !(edges.contains(&edge(ROOT_LOAD, SUB_LOAD)) && edges.contains(&edge(SUB_LOAD, ROOT_LOAD))),
        "load discovery edges form a cycle: {edges:?}"
    );
}

/// Whether Python would take `value` as true.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

#[buck_test]
async fn test_critical_path_longest_path_graph(buck: &Buck) -> anyhow::Result<()> {
    set_critical_path_backend(buck, "longest-path-graph")?;
    do_critical_path(buck).await
}

#[buck_test]
async fn test_critical_path_json(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:step_3", "--no-remote-cache"]).await?;
    let critical_path = buck
        .log(&["critical-path", "--format", "json"])
        .await?
        .stdout
        .trim()
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;

    let trimmed_critical_path: Vec<&Value> = critical_path
        .iter()
        .filter(|e| e["kind"] != "waiting")
        .collect();

    let expected = [
        ("buckd_command_init", None),
        ("file-watcher-wait", None),
        ("other-command-start-overhead", None),
        ("listing", Some("root//")),
        ("load", Some("root//")),
        ("configure_target", None),
        ("analysis", Some("root//:step_0")),
        ("analysis", Some("root//:step_1")),
        ("analysis", Some("root//:step_2")),
        ("analysis", Some("root//:step_3")),
        ("action", Some("root//:step_0")),
        ("action", Some("root//:step_1")),
        ("action", Some("root//:step_2")),
        ("build_key", None),
        ("action", Some("root//:step_3")),
        ("materialization", Some("root//:step_3")),
        ("compute-critical-path", None),
    ];

    // List of n.kind gives better failure messages
    assert_eq!(
        trimmed_critical_path
            .iter()
            .map(|n| n["kind"].as_str().context("No `kind`"))
            .collect::<anyhow::Result<Vec<_>>>()?,
        expected.iter().map(|e| e.0).collect::<Vec<_>>()
    );

    for (critical, exp) in trimmed_critical_path
        .iter()
        .rev()
        .zip(expected.iter().rev())
    {
        assert!(critical.get("kind").is_some());
        assert_eq!(critical["kind"], exp.0);

        if [
            "waiting",
            "compute-critical-path",
            "file-watcher-wait",
            "other-command-start-overhead",
            "buckd_command_init",
            "build_key",
            "configure_target",
        ]
        .contains(&exp.0)
        {
            assert!(critical.get("name").is_none());
        } else {
            assert!(critical.get("name").is_some());
            let name = string(&critical["name"])?;
            assert_eq!(name.split(' ').next(), exp.1);
        }

        if critical["kind"] == "action" {
            assert!(critical.get("execution_kind").is_some());
            assert_ne!(critical["execution_kind"], "");
            assert_ne!(critical["execution_kind"], "ACTION_EXECUTION_NOTSET");
        } else {
            assert!(critical.get("execution_kind").is_none());
        }
    }
    Ok(())
}

// Test that verifies the dicekey->node+deps graph that we produce for critical path
// calculations. It can be a lot easier to understand bugs and behavior here than
// only inspecting the final critical path output (like other tests).
#[buck_test]
async fn test_dynamic_input_events(buck: &Buck) -> anyhow::Result<()> {
    use_logging_backend(buck)?;

    buck.build(&["//:check_dynamic_input", "--no-remote-cache"])
        .await?;
    let mut events = Value::Array(data_with_key(
        &unstable_e2e_data(buck).await?,
        "critical_path_logging_node",
    )?);
    for ev in events.as_array_mut().into_iter().flatten() {
        if let Some(time_span) = ev.get_mut("time_span") {
            *time_span = serde_json::json!([null, null]);
        }
    }
    events.sort_all_objects();

    golden(
        &Json::from(events.clone()).dumps(Some(2), true),
        "events.golden.json",
    );
    Ok(())
}

// Test that we can compute critical paths that include edges as inputs of dynamic_output/actions.
#[buck_test]
async fn test_dynamic_input(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:check_dynamic_input", "--no-remote-cache"])
        .await?;
    let critical_path = buck
        .log(&["critical-path", "--format", "json"])
        .await?
        .stdout
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;

    assert!(!critical_path.is_empty());
    let mut transformed = Vec::new();
    for critical in &critical_path {
        assert!(critical.get("kind").is_some());
        let mut t = string(&critical["kind"])?;

        if t == "waiting" {
            continue;
        }

        if [
            "compute-critical-path",
            "file-watcher-wait",
            "other-command-start-overhead",
            "buckd_command_init",
            "configure_target",
            "build_key",
        ]
        .contains(&t.as_str())
        {
            assert!(critical.get("name").is_none());
        } else {
            assert!(critical.get("name").is_some());
            let name = string(&critical["name"])?;
            t = format!("{t} {}", name.split(' ').next().unwrap_or_default());
        }

        if critical["kind"] == "action" {
            assert!(critical.get("execution_kind").is_some());
            assert_ne!(critical["execution_kind"], "");
            assert_ne!(critical["execution_kind"], "ACTION_EXECUTION_NOTSET");
        } else {
            assert!(critical.get("execution_kind").is_none());
        }

        // there's nondeterminism in critical path here because step1 action
        // depends on both step0 action and step1 analysis, both of those depend
        // on step0 analysis.
        if ["analysis root//:step_1", "action root//:step_0"].contains(&t.as_str()) {
            continue;
        }

        transformed.push(t);
    }

    golden(
        &Json::from(transformed.clone()).dumps(Some(2), false),
        "dynamic_input.golden.json",
    );
    Ok(())
}

#[buck_test]
async fn test_critical_path_metadata(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:step_0",
        "--no-remote-cache",
        "--client-metadata",
        "id=myclient",
        "--oncall=myoncall",
    ])
    .await?;

    let build_graph_info = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "BuildGraphInfo"],
    )
    .await?;

    let build_graph_info = &build_graph_info[0];
    assert!(truthy(build_graph_info));
    assert!(build_graph_info["metadata"].get("username").is_some());
    assert_eq!(build_graph_info["metadata"]["client"], "myclient");
    assert_eq!(build_graph_info["metadata"]["oncall"], "myoncall");
    Ok(())
}

#[buck_test]
async fn test_critical_path_execution_kind(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:step_3", "--no-remote-cache"]).await?;

    let critical_path_actions = critical_path_helper(buck).await?;

    let mut has_action_execution = false;
    for action in &critical_path_actions {
        assert!(truthy(&action["entry"]));
        // Every ActionExecution should have an execution kind and it shouldn't be 0 (default)
        if let Some(execution) = action["entry"].get("ActionExecution") {
            has_action_execution = true;
            assert!(truthy(&execution["execution_kind"]));
            assert_ne!(execution["execution_kind"], 0);
        }
    }

    // Should have at least 1 ActionExecution or something went wrong
    assert!(has_action_execution);
    Ok(())
}

#[buck_test]
async fn test_critical_path_rule_type(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:step_0", "--no-remote-cache"]).await?;

    let critical_path_actions = critical_path_helper(buck).await?;

    for action in &critical_path_actions {
        assert!(truthy(&action["entry"]));

        if let Some(execution) = action["entry"].get("ActionExecution") {
            assert!(truthy(&execution["target_rule_type_name"]));
            assert_eq!(execution["target_rule_type_name"], "write");
        }
    }
    Ok(())
}

#[buck_test]
async fn test_critical_path_action_digest(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:step_3", "--no-remote-cache"]).await?;

    let critical_path_actions = critical_path_helper(buck).await?;

    let mut has_action_digest = false;
    for action in &critical_path_actions {
        assert!(truthy(&action["entry"]));
        if let Some(execution) = action["entry"].get("ActionExecution") {
            if execution.get("action_digest").is_some() {
                has_action_digest = true;
            }
        }
    }

    assert!(has_action_digest);
    Ok(())
}

#[buck_test]
async fn test_critical_path_top_level_targets(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:step_1", "//:step_2", "//:step_3", "--no-remote-cache"])
        .await?;

    let build_graph_info = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "BuildGraphInfo"],
    )
    .await?;

    let build_graph_info = &build_graph_info[0];
    let mut top_level_targets = build_graph_info["top_level_targets"]
        .as_array()
        .context("`top_level_targets` is not a list")?
        .iter()
        .map(|x| {
            Ok((
                string(&x["target"]["label"]["name"])?,
                x["duration_us"].as_u64().context("No `duration_us`")?,
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    // Sort by duration.
    top_level_targets.sort_by_key(|(_, duration)| *duration);
    let [(t0, d0), (t1, d1), (t2, d2)] = top_level_targets.as_slice() else {
        panic!("Expected 3 top-level targets, got {top_level_targets:?}");
    };

    assert_eq!(t0, "step_1");
    assert_eq!(t1, "step_2");
    assert_eq!(t2, "step_3");
    assert!(d0 <= d1);
    assert!(d1 <= d2);

    // The duration should match the relevant build steps:
    let total_duration = build_graph_info["critical_path2"]
        .as_array()
        .context("`critical_path2` is not a list")?
        .iter()
        .filter(|x| {
            ![
                "GenericEntry",
                "ComputeCriticalPath",
                "FinalMaterialization",
            ]
            .iter()
            .any(|k| x["entry"].get(k).is_some())
        })
        .map(|x| x["total_duration_us"].as_u64())
        .sum::<Option<u64>>()
        .context("A `total_duration_us` that is not a number")?;
    assert_eq!(total_duration, *d2);
    Ok(())
}

#[buck_test]
async fn test_critical_path_test_entries(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["//:long_running_test"]).await?;

    let critical_path_actions = critical_path_helper(buck).await?;

    // Should have exactly 1 TestListing.
    let test_listing_actions: Vec<&Value> = critical_path_actions
        .iter()
        .filter(|action| action["entry"].get("TestListing").is_some())
        .collect();
    assert_eq!(test_listing_actions.len(), 1);

    // Assert there is 1 TestExecution with the correct data.
    let test_execution_actions: Vec<&Value> = critical_path_actions
        .iter()
        .filter(|action| action["entry"].get("TestExecution").is_some())
        .collect();

    assert_eq!(test_execution_actions.len(), 1);
    let test_execution_action = test_execution_actions[0];
    assert_eq!(
        test_execution_action["entry"]["TestExecution"]["suite"],
        "root//:long_running_test"
    );
    assert!(
        test_execution_action["duration_us"]
            .as_u64()
            .is_some_and(|d| d > 100000), // 100ms
        "{test_execution_action}"
    );
    Ok(())
}

// Test that FinalMaterialization for artifacts reached via a transitive set projection
// depends on EnsureTransitiveSetProjectionKey in the critical path graph, not just BuildKey.
//
// The bug: when materializing artifacts from a tset projection, the critical path was
// recording a dependency on the BuildKey (the action that produced the artifact) instead
// of the EnsureTransitiveSetProjectionKey (which ensures the tset that contains the artifact).
// This matters because ensuring the tset transitively depends on the actions, and the
// critical path should reflect that the materialization waited on the tset, not just
// the individual action.
//
// Note: if an optimization is made to materialize tset artifacts individually (without
// going through EnsureTransitiveSetProjectionKey), the critical path dependency should
// still not be just BuildKey -- it should include the analysis that produces the tset.
#[buck_test]
async fn test_critical_path_tset_final_materialization(buck: &Buck) -> anyhow::Result<()> {
    use_logging_backend(buck)?;

    buck.build(&["//:tset_top", "--no-remote-cache"]).await?;
    let events = data_with_key(
        &unstable_e2e_data(buck).await?,
        "critical_path_logging_node",
    )?;

    // Find FinalMaterialization nodes
    let final_mat_nodes: Vec<&Value> = events
        .iter()
        .filter(|ev| {
            ev["key"]
                .as_str()
                .is_some_and(|k| k.starts_with("FinalMaterialization("))
        })
        .collect();

    assert!(
        !final_mat_nodes.is_empty(),
        "Expected at least one FinalMaterialization node"
    );

    // Find the FinalMaterialization node for tset_leaf's artifact.
    // tset_leaf's output is only reachable through the tset projection (it's not
    // a direct default_output of tset_top), so its FinalMaterialization must
    // reflect the tset dependency.
    let tset_leaf_mat: Vec<&Value> = final_mat_nodes
        .iter()
        .copied()
        .filter(|ev| ev["key"].as_str().is_some_and(|k| k.contains("tset_leaf")))
        .collect();
    assert_eq!(
        tset_leaf_mat.len(),
        1,
        "Expected exactly one FinalMaterialization for tset_leaf, got {}",
        tset_leaf_mat.len()
    );

    // The dependency should be EnsureTransitiveSetProjectionKey, not BuildKey.
    // With the bug, this would be BuildKey (the write action for tset_leaf).
    // With the fix, this correctly points to the tset ensure step.
    let tset_leaf_deps = tset_leaf_mat[0]["deps"]
        .as_array()
        .context("`deps` is not a list")?;
    assert!(
        tset_leaf_deps.iter().any(|dep| dep
            .as_str()
            .is_some_and(|d| d.starts_with("EnsureTransitiveSetProjectionKey("))),
        "Expected FinalMaterialization for tset_leaf to depend on \
         EnsureTransitiveSetProjectionKey, but got deps: {tset_leaf_deps:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_cross_package_load_edge(buck: &Buck) -> anyhow::Result<()> {
    use_logging_backend(buck)?;

    buck.build(&["//:cross_pkg", "--no-remote-cache"]).await?;
    let events = unstable_e2e_data(buck).await?;
    let nodes = node_keys(&events)?;
    let discovery_edges = edges(&events)?;

    // Both package loads must be present: root// (holds cross_pkg) and root//sub (loaded to
    // resolve the cross-package dep //sub:leaf).
    assert!(nodes.contains("InterpreterResultsKey(root//)"));
    assert!(
        nodes.contains("InterpreterResultsKey(root//sub)"),
        "root//sub should be loaded to resolve the cross-package dep"
    );

    // Configuring root//:cross_pkg requests the load of root//sub to resolve //sub:leaf, so
    // root//sub's load gets a discovery edge to root//'s load. That cross-package load edge is
    // exactly what this test guards.
    assert!(
        discovery_edges.contains(&edge(
            "InterpreterResultsKey(root//sub)",
            "InterpreterResultsKey(root//)"
        )),
        "expected cross-package load discovery edge (root//sub load depends on root// load), \
         got edges: {discovery_edges:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_build_package_cycle_has_no_load_cycle(buck: &Buck) -> anyhow::Result<()> {
    use_logging_backend(buck)?;
    buck.build(&["//:pkg_cycle_top", "--no-remote-cache"])
        .await?;
    let edges = discovery_edges(buck).await?;
    assert_no_load_cycle(&edges);
    // root// loaded first, so its request for //sub:pkg_cycle_mid is what discovered root//sub.
    assert!(
        edges.contains(&edge(SUB_LOAD, ROOT_LOAD)),
        "got edges: {edges:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_uquery_load_edge(buck: &Buck) -> anyhow::Result<()> {
    use_logging_backend(buck)?;
    buck.uquery(&["deps(//:cross_pkg)"]).await?;
    let edges = discovery_edges(buck).await?;
    // No tracked computation requests root//sub here: the deps traversal does, while
    // expanding //:cross_pkg, and reports root// as the requester.
    assert!(
        edges.contains(&edge(SUB_LOAD, ROOT_LOAD)),
        "expected the uquery traversal to credit root//sub's load to root//, got edges: {edges:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_uquery_package_cycle_has_no_load_cycle(buck: &Buck) -> anyhow::Result<()> {
    use_logging_backend(buck)?;
    buck.uquery(&["deps(//:pkg_cycle_top)"]).await?;
    let edges = discovery_edges(buck).await?;
    assert_no_load_cycle(&edges);
    assert!(
        edges.contains(&edge(SUB_LOAD, ROOT_LOAD)),
        "got edges: {edges:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_uquery_preloaded_package_has_no_discovery_edge(buck: &Buck) -> anyhow::Result<()> {
    use_logging_backend(buck)?;
    // The literal //sub:preloaded_literal loads root//sub alongside root//, before the
    // traversal expands //:preloaded_top, so nothing discovered root//sub.
    buck.uquery(&["deps(set(//:preloaded_top //sub:preloaded_literal))"])
        .await?;
    let edges = discovery_edges(buck).await?;
    assert!(
        !edges.contains(&edge(SUB_LOAD, ROOT_LOAD)),
        "root//sub was loaded by a literal, not discovered: {edges:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_configuration_dep_load_edge(buck: &Buck) -> anyhow::Result<()> {
    use_logging_backend(buck)?;

    buck.build(&["//:cfg_dep_pkg", "--no-remote-cache"]).await?;
    let events = unstable_e2e_data(buck).await?;
    let nodes = node_keys(&events)?;
    let discovery_edges = edges(&events)?;

    assert!(
        nodes.contains("InterpreterResultsKey(root//cfg)"),
        "root//cfg should be loaded to resolve the select key"
    );

    // Configuring root//:cfg_dep_pkg reaches root//cfg only through a select key, whose
    // configuration node loads root//cfg; root//cfg's load must still get a discovery edge to
    // root//'s load.
    assert!(
        discovery_edges.contains(&edge(
            "InterpreterResultsKey(root//cfg)",
            "InterpreterResultsKey(root//)"
        )),
        "expected a load discovery edge for the configuration dep (root//cfg load depends on \
         root// load), got edges: {discovery_edges:?}"
    );
    Ok(())
}

/// Test that anon target nodes appear on the critical path with correct
/// splitting when anon targets themselves have anon target dependencies.
///
/// The rule graph is:
///   uses_anon -> anon_outer -> anon_inner
///
/// uses_anon's analysis is split because it depends on anon_outer.
/// anon_outer's analysis is split because it depends on anon_inner.
/// anon_inner is a leaf (no anon deps) so it is NOT split.
///
/// Expected critical path ordering:
///   analysis[part1] -> anon_analysis[part1] -> anon_analysis -> anon_analysis[part2] -> analysis[part2]
#[buck_test]
async fn test_critical_path_anon_targets(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:anon_step", "--no-remote-cache"]).await?;

    let critical_path_actions = critical_path_helper(buck).await?;

    // Collect the entry types we care about
    let entry_kinds: Vec<&str> = critical_path_actions
        .iter()
        .filter_map(|action| entry_kind(&action["entry"]))
        .collect();

    // The critical path should contain all of these entries:
    // - analysis[part1]: uses_anon pre-anon-target analysis
    // - anon_analysis[part1]: outer anon target pre-inner-anon analysis
    // - anon_analysis: inner anon target (leaf, not split)
    // - anon_analysis[part2]: outer anon target post-inner-anon analysis
    // - analysis[part2]: uses_anon post-anon-target analysis
    for expected in [
        "analysis[part1]",
        "anon_analysis[part1]",
        "anon_analysis",
        "anon_analysis[part2]",
        "analysis[part2]",
    ] {
        assert!(
            entry_kinds.contains(&expected),
            "Expected {expected} in critical path, got: {entry_kinds:?}"
        );
    }

    // Verify ordering:
    //   analysis[part1] < anon_analysis[part1] < anon_analysis < anon_analysis[part2] < analysis[part2]
    let index = |kind: &str| entry_kinds.iter().position(|k| *k == kind).unwrap();
    let idx_analysis_p1 = index("analysis[part1]");
    let idx_anon_p1 = index("anon_analysis[part1]");
    let idx_anon_inner = index("anon_analysis");
    let idx_anon_p2 = index("anon_analysis[part2]");
    let idx_analysis_p2 = index("analysis[part2]");
    assert!(
        idx_analysis_p1 < idx_anon_p1
            && idx_anon_p1 < idx_anon_inner
            && idx_anon_inner < idx_anon_p2
            && idx_anon_p2 < idx_analysis_p2,
        "Expected analysis[part1] < anon_analysis[part1] < anon_analysis < anon_analysis[part2] < analysis[part2], \
         got indices {idx_analysis_p1}, {idx_anon_p1}, {idx_anon_inner}, {idx_anon_p2}, {idx_analysis_p2} \
         in {entry_kinds:?}"
    );
    Ok(())
}

fn entry_kind(entry: &Value) -> Option<&'static str> {
    if let Some(analysis) = entry.get("Analysis") {
        let part = analysis.get("part");
        return Some(match part {
            Some(p) if *p == 1 => "analysis[part1]",
            Some(p) if *p == 2 => "analysis[part2]",
            _ => "analysis",
        });
    }
    if let Some(anon_analysis) = entry.get("AnonAnalysis") {
        return match anon_analysis.get("part").filter(|p| !p.is_null()) {
            None => Some("anon_analysis"),
            Some(p) if *p == 0 => Some("anon_analysis"),
            Some(p) if *p == 1 => Some("anon_analysis[part1]"),
            Some(p) if *p == 2 => Some("anon_analysis[part2]"),
            Some(_) => None,
        };
    }
    if entry.get("ActionExecution").is_some() {
        return Some("action");
    }
    if entry.get("FinalMaterialization").is_some() {
        return Some("materialization");
    }
    None
}
