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
use std::collections::BTreeSet;

use buck2_e2e::golden::golden_replace_cfg_hash;
use buck2_e2e::json::Json;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

async fn cquery_node(buck: &Buck, target: &str, args: &[&str]) -> anyhow::Result<Value> {
    let mut cquery_args = vec!["--output-all-attributes", target];
    cquery_args.extend(args);
    let result = buck.cquery(&cquery_args).await?;
    let nodes: BTreeMap<String, Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(nodes.len(), 1, "{nodes:?}");
    Ok(nodes.into_values().next().unwrap())
}

/// Strips the configuration hash from a platform reported by cquery.
fn platform(platform: &str) -> &str {
    platform.split_once('#').map_or(platform, |(p, _)| p)
}

/// The platform of a cquery node's default execution group, without its hash.
fn execution_platform(node: &Value) -> &str {
    platform(
        node["buck.execution_platform"]
            .as_str()
            .expect("a `buck.execution_platform`"),
    )
}

/// The platforms of a cquery node's execution groups, without their hashes, by group.
fn exec_group_platforms(node: &Value) -> BTreeMap<&str, &str> {
    node["buck.exec_group_platforms"]
        .as_object()
        .expect("a `buck.exec_group_platforms`")
        .iter()
        .map(|(group, p)| (group.as_str(), platform(p.as_str().expect("a platform"))))
        .collect()
}

/// The platforms, without their hashes, of the lines of a cquery `deps()` that start with
/// `prefix`.
fn dep_platforms<'a>(stdout: &'a str, prefix: &str) -> Vec<&'a str> {
    stdout
        .lines()
        .filter(|dep| dep.starts_with(prefix))
        .map(|dep| {
            platform(
                dep.split(' ')
                    .nth(1)
                    .expect("a configuration")
                    .trim_matches(['(', ')']),
            )
        })
        .collect()
}

/// The `buck.deps` of the one node that `cquery --output-attribute=buck.deps --json` printed.
fn single_node_deps(stdout: &str) -> anyhow::Result<Vec<String>> {
    let nodes: BTreeMap<String, Value> = serde_json::from_str(stdout)?;
    assert_eq!(nodes.len(), 1, "{nodes:?}");
    let node = nodes.into_values().next().unwrap();
    Ok(serde_json::from_value(node["buck.deps"].clone())?)
}

/// Whether a `buck2 log what-ran` entry is an action of `category`: build action identities have
/// the form `<target> (<category> ...)`.
fn has_category(entry: &Value, category: &str) -> bool {
    let identity = entry["identity"].as_str().expect("an `identity`");
    identity.contains(&format!("({category} ")) || identity.contains(&format!("({category})"))
}

fn executor(entry: &Value) -> &str {
    entry["reproducer"]["executor"]
        .as_str()
        .expect("an executor")
}

/// The executors of the `buck2 log what-ran` entries of each category.
fn executors_by_category<'a>(
    entries: &'a [Value],
    categories: &[&'static str],
) -> BTreeMap<&'static str, Vec<&'a str>> {
    categories
        .iter()
        .map(|category| {
            let executors = entries
                .iter()
                .filter(|e| has_category(e, category))
                .map(executor)
                .collect();
            (*category, executors)
        })
        .collect()
}

/// The executors of the test runs among the `buck2 log what-ran` entries.
fn test_run_executors(entries: &[Value]) -> Vec<&str> {
    entries
        .iter()
        .filter(|e| e["reason"] == "test.run")
        .map(executor)
        .collect()
}

#[buck_test]
async fn test_toolchains_select_only_their_group_platform(buck: &Buck) -> anyhow::Result<()> {
    let node = cquery_node(buck, "root//groups:bundle", &[]).await?;
    // The macOS-only and remote-only toolchains constrain only their own groups, so the default
    // group has no constraints and resolves to the first platform.
    assert_eq!(execution_platform(&node), "root//config:linux_local");
    assert_eq!(
        exec_group_platforms(&node),
        BTreeMap::from([
            ("Resources", "root//config:macos_local"),
            ("Package", "root//config:linux_remote"),
        ])
    );
    Ok(())
}

#[buck_test]
async fn test_group_toolchain_deps_use_group_platform(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["deps(root//groups:bundle, 2)"]).await?;
    let deps: Vec<&str> = result.stdout.lines().collect();
    assert!(
        deps.iter()
            .any(|dep| dep.starts_with("root//groups:macos_tool (root//config:macos_local")),
        "{deps:?}"
    );
    assert!(
        deps.iter()
            .any(|dep| dep.starts_with("root//groups:remote_tool (root//config:linux_remote")),
        "{deps:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_exec_deps_select_only_their_group_platform(buck: &Buck) -> anyhow::Result<()> {
    let node = cquery_node(buck, "root//groups:binary", &[]).await?;
    assert_eq!(execution_platform(&node), "root//config:linux_remote");
    // `Unused` has no deps, so it resolves to the first platform.
    assert_eq!(
        exec_group_platforms(&node),
        BTreeMap::from([
            ("Link", "root//config:linux_local"),
            ("Unused", "root//config:linux_local"),
        ])
    );
    Ok(())
}

