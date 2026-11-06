/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Generally we test for basic functionality of things working here and do
//! more extensive testing in the uquery tests.

use buck2_e2e::golden::golden_replace_cfg_hash;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_hash;

#[buck_test(data_dir = "unsorted")]
async fn test_query_inputs(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&["inputs(set(root//bin:the_binary //lib:file1))"])
        .await?;
    assert_eq!(result.stdout, "bin/TARGETS.fixture\n");
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_query_cell(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["//stuff:magic"]).rel_cwd("special").await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "special//stuff:magic (root//platforms:platform1#<HASH>)\n"
    );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_query_relative(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["..."]).rel_cwd("special").await?;
    assert_eq!(
        replace_hash(&result.stdout),
        "special//stuff:magic (root//platforms:platform1#<HASH>)\n"
    );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_query_provider_names(buck: &Buck) -> anyhow::Result<()> {
    // FIXME(JakobDegen): Providers in a query literal should be an error; they are dropped.
    let result = buck
        .cquery(&["'root//bin:the_binary[provider_name]'"])
        .await?;
    assert!(result.stdout.contains("root//bin:the_binary ("));

    buck.cquery(&["'root//bin:the_binary#some_flavor'"])
        .await
        .expect_failure()
        // Soft error, upgraded to a hard error by the test harness.
        .assert_stderr_matches(
            "buck1 flavor `#some_flavor` on `root//bin:the_binary#some_flavor` is deprecated",
        );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_query_print_provider_text(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&["%s", "root//bin:the_binary", "--show-providers"])
        .await?;
    golden_replace_cfg_hash(
        &replace_hash(&out.stdout),
        "unsorted/query_print_provider_text.golden.txt",
    );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_query_print_provider_json(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&["%s", "root//bin:the_binary", "--show-providers", "--json"])
        .await?;
    golden_replace_cfg_hash(
        &replace_hash(&out.stdout),
        "unsorted/query_print_provider_json.golden.json",
    );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_query_chunked_stream(buck: &Buck) -> anyhow::Result<()> {
    let q = "deps(root//bin:the_binary)";
    let result1 = buck.cquery(&[q]).await?;
    buck.kill().await?;
    let result2 = buck
        .cquery(&[q])
        .env("BUCK2_DEBUG_RAWOUTPUT_CHUNK_SIZE", "5")
        .await?;
    assert_eq!(result1.stdout, result2.stdout);
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_attributes(buck: &Buck) -> anyhow::Result<()> {
    let attrs_out = buck
        .cquery(&[
            "--output-attribute",
            "buck\\..*",
            "--output-attribute",
            "srcs",
            "set(root//bin:the_binary //lib:file1)",
        ])
        .await?;
    let attrs_json_out = buck
        .cquery(&[
            "--output-attribute",
            "buck\\..*",
            "--output-attribute",
            "srcs",
            "--json",
            "set(root//bin:the_binary //lib:file1)",
        ])
        .await?;
    // specifying any attrs enables json output
    assert_eq!(attrs_json_out.stdout, attrs_out.stdout);
    let attrs_json_out: serde_json::Value =
        serde_json::from_str(&replace_hash(&attrs_json_out.stdout))?;
    assert_eq!(
        serde_json::json!({
            "root//bin:the_binary (root//platforms:platform1#<HASH>)": {
                "buck.deps": [
                    "root//:data (root//platforms:platform1#<HASH>)",
                    "root//lib:lib1 (root//platforms:platform1#<HASH>)",
                    "root//lib:lib2 (root//platforms:platform1#<HASH>)",
                    "root//lib:lib3 (root//platforms:platform1#<HASH>)",
                    "root//:foo_toolchain (root//platforms:platform1#<HASH>)",
                    "root//:bin (root//platforms:platform1#<HASH>)",
                ],
                "buck.execution_platform": "<legacy_global_exec_platform>",
                "buck.package": "root//bin:TARGETS.fixture",
                "buck.plugins": {},
                "buck.target_configuration": "root//platforms:platform1#<HASH>",
                "buck.type": "_foo_binary",
                "buck.oncall": null,
                "srcs": ["root//bin/TARGETS.fixture"],
            },
            "root//lib:file1 (root//platforms:platform1#<HASH>)": {
                "buck.deps": [],
                "buck.execution_platform": "<legacy_global_exec_platform>",
                "buck.package": "root//lib:TARGETS.fixture",
                "buck.plugins": {},
                "buck.target_configuration": "root//platforms:platform1#<HASH>",
                "buck.type": "_foo_genrule",
                "buck.oncall": null,
            },
        }),
        attrs_json_out
    );
    Ok(())
}

// Tests for "%Ss" uses
#[buck_test(data_dir = "unsorted")]
async fn test_args_as_set(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&["%Ss", "root//bin:the_binary", "//lib:file1"])
        .await?;
    assert_eq!(
        replace_hash(&out.stdout),
        "root//bin:the_binary (root//platforms:platform1#<HASH>)\nroot//lib:file1 (root//platforms:platform1#<HASH>)\n"
    );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_multi_query(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&["%s", "root//bin:the_binary", "//lib:file1"])
        .await?;
    assert_eq!(
        replace_hash(&out.stdout),
        "root//bin:the_binary (root//platforms:platform1#<HASH>)\nroot//lib:file1 (root//platforms:platform1#<HASH>)\n"
    );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_query_attrfilter(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .uquery(&["attrfilter(buck.package, 'root//bin:TARGETS.fixture',root//bin:the_binary)"])
        .await?;
    assert_eq!(out.stdout.trim(), "root//bin:the_binary");
    Ok(())
}

#[buck_test(data_dir = "multi_query_universe")]
async fn test_multi_query_universe(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&[
            "deps(%s)",
            "root//:macos-bin",
            "//:common-dep",
            "--output-format=json",
        ])
        .await?;
    // `common-dep` is configured for linux, so it must not include `only-on-macos` target.
    //   Which would be the case if we constructed universe from all the queries together
    //   instead of separate universes for each query.
    golden_replace_cfg_hash(
        &replace_hash(&out.stdout),
        "multi_query_universe/multi_query_universe.golden.json",
    );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_multi_query_print_provider_text(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&[
            "%s",
            "root//bin:the_binary",
            "//lib:lib1",
            "--show-providers",
        ])
        .await?;
    golden_replace_cfg_hash(
        &replace_hash(&out.stdout),
        "unsorted/multi_query_print_provider_text.golden.txt",
    );
    Ok(())
}

#[buck_test(data_dir = "unsorted")]
async fn test_multi_query_print_provider_json(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&[
            "%s",
            "root//bin:the_binary",
            "//lib:lib1",
            "--show-providers",
            "--json",
        ])
        .await?;

    golden_replace_cfg_hash(
        &replace_hash(&out.stdout),
        "unsorted/multi_query_print_provider_json.golden.json",
    );
    Ok(())
}

