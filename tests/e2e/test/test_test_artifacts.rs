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
use buck2_e2e::prelude::*;
use buck2_e2e::testx::artifacts_list;
use buck2_e2e::testx::extract_test_run_id;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::json_get;
use fbinit::FacebookInit;
use re_client_lib::CASDaemonClientCfg;
use re_client_lib::ClientBuilderCommonMethods;
use re_client_lib::ClientCfg;
use re_client_lib::EmbeddedCASDaemonClientCfg;
use re_client_lib::GetDigestsTtlRequest;
use re_client_lib::REClient;
use re_client_lib::REClientBuilder;
use re_client_lib::RemoteExecutionMetadata;
use re_client_lib::TDigest;
use re_client_lib::TDigestWithTtl;
use re_client_lib::create_default_config;
use serde_json::Value;

const ARTIFACTS_DIR_NAME: &str = "artifacts_directory";
const ANNOTATIONS_DIR_NAME: &str = "artifact_annotations_directory";
const TPX_EXEC_DIR: &str = "tpx_execution_dir";
const USE_CASE_ID: &str = "tpx-default";
const EMBEDDED_CAS_NAME: &str = "tpx";

/////////
// Helpers
/////////

fn has_dir(dir_name: &str, paths: &[Value]) -> anyhow::Result<bool> {
    for path in paths {
        let path = path
            .as_str()
            .with_context(|| format!("A materialized path that is not a string: {path}"))?;
        if path.contains(dir_name) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn assert_has_dir(dir_name: &str, paths: &[Value]) -> anyhow::Result<()> {
    assert!(
        has_dir(dir_name, paths)?,
        "Directory {dir_name} not found in {paths:?}"
    );
    Ok(())
}

fn assert_has_no_dir(dir_name: &str, paths: &[Value]) -> anyhow::Result<()> {
    assert!(
        !has_dir(dir_name, paths)?,
        "Directory {dir_name} found in {paths:?}"
    );
    Ok(())
}

/// The digest of an artifact TestX keeps in the CAS.
fn cas_digest(artifact: &Value) -> anyhow::Result<TDigest> {
    let digest = json_get(artifact, &["handle", "cas_digest"])
        .with_context(|| format!("The artifact has no CAS digest: {artifact}"))?;
    Ok(TDigest {
        hash: digest
            .get("hash")
            .and_then(Value::as_str)
            .with_context(|| format!("No `hash` in the digest {digest}"))?
            .to_owned(),
        size_in_bytes: digest
            .get("size_in_bytes")
            .and_then(Value::as_i64)
            .with_context(|| format!("No `size_in_bytes` in the digest {digest}"))?,
        ..Default::default()
    })
}

fn get_re_client(fb: FacebookInit) -> anyhow::Result<REClient> {
    let config = ClientCfg {
        cas_client_config: CASDaemonClientCfg::embedded_config(EmbeddedCASDaemonClientCfg {
            name: EMBEDDED_CAS_NAME.to_owned(),
            ..Default::default()
        }),
        ..create_default_config()
    };
    REClientBuilder::new(fb)
        .with_config(config)
        .build()
        .context("Creating an RE client")
}

async fn get_digests_ttl(
    re_client: &REClient,
    metadata: &RemoteExecutionMetadata,
    digests: Vec<TDigest>,
) -> anyhow::Result<Vec<TDigestWithTtl>> {
    let response = re_client
        .get_digests_ttl(
            metadata,
            GetDigestsTtlRequest {
                digests,
                ..Default::default()
            },
        )
        .await?;
    Ok(response.digests_with_ttl)
}

#[buck_test(inplace = true)]
async fn test_produce_artifacts(buck: &Buck) -> anyhow::Result<()> {
    buck.test(&["fbcode//buck2/tests/targets/rules/python_test:test_produce_artifacts"])
        .await?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_remote_artifact_directory_is_materialized_by_default(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python_test:test_produce_artifacts",
        "--remote-only",
    ])
    .await?;

    let materialized_paths = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "Materialization",
            "path",
        ],
    )
    .await?;

    assert_has_dir(ARTIFACTS_DIR_NAME, &materialized_paths)?;
    assert_has_dir(ANNOTATIONS_DIR_NAME, &materialized_paths)?;
    assert_has_dir(TPX_EXEC_DIR, &materialized_paths)?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_remote_artifact_directory_is_not_materialized_when_cas_support_enabled(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.test(&[
        "fbcode//buck2/tests/targets/rules/python_test:test_produce_artifacts_in_cas",
        "--no-remote-cache",
        "--remote-only",
    ])
    .await?;

    let materialized_paths = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "Materialization",
            "path",
        ],
    )
    .await?;
    assert_has_no_dir(ARTIFACTS_DIR_NAME, &materialized_paths)?;
    assert_has_dir(ANNOTATIONS_DIR_NAME, &materialized_paths)?;
    assert_has_dir(TPX_EXEC_DIR, &materialized_paths)?;
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_remote_artifact_has_cas_handle_with_right_ttl(buck: &Buck) -> anyhow::Result<()> {
    // SAFETY: as `#[fbinit::test]` does, at the start of the test, before anything else in this
    // process reads the state initialization sets up.
    let fb = unsafe { fbinit::perform_init() };

    let output = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python_test:test_produce_artifacts_in_cas",
            "--remote-only",
        ])
        .await?;

    let run_id = extract_test_run_id(&output.stderr)?;
    let tests_artifact_data = artifacts_list(run_id).await?;
    let re_metadata = RemoteExecutionMetadata {
        use_case_id: USE_CASE_ID.to_owned(),
        ..Default::default()
    };
    let re_client = get_re_client(fb)?;

    let expected_ttl = 13 * 86400; // TTL should be more than 13 days (in seconds)

    for data in &tests_artifact_data {
        for artifact in &data.artifacts {
            let tdigest = cas_digest(artifact)?;
            let digest_with_ttl = get_digests_ttl(&re_client, &re_metadata, vec![tdigest]).await?;
            assert_eq!(digest_with_ttl.len(), 1);
            assert!(digest_with_ttl[0].ttl >= expected_ttl);
        }
    }
    Ok(())
}
