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

use buck2_e2e::manifold::Bucket;
use buck2_e2e::manifold::use_ssl_cert_file;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;

const TEST_TRACE_ID: &str = "f115b5da-7d81-47cc-9c4a-57e283bfa384";
const BUCKET: &str = "buck2_logs";
const APIKEY: &str = "buck2_logs-key";

const EVENT_LOG_PLACEHOLDER: &str = "
Plants are living organisms that belong to the kingdom Plantae. They are characterized by their ability to produce their own food through photosynthesis, which is the process of converting sunlight, water, and carbon dioxide into glucose and oxygen. Plants come in many different shapes and sizes, from small mosses to towering trees, and can be found in almost every ecosystem on Earth. They play a vital role in the planet's ecology, serving as primary producers at the base of the food chain and providing habitats for a wide range of animal species. In addition to their ecological importance, plants have many practical uses for humans, such as food, medicine, clothing, and shelter.
";

/// 8 random characters.
fn random_name() -> String {
    random_string()[..8].to_owned()
}

#[buck_test(
    inplace = true,
    env = [
        ("BUCK2_TEST_MANIFOLD_CHUNK_BYTES", "32"),
        ("BUCK2_TEST_MANIFOLD_TTL_S", "84000"), // 1 day
    ],
)]
async fn test_persist_event_logs(buck: &mut Buck, tmp_path: &Path) -> anyhow::Result<()> {
    use_ssl_cert_file(buck);
    let local_log = tmp_path.join("test.txt");

    let manifold_name = format!("test_{}.txt", random_name());
    buck.debug(&[
        "persist-event-logs",
        "--manifold-name",
        &manifold_name,
        "--local-path",
        &local_log.to_string_lossy(),
        "--trace-id",
        TEST_TRACE_ID,
    ])
    .stdin(EVENT_LOG_PLACEHOLDER)
    .await?;

    assert!(local_log.exists());

    assert_eq!(std::fs::read_to_string(&local_log)?, EVENT_LOG_PLACEHOLDER);

    let manifold_size = Bucket::new(BUCKET, APIKEY)
        .file_size(&format!("flat/{manifold_name}"))
        .await?;
    // Some OSs return str, others int. Just enforce str to compare
    assert_eq!(manifold_size, EVENT_LOG_PLACEHOLDER.len().to_string());
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_persist_event_logs_not_uploaded(
    buck: &mut Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    use_ssl_cert_file(buck);
    let local_log = tmp_path.join("test.txt");

    let manifold_name = format!("test_{}.txt", random_name());
    buck.debug(&[
        "persist-event-logs",
        "--manifold-name",
        &manifold_name,
        "--local-path",
        &local_log.to_string_lossy(),
        "--no-upload",
        "--trace-id",
        TEST_TRACE_ID,
    ])
    .stdin(EVENT_LOG_PLACEHOLDER)
    .await?;

    assert!(local_log.exists());

    assert_eq!(std::fs::read_to_string(&local_log)?, EVENT_LOG_PLACEHOLDER);

    assert!(
        !Bucket::new(BUCKET, APIKEY)
            .exists(&format!("flat/{manifold_name}"))
            .await?
    );
    Ok(())
}
