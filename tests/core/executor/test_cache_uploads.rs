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
use buck2_e2e::utils::json_get;
use buck2_e2e::utils::random_string;
use serde_json::Value;

async fn assert_locally_executed_upload_attempted(buck: &Buck, count: usize) -> anyhow::Result<()> {
    assert_upload_attempted(buck, count).await
}

async fn assert_upload_attempted(buck: &Buck, count: usize) -> anyhow::Result<()> {
    let log = buck.log(&["show"]).await?.stdout;
    let mut uploads = Vec::new();
    let mut excluded_uploads = Vec::new();

    // CI lacks reliable write access to CAS, so count any upload that was
    // *attempted* — both genuine successes and infra-level rejections that
    // prove the action reached the cache-upload stage. PERMISSION_DENIED is
    // the missing-write-ACL case. INVALID_ARGUMENT is only tolerated when the
    // message identifies the specific "Outputs TTL -1 is too low" rejection
    // (raised when supporting CAS objects haven't been uploaded).
    for line in log.trim().lines() {
        let event: Value = serde_json::from_str(line)?;
        let Some(e) = json_get(&event, &["Event", "data", "SpanEnd", "data", "CacheUpload"]) else {
            continue;
        };
        let tolerated = e["success"].as_bool() == Some(true)
            || e["re_error_code"] == "PERMISSION_DENIED"
            || (e["re_error_code"] == "INVALID_ARGUMENT"
                && e["error"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("Outputs TTL -1 is too low"));
        if tolerated {
            uploads.push(e.clone());
        } else {
            excluded_uploads.push(e.clone());
        }
    }

    if uploads.len() != count {
        eprintln!("Expected {count} uploads");
        eprintln!("Actual uploads: {uploads:?}");
        eprintln!("Excluded uploads: {excluded_uploads:?}");
        panic!("Wrong number of uploads, see above");
    }
    Ok(())
}

#[buck_test]
async fn test_re_uploads(buck: &Buck) -> anyhow::Result<()> {
    let text = format!("write.text={}", random_string());
    buck.build(&["root//:write", "-c", &text]).await?;
    assert_locally_executed_upload_attempted(buck, 1).await
}

#[buck_test]
async fn test_re_uploads_dir(buck: &Buck) -> anyhow::Result<()> {
    let text = format!("write.text={}", random_string());
    buck.build(&["root//:write_in_dir", "-c", &text]).await?;
    assert_locally_executed_upload_attempted(buck, 1).await
}

#[buck_test]
async fn test_re_uploads_limit(buck: &Buck) -> anyhow::Result<()> {
    let text = format!("write.text={}", random_string());
    buck.build(&["root//:write_xxl", "-c", &text]).await?;
    assert_locally_executed_upload_attempted(buck, 0).await
}

#[buck_test]
async fn test_re_uploads_default(buck: &Buck) -> anyhow::Result<()> {
    let text = format!("write.text={}", random_string());
    buck.build(&["root//:write_default", "-c", &text]).await?;
    assert_locally_executed_upload_attempted(buck, 0).await?;

    let text = format!("write.text={}", random_string());
    buck.build(&[
        "root//:write_default",
        "-c",
        &text,
        "-c",
        "buck2.default_allow_cache_upload=true",
    ])
    .await?;
    assert_locally_executed_upload_attempted(buck, 1).await
}
