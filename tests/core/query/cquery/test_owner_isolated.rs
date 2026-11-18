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

async fn log_show(buck: &Buck) -> anyhow::Result<String> {
    Ok(buck.log(&["show"]).await?.stdout)
}

#[buck_test(data_dir = "simple")]
async fn test_query_owner(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "--target-universe=root//bin:the_binary",
            "owner(bin/TARGETS.fixture)",
        ])
        .await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(
        replace_hash(&result.stdout),
        "root//bin:the_binary (root//platforms:platform1#<HASH>)\n"
    );
    Ok(())
}

#[buck_test(data_dir = "simple", allow_soft_errors = true)]
async fn test_cquery_owner_missing_file(buck: &Buck) -> anyhow::Result<()> {
    let absolute = buck
        .cwd()
        .join("bin/missing.file")
        .to_string_lossy()
        .replace('\\', "/");
    for path in [
        "bin/missing.file",
        "missing/file",
        "root//bin/missing.file",
        &absolute,
    ] {
        let result = buck
            .cquery(&[
                "--target-universe=root//bin:the_binary",
                &format!("owner('{path}')"),
            ])
            .await?;
        assert_eq!(result.exit_code(), Some(0));
        assert_eq!(result.stdout, "");
        assert!(log_show(buck).await?.contains("owner_invalid_path"));
    }

    let result = buck
        .cquery(&[
            "--target-universe=root//bin:the_binary",
            "owner(bin/TARGETS.fixture)",
        ])
        .rel_cwd("bin")
        .await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");
    let log = log_show(buck).await?;
    assert!(log.contains("owner_invalid_path"));
    assert!(log.contains("File not found: root//bin/bin/TARGETS.fixture"));
    Ok(())
}

#[buck_test(data_dir = "simple", allow_soft_errors = true)]
async fn test_cquery_owner_missing_file_without_explicit_universe(
    buck: &Buck,
) -> anyhow::Result<()> {
    for query in [
        "owner(bin/missing.file)",
        "deps(root//bin:the_binary) intersect owner(bin/missing.file)",
    ] {
        let result = buck.cquery(&[query]).await?;
        assert_eq!(result.exit_code(), Some(0));
        assert_eq!(result.stdout, "");
        assert!(log_show(buck).await?.contains("owner_invalid_path"));
    }
    Ok(())
}

#[buck_test(data_dir = "simple", allow_soft_errors = true)]
async fn test_cquery_owner_mixed_existing_and_missing_files(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "--target-universe=root//bin:the_binary",
            "owner(fileset(bin/TARGETS.fixture missing/file))",
        ])
        .await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(
        replace_hash(&result.stdout),
        "root//bin:the_binary (root//platforms:platform1#<HASH>)\n"
    );
    assert!(log_show(buck).await?.contains("owner_invalid_path"));
    Ok(())
}

#[buck_test(data_dir = "simple")]
async fn test_cquery_owner_existing_file_without_owner(buck: &Buck) -> anyhow::Result<()> {
    for (universe, path) in [
        ("root//bin:the_binary", ".buckconfig"),
        ("root//:bin", "bin/TARGETS.fixture"),
    ] {
        let result = buck
            .cquery(&[
                &format!("--target-universe={universe}"),
                &format!("owner('{path}')"),
            ])
            .await?;
        assert_eq!(result.exit_code(), Some(0));
        assert_eq!(result.stdout, "");
    }
    Ok(())
}

#[buck_test(data_dir = "simple", allow_soft_errors = true)]
async fn test_cquery_owner_file_created(buck: &Buck) -> anyhow::Result<()> {
    let path = buck.cwd().join("bin/new.file");
    let args = [
        "--target-universe=root//bin:the_binary",
        "owner(bin/new.file)",
    ];
    let result = buck.cquery(&args).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");
    assert!(log_show(buck).await?.contains("owner_invalid_path"));

    std::fs::write(&path, "")?;
    let result = buck.cquery(&args).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");
    assert!(!log_show(buck).await?.contains("owner_invalid_path"));

    std::fs::remove_file(&path)?;
    let result = buck.cquery(&args).await?;
    assert_eq!(result.exit_code(), Some(0));
    assert_eq!(result.stdout, "");
    assert!(log_show(buck).await?.contains("owner_invalid_path"));
    Ok(())
}
