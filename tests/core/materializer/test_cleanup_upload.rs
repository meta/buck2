/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;
use std::path::PathBuf;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::read_what_ran;
use serde_json::Value;

async fn unmaterialize_payload(buck: &Buck, payload: &Path) -> anyhow::Result<()> {
    // Clean-stale handles the local-only payload in three steps: upload it to
    // CAS, record it as "declared: cas download", then delete the local copy.
    // The run's own upload finishing must not interrupt it.
    buck.clean(&["--stale"]).await?;
    let results = filter_events(
        buck,
        &["Event", "data", "Instant", "data", "CleanStaleResult"],
    )
    .await?;
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].get("error").is_some_and(Value::is_null),
        "{results:?}"
    );
    let stats = &results[0]["stats"];
    // An interrupted run would already have uploaded and declared the payload,
    // so the audit check below would still pass; these stats and the deletion
    // check are what catch it.
    assert_eq!(
        stats["unmaterialization_upload_attempted_artifact_count"], 1,
        "{stats}"
    );
    assert_eq!(stats["delete_interrupted_artifact_count"], 0, "{stats}");
    assert_eq!(
        stats["unmaterialization_upload_succeeded_artifact_count"], 1,
        "{stats}"
    );
    assert_eq!(stats["unmaterialized_only_artifact_count"], 1, "{stats}");
    assert!(!payload.exists());
    let audit = buck.audit(&["deferred-materializer", "list"]).await?.stdout;
    assert!(audit.contains("declared: cas download"), "{audit}");
    Ok(())
}

async fn local_consumer_after_cleanup(buck: &Buck, cleanup: bool) -> anyhow::Result<()> {
    let before = buck.build(&["root//:before", "--no-remote-cache"]).await?;
    let output = before.build_report()?.output_for_target("root//:before")?;
    assert_eq!(std::fs::read_to_string(output)?, "payload");
    let commands = read_what_ran(buck, &[]).await?;
    assert_eq!(commands.len(), 2, "{commands:?}");
    assert!(
        commands
            .iter()
            .all(|command| command["reproducer"]["executor"] == "Local"),
        "{commands:?}"
    );

    let audit = buck.audit(&["deferred-materializer", "list"]).await?.stdout;
    let paths: Vec<PathBuf> = audit
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(path, _)| path)
        .filter(|path| path.ends_with("/payload.txt"))
        .map(|path| buck.cwd().join(path))
        .collect();
    assert_eq!(paths.len(), 1, "{audit}");
    let payload = &paths[0];
    assert_eq!(std::fs::read_to_string(payload)?, "payload");
    if cleanup {
        unmaterialize_payload(buck, payload).await?;
    }

    // Without cleanup, the second build reads the payload already on disk.
    // With cleanup, the local copy was deleted and buck2 tracks the payload
    // as "download from CAS", so the second build downloads it from CAS
    // (the `Materialization` check below). Either way `write_payload` does
    // not run again: the only command is the `root//:after` consumer.
    let after = buck.build(&["root//:after", "--no-remote-cache"]).await?;
    let output = after.build_report()?.output_for_target("root//:after")?;
    assert_eq!(std::fs::read_to_string(output)?, "payload");
    let commands = read_what_ran(buck, &[]).await?;
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert!(
        commands[0]["identity"]
            .as_str()
            .expect("an `identity`")
            .contains("root//:after"),
        "{commands:?}"
    );
    assert_eq!(
        commands[0]["reproducer"]["executor"], "Local",
        "{commands:?}"
    );
    assert_eq!(std::fs::read_to_string(payload)?, "payload");
    if cleanup {
        let events = filter_events(
            buck,
            &["Event", "data", "SpanEnd", "data", "Materialization"],
        )
        .await?;
        assert!(
            events.iter().any(|event| {
                buck.cwd().join(event["path"].as_str().expect("a `path`")) == *payload
                    && event["success"] == true
            }),
            "{events:?}"
        );
    }
    Ok(())
}

#[buck_test(skip_for_os = ["darwin", "windows"])]
#[expect(
    non_snake_case,
    reason = "the Python harness named the case after its id"
)]
async fn test_local_consumer_after_cleanup_False(buck: &Buck) -> anyhow::Result<()> {
    local_consumer_after_cleanup(buck, false).await
}

#[buck_test(skip_for_os = ["darwin", "windows"])]
#[expect(
    non_snake_case,
    reason = "the Python harness named the case after its id"
)]
async fn test_local_consumer_after_cleanup_True(buck: &Buck) -> anyhow::Result<()> {
    local_consumer_after_cleanup(buck, true).await
}