#[buck_test]
async fn test_exec_dep_in_two_groups_is_configured_for_each(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["deps(root//groups:binary, 1)"]).await?;
    let mut any_tool = dep_platforms(&result.stdout, "root//groups:any_tool ");
    any_tool.sort();
    assert_eq!(
        any_tool,
        ["root//config:linux_local", "root//config:linux_remote"]
    );
    Ok(())
}

#[buck_test]
async fn test_dep_configured_identically_in_two_groups_is_one_dep(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//groups:same_platform",
            "--output-attribute=buck.deps",
            "--json",
        ])
        .await?;
    let deps = single_node_deps(&result.stdout)?;
    let any_tool: Vec<&String> = deps
        .iter()
        .filter(|d| d.starts_with("root//groups:any_tool "))
        .collect();
    assert_eq!(any_tool.len(), 1, "{deps:?}");
    Ok(())
}

#[buck_test]
async fn test_toolchain_configured_identically_in_two_groups_is_one_dep(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//groups:same_platform_toolchain",
            "--output-attribute=buck.deps",
            "--json",
        ])
        .await?;
    let deps = single_node_deps(&result.stdout)?;
    let toolchain: Vec<&String> = deps
        .iter()
        .filter(|d| d.starts_with("root//groups:any_toolchain "))
        .collect();
    assert_eq!(toolchain.len(), 1, "{deps:?}");
    Ok(())
}

#[buck_test]
async fn test_group_exec_dep_passed_to_anon_target(buck: &Buck) -> anyhow::Result<()> {
    buck.aquery(&["root//anon_tool:t"]).await?;
    Ok(())
}

#[buck_test]
async fn test_audit_reports_skipped_platforms_per_group(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "execution-platform-resolution",
            "root//groups:binary",
            "root//groups:bundle",
            "root//groups:same_platform",
            "root//groups:plugin_user",
        ])
        .await?;
    golden_replace_cfg_hash(&result.stdout, "audit_groups.txt.golden");
    Ok(())
}

#[buck_test]
async fn test_audit_explains_group_without_platform(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "execution-platform-resolution",
            "root//errors/no_group_platform:t",
        ])
        .await?;
    golden_replace_cfg_hash(&result.stdout, "audit_no_group_platform.txt.golden");
    Ok(())
}

#[buck_test]
async fn test_audit_reports_group_diagnosis_error_under_group(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "execution-platform-resolution",
            "root//errors/group_diagnosis_error:t",
        ])
        .await?;
    golden_replace_cfg_hash(&result.stdout, "audit_group_diagnosis_error.txt.golden");
    Ok(())
}

#[buck_test]
async fn test_audit_reports_default_diagnosis_error_under_default(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "execution-platform-resolution",
            "root//errors/default_diagnosis_error:t",
        ])
        .await?;
    golden_replace_cfg_hash(&result.stdout, "audit_default_diagnosis_error.txt.golden");
    Ok(())
}

#[buck_test]
async fn test_audit_explains_group_without_platform_error_fallback(
    buck: &Buck,
) -> anyhow::Result<()> {
    // With `fallback = "error"` the group's resolution fails configuration, and the error points
    // to this audit command.
    let result = buck
        .audit(&[
            "execution-platform-resolution",
            "root//errors/no_group_platform:t",
            "-c",
            "build.execution_platforms=root//config:platforms_error_fallback",
        ])
        .await?;
    golden_replace_cfg_hash(
        &result.stdout,
        "audit_no_group_platform_error_fallback.txt.golden",
    );
    Ok(())
}

#[buck_test]
async fn test_rule_without_groups_has_no_group_platforms(buck: &Buck) -> anyhow::Result<()> {
    let node = cquery_node(buck, "root//groups:plain", &[]).await?;
    assert!(node.get("buck.execution_platform").is_some());
    assert!(node.get("buck.exec_group_platforms").is_none());
    Ok(())
}

// Runs `python3` tool scripts locally and on a Linux RE platform.
#[buck_test(skip_for_os = ["windows"])]
async fn test_actions_run_on_group_platform(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//groups:bundle",
        "root//groups:binary",
        "--no-remote-cache",
    ])
    .await?;
    let entries = read_what_ran(buck, &[]).await?;
    let expected = [
        ("compile_resources", "Local"),
        ("package", "Re"),
        ("compile", "Re"),
        ("explicit_default", "Re"),
        ("link", "Local"),
        ("unused_group", "Local"),
    ];
    let categories: Vec<&str> = expected.iter().map(|(category, _)| *category).collect();
    assert_eq!(
        executors_by_category(&entries, &categories),
        expected
            .iter()
            .map(|(category, executor)| (*category, vec![*executor]))
            .collect::<BTreeMap<_, _>>(),
        "{entries:#?}"
    );
    Ok(())
}

