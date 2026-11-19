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

use buck2_e2e::InvocationRecord;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use serde_json::Value;

async fn observed_settings(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    filter_events(
        buck,
        &["Event", "data", "Instant", "data", "BuckSettingsObserved"],
    )
    .await
}

fn settings_hash(path: &Path) -> anyhow::Result<String> {
    Ok(InvocationRecord::read(path)?["settings_hash"]
        .as_str()
        .expect("a settings hash")
        .to_owned())
}

#[buck_test]
async fn test_settings_reported_once_per_daemon(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let first = tmp_path.join("first.json");
    buck.audit_config(&[
        "--unstable-write-invocation-record",
        &first.to_string_lossy(),
    ])
    .await?;
    let observed = observed_settings(buck).await?;
    assert_eq!(observed.len(), 1);
    let first_hash = settings_hash(&first)?;
    assert!(first_hash.starts_with("buck2settings-"));
    assert_eq!(observed[0]["hash"], first_hash.as_str());
    let settings: Value =
        serde_json::from_str(observed[0]["settings_json"].as_str().expect("settings"))?;
    assert_eq!(
        settings["analysis"].get("record_requested_anon_targets"),
        Some(&Value::Null)
    );

    // The same daemon does not report the same settings again.
    buck.audit_config(&[]).await?;
    assert_eq!(observed_settings(buck).await?, Vec::<Value>::new());

    // A changed daemon setting starts a new daemon, which reports the new
    // value.
    let second = tmp_path.join("second.json");
    let result = buck
        .audit_config(&[
            "--setting",
            "analysis.record_requested_anon_targets=false",
            "--unstable-write-invocation-record",
            &second.to_string_lossy(),
        ])
        .await?;
    assert!(result.stderr.contains("restarting daemon"));
    let observed = observed_settings(buck).await?;
    assert_eq!(observed.len(), 1);
    let second_hash = settings_hash(&second)?;
    assert_eq!(observed[0]["hash"], second_hash.as_str());
    assert_ne!(second_hash, first_hash);
    let settings: Value =
        serde_json::from_str(observed[0]["settings_json"].as_str().expect("settings"))?;
    assert_eq!(settings["analysis"]["record_requested_anon_targets"], false);

    // A changed invocation setting keeps the daemon, which still reports
    // the combination it has not served before.
    let third = tmp_path.join("third.json");
    let result = buck
        .audit_config(&[
            "--setting",
            "analysis.record_requested_anon_targets=false",
            "--setting",
            "agent_materialization.enabled=true",
            "--unstable-write-invocation-record",
            &third.to_string_lossy(),
        ])
        .await?;
    assert!(!result.stderr.contains("restarting daemon"));
    let observed = observed_settings(buck).await?;
    assert_eq!(observed.len(), 1);
    let third_hash = settings_hash(&third)?;
    assert_eq!(observed[0]["hash"], third_hash.as_str());
    assert!(third_hash != first_hash && third_hash != second_hash);
    let settings: Value =
        serde_json::from_str(observed[0]["settings_json"].as_str().expect("settings"))?;
    assert_eq!(settings["agent_materialization"]["enabled"], true);
    Ok(())
}
