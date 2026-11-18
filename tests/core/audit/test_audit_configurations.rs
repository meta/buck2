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

fn parse_audit_configurations(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|x| !x.starts_with(' '))
        .map(|x| x.trim_end_matches(':').to_owned())
        .collect()
}

#[buck_test]
async fn test_audit_configurations_all(buck: &Buck) -> anyhow::Result<()> {
    // Evaluate a target to make sure configuration is loaded.
    buck.cquery(&["//:genrule"]).await?;

    let result = buck.audit(&["configurations"]).await?;
    let configurations: Vec<String> = parse_audit_configurations(&result.stdout)
        .iter()
        .map(|x| replace_hash(x))
        .collect();
    assert!(configurations.contains(&"root//:p#<HASH>".to_owned()));
    Ok(())
}

#[buck_test]
async fn test_audit_configurations_specific(buck: &Buck) -> anyhow::Result<()> {
    // Evaluate a target to make sure configuration is loaded.
    buck.cquery(&["//:genrule"]).await?;

    // Load configurations so we can learn the hash.
    let result = buck.audit(&["configurations"]).await?;
    let configurations: Vec<String> = parse_audit_configurations(&result.stdout)
        .into_iter()
        .filter(|c| c.starts_with("root//:p#"))
        .collect();
    let [configuration] = configurations.as_slice() else {
        panic!("expected one `root//:p` configuration, got {configurations:?}");
    };

    // Now audit the specific configuration.
    let result = buck.audit(&["configurations", configuration]).await?;
    assert_eq!(
        vec![configuration.clone()],
        parse_audit_configurations(&result.stdout)
    );
    Ok(())
}
