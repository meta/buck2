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
async fn test_cquery_owner(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/cquery.bxl:owner_test"]).await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary (root//platforms:platform1#<HASH>)]\n"
    );

    let result = buck
        .bxl(&["//bxl/cquery.bxl:owner_with_cell_path_test"])
        .await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary (root//platforms:platform1#<HASH>)]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_owner_list(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/cquery.bxl:owner_list_test"]).await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary (root//platforms:platform1#<HASH>), root//bin:the_binary_with_dir_srcs (root//platforms:platform1#<HASH>)]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_kind(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:cquery.bxl:kind_test"]).await?;

    assert!(result.stdout.contains("foo"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_inputs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:cquery.bxl:inputs_test"]).await?;

    assert!(result.stdout.contains("TARGETS.fixture"));
    assert!(result.stdout.contains("bxl.FileSet"));
    assert!(result.stdout.contains("1"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_filter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:cquery.bxl:filter_test"]).await?;

    assert!(result.stdout.contains("root//bin:the_binary"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_attrregex_filter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/cquery.bxl:attrregexfilter_test"]).await?;

    assert!(result.stdout.contains("foo"));
    assert!(result.stdout.contains("bzzt"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_attrfilter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/cquery.bxl:attrfilter_test"]).await?;

    assert!(result.stdout.contains("foo"));
    assert!(!result.stdout.contains("bzzt"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_nattrfilter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/cquery.bxl:nattrfilter_test"]).await?;

    assert!(!result.stdout.contains("foo"));
    assert!(result.stdout.contains("bzzt"));
    assert!(result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_rdeps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/cquery.bxl:rdeps_test"]).await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary (root//platforms:platform1#<HASH>), root//lib:lib1 (root//platforms:platform1#<HASH>), root//lib:file1 (root//platforms:platform1#<HASH>)]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_deps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/cquery.bxl:deps_test"]).await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary (root//platforms:platform1#<HASH>), root//:data (root//platforms:platform1#<HASH>), root//lib:lib1 (root//platforms:platform1#<HASH>), root//lib:lib2 (root//platforms:platform1#<HASH>), root//lib:lib3 (root//platforms:platform1#<HASH>), root//:foo_toolchain (root//platforms:platform1#<HASH>), root//:bin (root//platforms:platform1#<HASH>)]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", allow_soft_errors = true)]
async fn test_cquery_buildfile(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/cquery.bxl:buildfile_test"]).await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_incompatible_configured_targets(buck: &Buck) -> anyhow::Result<()> {
    // incompatible target should be skipped and the cquery should return compatible targets
    let result = buck
        .bxl(&["//bxl/cquery.bxl:incompatible_configured_targets_test"])
        .await?;
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets:incompatible")
    );
    assert!(result.stdout.contains("root//incompatible_targets:foo"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_incompatible_configured_targets_single_label(
    buck: &Buck,
) -> anyhow::Result<()> {
    // incompatible target should be skipped and the cquery should not fail
    let result = buck
        .bxl(&["//bxl/cquery.bxl:incompatible_configured_targets_single_label_test"])
        .await?;
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets:incompatible")
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_incompatible_targets(buck: &Buck) -> anyhow::Result<()> {
    // incompatible target should be skipped and the cquery should not fail
    let result = buck
        .bxl(&["//bxl/cquery.bxl:incompatible_targets_test"])
        .await?;
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets:incompatible")
    );
    assert!(!result.stdout.contains("root//incompatible_targets:foo"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_incompatible_targets_recursive(buck: &Buck) -> anyhow::Result<()> {
    // incompatible target should be skipped and the cquery should return compatible targets
    let result = buck
        .bxl(&["//bxl/cquery.bxl:incompatible_targets_test_recursive"])
        .await?;
    assert!(result.stderr.contains("Skipped 2 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets:incompatible")
    );
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets/inner_folder:incompatible_inner")
    );
    assert!(
        result
            .stdout
            .contains("root//incompatible_targets/inner_folder:foo_inner")
    );
    assert!(result.stdout.contains("root//incompatible_targets:foo"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_configured_label(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/cquery.bxl:cquery_configured_label"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "testsof")]
async fn test_cquery_testsof(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//cquery.bxl:testsof_test"]).await?;
    assert!(
        result
            .stdout
            .contains("root//:foo_test (root//:platform_default_tests")
    );

    let result = buck
        .bxl(&["//cquery.bxl:testsof_with_default_target_platform_test"])
        .await?;
    assert!(
        result
            .stdout
            .contains("root//:foo_test_with_default_platform (root//:foo_test_default_platform")
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_allpaths(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:allpaths_test"]).await?;

    assert_eq!(
        "[root//graph:one, root//graph:ten, root//graph:eleven, root//graph:two, root//graph:three]\n",
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_allpaths_filtered(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl:uquery.bxl:allpaths_filtered_test"])
        .await?;

    assert_eq!(
        "[root//graph:one, root//graph:two, root//graph:three]\n",
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_allpaths(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:lazy_allpaths_test"]).await?;

    assert_eq!(
        "[root//graph:one, root//graph:ten, root//graph:eleven, root//graph:two, root//graph:three]\n",
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_allpaths_filtered(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl:uquery.bxl:lazy_allpaths_filtered_test"])
        .await?;

    assert_eq!(
        "[root//graph:one, root//graph:two, root//graph:three]\n",
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_somepath(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:somepath_test"]).await?;

    assert_eq!(
        "[root//graph:one, root//graph:two, root//graph:three]\n",
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_somepath_filtered(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl:uquery.bxl:somepath_filtered_test"])
        .await?;

    assert_eq!(
        concat!(
            "[root//graph:one, root//graph:ten, root//graph:twenty]\n",
            "[root//graph:one, root//graph:five, root//graph:six, root//graph:twenty]\n",
        ),
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_somepath(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:lazy_somepath_test"]).await?;

    assert_eq!(
        "[root//graph:one, root//graph:two, root//graph:three]\n",
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_somepath_filtered(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl:uquery.bxl:lazy_somepath_filtered_test"])
        .await?;

    assert_eq!(
        concat!(
            "[root//graph:one, root//graph:ten, root//graph:twenty]\n",
            "[root//graph:one, root//graph:five, root//graph:six, root//graph:twenty]\n",
        ),
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_kind(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:kind_test"]).await?;

    assert!(result.stdout.contains("foo"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_kind(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:lazy_kind_test"]).await?;

    assert!(result.stdout.contains("foo"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_inputs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:inputs_test"]).await?;

    assert!(result.stdout.contains("TARGETS.fixture"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_inputs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:lazy_inputs_test"]).await?;

    assert!(result.stdout.contains("TARGETS.fixture"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_filter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:filter_test"]).await?;

    assert!(result.stdout.contains("root//bin:the_binary"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_filter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:uquery.bxl:lazy_filter_test"]).await?;

    assert!(result.stdout.contains("root//bin:the_binary"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_attrregex_filter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:attrregexfilter_test"]).await?;

    assert!(result.stdout.contains("foo"));
    assert!(result.stdout.contains("bzzt"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_attrregex_filter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl/uquery.bxl:lazy_attrregexfilter_test"])
        .await?;

    assert!(result.stdout.contains("foo"));
    assert!(result.stdout.contains("bzzt"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_attrfilter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:attrfilter_test"]).await?;

    assert!(result.stdout.contains("foo"));
    assert!(!result.stdout.contains("bzzt"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_attrfilter(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:lazy_attrfilter_test"]).await?;

    assert!(result.stdout.contains("foo"));
    assert!(!result.stdout.contains("bzzt"));
    assert!(!result.stdout.contains("bar"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_owner(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:owner_test"]).await?;
    assert_eq!(result.stdout, "[root//bin:the_binary]\n");

    let result = buck
        .bxl(&["//bxl/uquery.bxl:owner_with_cell_path_test"])
        .await?;
    assert_eq!(replace_hash(&result.stdout), "[root//bin:the_binary]\n");
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_owner_list(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:owner_list_test"]).await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary, root//bin:the_binary_with_dir_srcs]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_owner(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:lazy_owner_test"]).await?;
    assert_eq!(result.stdout, "[root//bin:the_binary]\n");

    let result = buck
        .bxl(&["//bxl/uquery.bxl:lazy_owner_with_cell_path_test"])
        .await?;
    assert_eq!(replace_hash(&result.stdout), "[root//bin:the_binary]\n");
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_owner_list(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:lazy_owner_list_test"]).await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "[root//bin:the_binary, root//bin:the_binary_with_dir_srcs]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_targets_in_buildfile(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl/uquery.bxl:targets_in_buildfile_test"])
        .await?;
    assert_eq!(
        result.stdout,
        "[root//bin:the_binary, root//bin:the_binary_with_dir_srcs, root//bin:platform]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_targets_in_buildfile(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl/uquery.bxl:lazy_targets_in_buildfile_test"])
        .await?;
    assert_eq!(
        result.stdout,
        "[root//bin:the_binary, root//bin:the_binary_with_dir_srcs, root//bin:platform]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", allow_soft_errors = true)]
async fn test_uquery_buildfile(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/uquery.bxl:buildfile_test"]).await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", allow_soft_errors = true)]
async fn test_uquery_lazy_buildfile(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/uquery.bxl:lazy_buildfile_test"]).await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_rdeps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:rdeps_test"]).await?;
    assert_eq!(
        result.stdout,
        "[root//bin:the_binary, root//lib:lib1, root//lib:file1]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_rdeps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:lazy_rdeps_test"]).await?;
    assert_eq!(
        result.stdout,
        "[root//bin:the_binary, root//lib:lib1, root//lib:file1]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_query_deps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:deps_test"]).await?;
    assert_eq!(
        result.stdout,
        "[root//bin:the_binary, root//:data, root//lib:lib1, root//lib:lib2, root//lib:lib3, root//:foo_toolchain, root//:bin]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_deps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:lazy_deps_test"]).await?;
    assert_eq!(
        result.stdout,
        "[root//bin:the_binary, root//:data, root//lib:lib1, root//lib:lib2, root//lib:lib3, root//:foo_toolchain, root//:bin]\n"
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "testsof")]
async fn test_uquery_testsof(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//uquery.bxl:testsof_test"]).await?;
    assert!(result.stdout.contains("root//:foo_test"));
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_eval(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:eval_query_test"]).await?;
    assert_eq!(result.stdout, "[root//bin/TARGETS.fixture]\n");

    buck.bxl(&["//bxl/uquery.bxl:eval_query_with_query_args"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_uquery_lazy_eval(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl/uquery.bxl:lazy_eval_query_test"]).await?;
    assert_eq!(result.stdout, "[root//bin/TARGETS.fixture]\n");

    buck.bxl(&["//bxl/uquery.bxl:lazy_eval_query_with_query_args"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_bxl_aquery_incompatible_targets(buck: &Buck) -> anyhow::Result<()> {
    // incompatible target should be skipped and the aquery should not fail
    let result = buck.bxl(&["//bxl/aquery.bxl:incompatible_targets"]).await?;
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets:incompatible")
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_eval(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:cquery.bxl:eval_query_test"]).await?;

    assert!(result.stdout.contains("TARGETS.fixture"));

    buck.bxl(&["//bxl/cquery.bxl:eval_query_with_query_args"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_allpaths(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:cquery.bxl:allpaths_test"]).await?;

    assert_eq!(
        "[root//graph:one (<unspecified>), root//graph:ten (<unspecified>), root//graph:eleven (<unspecified>), root//graph:two (<unspecified>), root//graph:three (<unspecified>)]\n",
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_somepath(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:cquery.bxl:somepath_test"]).await?;

    assert_eq!(
        "[root//graph:one (<unspecified>), root//graph:two (<unspecified>), root//graph:three (<unspecified>)]\n",
        result.stdout
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_cquery_somepath_filtered(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl:cquery.bxl:somepath_filtered_test"])
        .await?;

    assert_eq!(
        concat!(
            "[root//graph:one (<unspecified>), root//graph:ten (<unspecified>), root//graph:twenty (<unspecified>)]\n",
            "[root//graph:one (<unspecified>), root//graph:five (<unspecified>), root//graph:six (<unspecified>), root//graph:twenty (<unspecified>)]\n",
        ),
        result.stdout
    );
    Ok(())
}
