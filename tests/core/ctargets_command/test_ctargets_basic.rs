/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_hash;
use serde_json::Value;
use serde_json::json;

#[buck_test]
async fn test_ctargets_basic(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&["root//:gum", "--target-platforms=root//:p"])
        .await?;
    let lines: Vec<&str> = result.stdout.lines().collect();
    let [line] = lines.as_slice() else {
        panic!("expected one line: {lines:?}");
    };
    let line = replace_hash(line);
    assert_eq!(line, "root//:gum (root//:p#<HASH>)");
    Ok(())
}

/// Panics unless the target `output` has the attribute `key`.
#[track_caller]
fn attribute<'a>(output: &'a Value, key: &str) -> &'a Value {
    output
        .get(key)
        .unwrap_or_else(|| panic!("no `{key}` in {output}"))
}

#[buck_test]
async fn test_ctargets_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.ctargets(&["root//:chocolate", "--json"]).await?;

    let outputs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    let [output] = outputs.as_slice() else {
        panic!("expected one target: {outputs:?}");
    };

    attribute(output, "buck.type");
    attribute(output, "buck.deps");
    attribute(output, "buck.inputs");
    attribute(output, "buck.package");
    attribute(output, "name");
    assert_eq!(attribute(output, "default_target_platform"), "root//:p");
    attribute(output, "visibility");
    attribute(output, "within_view");
    Ok(())
}

#[buck_test]
async fn test_ctargets_multi_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.ctargets(&["root//:", "--json"]).await?;

    let outputs: Vec<Value> = serde_json::from_str(&result.stdout)?;

    assert_eq!(outputs.len(), 3);

    for output in &outputs {
        attribute(output, "buck.type");
        attribute(output, "buck.deps");
        attribute(output, "buck.inputs");
        attribute(output, "buck.package");

        let name = attribute(output, "name");
        if name == "chocolate" {
            assert_eq!(attribute(output, "default_target_platform"), "root//:p");
        }

        attribute(output, "visibility");
        attribute(output, "within_view");
    }
    Ok(())
}

/// The attributes of each target, in the order `ctargets --json` printed them.
fn printed_names(stdout: &str) -> Vec<Vec<&str>> {
    let mut targets = Vec::new();
    for line in stdout.lines() {
        if line == "  {" {
            targets.push(Vec::new());
        } else if let Some(attribute) = line.strip_prefix("    \"") {
            let name = attribute.split('"').next().expect("should have a name");
            targets
                .last_mut()
                .expect("should be in a target")
                .push(name);
        }
    }
    targets
}

#[buck_test]
async fn test_ctargets_output_attribute_selects_from_all_attributes(
    buck: &Buck,
) -> anyhow::Result<()> {
    let all = buck.ctargets(&["root//:", "--json"]).await?.stdout;
    let all_names = printed_names(&all);
    let all: Vec<serde_json::Map<String, Value>> = serde_json::from_str(&all)?;
    assert_eq!(3, all.len());

    for names in [
        &["buck.type"][..],
        &["buck.deps", "buck.inputs", "buck.package"],
        &["name", "visibility", "buck.type"],
        &["default_target_platform"],
        &["no_such_attribute"],
    ] {
        let patterns: Vec<String> = names
            .iter()
            .map(|name| format!("--output-attribute=^{}$", name.replace('.', "\\.")))
            .collect();
        let mut args = vec!["root//:", "--json"];
        args.extend(patterns.iter().map(String::as_str));
        let selected = buck.ctargets(&args).await?.stdout;

        let expected: Vec<serde_json::Map<String, Value>> = all
            .iter()
            .map(|attributes| {
                attributes
                    .iter()
                    .filter(|(name, _)| names.contains(&name.as_str()))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .collect();
        assert_eq!(
            expected,
            serde_json::from_str::<Vec<serde_json::Map<String, Value>>>(&selected)?,
            "{names:?}"
        );

        // And in the order they have among all the attributes.
        let expected: Vec<Vec<&str>> = all_names
            .iter()
            .map(|target| {
                let selected = target.iter().filter(|name| names.contains(name));
                selected.copied().collect()
            })
            .collect();
        assert_eq!(expected, printed_names(&selected), "{names:?}");
    }
    Ok(())
}

/// An attribute that cannot be printed fails the command only if it is asked for.
#[buck_test]
async fn test_ctargets_output_attribute_skips_what_it_cannot_print(
    buck: &Buck,
) -> anyhow::Result<()> {
    let target = "root//unprintable:numbered";
    let error = "Cannot serialize dict attr with non-string key type";
    buck.ctargets(&[target, "--json"])
        .await
        .expect_failure()
        .assert_stderr_matches(error);
    buck.ctargets(&[target, "--json", "--output-attribute=^by_number$"])
        .await
        .expect_failure()
        .assert_stderr_matches(error);

    let result = buck
        .ctargets(&[target, "--json", "--output-attribute=^name$"])
        .await?;
    let outputs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    assert_eq!(outputs, [json!({"name": "numbered"})]);
    Ok(())
}

#[buck_test]
async fn test_ctargets_output_attribute(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "root//:chocolate",
            "--output-attribute=default_*",
            "--output-attribute=name",
        ])
        .await?;

    let outputs: Vec<Value> = serde_json::from_str(&result.stdout)?;
    let [output] = outputs.as_slice() else {
        panic!("expected one target: {outputs:?}");
    };

    assert_eq!(
        &json!({
            "name": "chocolate",
            "default_target_platform": "root//:p",
        }),
        output
    );
    Ok(())
}
