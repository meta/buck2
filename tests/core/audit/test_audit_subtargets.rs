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

use buck2_e2e::prelude::*;
use regex::Regex;
use serde_json::json;

fn extract_configuration(s: &str) -> Vec<String> {
    static CONFIGURATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((.*?)\)").unwrap());
    CONFIGURATION
        .captures_iter(s)
        .map(|c| c[1].to_owned())
        .collect()
}

#[buck_test]
async fn test_audit_subtargets_basic(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["subtargets", "//:no_subtargets"]).await?;
    assert_eq!(result.stdout, "");

    let result = buck.audit(&["subtargets", "//:foo"]).await?;
    assert_eq!(
        vec![
            "root//:foo[bar] (<unspecified>)",
            "root//:foo[baz] (<unspecified>)",
        ],
        result.stdout.lines().collect::<Vec<_>>()
    );
    Ok(())
}

#[buck_test]
async fn test_audit_subtargets_of_subtarget(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.audit(&["subtargets", "//:nested[sub1]"]).await?;
    assert_eq!(
        vec![
            "root//:nested[sub1][sub2] (<unspecified>)",
            "root//:nested[sub1][sub3] (<unspecified>)",
        ],
        result.stdout.lines().collect::<Vec<_>>()
    );

    let result = buck.audit(&["subtargets", "//:nested[sub4]"]).await?;
    assert_eq!(result.stdout, "");
    Ok(())
}

#[buck_test]
async fn test_audit_subtargets_shallow(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["subtargets", "//:deeply_nested", "--shallow"])
        .await?;
    assert_eq!(
        vec![
            "root//:deeply_nested[sub1] (<unspecified>)",
            "root//:deeply_nested[sub2] (<unspecified>)",
        ],
        result.stdout.lines().collect::<Vec<_>>()
    );

    let result = buck.audit(&["subtargets", "//:deeply_nested"]).await?;
    assert_eq!(
        vec![
            "root//:deeply_nested[sub1] (<unspecified>)",
            "root//:deeply_nested[sub2] (<unspecified>)",
            "root//:deeply_nested[sub2][sub3] (<unspecified>)",
            "root//:deeply_nested[sub2][sub3][sub4] (<unspecified>)",
            "root//:deeply_nested[sub2][sub5] (<unspecified>)",
        ],
        result.stdout.lines().collect::<Vec<_>>()
    );

    let result = buck
        .audit(&["subtargets", "//:deeply_nested[sub2]", "--shallow"])
        .await?;
    assert_eq!(
        vec![
            "root//:deeply_nested[sub2][sub3] (<unspecified>)",
            "root//:deeply_nested[sub2][sub5] (<unspecified>)",
        ],
        result.stdout.lines().collect::<Vec<_>>()
    );

    let result = buck
        .audit(&["subtargets", "//:deeply_nested[sub2]"])
        .await?;
    assert_eq!(
        vec![
            "root//:deeply_nested[sub2][sub3] (<unspecified>)",
            "root//:deeply_nested[sub2][sub3][sub4] (<unspecified>)",
            "root//:deeply_nested[sub2][sub5] (<unspecified>)",
        ],
        result.stdout.lines().collect::<Vec<_>>()
    );
    Ok(())
}

#[buck_test]
async fn test_audit_subtargets_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["subtargets", "//:no_subtargets", "--json"])
        .await?;
    golden(&result.stdout, "json/golden.has_no_subtargets.json");

    let result = buck.audit(&["subtargets", "//:foo", "--json"]).await?;
    golden(&result.stdout, "json/golden.basic.json");

    let result = buck
        .audit(&["subtargets", "//:nested[sub1]", "--json"])
        .await?;
    golden(&result.stdout, "json/golden.nested.json");

    let result = buck
        .audit(&["subtargets", "//:deeply_nested", "--json"])
        .await?;
    golden(&result.stdout, "json/golden.deeply_nested_basic.json");

    let result = buck
        .audit(&["subtargets", "//:deeply_nested[sub2]", "--json"])
        .await?;
    golden(&result.stdout, "json/golden.deeply_nested_subs_of_sub.json");
    Ok(())
}

