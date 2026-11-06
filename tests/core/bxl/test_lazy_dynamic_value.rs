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
use indexmap::IndexMap;
use serde_json::Value;
use serde_json::json;

#[buck_test]
async fn test_resolve(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:lazy_dynamic_value.bxl:resolve"]).await?;
    Ok(())
}

#[buck_test]
async fn test_callback_failure_propagates_without_catch(buck: &Buck) {
    buck.bxl(&[
        "//:lazy_dynamic_value.bxl:resolve_without_catch",
        "--",
        "--target",
        "callback_fails",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("producer callback failed");
}

#[buck_test]
async fn test_catch_callback_failure(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .bxl(&[
            "//:lazy_dynamic_value.bxl:catch",
            "--",
            "--target",
            "callback_fails",
        ])
        .await?;
    assert!(res.stdout.contains("producer callback failed"));
    Ok(())
}

#[buck_test]
async fn test_catch_upstream_artifact_failure(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .bxl(&[
            "//:lazy_dynamic_value.bxl:catch",
            "--",
            "--target",
            "artifact_fails",
        ])
        .await?;
    assert!(res.stdout.contains("failing_upstream"));
    assert!(!res.stdout.contains("producer callback should not run"));
    Ok(())
}

#[buck_test]
async fn test_mixed_batch(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.bxl(&["//:lazy_dynamic_value.bxl:mixed_batch"]).await?;
    let summary: IndexMap<String, Value> = serde_json::from_str(&res.stdout)?;
    assert_eq!(
        summary.keys().collect::<Vec<_>>(),
        ["ok", "callback_fails", "ok2", "artifact_fails"]
    );
    assert_eq!(summary["ok"], json!({"ok": "ok"}));
    assert_eq!(summary["ok2"], json!({"ok": "ok2"}));
    let error = |key: &str| {
        summary[key]["error"]
            .as_str()
            .expect("an error message")
            .to_owned()
    };
    assert!(error("callback_fails").contains("producer callback failed"));
    assert!(error("artifact_fails").contains("failing_upstream"));
    assert!(!error("artifact_fails").contains("producer callback should not run"));
    Ok(())
}

#[buck_test]
async fn test_resolve_anon_target_dynamic_value(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:lazy_dynamic_value.bxl:anon_target_dynamic_value"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_rejects_bxl_dynamic_value(buck: &Buck) {
    buck.bxl(&["//:lazy_dynamic_value.bxl:bxl_dynamic_value"])
        .await
        .expect_failure()
        .assert_stderr_matches("does not accept dynamic value .* owned by a BXL invocation");
}
