/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;

use buck2_e2e::prelude::*;

fn targets(stdout: &str) -> BTreeSet<&str> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

#[buck_test]
async fn test_uquery_deps_without_allow_partial_graph_fails(buck: &Buck) {
    // `root//:root` depends on `//broken:b`, whose package fails to parse, so a
    // plain `deps` traversal aborts.
    buck.uquery(&["deps(root//:root)"])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_uquery_deps_allow_partial_graph_skips_broken_edge(buck: &Buck) -> anyhow::Result<()> {
    // The `deps` traversal is open-ended, so a broken dependency edge discovered
    // while searching is skipped; the loadable part is returned.
    let result = buck
        .uquery(&["deps(root//:root)", "--allow-partial-graph"])
        .await?;
    let targets = targets(&result.stdout);
    assert!(targets.contains("root//:root"));
    assert!(targets.contains("root//good:g"));
    assert!(!targets.iter().any(|t| t.starts_with("root//broken:")));
    Ok(())
}

#[buck_test]
async fn test_uquery_rdeps_without_allow_partial_graph_fails(buck: &Buck) {
    // The universe `root//...` contains the broken package, so enumerating it
    // aborts the reverse query.
    buck.uquery(&["rdeps(root//..., root//good:g)"])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_uquery_rdeps_allow_partial_graph_skips_broken_package(
    buck: &Buck,
) -> anyhow::Result<()> {
    // `root//...` is open-ended, so the broken package under the universe is
    // skipped instead of aborting. The explicit `root//good:g` argument resolves
    // fine.
    let result = buck
        .uquery(&["rdeps(root//..., root//good:g)", "--allow-partial-graph"])
        .await?;
    let targets = targets(&result.stdout);
    // `root` and `good_only` both depend on `good:g`.
    assert!(targets.contains("root//good:g"));
    assert!(targets.contains("root//:root"));
    assert!(targets.contains("root//:good_only"));
    assert!(!targets.iter().any(|t| t.starts_with("root//broken:")));
    Ok(())
}

#[buck_test]
async fn test_uquery_recursive_allow_partial_graph_skips_broken_package(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Without the flag, enumerating the recursive pattern aborts on the broken
    // package.
    buck.uquery(&["root//..."])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");

    // With the flag, the open-ended `root//...` enumeration skips the broken
    // package and returns everything else.
    let result = buck.uquery(&["root//...", "--allow-partial-graph"]).await?;
    let targets = targets(&result.stdout);
    assert!(targets.contains("root//:root"));
    assert!(targets.contains("root//:leaf"));
    assert!(targets.contains("root//:good_only"));
    assert!(targets.contains("root//good:g"));
    assert!(!targets.iter().any(|t| t.starts_with("root//broken:")));
    Ok(())
}

#[buck_test]
async fn test_uquery_allow_partial_graph_explicit_missing_target_still_fails(buck: &Buck) {
    // An explicitly named missing target must resolve; `--allow-partial-graph`
    // only tolerates breakage in open-ended (recursive) patterns, so this fails
    // with or without the flag.
    buck.uquery(&["root//:root + root//:nonexistent"])
        .await
        .expect_failure()
        .assert_stderr_matches("Unknown target `nonexistent`");
    buck.uquery(&["root//:root + root//:nonexistent", "--allow-partial-graph"])
        .await
        .expect_failure()
        .assert_stderr_matches("Unknown target `nonexistent`");
}

#[buck_test]
async fn test_uquery_allow_partial_graph_explicit_broken_package_still_fails(buck: &Buck) {
    // An explicitly named target in a broken package must resolve, so this fails
    // with or without the flag.
    buck.uquery(&["root//broken:b"])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
    buck.uquery(&["root//broken:b", "--allow-partial-graph"])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_uquery_rdeps_explicit_broken_arg_still_fails(buck: &Buck) {
    // The universe `root//...` is open-ended and tolerated, but the explicit
    // `root//broken:b` argument names a broken package, so the query still fails
    // even with `--allow-partial-graph`.
    buck.uquery(&["rdeps(root//..., root//broken:b)", "--allow-partial-graph"])
        .await
        .expect_failure()
        .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_uquery_rdeps_explicit_broken_universe_still_fails(buck: &Buck) {
    // Contrast with the open-ended `root//...` universe: naming a broken package
    // explicitly as the universe fails even with `--allow-partial-graph`.
    buck.uquery(&[
        "rdeps(root//broken:b, root//good:g)",
        "--allow-partial-graph",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("intentional parse error");
}

#[buck_test]
async fn test_uquery_allow_partial_graph_clean_query_unaffected(buck: &Buck) -> anyhow::Result<()> {
    // `--allow-partial-graph` on an error-free query returns the normal result.
    let result = buck
        .uquery(&["deps(root//good:g)", "--allow-partial-graph"])
        .await?;
    assert_eq!(targets(&result.stdout), BTreeSet::from(["root//good:g"]));
    Ok(())
}
