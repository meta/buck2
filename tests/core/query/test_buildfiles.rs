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

/// The sorted lines of `out`, without the nano prelude's files.
fn without_prelude(out: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = out
        .lines()
        .filter(|x| !x.starts_with("nano_prelude/"))
        .collect();
    lines.sort();
    lines
}

#[buck_test]
async fn test_allbuildfiles(buck: &Buck) -> anyhow::Result<()> {
    let target1 = "root//load:abc";
    let target2 = "root//transitive_load:def";
    let target3 = "root//transitive_load:ghi";
    let out1 = buck
        .uquery(&[&format!("allbuildfiles({target1})")])
        .await?
        .stdout;
    let out2 = buck
        .uquery(&[&format!("allbuildfiles({target2})")])
        .await?
        .stdout;
    let out3 = buck
        .uquery(&[&format!("allbuildfiles({target3})")])
        .await?
        .stdout;
    let out4 = buck
        .uquery(&[&format!("allbuildfiles(set({target1} {target2}))")])
        .await?
        .stdout;

    // First, check that these are the same for cquery
    assert_eq!(
        out1,
        buck.cquery(&[&format!("allbuildfiles({target1})")])
            .await?
            .stdout
    );
    assert_eq!(
        out2,
        buck.cquery(&[&format!("allbuildfiles({target2})")])
            .await?
            .stdout
    );
    assert_eq!(
        out3,
        buck.cquery(&[&format!("allbuildfiles({target3})")])
            .await?
            .stdout
    );
    assert_eq!(
        out4,
        buck.cquery(&[&format!("allbuildfiles(set({target1} {target2}))")])
            .await?
            .stdout
    );

    let out1 = without_prelude(&out1);
    let out2 = without_prelude(&out2);
    let out3 = without_prelude(&out3);
    let out4 = without_prelude(&out4);

    // verify loads
    let expected1 = vec!["load/TARGETS.fixture", "load/a.bzl", "load/a.json"];
    assert_eq!(out1, expected1);

    // verify transitive loads
    let expected2 = vec![
        "transitive_load/TARGETS.fixture",
        "transitive_load/b.bzl",
        "transitive_load/c.bzl",
        "transitive_load/c.json",
    ];
    assert_eq!(out2, expected2);
    assert_eq!(out3, expected2);

    // correctly handle multiple inputs
    let mut expected4 = [expected1, expected2].concat();
    expected4.sort();
    assert_eq!(out4, expected4);
    Ok(())
}

#[buck_test]
async fn test_rbuildfiles(buck: &Buck) -> anyhow::Result<()> {
    let target_file = "transitive_load/TARGETS.fixture";
    let out1 = buck
        .uquery(&[&format!(
            "rbuildfiles({target_file}, transitive_load/c.bzl)"
        )])
        .await?
        .stdout;
    let out2 = buck
        .uquery(&[&format!("rbuildfiles({target_file}, {target_file})")])
        .await?
        .stdout;

    // Check that these are the same for cquery
    assert_eq!(
        out1,
        buck.cquery(&[&format!(
            "rbuildfiles({target_file}, transitive_load/c.bzl)"
        )])
        .await?
        .stdout
    );
    assert_eq!(
        out2,
        buck.cquery(&[&format!("rbuildfiles({target_file}, {target_file})")])
            .await?
            .stdout
    );

    assert!(out1.contains("transitive_load/b.bzl"));
    assert!(out1.contains("transitive_load/c.bzl"));
    assert!(out1.contains("transitive_load/TARGETS"));

    assert_eq!(out2, format!("{target_file}\n"));
    Ok(())
}
