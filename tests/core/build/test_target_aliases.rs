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
use serde_json::json;

#[buck_test]
async fn test_target_aliases(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["alias"]).await?;
    buck.cquery(&["deps(alias)"]).await?;

    buck.targets(&["chain"]).await?;
    buck.cquery(&["deps(chain)"]).await?;

    let res = buck
        .targets(&["--resolve-alias", "alias", "chain", "//targets:target"])
        .await?;
    assert_eq!(
        res.stdout.lines().map(str::trim).collect::<Vec<_>>(),
        vec!["root//targets:target"; 3]
    );

    // Following a broken alias should fail
    buck.targets(&["--resolve-alias", "bad"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid alias: `bad`");

    // Asking for a non-existent alias / target should also fail. Note that
    // we're not capable of telling the difference between an alias that doesn't
    // exist vs. one that is broken.
    buck.targets(&["--resolve-alias", "oops"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid alias: `oops`");

    buck.targets(&["--resolve-alias", "targets:not_existent"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid alias:.*Target does not exist in package");
    buck.targets(&["--resolve-alias", "broken:broken"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid alias:.*Package cannot be evaluated.*Parse error");
    buck.targets(&["--resolve-alias", "not_existent:not_existent"])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid alias:.*Package cannot be evaluated.*does not exist");
    buck.targets(&["--resolve-alias", "..."])
        .await
        .expect_failure()
        .assert_stderr_matches("Invalid alias.*does not expand to a single target");
    Ok(())
}

fn expected_aliases() -> Vec<serde_json::Value> {
    vec![
        json!({
            "alias": "alias",
            "buck.package": "root//targets",
            "name": "target",
        }),
        json!({
            "alias": "chain",
            "buck.package": "root//targets",
            "name": "target",
        }),
        json!({
            "alias": "//targets:target",
            "buck.package": "root//targets",
            "name": "target",
        }),
    ]
}

#[buck_test]
async fn test_resolve_alias_json(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .targets(&[
            "--resolve-alias",
            "alias",
            "chain",
            "//targets:target",
            "--json",
        ])
        .await?;

    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&res.stdout)?,
        serde_json::Value::Array(expected_aliases())
    );
    Ok(())
}

#[buck_test]
async fn test_resolve_alias_json_lines(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .targets(&[
            "--resolve-alias",
            "alias",
            "chain",
            "//targets:target",
            "--json-lines",
        ])
        .await?;

    let lines = res
        .stdout
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(lines, expected_aliases());
    Ok(())
}