#[buck_test(data_dir = "visibility")]
async fn test_visibility(buck: &Buck) -> anyhow::Result<()> {
    for good in [
        "self//:pass1",
        "self//:pass2",
        "self//:pass3",
        "self//:pass4",
    ] {
        let out = buck.cquery(&[good]).await?;
        assert!(out.stdout.contains(good));
    }

    for bad in [
        "self//:fail1",
        "self//:fail2",
        "self//:fail3",
        "self//:fail4",
    ] {
        eprintln!("{bad}");
        let failure = buck.cquery(&[bad]).await.expect_failure();
        assert!(failure.stderr.contains(&format!("not visible to `{bad}`")));
    }
    Ok(())
}

#[buck_test(data_dir = "testsof")]
async fn test_testsof(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .cquery(&[
            "testsof(//:foo_lib)",
            "--target-platforms",
            "//:platform_default_tests",
        ])
        .await?;

    assert!(out.stdout.contains("root//:foo_test"));
    assert!(!out.stdout.contains("root//:foo_extra_test"));
    assert!(!out.stdout.contains("root//:foo_lib"));

    let out = buck
        .cquery(&[
            "testsof(//:foo_lib)",
            "--target-platforms",
            "//:platform_more_tests",
        ])
        .await?;

    assert!(out.stdout.contains("root//:foo_test"));
    assert!(out.stdout.contains("root//:foo_extra_test"));
    assert!(!out.stdout.contains("root//:foo_lib"));
    Ok(())
}

// DICE currently may re-evaluate dead nodes ignoring errors, but it cannot ignore panics.
// The disabling of execution platforms through a buckconfig ended up causing a panic
// that was the root cause of non-deterministic buck2 failures on 10% of fbcode TD in S303188.
//
// TODO(scottcao): Disabling execution platforms is a hack that we need to get rid of
// because it's not how buck2 should be used. Get rid of this test case once fbcode TD
// stops disabling execution platforms
#[buck_test(data_dir = "toolchain_deps")]
async fn test_disabling_of_execution_platforms(buck: &Buck) -> anyhow::Result<()> {
    // Run these commands 10x such that a stress run of 10 on continuous CI would run these commands 100x.
    // If there is a regression then the stress run would for sure detect it.
    for _ in 0..10 {
        let query = "deps(set(tests/...))";
        buck.cquery(&[query]).await?;
        buck.cquery(&[query, "-c", "build.execution_platforms="])
            .await?;
    }
    Ok(())
}

#[buck_test(data_dir = "deps_query")]
async fn test_declared_deps_query(buck: &Buck) {
    buck.cquery(&["root//:declared_deps"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Error parsing target pattern `\$declared_deps`");
}

// Tests for intersect and except operators on FileSet, TargetSet, and String types
// These tests verify the fix for https://github.com/facebook/buck2/issues/1109
/// Test FileSet intersect FileSet using inputs().
#[buck_test(data_dir = "set_operators")]
async fn test_cquery_fileset_intersect(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&["inputs(root//:lib_a) intersect inputs(root//:lib_b)"])
        .await?;
    assert_eq!(result.stdout, "common.txt\n");
    Ok(())
}

/// Test TargetSet except TargetSet using set().
#[buck_test(data_dir = "set_operators")]
async fn test_cquery_targetset_except(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&["set(root//:lib_a root//:app) except set(root//:app)"])
        .await?;
    assert!(result.stdout.contains("root//:lib_a"));
    assert!(!result.stdout.contains("root//:app"));
    Ok(())
}
