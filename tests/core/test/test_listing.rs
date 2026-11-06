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
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::random_string;
use serde_json::Value;

enum TestDiscovery {
    Executed,
    Cached,
    Skipped,
}

async fn check_cache_uploaded(buck: &Buck) -> anyhow::Result<()> {
    let result = cache_uploads(buck).await?;
    assert_eq!(result.len(), 1);
    assert!(result[0]["success"].as_bool().unwrap_or_default());
    Ok(())
}

async fn cache_uploads(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    filter_events(buck, &["Event", "data", "SpanEnd", "data", "CacheUpload"]).await
}

async fn run_test_and_check_discovery_presence(
    buck: &Buck,
    discovery: TestDiscovery,
    args: &[&str],
) -> anyhow::Result<()> {
    buck.test(args).await?;
    let stdout = buck.log(&["what-ran"]).await?.stdout;

    assert!(stdout.contains("test.run"));
    match discovery {
        TestDiscovery::Executed => {
            for line in stdout.lines() {
                if line.contains("test.discovery") {
                    if line.contains("cached") {
                        anyhow::bail!("test.discovery was cached");
                    } else {
                        return Ok(());
                    }
                }
            }
            anyhow::bail!("test.discovery was not skipped");
        }
        TestDiscovery::Cached => {
            for line in stdout.lines() {
                if line.contains("test.discovery") {
                    if line.contains("cache") {
                        return Ok(());
                    } else {
                        anyhow::bail!("test.discovery was executed");
                    }
                }
            }
            anyhow::bail!("test.discovery was not skipped");
        }
        TestDiscovery::Skipped => {
            assert!(!stdout.contains("test.discovery"));
        }
    }
    Ok(())
}

#[buck_test]
async fn test_discovery_output_dir(buck: &Buck) -> anyhow::Result<()> {
    let args = ["//:ok"];
    run_test_and_check_discovery_presence(buck, TestDiscovery::Executed, &args).await?;

    let bad_buck_out_path = buck
        .cwd()
        .join("buck-out")
        .join("v2")
        .join("test")
        .join("buck-out");
    assert!(!bad_buck_out_path.exists());

    let discovery_output_path = buck
        .cwd()
        .join("buck-out")
        .join("v2")
        .join("test")
        .join("discovery");
    assert!(discovery_output_path.exists());
    Ok(())
}

#[buck_test]
async fn test_discovery_cached_on_dice(buck: &Buck) -> anyhow::Result<()> {
    let args = ["//:ok"];
    run_test_and_check_discovery_presence(buck, TestDiscovery::Executed, &args).await?;
    run_test_and_check_discovery_presence(buck, TestDiscovery::Skipped, &args).await
}

#[buck_test]
async fn test_failed_discovery_not_cached_on_dice(buck: &Buck) -> anyhow::Result<()> {
    let args = ["//:bad"];
    buck.test(&args)
        .await
        .expect_failure()
        .assert_stderr_matches("Failed to list tests");
    let whatran = buck.log(&["what-ran"]).await?.stdout;
    assert!(whatran.contains("test.discovery"));
    assert!(!whatran.contains("test.run"));

    buck.test(&args)
        .await
        .expect_failure()
        .assert_stderr_matches("Failed to list tests");
    let whatran = buck.log(&["what-ran"]).await?.stdout;
    assert!(whatran.contains("test.discovery"));
    Ok(())
}

#[buck_test]
async fn test_listing_uncacheable(buck: &Buck) -> anyhow::Result<()> {
    let seed = format!("test.seed={}", random_string());
    let args = [
        "-c",
        &seed,
        "-c",
        "test.remote_enabled=false",
        "-c",
        "test.local_enabled=true",
        "-c",
        "test.remote_cache_enabled=true",
        "//:listing_uncacheable",
    ];
    // Check it executed locally consistently
    run_test_and_check_discovery_presence(buck, TestDiscovery::Executed, &args).await?;
    buck.kill().await?;
    run_test_and_check_discovery_presence(buck, TestDiscovery::Executed, &args).await?;
    // Check cache is not uploaded
    let cached = cache_uploads(buck).await?;
    assert_eq!(cached.len(), 0);
    Ok(())
}

#[buck_test]
async fn test_discovery_cached_on_re(buck: &Buck) -> anyhow::Result<()> {
    let seed = format!("test.seed={}", random_string());
    let args = [
        "-c",
        &seed,
        "-c",
        "test.local_enabled=false",
        "-c",
        "test.remote_enabled=true",
        "-c",
        "test.remote_cache_enabled=true",
        "//:test",
    ];
    run_test_and_check_discovery_presence(buck, TestDiscovery::Executed, &args).await?;
    buck.kill().await?;
    run_test_and_check_discovery_presence(buck, TestDiscovery::Cached, &args).await?;
    buck.kill().await?;
    let args = [
        "-c",
        &seed,
        "-c",
        "test.remote_enabled=false",
        "-c",
        "test.local_enabled=true",
        "-c",
        "test.remote_cache_enabled=true",
        "//:test",
    ];
    run_test_and_check_discovery_presence(buck, TestDiscovery::Cached, &args).await
}

#[buck_test(env = [("BUCK2_TEST_SKIP_ACTION_CACHE_WRITE", "true")])]
async fn test_local_discovery_uploaded_to_cache(buck: &Buck) -> anyhow::Result<()> {
    let seed = format!("test.seed={}", random_string());
    let args = [
        "-c",
        &seed,
        "-c",
        "test.allow_cache_uploads=true",
        "-c",
        "test.remote_cache_enabled=true",
        "//:ok",
    ];
    run_test_and_check_discovery_presence(buck, TestDiscovery::Executed, &args).await?;
    check_cache_uploaded(buck).await
}
