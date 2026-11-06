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

/// What each `(...)` on a line of `s` holds, as `re.findall(r"\((.*?)\)", s)` finds them.
fn extract_configuration(s: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let after = &rest[open + 1..];
        let line = after.split('\n').next().unwrap_or_default();
        match line.find(')') {
            Some(close) => {
                found.push(&after[..close]);
                rest = &after[close + 1..];
            }
            None => rest = after,
        }
    }
    found
}

#[buck_test]
async fn test_cquery_fails_with_modifier_syntax_in_query(buck: &Buck) {
    buck.cquery(&["root//:target?modifier"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"root//:target\?modifier\n\s*\^");
}

#[buck_test]
async fn test_cquery_fails_with_global_modifier(buck: &Buck) {
    buck.cquery(&[
        "root//:target",
        "--modifier",
        "root//:macos",
        "--target-universe",
        "root//:target?root//:arm",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
    );
}

#[buck_test]
async fn test_cquery_with_single_universe_single_modifier(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//:target",
            "--target-universe",
            "root//:target?root//:macos",
        ])
        .await?;

    let [configuration] = extract_configuration(&result.stdout)[..] else {
        panic!("Expected one configuration in {}", result.stdout);
    };

    let cfg = buck.audit_configurations(&[configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_cquery_with_single_universe_multiple_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//:target",
            "--target-universe",
            "root//:target?root//:macos+root//:arm",
        ])
        .await?;

    let [configuration] = extract_configuration(&result.stdout)[..] else {
        panic!("Expected one configuration in {}", result.stdout);
    };

    let cfg = buck.audit_configurations(&[configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    assert!(cfg.stdout.contains("root//:arm"));
    Ok(())
}

#[buck_test]
async fn test_cquery_with_multiple_universes_single_modifier(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//:target",
            "--target-universe",
            "root//:target?root//:macos,root//:target?root//:linux",
        ])
        .await?;

    let [linux_configuration, mac_configuration] = extract_configuration(&result.stdout)[..] else {
        panic!("Expected two configurations in {}", result.stdout);
    };

    let linux_cfg = buck.audit_configurations(&[linux_configuration]).await?;
    assert!(linux_cfg.stdout.contains("root//:linux"));

    let mac_cfg = buck.audit_configurations(&[mac_configuration]).await?;
    assert!(mac_cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_cquery_with_multiple_universes_multiple_modifier(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//:target",
            "--target-universe",
            "root//:target?root//:macos+root//:arm,root//:target?root//:linux+root//:x86",
        ])
        .await?;

    let [mac_configuration, linux_configuration] = extract_configuration(&result.stdout)[..] else {
        panic!("Expected two configurations in {}", result.stdout);
    };

    let mac_cfg = buck.audit_configurations(&[mac_configuration]).await?;
    assert!(mac_cfg.stdout.contains("root//:macos"));
    assert!(mac_cfg.stdout.contains("root//:arm"));

    let linux_cfg = buck.audit_configurations(&[linux_configuration]).await?;
    assert!(linux_cfg.stdout.contains("root//:linux"));
    assert!(linux_cfg.stdout.contains("root//:x86"));
    Ok(())
}

#[buck_test]
async fn test_cquery_same_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//:target",
            "--target-universe",
            "root//:target?root//:macos,root//:target?root//:macos",
        ])
        .await?;

    let [configuration] = extract_configuration(&result.stdout)[..] else {
        panic!("Expected one configuration in {}", result.stdout);
    };

    let cfg = buck.audit_configurations(&[configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_cquery_order_of_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "root//:target",
            "--target-universe",
            "root//:target?root//:linux+root//:macos",
        ])
        .await?;

    let [configuration] = extract_configuration(&result.stdout)[..] else {
        panic!("Expected one configuration in {}", result.stdout);
    };

    let cfg = buck.audit_configurations(&[configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

// There should be an error thrown whenever ?modifier syntax is used outside of a target universe.
// However, the ? character can still show up in regex expressions and those should still be considered valid queries.
#[buck_test]
async fn test_cquery_with_attrregexfilter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&["attrregexfilter(attribute, 'test_greedys?', root//:attr_regex)"])
        .await?;

    assert!(result.stdout.contains("root//:attr_regex"));

    let result = buck
        .cquery(&[
            "attrregexfilter(attribute, 'test_greedys?', root//:attr_regex)",
            "--target-universe",
            "root//:attr_regex?root//:macos",
        ])
        .await?;

    let [configuration] = extract_configuration(&result.stdout)[..] else {
        panic!("Expected one configuration in {}", result.stdout);
    };

    let cfg = buck.audit_configurations(&[configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}
