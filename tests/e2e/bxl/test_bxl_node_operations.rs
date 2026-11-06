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

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_label_functions(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl/label_functions.bxl:label_func_test"])
        .await?;

    assert_eq!(
        replace_hash(&result.stdout).lines().collect::<Vec<_>>(),
        [
            "root//bin:the_binary (root//platforms:platform1#<HASH>)",
            "root//bin:the_binary[sub] (root//platforms:platform1#<HASH>)",
            // configured_target() called for below, should only return configured target
            "root//bin:the_binary (root//platforms:platform1#<HASH>)",
            "root//bin:the_binary[sub1][sub2] (root//platforms:platform1#<HASH>)",
            // configured_target() called for below, should only return configured target
            "root//bin:the_binary (root//platforms:platform1#<HASH>)",
            "root//bin:the_binary",
            "root//bin:the_binary[sub]",
            "root//bin:the_binary[sub1][sub2]",
        ]
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_coerced_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/coerced_attributes.bxl:coerced_attrs_test"])
        .await?;

    let result = buck
        .bxl(&["//bxl/coerced_attributes.bxl:coerced_attributes_display_json_test"])
        .await?;
    let cmd_select: serde_json::Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(cmd_select["__type"], "selector");
    assert_eq!(
        cmd_select["entries"],
        serde_json::json!({
            "DEFAULT": "foo",
            "config//os:macos": "bar",
            "config//os:windows": "foobar",
        })
    );

    let result = buck
        .bxl(&["//bxl/coerced_attributes.bxl:coerced_attributes_display_test"])
        .await?;

    let output = result.stdout;

    assert!(output.contains("root//platforms:platform1"));
    assert!(output.contains("genrule_with_selects"));
    assert!(output.contains(
        r#"select({"config//os:macos": "bar", "config//os:windows": "foobar", "DEFAULT": "foo"})"#
    ));
    assert!(output.contains("PUBLIC"));
    assert!(output.contains("magic"));

    buck.bxl(&["//bxl/coerced_attributes.bxl:selector_attrs_test"])
        .await?;
    buck.bxl(&["//bxl/coerced_attributes.bxl:concat_attrs_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_configured_node(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/node.bxl:configured_node_test"]).await?;

    assert_eq!(
        replace_hash(&result.stdout).lines().collect::<Vec<_>>(),
        [
            "root//bin:the_binary (root//platforms:platform1#<HASH>)",
            "root//rules/rules.bzl:_foo_binary",
            "normal",
        ]
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_unconfigured_node(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/node.bxl:unconfigured_node_test"]).await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        [
            "root//bin:the_binary",
            "root//rules/rules.bzl:_foo_binary",
            "normal",
            "[root//bin/TARGETS.fixture]",
        ]
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_node_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:node_attributes.bxl:attrs_test"]).await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_node_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:node_attributes.bxl:lazy_attrs_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_node_attrs_with_special_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:node_attributes.bxl:lazy_attrs_with_special_attrs_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_resolved_node_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:resolved_node_attributes.bxl:resolved_attrs_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_resolved_node_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:resolved_node_attributes.bxl:lazy_resolved_attrs_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_resolved_node_with_special_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:resolved_node_attributes.bxl:lazy_resolved_attrs_with_special_attrs_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_unconfigured_target_node_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:unconfigure_target_node_attrs.bxl:node_attrs"])
        .await?;

    let result = buck
        .bxl(&["//bxl/unconfigure_target_node_attrs.bxl:node_attrs_display"])
        .await?;

    let output = result.stdout;

    assert!(output.contains("root//platforms:platform1"));
    assert!(output.contains("genrule_with_selects"));
    assert!(output.contains(
        r#"select({"config//os:macos": "bar", "config//os:windows": "foobar", "DEFAULT": "foo"})"#
    ));
    assert!(output.contains("PUBLIC"));
    assert!(output.contains("magic"));

    buck.bxl(&["//bxl/unconfigure_target_node_attrs.bxl:selector_attrs_test"])
        .await?;
    buck.bxl(&["//bxl/unconfigure_target_node_attrs.bxl:concat_attrs_test"])
        .await?;

    buck.bxl(&["//bxl/unconfigure_target_node_attrs.bxl:attr_metadata"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_node_constraints(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/node_constraints.bxl:constraints_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_node_empty_constraints(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/node_constraints.bxl:empty_constraints_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_configured_target_node_attrs(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:configured_target_node_attrs.bxl:attrs_test"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_providers_info(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/providers_info.bxl:providers_info_test"])
        .await?;
    Ok(())
}
