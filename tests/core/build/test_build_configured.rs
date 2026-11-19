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

/// The configuration hash of a `cquery` line like `root//:simple (<astrologer>#0123456789abcdef)`.
fn cfg_hash(line: &str) -> String {
    let hash = line
        .rsplit_once('#')
        .map_or(line, |(_, hash)| hash.strip_suffix(')').unwrap_or(hash));
    assert!(
        hash.len() == 16
            && hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "{line}"
    );
    hash.to_owned()
}

/// Obtain hashes of `<astrologer>` and `<vagabond>` configurations.
async fn obtain_cfg_hashes(buck: &Buck) -> anyhow::Result<(String, String)> {
    let result = buck
        .cquery(&["root//:simple", "--target-universe", "root//:universe"])
        .await?;
    let lines: Vec<&str> = result.stdout.lines().collect();
    let [astrologer, vagabond] = lines.as_slice() else {
        anyhow::bail!("Expected two configurations, got {lines:?}");
    };
    assert!(astrologer.starts_with("root//:simple (<astrologer>#"));
    assert!(vagabond.starts_with("root//:simple (<vagabond>#"));
    Ok((cfg_hash(astrologer), cfg_hash(vagabond)))
}

#[buck_test]
async fn test_build_configured_full_configuration(buck: &Buck) -> anyhow::Result<()> {
    let (astrologer_hash, _) = obtain_cfg_hashes(buck).await?;

    let result = buck
        .build(&[
            &format!("root//:simple (<astrologer>#{astrologer_hash})"),
            "--target-universe",
            "root//:universe",
        ])
        .await?;
    let out = std::fs::read_to_string(result.build_report()?.output_for_target("root//:simple")?)?;
    assert_eq!(
        format!("$$$root//:simple (<astrologer>#{astrologer_hash})$$$"),
        out
    );
    Ok(())
}

#[buck_test]
async fn test_build_configured_no_hash(buck: &Buck) -> anyhow::Result<()> {
    let (_, vagabond_hash) = obtain_cfg_hashes(buck).await?;
    let result = buck
        .build(&[
            "root//:simple (<vagabond>)",
            "--target-universe",
            "root//:universe",
        ])
        .await?;
    let out = std::fs::read_to_string(result.build_report()?.output_for_target("root//:simple")?)?;
    assert_eq!(
        format!("$$$root//:simple (<vagabond>#{vagabond_hash})$$$"),
        out
    );
    Ok(())
}

#[buck_test]
async fn test_build_configured_wrong_hash(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "root//:simple (<vagabond>#0123456789abcdef)",
            "--target-universe",
            "root//:universe",
        ])
        .await?;
    // TODO(nga): this should either fail or emit a warning.
    assert!(
        !result
            .build_report()?
            .results()
            .contains_key("root//:simple")
    );
    Ok(())
}

#[buck_test]
async fn test_build_configured_no_universe(buck: &Buck) {
    buck.build(&["root//:simple (<vagabond>)"])
        .await
        .expect_failure()
        .assert_stderr_matches("Targets with explicit configuration can only be built when the");
}
