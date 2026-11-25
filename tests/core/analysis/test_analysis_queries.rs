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
use std::io::Read;
use std::path::Path;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::replace_in_file;

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

fn targets(labels: &[&str]) -> BTreeSet<String> {
    labels.iter().map(|label| (*label).to_owned()).collect()
}

/// The targets a `query` rule of the fixture wrote out.
async fn query_targets(buck: &Buck, args: &[&str]) -> anyhow::Result<BTreeSet<String>> {
    let result = buck.build_without_report(args).await?;
    Ok(result
        .stdout
        .split_whitespace()
        .map(str::to_owned)
        .collect())
}

/// The `AnalysisQueryKey`s in the daemon's DICE graph, sorted.
async fn analysis_query_keys(buck: &Buck, dump: &Path) -> anyhow::Result<Vec<String>> {
    buck.debug(&["dice-dump", "--path", dump.to_str().unwrap()])
        .await?;
    let compressed = tokio::fs::read(dump.join("nodes.gz")).await?;
    let mut nodes = String::new();
    flate2::read::GzDecoder::new(compressed.as_slice()).read_to_string(&mut nodes)?;
    let mut keys: Vec<String> = nodes
        .lines()
        .filter_map(|line| {
            let row: Vec<&str> = line.split('\t').collect();
            (row.len() == 3 && row[1] == "AnalysisQueryKey").then(|| row[2].to_owned())
        })
        .collect();
    keys.sort();
    Ok(keys)
}

#[buck_test(data_dir = "analysis_query_sharing")]
async fn test_analysis_query_is_shared_by_its_text_and_literals(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    // `:foo` in another package is another target, and so another query.
    assert_eq!(
        query_targets(buck, &["//a:q", "--out=-"]).await?,
        targets(&[])
    );
    assert_eq!(
        query_targets(buck, &["//a:q2", "--out=-"]).await?,
        targets(&[])
    );
    assert_eq!(
        query_targets(buck, &["//b:q", "--out=-"]).await?,
        targets(&["root//b:foo"])
    );
    let keys = analysis_query_keys(buck, &tmp_path.join("dice")).await?;
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys[0].contains("deps(root//a:foo)"), "{keys:?}");
    assert!(keys[1].contains("deps(root//b:foo)"), "{keys:?}");

    // The same text over the same `:foo` in another configuration is
    // another query too.
    assert_eq!(
        query_targets(
            buck,
            &["//d:q", "--target-platforms=root//:chosen", "--out=-"]
        )
        .await?,
        targets(&["root//d:foo"])
    );
    assert_eq!(
        query_targets(
            buck,
            &["//d:q", "--target-platforms=root//:other", "--out=-"]
        )
        .await?,
        targets(&[])
    );
    let keys = analysis_query_keys(buck, &tmp_path.join("dice-configured")).await?;
    let configured: BTreeSet<&String> = keys
        .iter()
        .filter(|key| key.contains("deps(root//d:foo)"))
        .collect();
    assert_eq!(configured.len(), 2, "{keys:?}");

    // A change below the literal reaches both targets sharing the query,
    // and so does a change to the literal.
    replace_in_file(
        r#"labels = ["no"]"#,
        r#"labels = ["yes"]"#,
        &buck.cwd().join("c/TARGETS.fixture"),
    );
    let below = targets(&["root//c:bar"]);
    assert_eq!(query_targets(buck, &["//a:q", "--out=-"]).await?, below);
    assert_eq!(query_targets(buck, &["//a:q2", "--out=-"]).await?, below);
    assert_eq!(
        query_targets(buck, &["//b:q", "--out=-"]).await?,
        targets(&["root//b:foo"])
    );
    let a = buck.cwd().join("a/TARGETS.fixture");
    replace_in_file(r#"labels = ["no"]"#, r#"labels = ["yes"]"#, &a);
    let both = targets(&["root//a:foo", "root//c:bar"]);
    assert_eq!(query_targets(buck, &["//a:q", "--out=-"]).await?, both);
    assert_eq!(query_targets(buck, &["//a:q2", "--out=-"]).await?, both);

    // The targets sharing a query get its result in the query's order.
    for (target, order) in [
        ("//a:ordered", "root//c:bar root//a:foo"),
        ("//a:ordered2", "root//c:bar root//a:foo"),
        ("//a:reversed", "root//a:foo root//c:bar"),
    ] {
        let result = buck.build_without_report(&[target, "--out=-"]).await?;
        assert_eq!(result.stdout.trim(), order, "{target}");
    }

    // A failing query fails each target that carries it. Its error is not
    // kept, and the query is evaluated again once a node it read changes.
    for target in ["//e:q", "//e:q2"] {
        buck.build(&[target])
            .await
            .expect_failure()
            .assert_stderr_matches("backtracking");
    }
    let kept = |keys: &[String]| {
        keys.iter()
            .filter(|key| key.contains("deps(root//e:foo)"))
            .count()
    };
    let keys = analysis_query_keys(buck, &tmp_path.join("dice-failed")).await?;
    assert_eq!(kept(&keys), 0, "{keys:?}");
    // Of two failing queries in one attribute, the first one's error is the
    // one reported.
    let (regex, depth) = ("backtracking", "Invalid traversal depth");
    for (target, first, second) in [
        ("//e:regex_then_depth", regex, depth),
        ("//e:depth_then_regex", depth, regex),
    ] {
        let failure = buck.build(&[target]).await.expect_failure();
        failure.assert_stderr_matches(first);
        assert!(!failure.stderr.contains(second), "{}", failure.stderr);
    }

    replace_in_file(
        r#"labels = ["a" * 80 + "!"]"#,
        r#"labels = ["MATCH"]"#,
        &buck.cwd().join("e/TARGETS.fixture"),
    );
    let matched = targets(&["root//e:foo"]);
    assert_eq!(query_targets(buck, &["//e:q", "--out=-"]).await?, matched);
    assert_eq!(query_targets(buck, &["//e:q2", "--out=-"]).await?, matched);
    let keys = analysis_query_keys(buck, &tmp_path.join("dice-recovered")).await?;
    assert_eq!(kept(&keys), 1, "{keys:?}");
    Ok(())
}
