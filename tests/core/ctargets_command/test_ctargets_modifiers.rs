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
use regex::Regex;

fn extract_configuration(s: &str) -> Vec<String> {
    Regex::new(r"\((.*?)\)")
        .unwrap()
        .captures_iter(s)
        .map(|c| c[1].to_owned())
        .collect()
}

#[buck_test]
async fn test_ctargets_modifier_single_pattern(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.ctargets(&["root//:target?root//:macos"]).await?;

    let configurations = extract_configuration(&result.stdout);
    let [configuration] = configurations.as_slice() else {
        panic!("expected one configuration: {configurations:?}");
    };

    let macos_cfg = buck.audit_configurations(&[configuration.as_str()]).await?;

    assert!(macos_cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_ctargets_modifier_multiple_patterns(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&[
            "root//:target?root//:macos",
            "root//:other_target?root//:macos",
        ])
        .await?;

    let configurations = extract_configuration(&result.stdout);
    let [target_configuration, other_configuration] = configurations.as_slice() else {
        panic!("expected two configurations: {configurations:?}");
    };

    let target_cfg = buck
        .audit_configurations(&[target_configuration.as_str()])
        .await?;
    assert!(target_cfg.stdout.contains("root//:macos"));

    let other_cfg = buck
        .audit_configurations(&[other_configuration.as_str()])
        .await?;
    assert!(other_cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_ctargets_modifier_multiple_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .ctargets(&["root//:target?root//:macos+root//:arm"])
        .await?;

    let configurations = extract_configuration(&result.stdout);
    let [configuration] = configurations.as_slice() else {
        panic!("expected one configuration: {configurations:?}");
    };

    let multi_cfg = buck.audit_configurations(&[configuration.as_str()]).await?;

    assert!(multi_cfg.stdout.contains("root//:macos"));
    assert!(multi_cfg.stdout.contains("root//:arm"));
    Ok(())
}

#[buck_test]
async fn test_ctargets_modifier_order_of_modifiers(buck: &Buck) -> anyhow::Result<()> {
    // if passing in modifiers of the same constraint setting,
    // the last one should be the one that applies
    let result = buck
        .ctargets(&["root//:target?root//:macos+root//:linux"])
        .await?;

    let configurations = extract_configuration(&result.stdout);
    let [configuration] = configurations.as_slice() else {
        panic!("expected one configuration: {configurations:?}");
    };

    let cfg = buck.audit_configurations(&[configuration.as_str()]).await?;

    assert!(cfg.stdout.contains("root//:linux"));
    assert!(!cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_ctargets_modifier_multi_target_pattern(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.ctargets(&["root//:?root//:macos"]).await?;

    let configurations = extract_configuration(&result.stdout);
    let [other_configuration, _, configuration] = &configurations[5..] else {
        panic!("expected eight configurations: {configurations:?}");
    };

    let other_cfg = buck
        .audit_configurations(&[other_configuration.as_str()])
        .await?;
    assert!(other_cfg.stdout.contains("root//:macos"));

    let cfg = buck.audit_configurations(&[configuration.as_str()]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_ctargets_modifier_same_target(buck: &Buck) -> anyhow::Result<()> {
    // if the same target has the same modifiers there should only be one instance of it
    let result = buck
        .ctargets(&["root//:target?root//:macos", "root//:target?root//:macos"])
        .await?;

    let configurations = extract_configuration(&result.stdout);
    let [configuration] = configurations.as_slice() else {
        panic!("expected one configuration: {configurations:?}");
    };

    let cfg = buck.audit_configurations(&[configuration.as_str()]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test]
async fn test_ctargets_fails_with_global_modifier(buck: &Buck) {
    buck.ctargets(&["--modifier", "root//:linux", "root//:target?root//:macos"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
        );
}
