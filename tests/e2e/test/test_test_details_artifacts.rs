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
use buck2_e2e::testx::artifacts_list;
use buck2_e2e::testx::extract_test_run_id;
use buck2_e2e::utils::json_get;

#[buck_test(inplace = true)]
async fn test_passing_test_details_uploaded_to_artifacts(buck: &Buck) -> anyhow::Result<()> {
    let output = buck
        .test(&[
            "fbcode//buck2/tests/targets/rules/python_test:test_produce_test_details",
            "--remote-only",
            "--",
            "--upload-passing-details",
        ])
        .await?;

    let run_id = extract_test_run_id(&output.stderr)?;

    let tests_artifact_data = artifacts_list(run_id).await?;

    assert!(
        !tests_artifact_data.is_empty(),
        "Should have artifact data for test run"
    );

    let details_artifact = tests_artifact_data
        .iter()
        .flat_map(|data| &data.artifacts)
        .find(|artifact| {
            json_get(artifact, &["annotations", "type", "test_details_artifact"]).is_some()
        });

    let Some(details_artifact) = details_artifact else {
        panic!("Should have artifact of type test_details_artifact: {tests_artifact_data:?}");
    };

    assert!(
        json_get(details_artifact, &["handle", "everstore_handle"]).is_some(),
        "Details artifact everstore_handle should not be None"
    );
    assert!(
        json_get(details_artifact, &["handle", "everstore_handle", "handle"]).is_some(),
        "Everstore handle string should not be None"
    );
    Ok(())
}
