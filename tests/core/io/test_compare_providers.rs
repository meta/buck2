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

async fn run_test(buck: &Buck, name: &str) -> anyhow::Result<()> {
    let (kind, targets) = if cfg!(windows) {
        ("win", vec!["file_metadata/nested"])
    } else {
        // Set up some symlinks. Do this here to avoid relying on the test runner
        // copying these correctly.
        let symlink_dir = buck.cwd().join("file_metadata").join("symlinks");
        std::fs::create_dir(&symlink_dir)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("../file", symlink_dir.join("internal"))?;
            std::os::unix::fs::symlink("/absolute", symlink_dir.join("external"))?;
        }

        (
            "unix",
            vec![
                "file_metadata",
                "file_metadata/symlinks/internal/traverse",
                "file_metadata/symlinks/external/traverse",
            ],
        )
    };

    buck.build(&[]).await?; // Start Buck2

    let mut args = vec!["file-status", "--show-matches"];
    args.extend(targets);
    let res = buck.debug(&args).await?;
    golden(&res.stdout, &format!("golden/{name}.{kind}.out"));
    assert!(!res.stdout.contains("MISMATCH"), "{}", res.stdout);
    Ok(())
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [
        ("buck2", "allow_eden_io", "false"),
        ("buck2", "source_digest_algorithm", "SHA1"),
    ],
)]
async fn test_default(buck: &Buck) -> anyhow::Result<()> {
    run_test(buck, "default").await
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [
        ("buck2", "allow_eden_io", "true"),
        ("buck2", "source_digest_algorithm", "SHA1"),
    ],
)]
async fn test_eden(buck: &Buck) -> anyhow::Result<()> {
    run_test(buck, "default").await
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [
        ("buck2", "allow_eden_io", "false"),
        ("buck2", "source_digest_algorithm", "BLAKE3-KEYED"),
    ],
)]
async fn test_blake3(buck: &Buck) -> anyhow::Result<()> {
    run_test(buck, "blake3").await
}

#[buck_test(
    setup_eden = true,
    extra_buck_config = [("buck2", "source_digest_algorithm", "BLAKE3-KEYED")],
)]
async fn test_eden_blake3(buck: &Buck) -> anyhow::Result<()> {
    run_test(buck, "blake3").await
}

// FIXME(minglunli): Test was passing for the wrong reason, reached out to Eden and will disable the test for now

// Thrift Read API is only enabled for MacOS
// @buck_test(
//     setup_eden=True,
//     extra_buck_config={
//         "buck2": {
//             "allow_eden_io": "true",
//             "use_eden_thrift_read": "true",
//         }
//     },
//     skip_for_os=["windows", "linux"],
// )
// async def test_eden_large_config_file(buck: Buck) -> None:
//     # String below is ~55 bytes, so this writes ~2.7GB which is comfortably over 2GB
//     writes = 50000000
//
//     with open(buck.cwd / ".buckconfig", "a") as f:
//         # Picked a random config and writing that a bunch of times to make the file large
//         # This will result in a .buckconfig file to actually be valid, so an error should
//         # either be a size issue or it should succeed
//         while writes > 0:
//             f.write("[buck2]\n")
//             f.write("compute_action_inputs_hash_enabled = false\n")
//             writes -= 1
//
//     await expect_failure(
//         buck.build(),
//         # TODO(minglunli): Should check for Thrift size limit error message, but CI is on old Eden version 20250403-074430
//         #                  There's a bug that's fixed in 20250422-124337 which lets Eden return the correct error message
//         # stderr_regex="Thrift size limit",
//     )

#[buck_test(setup_eden = true, data_dir = "test_large_action_input")]
async fn test_large_action_input(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:large_action_input"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//:large_action_input")?;
    let size: u64 = std::fs::read_to_string(output)?.trim_end().parse()?;
    assert!(size > 2147483648, "{size}"); // Wrote more than 2GB
    Ok(())
}