#[buck_test]
async fn test_aquery_shows_execution_group(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .aquery(&[
            "kind(run, deps(root//groups:binary))",
            "--output-attribute=category",
            "--output-attribute=buck.exec_group",
        ])
        .await?;
    let actions: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(&result.stdout)?;
    let group_by_category: BTreeMap<&str, &str> = actions
        .values()
        .map(|a| (a["category"].as_str(), a["buck.exec_group"].as_str()))
        .collect();
    assert_eq!(
        group_by_category,
        BTreeMap::from([
            ("compile", "DefaultExecGroup"),
            ("explicit_default", "DefaultExecGroup"),
            ("link", "Link"),
            ("unused_group", "Unused"),
        ])
    );
    Ok(())
}

#[buck_test]
async fn test_aquery_shows_execution_group_of_write(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .aquery(&[
            "kind(write, deps(root//groups:binary))",
            "--output-attribute=identifier",
            "--output-attribute=buck.exec_group",
        ])
        .await?;
    let actions: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(&result.stdout)?;
    let group_by_identifier: BTreeMap<&str, Option<&str>> = actions
        .values()
        .map(|a| {
            (
                a["identifier"].as_str(),
                a.get("buck.exec_group").map(String::as_str),
            )
        })
        .collect();
    // The tools' rule declares no execution groups, so their scripts name none.
    assert_eq!(
        group_by_identifier,
        BTreeMap::from([("link_args.txt", Some("Link")), ("tool.py", None)]),
        "{group_by_identifier:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_aquery_execution_group_golden(buck: &Buck) -> anyhow::Result<()> {
    // Every action of `binary`, whose rule declares execution groups, names its group,
    // `DefaultExecGroup` included; the actions of the groupless `tool` rule name none.
    let result = buck
        .aquery(&[
            "kind('run|write', deps(root//groups:binary))",
            "--output-attribute=category",
            "--output-attribute=identifier",
            "--output-attribute=buck.exec_group",
        ])
        .await?;
    // The actions sorted, each with its attributes in buck2's order.
    let mut actions = Json::loads(&result.stdout)?;
    let Json::Object(entries) = &mut actions else {
        anyhow::bail!("The aquery output is not an object: {actions}");
    };
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    golden_replace_cfg_hash(
        &actions.dumps(Some(2), false),
        "aquery_exec_group.golden.json",
    );
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_action_execution_event_records_execution_group(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//groups:binary", "--no-remote-cache"])
        .await?;
    let ends = filter_events(
        buck,
        &["Event", "data", "SpanEnd", "data", "ActionExecution"],
    )
    .await?;
    let group_by_category: BTreeMap<&str, &str> = ends
        .iter()
        .filter(|end| {
            let category = end
                .pointer("/name/category")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("No `name.category` in {end}"));
            ["compile", "explicit_default", "link", "unused_group"].contains(&category)
        })
        .map(|end| {
            (
                end["name"]["category"].as_str().expect("a category"),
                end.get("exec_group")
                    .map_or("", |g| g.as_str().expect("a string `exec_group`")),
            )
        })
        .collect();
    // The default group is recorded as the empty string, which the event omits.
    assert_eq!(
        group_by_category,
        BTreeMap::from([
            ("compile", ""),
            ("explicit_default", ""),
            ("link", "Link"),
            ("unused_group", "Unused"),
        ])
    );
    Ok(())
}

// Runs `python3` tool scripts locally and on a Linux RE platform.
#[buck_test(skip_for_os = ["windows"])]
async fn test_dynamic_output_actions_run_on_group_platform(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//groups:dynamic_binary", "--no-remote-cache"])
        .await?;
    let entries = read_what_ran(buck, &[]).await?;
    assert_eq!(
        executors_by_category(&entries, &["dynamic_compile", "dynamic_link"]),
        BTreeMap::from([
            ("dynamic_compile", vec!["Re"]),
            ("dynamic_link", vec!["Local"])
        ]),
        "{entries:#?}"
    );
    Ok(())
}

#[buck_test]
async fn test_exec_compatible_with_attr_constrains_its_group(buck: &Buck) -> anyhow::Result<()> {
    // Constraint labels are also matched in the target configuration, as for the target's
    // `exec_compatible_with`, so these targets need a real target platform.
    let target_platform = "--target-platforms=root//config:linux_local";
    let macos = cquery_node(buck, "root//groups:link_on_macos", &[target_platform]).await?;
    assert_eq!(execution_platform(&macos), "root//config:linux_remote");
    assert_eq!(
        exec_group_platforms(&macos),
        BTreeMap::from([("Link", "root//config:macos_local")])
    );

    // The select keys on the target platform's `linux` constraint; without it `Link` would
    // resolve to `linux_local`, the first platform.
    let via_select = cquery_node(
        buck,
        "root//groups:link_on_macos_via_select",
        &[target_platform],
    )
    .await?;
    assert_eq!(
        exec_group_platforms(&via_select),
        BTreeMap::from([("Link", "root//config:macos_local")])
    );
    Ok(())
}

#[buck_test]
async fn test_audit_explains_group_constraints(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "execution-platform-resolution",
            "root//groups:link_on_macos_remote",
            "--target-platforms=root//config:linux_local",
            "-c",
            "build.execution_platforms=root//config:platforms_error_fallback",
        ])
        .await?;
    golden_replace_cfg_hash(&result.stdout, "audit_group_constraints.txt.golden");
    Ok(())
}

