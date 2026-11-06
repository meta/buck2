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

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::replace_hash;
use serde_json::Value;

#[buck_test]
async fn test_configuration_transition_rule_cquery(buck: &Buck) -> anyhow::Result<()> {
    // For the reference, cquery output is: P467297091. Note the "forward" node.
    let result = buck.cquery(&["deps(root//:the-test)"]).await?;
    // Watchos resource should be present twice: as forward and as transitioned.
    assert_eq!(result.stdout.matches(":watchos-resource").count(), 2);
    // No transition for default resource, so it appears once in cquery output.
    assert_eq!(result.stdout.matches(":default-resource").count(), 1);
    Ok(())
}

#[buck_test]
async fn test_configuration_transition_rule_cquery_actual_attr(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "--target-platforms=root//:iphoneos-p",
            "root//:watchos-resource",
            "--output-attribute=actual",
        ])
        .await?;
    let q: BTreeMap<String, serde_json::Map<String, Value>> = serde_json::from_str(&result.stdout)?;

    // Each key in the JSON output is a different configuration of the same rule `watchos-resource`
    let configuration_default = "root//:watchos-resource (<transitioned-to-watch>#<HASH>)";
    let configuration_transition = "root//:watchos-resource (root//:iphoneos-p#<HASH>)";
    let configurations: Vec<String> = q.keys().map(|c| replace_hash(c)).collect();
    assert!(configurations.iter().any(|c| c == configuration_default));
    assert!(configurations.iter().any(|c| c == configuration_transition));

    let config_default_has_attribute_actual = q.values().any(|attributes| {
        attributes
            .get("actual")
            .is_some_and(|actual| !actual.is_null() && actual != "")
    });
    let config_transition_has_no_attributes = q.values().any(|attributes| attributes.is_empty());

    assert!(config_default_has_attribute_actual);
    assert!(config_transition_has_no_attributes);
    Ok(())
}

#[buck_test]
async fn test_configuration_transition_rule_build(buck: &Buck) -> anyhow::Result<()> {
    // Rule implementations do the assertions.
    buck.build(&["root//:the-test"]).await?;
    Ok(())
}

#[buck_test]
async fn test_configuration_transition_yields_multiple_configurations_created_events(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&["root//:the-test"]).await?;
    let configuration_created_events = filter_events(
        buck,
        &[
            "Event",
            "data",
            "Instant",
            "data",
            "ConfigurationCreated",
            "cfg",
        ],
    )
    .await?;

    assert_eq!(configuration_created_events.len(), 2);
    let configuration_names: Vec<&str> = configuration_created_events
        .iter()
        .map(|cfg| cfg["full_name"].as_str().expect("a `full_name`"))
        .collect();
    assert!(configuration_names[0].starts_with("root//:iphoneos-p"));
    assert!(configuration_names[1].starts_with("<transitioned-to-watch>"));
    Ok(())
}
