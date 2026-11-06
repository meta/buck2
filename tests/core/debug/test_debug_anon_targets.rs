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

use anyhow::Context;
use buck2_e2e::golden::golden_replace_cfg_hash;
use buck2_e2e::prelude::*;
use serde_json::Map;
use serde_json::Value;

fn parse_entries(stdout: &str) -> anyhow::Result<Map<String, Value>> {
    match serde_json::from_str(stdout)? {
        Value::Object(entries) => Ok(entries),
        other => anyhow::bail!("Expected a JSON object, got {other}"),
    }
}

fn requested_by(entry: &Value) -> anyhow::Result<Vec<&str>> {
    entry["buck.requested_by"]
        .as_array()
        .with_context(|| format!("No `buck.requested_by` in {entry}"))?
        .iter()
        .map(|r| r.as_str().context("A non-string requester"))
        .collect()
}

#[buck_test]
async fn test_anon_targets_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .debug(&["anon-targets", "//:root", "--json", "--with-requesters"])
        .await?;
    golden_replace_cfg_hash(
        &result.stdout,
        "anon_targets_json_with_requesters.golden.json",
    );
    let entries = parse_entries(&result.stdout)?;

    // `//:with_anon` requests `_anon_mid`, which in turn requests `_anon_leaf`.
    assert_eq!(entries.len(), 2, "{entries:?}");
    // Entries are keyed by the anon target's `BaseDeferredKey` rendering, which each
    // entry also carries in `buck.key`.
    for (key, entry) in &entries {
        assert_eq!(entry["buck.key"], *key, "{entries:?}");
        assert!(key.contains("(anon: "), "{entries:?}");
    }
    let by_rule: BTreeMap<&str, &Value> = entries
        .values()
        .map(|entry| {
            let rule_type = entry["buck.type"].as_str().context("No `buck.type`")?;
            Ok((rule_type.rsplit(':').next().unwrap_or(rule_type), entry))
        })
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(
        by_rule.keys().copied().collect::<Vec<_>>(),
        ["_anon_leaf", "_anon_mid"],
        "{entries:?}"
    );

    let mid = by_rule["_anon_mid"];
    assert_eq!(mid["buck.variant"], "bzl");
    assert_eq!(mid["level"], 1);
    assert!(
        requested_by(mid)?
            .iter()
            .any(|requester| requester.contains("with_anon")),
        "{mid}"
    );

    // The leaf was requested by the mid anon target, not by any configured target.
    let leaf = by_rule["_anon_leaf"];
    assert_eq!(leaf["suffix"], "from-mid");
    assert!(
        requested_by(leaf)?
            .iter()
            .any(|requester| requester.contains("_anon_mid")),
        "{leaf}"
    );
    assert!(
        !requested_by(leaf)?
            .iter()
            .any(|requester| requester.contains("with_anon")),
        "{leaf}"
    );
    Ok(())
}

#[buck_test]
async fn test_anon_targets_output_attribute(buck: &Buck) -> anyhow::Result<()> {
    // `-a` filters attributes with regexes in search mode and implies JSON.
    let result = buck
        .debug(&["anon-targets", "//:root", "-a", "^level$"])
        .await?;
    let entries = parse_entries(&result.stdout)?;
    assert_eq!(entries.len(), 2, "{entries:?}");
    let mut attr_sets: Vec<Vec<&str>> = entries
        .values()
        .map(|entry| {
            let mut attrs: Vec<&str> = entry
                .as_object()
                .context("An entry that is not an object")?
                .keys()
                .map(String::as_str)
                .collect();
            attrs.sort();
            Ok(attrs)
        })
        .collect::<anyhow::Result<_>>()?;
    attr_sets.sort();
    assert_eq!(attr_sets, [vec![], vec!["level"]], "{entries:?}");
    Ok(())
}

#[buck_test]
async fn test_anon_targets_basic_attributes(buck: &Buck) -> anyhow::Result<()> {
    // `-B` selects the user-suppliable attrs plus `buck.package` and `buck.type`.
    let result = buck.debug(&["anon-targets", "//:root", "-B"]).await?;
    golden_replace_cfg_hash(&result.stdout, "anon_targets_basic_attributes.golden.json");
    let entries = parse_entries(&result.stdout)?;
    assert_eq!(entries.len(), 2, "{entries:?}");
    for entry in entries.values() {
        assert!(entry.get("buck.type").is_some(), "{entry}");
        assert!(entry.get("buck.package").is_some(), "{entry}");
        assert!(
            entry.get("buck.execution_configuration").is_none(),
            "{entry}"
        );
    }
    assert!(
        entries.values().any(|entry| entry.get("level").is_some()),
        "{entries:?}"
    );
    assert!(
        entries.values().any(|entry| entry.get("suffix").is_some()),
        "{entries:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_anon_targets_text(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.debug(&["anon-targets", "//:root"]).await?;
    golden_replace_cfg_hash(&result.stdout, "anon_targets_text.golden.txt");
    assert!(result.stdout.contains("_anon_mid"));
    assert!(result.stdout.contains("_anon_leaf"));
    // Entries are headed by the anon target's `BaseDeferredKey` rendering.
    assert!(result.stdout.contains("(anon: "));
    // Without --with-requesters, requester edges are not printed.
    assert!(!result.stdout.contains("requested_by"));
    Ok(())
}

#[buck_test]
async fn test_anon_targets_none(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.debug(&["anon-targets", "//:no_anon"]).await?;
    assert_eq!(result.stdout.trim(), "");
    Ok(())
}

#[buck_test]
async fn test_anon_targets_requires_recording(buck: &Buck) -> anyhow::Result<()> {
    // Recording is on by default; turning it off must produce a hint rather
    // than silently-empty output.
    std::fs::write(
        buck.cwd().join(".bucksettings.toml"),
        "[analysis]\nrecord_requested_anon_targets = false\n",
    )?;
    buck.debug(&["anon-targets", "//:root"])
        .await
        .expect_failure()
        .assert_stderr_matches("Anon target recording is disabled");
    Ok(())
}