#[buck_test]
async fn test_run_in_group_without_platform(buck: &Buck) {
    buck.build(&[
        "root//errors/run_no_group_platform:t",
        "--target-platforms=root//config:linux_local",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Action with category `compile` and identifier `main` runs in execution group `G`, which has no compatible execution platform",
    );
}

#[buck_test]
async fn test_exec_compatible_with_attr_for_default_group(buck: &Buck) -> anyhow::Result<()> {
    let target_platform = "--target-platforms=root//config:linux_local";
    // Only the default group is constrained to macOS; `Link` still takes the first platform.
    let node = cquery_node(buck, "root//groups:default_on_macos", &[target_platform]).await?;
    assert_eq!(execution_platform(&node), "root//config:macos_local");
    assert_eq!(
        exec_group_platforms(&node),
        BTreeMap::from([("Link", "root//config:linux_local")])
    );

    let no_groups =
        cquery_node(buck, "root//groups:no_groups_on_remote", &[target_platform]).await?;
    assert_eq!(execution_platform(&no_groups), "root//config:linux_remote");
    Ok(())
}

#[buck_test]
async fn test_two_exec_compatible_with_attrs_for_default_group(buck: &Buck) {
    buck.uquery(&["root//errors/exec_compatible_with_default:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Execution group `DefaultExecGroup` has more than one `attrs.exec_compatible_with\(\)` attribute: `a` and `b`",
        );
}

#[buck_test]
async fn test_exec_compatible_with_attr_in_default_only(buck: &Buck) {
    buck.uquery(&["root//errors/exec_compatible_with_default_only:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"`attrs.default_only\(\)` cannot wrap `attrs.exec_compatible_with\(\)`",
        );
}

#[buck_test]
async fn test_two_exec_compatible_with_attrs_for_one_group(buck: &Buck) {
    buck.uquery(&["root//errors/exec_compatible_with_twice:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Execution group `G` has more than one `attrs.exec_compatible_with\(\)` attribute: `a` and `b`",
        );
}

#[buck_test]
async fn test_exec_compatible_with_attr_with_undeclared_group(buck: &Buck) {
    buck.uquery(&["root//errors/exec_compatible_with_undeclared:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Attribute `c` uses execution group `G`, which the rule does not declare",
        );
}

#[buck_test]
async fn test_arg_tools_select_only_their_group_platform(buck: &Buck) -> anyhow::Result<()> {
    let exe = cquery_node(buck, "root//groups:arg_exe", &[]).await?;
    // The remote-only compiler is in the default group and the local-only tool in `Link`; with
    // the tool in the default group, no platform would satisfy both.
    assert_eq!(execution_platform(&exe), "root//config:linux_remote");
    assert_eq!(
        exec_group_platforms(&exe),
        BTreeMap::from([("Link", "root//config:linux_local")])
    );

    let location = cquery_node(buck, "root//groups:arg_location", &[]).await?;
    assert_eq!(
        exec_group_platforms(&location),
        BTreeMap::from([("Link", "root//config:macos_local")])
    );
    Ok(())
}

#[buck_test]
async fn test_arg_tool_is_configured_for_its_group(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["deps(root//groups:arg_exe, 1)"]).await?;
    let local_tool = dep_platforms(&result.stdout, "root//groups:local_tool ");
    assert_eq!(local_tool, ["root//config:linux_local"]);
    Ok(())
}

#[buck_test]
async fn test_arg_tool_resolves_in_group_action(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .aquery(&[
            "kind(run, deps(root//groups:arg_exe))",
            "--output-attribute=category",
        ])
        .await?;
    let actions: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(&result.stdout)?;
    let categories: Vec<&str> = actions.values().map(|a| a["category"].as_str()).collect();
    assert_eq!(categories, ["arg_tool"]);
    Ok(())
}

#[buck_test]
async fn test_strip_cfg_of_arg_in_group(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["root//groups/strip_cfg.bxl:strip_cfg"]).await?;
    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        [
            r#""$(exe root//groups:local_tool)""#,
            r#""$(location_exec root//groups:macos_tool) $(location_toolchain root//groups:resource_toolchain)""#,
        ]
    );
    Ok(())
}

#[buck_test]
async fn test_arg_with_undeclared_group(buck: &Buck) {
    buck.uquery(&["root//errors/arg_undeclared:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Attribute `cmd` uses execution group `G`, which the rule does not declare",
        );
}

#[buck_test]
async fn test_arg_in_group_passed_to_anon_target(buck: &Buck) -> anyhow::Result<()> {
    // `G` and the default group resolve to the same platform, so `G`'s tool can run there.
    buck.aquery(&["root//errors/arg_in_anon:t"]).await?;
    // A rule with execution groups cannot be an anon target at all.
    buck.aquery(&["root//errors/arg_in_anon:grouped_anon"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Rule `grouped_anon` declares execution groups, which anon targets do not have",
        );
    Ok(())
}

// Runs a `python3` test locally; the default group would run it on a Linux RE platform.
#[buck_test(skip_for_os = ["windows"])]
async fn test_test_runs_on_its_group_platform(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["root//groups:group_test", "--no-remote-cache"])
        .await?;
    let entries = read_what_ran(buck, &[]).await?;
    assert_eq!(test_run_executors(&entries), ["Local"], "{entries:#?}");
    Ok(())
}

// Runs a `python3` test locally through a rule that re-creates the test info field by field.
#[buck_test(skip_for_os = ["windows"])]
async fn test_test_info_copied_field_by_field(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["root//groups:copied_group_test", "--no-remote-cache"])
        .await?;
    let entries = read_what_ran(buck, &[]).await?;
    assert_eq!(test_run_executors(&entries), ["Local"], "{entries:#?}");
    Ok(())
}

// Runs a `python3` test locally through a forwarding rule.
#[buck_test(skip_for_os = ["windows"])]
async fn test_test_info_forwarded_from_other_rule(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["root//groups:forwarded_group_test", "--no-remote-cache"])
        .await?;
    let entries = read_what_ran(buck, &[]).await?;
    assert_eq!(test_run_executors(&entries), ["Local"], "{entries:#?}");
    Ok(())
}

#[buck_test]
async fn test_test_group_without_platform_fails_when_run(buck: &Buck) -> anyhow::Result<()> {
    let target_platform = "--target-platforms=root//config:linux_local";
    buck.aquery(&["root//errors/test_group_no_platform:t", target_platform])
        .await?;
    buck.test(&["root//errors/test_group_no_platform:t", target_platform])
        .await
        .expect_failure()
        .assert_stderr_matches("No compatible execution platform");
    Ok(())
}

#[buck_test]
async fn test_test_info_without_executor_copied_field_by_field(buck: &Buck) -> anyhow::Result<()> {
    let target_platform = "--target-platforms=root//config:linux_local";
    buck.build(&[
        "root//errors/test_group_no_platform:copied",
        target_platform,
    ])
    .await?;
    buck.test(&["root//errors/test_group_no_platform:copied", target_platform])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "names execution group `TestRun`, but no executor was recorded for it when the test info was created",
        );
    Ok(())
}

