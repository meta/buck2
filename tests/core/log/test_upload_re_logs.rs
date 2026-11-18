/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::manifold::Bucket;
use buck2_e2e::manifold::use_ssl_cert_file;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::json_get;
use serde_json::Value;

const BUCKET: &str = "buck2_re_logs";
const APIKEY: &str = "buck2_re_logs-key";

async fn extract_re_session_id(buck: &Buck) -> anyhow::Result<String> {
    let result = buck.log(&["show"]).await?;
    let mut session_id = None;
    for line in result.stdout.lines() {
        let event: Value = serde_json::from_str(line)?;
        session_id = json_get(
            &event,
            &[
                "Event",
                "data",
                "Instant",
                "data",
                "ReSession",
                "session_id",
            ],
        )
        .cloned();
        if session_id
            .as_ref()
            .is_some_and(|id| id.as_str().is_none_or(|id| !id.is_empty()))
        {
            break;
        }
    }
    let session_id = session_id.expect("no RE session id in the event log");
    Ok(session_id
        .as_str()
        .with_context(|| format!("The RE session id `{session_id}` is not a string"))?
        .to_owned())
}

#[buck_test(env = [("SANDCASTLE", "1")])] // wait for logs to finish uploading
async fn test_upload_re_logs(buck: &mut Buck) -> anyhow::Result<()> {
    use_ssl_cert_file(buck);
    // Build a trivial action
    buck.build(&["root//:run"]).await?;

    let session_id = extract_re_session_id(buck).await?;
    buck.debug(&["upload-re-logs", "--session-id", &session_id])
        .await?;
    assert!(
        Bucket::new(BUCKET, APIKEY)
            .exists(&format!("flat/{session_id}.log.zst"))
            .await?
    );
    Ok(())
}
