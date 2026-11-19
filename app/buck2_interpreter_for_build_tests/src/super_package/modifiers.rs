/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::fs::project::ProjectRootTemp;
use buck2_core::target::label::label::TargetLabel;
use buck2_node::nodes::frontend::TargetGraphCalculation;
use indoc::indoc;

use crate::tests::calculation;

const RULES_BZL: &str = r#"
simple = rule(
    impl = lambda ctx: fail(),
    attrs = {},
)
"#;

#[tokio::test]
async fn test_set_package_cfg_modifiers() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file("foo/PACKAGE", "set_modifiers(['aaabbbccc'])");
    fs.write_file(
        "foo/BUCK",
        indoc!(
            r#"
                load("//:rules.bzl", "simple")
                simple(name = "a")
        "#
        ),
    );

    let ctx = calculation(&fs).await;

    let target_node = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//foo:a"))
        .await
        .unwrap();

    let cfg_modifiers = target_node
        .package_cfg_modifiers()
        .unwrap()
        .to_json()
        .to_string();
    assert_eq!("[\"aaabbbccc\"]", cfg_modifiers);
}

#[tokio::test]
async fn test_get_parent_package_cfg_modifiers() {
    let fs = ProjectRootTemp::new().unwrap();

    fs.write_file("rules.bzl", RULES_BZL);
    fs.write_file("PACKAGE", "set_modifiers(['aaaa'])");
    fs.write_file(
        "foo/PACKAGE",
        "set_modifiers(get_parent_modifiers() + ['bbb'])",
    );
    fs.write_file(
        "foo/BUCK",
        indoc!(
            r#"
                load("//:rules.bzl", "simple")
                simple(name = "a")
        "#
        ),
    );

    let ctx = calculation(&fs).await;

    let target_node = ctx
        .ctx()
        .get_target_node(&TargetLabel::testing_parse("root//foo:a"))
        .await
        .unwrap();

    let cfg_modifiers = target_node
        .package_cfg_modifiers()
        .unwrap()
        .to_json()
        .to_string();
    assert_eq!("[\"aaaa\",\"bbb\"]", cfg_modifiers);
}