#[buck_test]
async fn test_test_info_with_undeclared_group(buck: &Buck) {
    buck.test(&["root//errors/test_group_undeclared:t"])
        .await
        .expect_failure()
        .assert_stderr_matches("Execution group `G` is not declared by this rule");
}

#[buck_test]
async fn test_group_constraints_select_on_target_platform(buck: &Buck) -> anyhow::Result<()> {
    for (target_platform, link_platform) in [
        ("root//config:macos_local", "root//config:macos_local"),
        ("root//config:linux_local", "root//config:linux_local"),
    ] {
        let node = cquery_node(
            buck,
            "root//groups:link_follows_target_os",
            &[&format!("--target-platforms={target_platform}")],
        )
        .await?;
        assert_eq!(
            exec_group_platforms(&node),
            BTreeMap::from([("Link", link_platform)]),
            "{target_platform}"
        );
    }
    Ok(())
}

// Runs a `python3` tool locally in an anon target; the default group is a Linux RE platform.
#[buck_test(skip_for_os = ["windows"])]
async fn test_anon_target_runs_on_its_group_platform(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//groups:anon_in_group",
        "root//groups:anon_targets_in_group",
        "--no-remote-cache",
    ])
    .await?;
    let entries = read_what_ran(buck, &[]).await?;
    let anon_links: Vec<&str> = entries
        .iter()
        .filter(|e| has_category(e, "anon_link"))
        .map(executor)
        .collect();
    // Both targets create the same anon target, so it runs once.
    assert_eq!(anon_links, ["Local"], "{entries:#?}");
    Ok(())
}

#[buck_test]
async fn test_anon_target_shared_by_creators_with_different_resolutions(
    buck: &Buck,
) -> anyhow::Result<()> {
    let targets = ["root//anon_args:shared_a", "root//anon_args:shared_b"];
    let result = buck.build(&targets).await?;
    let report = result.build_report()?;
    let outputs = targets
        .iter()
        .map(|t| report.output_for_target(t))
        .collect::<anyhow::Result<BTreeSet<_>>>()?;
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    Ok(())
}

