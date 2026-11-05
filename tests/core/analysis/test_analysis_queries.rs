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

async fn test_analysis_query_invalidation_impl(buck: &Buck, name: &str) -> anyhow::Result<()> {
    let linux = buck
        .build_without_report(&[":root", "-c", "test.configuration=linux", "--out=-"])
        .await?;
    let macos = buck
        .build_without_report(&[":root", "-c", "test.configuration=macos", "--out=-"])
        .await?;

    golden(&linux.stdout, &format!("{name}/linux.txt.golden"));
    golden(&macos.stdout, &format!("{name}/macos.txt.golden"));

    // Mostly here to really be safe but in practice this fails with an
    // incompatible target earlier if we have a bug.
    assert!(linux.stdout.contains("linux-select-dep"));
    assert!(macos.stdout.contains("macos-select-dep"));
    Ok(())
}

/// This is a regression test for T133069783.
#[buck_test(data_dir = "analysis_query_invalidation")]
async fn test_analysis_query_invalidation_deps(buck: &Buck) -> anyhow::Result<()> {
    test_analysis_query_invalidation_impl(buck, "analysis_query_invalidation").await
}

/// Equivalent of T133069783 for `classpath()` instead of `deps()` queries.
#[buck_test(data_dir = "analysis_query_invalidation_classpath")]
async fn test_analysis_query_invalidation_classpath(buck: &Buck) -> anyhow::Result<()> {
    test_analysis_query_invalidation_impl(buck, "analysis_query_invalidation_classpath").await
}

#[buck_test(data_dir = "analysis_query_deps")]
async fn test_analysis_query_deps(buck: &Buck) -> anyhow::Result<()> {
    let deps = buck.build_without_report(&[":deps", "--out=-"]).await?;
    golden(&deps.stdout, "analysis_query_deps/deps.txt.golden");
    assert!(deps.stdout.contains(":foo"));
    assert!(deps.stdout.contains(":bar"));
    assert!(deps.stdout.contains(":baz"));
    assert!(deps.stdout.contains(":qux"));
    Ok(())
}

#[buck_test(data_dir = "analysis_query_deps")]
async fn test_duplicate_analysis_query_expansions(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build_without_report(&[":duplicate_queries", "--out=-"])
        .await?;
    let fields: Vec<&str> = result.stdout.trim().split('|').collect();
    assert_eq!(fields.len(), 4);
    assert!(fields.iter().all(|f| !f.is_empty()));
    assert_eq!(fields[0], fields[2]);
    assert_eq!(fields[1], fields[3]);
    assert!(fields[0].ends_with(":bar"));
    Ok(())
}

#[buck_test(data_dir = "analysis_query_deps")]
async fn test_analysis_query_deps_with_depth(buck: &Buck) -> anyhow::Result<()> {
    let deps = buck.build_without_report(&[":deps1", "--out=-"]).await?;
    golden(&deps.stdout, "analysis_query_deps/deps1.txt.golden");
    assert!(deps.stdout.contains(":foo"));
    assert!(deps.stdout.contains(":bar"));
    assert!(deps.stdout.contains(":baz"));
    assert!(!deps.stdout.contains(":qux"));
    Ok(())
}

#[buck_test(setup_eden = true, data_dir = "analysis_query_deps")]
async fn test_analysis_query_target_deps(buck: &Buck) -> anyhow::Result<()> {
    let deps = buck
        .build_without_report(&[":target_deps", "--out=-"])
        .await?;
    golden(&deps.stdout, "analysis_query_deps/target_deps.txt.golden");
    assert!(deps.stdout.contains(":foo"));
    assert!(deps.stdout.contains(":bar"));
    assert!(!deps.stdout.contains(":baz"));
    assert!(!deps.stdout.contains(":qux"));
    Ok(())
}