#[buck_test]
async fn test_audit_subtarget_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["subtargets", "//:deeply_nested?root//:linux"])
        .await?;

    let cfgs = extract_configuration(&result.stdout);
    for cfg in &cfgs {
        let cfg = buck.audit_configurations(&[cfg]).await?;
        assert!(cfg.stdout.contains("root//:linux"));
    }

    let result = buck
        .audit(&[
            "subtargets",
            "//:nested?root//:macos",
            "//:deeply_nested?root//:linux",
            "--json",
        ])
        .await?;

    let cfgs = extract_configuration(&result.stdout);
    let [linux_cfg, macos_cfg] = cfgs.as_slice() else {
        panic!("expected two configurations, got {cfgs:?}");
    };

    let linux_cfg = buck.audit_configurations(&[linux_cfg]).await?;
    assert!(linux_cfg.stdout.contains("root//:linux"));

    let macos_cfg = buck.audit_configurations(&[macos_cfg]).await?;
    assert!(macos_cfg.stdout.contains("root//:macos"));

    let result = buck
        .audit(&[
            "subtargets",
            "//:foo?root//:macos+root//:arm",
            "//:foo?root//:arm+root//:macos",
            "--json",
        ])
        .await?;

    let cfgs = extract_configuration(&result.stdout);
    let [cfg] = cfgs.as_slice() else {
        panic!("expected one configuration, got {cfgs:?}");
    };

    let cfg = buck.audit_configurations(&[cfg]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_audit_subtarget_modifiers_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&[
            "subtargets",
            "//:deeply_nested",
            "--target-universe",
            "//:deeply_nested?root//:linux",
        ])
        .await?;

    let cfgs = extract_configuration(&result.stdout);
    for cfg in &cfgs {
        let cfg = buck.audit_configurations(&[cfg]).await?;
        assert!(cfg.stdout.contains("root//:linux"));
    }

    let result = buck
        .audit(&[
            "subtargets",
            "//:deeply_nested",
            "--target-universe",
            "//:deeply_nested?root//:linux,//:deeply_nested?root//:macos",
            "--json",
        ])
        .await?;

    let cfgs = extract_configuration(&result.stdout);
    assert_eq!(cfgs.len(), 2);

    // Audit each configuration to determine which is linux and which is macos
    let mut linux_cfg = None;
    let mut macos_cfg = None;

    for cfg in &cfgs {
        let audited_cfg = buck.audit_configurations(&[cfg]).await?;
        if audited_cfg.stdout.contains("root//:linux") {
            linux_cfg = Some(audited_cfg);
        } else if audited_cfg.stdout.contains("root//:macos") {
            macos_cfg = Some(audited_cfg);
        }
    }

    assert!(linux_cfg.is_some(), "Could not find linux configuration");
    assert!(macos_cfg.is_some(), "Could not find macos configuration");

    let result = buck
        .audit(&[
            "subtargets",
            "//:foo",
            "--target-universe",
            "//:foo?root//:macos+root//:arm,//:foo?root//:arm+root//:macos",
            "--json",
        ])
        .await?;

    let cfgs = extract_configuration(&result.stdout);
    let [cfg] = cfgs.as_slice() else {
        panic!("expected one configuration, got {cfgs:?}");
    };

    let cfg = buck.audit_configurations(&[cfg]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_audit_subtarget_fails_with_global_modifiers(buck: &Buck) {
    buck.audit(&[
        "subtargets",
        "--modifier",
        "root//:linux",
        "//:deeply_nested?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
    );

    buck.audit(&[
        "subtargets",
        "--modifier",
        "root//:linux",
        "//:deeply_nested",
        "--target-universe",
        "//:deeply_nested?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
    );
}

#[buck_test]
async fn test_audit_subtarget_fails_with_pattern_modifier_and_target_universe_modifier(
    buck: &Buck,
) {
    buck.audit(&[
        "subtargets",
        "//:deeply_nested?root//:linux",
        "--target-universe",
        "//:deeply_nested?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot use \?modifier syntax in target pattern expression with --target-universe flag",
    );
}

#[buck_test]
async fn test_audit_subtargets_shallow_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["subtargets", "//:deeply_nested", "--shallow", "--json"])
        .await?;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result.stdout)?,
        json!({
            "root//:deeply_nested (<unspecified>)": {"sub1": {}, "sub2": {}},
        })
    );
    let result = buck
        .audit(&[
            "subtargets",
            "//:deeply_nested[sub2]",
            "--shallow",
            "--json",
        ])
        .await?;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result.stdout)?,
        json!({
            "root//:deeply_nested[sub2] (<unspecified>)": {"sub3": {}, "sub5": {}},
        })
    );
    Ok(())
}