#[buck_test]
async fn test_groupless_creator_passes_arg_tool_of_other_platform(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Without execution groups the anon target does not check the platforms of arg tools.
    buck.build(&["root//anon_args:remote_arg_on_local"]).await?;
    Ok(())
}

#[buck_test]
async fn test_anon_target_of_rule_with_groups(buck: &Buck) {
    buck.aquery(&["root//errors/anon_rule_with_groups:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Rule `_grouped` declares execution groups, which anon targets do not have",
        );
}

#[buck_test]
async fn test_anon_target_rejects_arg_tool_of_other_platform(buck: &Buck) {
    buck.aquery(&["root//groups:anon_default_arg_in_link"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"`\$\(exe root//groups:remote_tool\)` refers to a tool built for execution platform `root//config:linux_remote`, but the anon target runs on `root//config:linux_local`",
        );
    buck.aquery(&["root//groups:anon_link_arg_in_default"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"`\$\(exe root//groups:local_tool\)` refers to a tool built for execution platform `root//config:linux_local`, but the anon target runs on `root//config:linux_remote`",
        );
}

// Runs a `python3` tool script locally.
#[buck_test(skip_for_os = ["windows"])]
async fn test_anon_target_runs_arg_tool_of_its_group(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//groups:anon_link_arg_in_link";
    let result = buck.build(&[target, "--no-remote-cache"]).await?;
    let output = result.build_report()?.output_for_target(target)?;
    assert_eq!(std::fs::read_to_string(output)?, "local_tool");
    let entries = read_what_ran(buck, &[]).await?;
    let executors: Vec<&str> = entries
        .iter()
        .filter(|e| has_category(e, "anon_arg"))
        .map(executor)
        .collect();
    assert_eq!(executors, ["Local"], "{entries:#?}");
    Ok(())
}

// Runs a `python3` tool script locally.
#[buck_test(skip_for_os = ["windows"])]
async fn test_anon_target_runs_toolchain_arg_tool_of_its_group(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//groups:anon_toolchain_arg_in_link";
    let result = buck.build(&[target, "--no-remote-cache"]).await?;
    let output = result.build_report()?.output_for_target(target)?;
    assert_eq!(std::fs::read_to_string(output)?, "local_tool");
    Ok(())
}

#[buck_test]
async fn test_anon_target_rejects_toolchain_arg_tool_of_other_platform(buck: &Buck) {
    buck.aquery(&["root//groups:anon_toolchain_arg_in_default"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"`\$\(exe root//groups:local_tool\)` refers to a tool built for execution platform `root//config:linux_local`, but the anon target runs on `root//config:linux_remote`",
        );
}

#[buck_test]
async fn test_anon_target_without_group_rejects_group_tool(buck: &Buck) {
    buck.aquery(&["root//groups:anon_without_group"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Exec deps and the current anon target must have the same execution platform resolution",
        );
}

#[buck_test]
async fn test_unbound_node_keeps_group_deps(buck: &Buck) -> anyhow::Result<()> {
    // The `select()` key `:constraint` is configured unbound, which resolves no execution
    // platforms. Its analysis reads its grouped exec dep and toolchain dep, so they must still
    // be deps.
    let result = buck
        .cquery(&[
            "root//unbound:t",
            "--target-platforms",
            "root//config:linux_local",
        ])
        .await?;
    assert!(
        result.stdout.starts_with("root//unbound:t "),
        "{}",
        result.stdout
    );
    Ok(())
}

#[buck_test]
async fn test_unbound_node_analyzes_group_exec_dep(buck: &Buck) -> anyhow::Result<()> {
    // Analysis of the unbound `:constraint` resolves its grouped exec dep and toolchain dep
    // against the unspecified execution platform resolution.
    buck.build(&[
        "root//unbound:t",
        "--target-platforms",
        "root//config:linux_local",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_attr_with_undeclared_group(buck: &Buck) {
    buck.uquery(&["root//errors/undeclared:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Attribute `_tool` uses execution group `G`, which the rule does not declare. Declared execution groups: \[\]",
        );
}

#[buck_test]
async fn test_attr_with_group_of_declared_name_from_other_file(buck: &Buck) {
    buck.uquery(&["root//errors/undeclared_same_name:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Attribute `_tool` uses execution group `G`, which the rule does not declare. Declared execution groups: \[`G`\]. The declared `G` is defined in `errors/undeclared_same_name/rules.bzl`, this one in `errors/same_name/other.bzl`",
        );
}

#[buck_test]
async fn test_default_group_listed(buck: &Buck) -> anyhow::Result<()> {
    let node = cquery_node(buck, "root//errors/default_listed:t", &[]).await?;
    assert!(node.get("buck.exec_group_platforms").is_none());
    Ok(())
}

#[buck_test]
async fn test_default_group_listed_twice(buck: &Buck) {
    buck.uquery(&["root//errors/default_twice:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Execution groups of a rule must have distinct names; `DefaultExecGroup` is listed more than once",
        );
}

#[buck_test]
async fn test_default_group_undeclared_dep(buck: &Buck) {
    buck.uquery(&["root//errors/default_undeclared_dep:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Attribute `_tool` uses the default execution group, which the rule does not declare",
        );
}

#[buck_test]
async fn test_default_group_undeclared_constraints(buck: &Buck) {
    buck.uquery(&["root//errors/default_undeclared_constraints:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Attribute `constraints` constrains the default execution group, which the rule does not declare",
        );
}

#[buck_test]
async fn test_default_group_undeclared_with_plugins(buck: &Buck) {
    buck.uquery(&["root//errors/plugins_default_undeclared:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "The rule uses plugins, which are dependencies in the default execution group, but does not declare it",
        );
}

#[buck_test]
async fn test_default_group_undeclared_run(buck: &Buck) {
    buck.aquery(&["root//errors/default_undeclared_run:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "The action runs in the default execution group, which the rule does not declare",
        );
}

#[buck_test]
async fn test_default_group_undeclared_write(buck: &Buck) {
    buck.aquery(&["root//errors/default_undeclared_write:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "The action runs in the default execution group, which the rule does not declare",
        );
}

#[buck_test]
async fn test_default_group_undeclared_arg(buck: &Buck) {
    buck.uquery(&["root//errors/default_undeclared_arg:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Attribute `cmd` uses the default execution group, which the rule does not declare",
        );
}

#[buck_test]
async fn test_arg_without_exec_group_holds_flags(buck: &Buck) -> anyhow::Result<()> {
    buck.uquery(&["root//errors/unmarked_arg:t"]).await?;
    Ok(())
}

#[buck_test]
async fn test_arg_without_exec_group_rejects_tool(buck: &Buck) {
    for (package, attr, macro_name) in [
        ("unmarked_arg_tool", "cmd", "exe"),
        ("unmarked_arg_select", "flags", "location_exec"),
    ] {
        buck.uquery(&[&format!("root//errors/{package}:t")])
            .await
            .expect_failure()
            .assert_stderr_matches(&format!(
                r"Target `root//errors/{package}:t`: attribute `{attr}` contains `\$\({macro_name} root//errors/{package}:tool\)`, a tool, in an `attrs\.arg\(\)` without `exec_group`"
            ));
    }
}

#[buck_test]
async fn test_mixed_arg_attrs_check_only_args_without_exec_group(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Tools in the args of `G` and of `DefaultExecGroup` are accepted, in values and defaults.
    buck.uquery(&["root//errors/mixed_args:t"]).await?;
    for (package, attr) in [
        ("mixed_args_tuple", "pair"),
        ("mixed_args_one_of", "either"),
        ("mixed_args_dict", "mapping"),
    ] {
        buck.uquery(&[&format!("root//errors/{package}:t")])
            .await
            .expect_failure()
            .assert_stderr_matches(&format!(
                r"Target `root//errors/{package}:t`: attribute `{attr}` contains `\$\(exe root//errors/{package}:tool\)`, a tool, in an `attrs\.arg\(\)` without `exec_group`"
            ));
    }
    buck.uquery(&["root//errors/mixed_args_default:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"The default of attribute `with_default` contains `\$\(exe root//errors/mixed_args_default:tool\)`, a tool, in an `attrs\.arg\(\)` without `exec_group`",
        );
    Ok(())
}

#[buck_test]
async fn test_arg_default_without_exec_group_rejects_tool(buck: &Buck) {
    buck.uquery(&["root//errors/unmarked_arg_default:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"The default of attribute `cmd` contains `\$\(exe root//errors/unmarked_arg_default:tool\)`, a tool, in an `attrs\.arg\(\)` without `exec_group`",
        );
}

#[buck_test]
async fn test_tool_attrs_without_exec_group(buck: &Buck) {
    for (package, attr, function) in [
        ("unmarked_exec_dep", "_tool", "exec_dep"),
        ("unmarked_toolchain_dep", "_toolchain", "toolchain_dep"),
        ("unmarked_nested", "tools", "exec_dep"),
    ] {
        buck.uquery(&[&format!("root//errors/{package}:t")])
            .await
            .expect_failure()
            .assert_stderr_matches(&format!(
                r"Attribute `{attr}` contains an `attrs\.{function}\(\)` without `exec_group`"
            ));
    }
}

#[buck_test]
async fn test_default_group_undeclared_anon_target(buck: &Buck) {
    buck.aquery(&["root//errors/default_undeclared_anon:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "The anon target uses the default execution group, which the rule does not declare",
        );
}

#[buck_test]
async fn test_default_group_undeclared_test(buck: &Buck) {
    // The test info fails when it is created, so a rule that returns it fails too.
    for target in ["t", "alias"] {
        buck.test(&[&format!("root//errors/default_undeclared_test:{target}")])
            .await
            .expect_failure()
            .assert_stderr_matches(
                "The test info uses the default execution group, which the rule does not declare. Set `exec_group` or `default_executor`",
            );
    }
}

#[buck_test]
async fn test_default_group_undeclared_test_with_only_executor_overrides(buck: &Buck) {
    buck.build(&["root//errors/default_undeclared_test:overrides_only"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "The test info uses the default execution group, which the rule does not declare. Set `exec_group` or `default_executor`",
        );
}

#[buck_test]
async fn test_default_group_undeclared_test_forwarded(buck: &Buck) {
    // The test info's creator has the default group, the tested target's rule does not.
    buck.test(&["root//errors/default_undeclared_test:grouped_alias"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"The test uses the default execution group, which the rule of `root//errors/default_undeclared_test:grouped_alias[^`]*` does not declare \(the test info may come from another rule that created it\)\. Set `exec_group` or `default_executor` where the test info is created, or add `DefaultExecGroup` to `exec_groups` of the rule of `root//errors/default_undeclared_test:grouped_alias",
        );
}

#[buck_test]
async fn test_default_group_undeclared_test_explicit_default(buck: &Buck) {
    buck.test(&["root//errors/default_undeclared_test:explicit_default"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "The test info uses the default execution group, which the rule does not declare. Set `exec_group` to a declared group",
        );
}

#[buck_test]
async fn test_duplicate_group(buck: &Buck) {
    buck.uquery(&["root//errors/duplicate:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Execution groups of a rule must have distinct names; `G` is listed more than once",
        );
}

#[buck_test]
async fn test_groups_with_same_name_from_different_files(buck: &Buck) {
    buck.uquery(&["root//errors/same_name:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Execution groups of a rule must have distinct names; `G` is listed more than once \(defined in `errors/same_name/rules.bzl` and in `errors/same_name/other.bzl`\)",
        );
}

#[buck_test]
async fn test_toolchain_rule_with_groups(buck: &Buck) {
    buck.uquery(&["root//errors/toolchain_rule:t"])
        .await
        .expect_failure()
        .assert_stderr_matches("Toolchain rules cannot use execution groups");
}

#[buck_test]
async fn test_toolchain_rule_with_default_group(buck: &Buck) {
    buck.uquery(&["root//errors/toolchain_default:t"])
        .await
        .expect_failure()
        .assert_stderr_matches("Toolchain rules cannot use execution groups");
}

#[buck_test]
async fn test_configuration_rule_with_groups(buck: &Buck) {
    buck.uquery(&["root//errors/configuration_rule:t"])
        .await
        .expect_failure()
        .assert_stderr_matches("Configuration rules cannot use execution groups");
}

#[buck_test]
async fn test_group_named_default(buck: &Buck) {
    buck.uquery(&["root//errors/named_default:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"`DefaultExecGroup` names the built-in default execution group and cannot be assigned an `exec_group\(\)`",
        );
}

#[buck_test]
async fn test_group_not_assigned_to_global(buck: &Buck) {
    buck.uquery(&["root//errors/unbound:t"])
        .await
        .expect_failure()
        .assert_stderr_matches("Execution group has not yet been assigned to a global");
}

#[buck_test]
async fn test_run_in_undeclared_group(buck: &Buck) {
    buck.build(&["root//errors/run_undeclared:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Execution group `Other` is not declared by this rule. Declared groups: \[`Declared`\]",
        );
}

#[buck_test]
async fn test_write_in_undeclared_group(buck: &Buck) {
    buck.build(&["root//errors/write_undeclared:t"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Execution group `Other` is not declared by this rule. Declared groups: \[`Declared`\]",
        );
}

#[buck_test]
async fn test_run_with_non_group_value(buck: &Buck) {
    buck.build(&["root//errors/run_wrong_type:t"])
        .await
        .expect_failure()
        .assert_stderr_matches("parameter `exec_group`.*ExecGroup");
}

#[buck_test]
async fn test_anon_target_in_group_of_unbound_node(buck: &Buck) {
    // An unbound node resolves no group: an anon target in a group fails at creation with the
    // error an action in a group fails with.
    for target in ["run", "anon"] {
        buck.build(&[
            &format!("root//errors/anon_unbound:{target}"),
            "--target-platforms=root//config:linux_local",
        ])
        .await
        .expect_failure()
        .assert_stderr_matches("Execution platform resolution was not performed");
    }
}

#[buck_test]
async fn test_anon_target_in_group_without_platform(buck: &Buck) {
    for target in ["t", "plural"] {
        buck.build(&[
            &format!("root//errors/anon_no_group_platform:{target}"),
            "--target-platforms=root//config:linux_local",
        ])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Anon target of rule `_anon` is created in execution group `G`, which has no compatible execution platform",
        );
    }
}
