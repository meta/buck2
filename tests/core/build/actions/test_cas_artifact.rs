/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::BuckOutput;
use buck2_e2e::http::sha1_hex;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;

fn read_output(result: &BuckOutput, target: &str) -> anyhow::Result<Vec<u8>> {
    Ok(std::fs::read(
        result.build_report()?.output_for_target(target)?,
    )?)
}

fn not_found(digest: &str) -> String {
    format!("The digest `{digest}` was not found in the CAS under use case `buck2-testing`")
}

#[buck_test]
async fn test_cas_artifact_expiration_out_of_range(buck: &Buck) {
    // Regression test: this used to panic the daemon instead of reporting an error.
    buck.build(&["root//:out_of_range_expiration"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Out-of-range value `4611686018427387904` for expires_after_timestamp",
        );
}

#[buck_test]
async fn test_cas_artifact_missing_digest_is_a_clear_error(buck: &Buck) {
    let content = random_string();
    let digest = format!("{}:{}", sha1_hex(content.as_bytes()), content.len());
    buck.build(&[
        "root//:missing",
        "-c",
        &format!("test.missing_digest={digest}"),
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(&not_found(&digest));
}

#[buck_test]
async fn test_cas_artifact_from_another_namespace_is_copied_into_the_daemons(
    buck: &Buck,
) -> anyhow::Result<()> {
    // `buck2-default` and the `buck2-testing` use case this daemon runs under are separate CAS
    // namespaces (see the harness). Seed a blob in the former by running a remote action on it
    // there.
    let content = random_string();
    let content_config = format!("test.content={content}");
    buck.build(&[
        "root//:seed",
        "--remote-only",
        "--no-remote-cache",
        "-c",
        "buck2_re_client.override_use_case=buck2-default",
        "-c",
        &content_config,
    ])
    .await?;
    let seed = buck
        .build(&["root//:seed_content", "-c", &content_config])
        .await?;
    let written = read_output(&seed, "root//:seed_content")?;
    let digest = format!("{}:{}", sha1_hex(&written), written.len());
    let digest_config = format!("test.digest={digest}");

    // The seeding command ran under the other use case in this daemon and left that namespace's
    // expiration on the shared digest, from which the daemon would answer "present" without asking
    // RE. A fresh daemon asks. Its own namespace does not have the blob yet; without this check the
    // rest of the test would pass just as well if the two use cases shared a namespace and no copy
    // ever happened.
    buck.kill().await?;
    buck.build(&["root//:canonical", "-c", &digest_config])
        .await
        .expect_failure()
        .assert_stderr_matches(&not_found(&digest));

    // The artifact names the other namespace's use case; the daemon copies the blob into its own.
    let res = buck
        .build(&["root//:reconciled", "-c", &digest_config])
        .await?;
    assert_eq!(read_output(&res, "root//:reconciled")?, written);

    // Now the same digest resolves in the daemon's own namespace by itself...
    let canonical = buck
        .build(&["root//:canonical", "-c", &digest_config])
        .await?;
    assert_eq!(read_output(&canonical, "root//:canonical")?, written);

    // ...and a remote action in that namespace can consume it, which the copy is for.
    let check = buck
        .build(&[
            "root//:check",
            "--remote-only",
            "--no-remote-cache",
            "-c",
            &digest_config,
        ])
        .await?;
    assert_eq!(read_output(&check, "root//:check")?, written);
    Ok(())
}

#[buck_test]
async fn test_cas_artifact_larger_than_an_inline_rpc_is_copied(buck: &Buck) -> anyhow::Result<()> {
    // As above, with a blob well past what any RPC inlines, so the copy has to go through the
    // file-shaped download and upload.
    let seed_config = format!("test.seed={}", random_string());
    buck.build(&[
        "root//:seed_big",
        "--remote-only",
        "--no-remote-cache",
        "-c",
        "buck2_re_client.override_use_case=buck2-default",
        "-c",
        &seed_config,
    ])
    .await?;
    let built = buck
        .build(&["root//:big_content", "-c", &seed_config])
        .await?;
    let written = read_output(&built, "root//:big_content")?;
    assert_eq!(written.len(), 5_000_000);
    let digest = format!("{}:{}", sha1_hex(&written), written.len());
    let digest_config = format!("test.big_digest={digest}");

    buck.kill().await?;
    buck.build(&["root//:canonical_big", "-c", &digest_config])
        .await
        .expect_failure()
        .assert_stderr_matches(&not_found(&digest));

    let res = buck
        .build(&["root//:reconciled_big", "-c", &digest_config])
        .await?;
    assert_eq!(read_output(&res, "root//:reconciled_big")?, written);

    let canonical = buck
        .build(&["root//:canonical_big", "-c", &digest_config])
        .await?;
    assert_eq!(read_output(&canonical, "root//:canonical_big")?, written);

    let check = buck
        .build(&[
            "root//:check_big",
            "--remote-only",
            "--no-remote-cache",
            "-c",
            &digest_config,
        ])
        .await?;
    assert_eq!(read_output(&check, "root//:check_big")?, written);
    Ok(())
}

#[buck_test(data_dir = "sha1_under_blake3")]
async fn test_cas_artifact_with_a_sha1_digest_reconciles_under_a_blake3_daemon(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Production daemons prefer BLAKE3-KEYED and accept sha1 for digests rules supply. The copy
    // verifies what it downloaded against the artifact's digest, which only works if it hashes
    // with that digest's own algorithm rather than the daemon's preferred one. Sources are hashed
    // with sha1 in this fixture, so a remote action's upload of a source file seeds a sha1-named
    // blob in the other namespace.
    let content = random_string().into_bytes();
    std::fs::write(buck.cwd().join("seed.txt"), &content)?;
    buck.build(&[
        "root//:seed",
        "--remote-only",
        "--no-remote-cache",
        "-c",
        "buck2_re_client.override_use_case=buck2-default",
    ])
    .await?;
    let digest = format!("{}:{}", sha1_hex(&content), content.len());
    let digest_config = format!("test.digest={digest}");

    buck.kill().await?;
    buck.build(&["root//:canonical", "-c", &digest_config])
        .await
        .expect_failure()
        .assert_stderr_matches(&not_found(&digest));

    let res = buck
        .build(&["root//:reconciled", "-c", &digest_config])
        .await?;
    assert_eq!(read_output(&res, "root//:reconciled")?, content);

    let canonical = buck
        .build(&["root//:canonical", "-c", &digest_config])
        .await?;
    assert_eq!(read_output(&canonical, "root//:canonical")?, content);
    Ok(())
}
