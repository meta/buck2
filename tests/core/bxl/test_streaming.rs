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

fn assert_file_content_matches(file_path: &str, exptected_content: &str) -> std::io::Result<()> {
    let content = std::fs::read_to_string(file_path)?;
    assert_eq!(content, exptected_content);
    Ok(())
}

#[buck_test]
async fn test_streaming_output_ensured_artifact(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//streaming.bxl:streaming_output_ensured_artifact"])
        .await?;

    let mut streaming_output_artifact_idx = None;
    let mut output_file_path = "";
    let mut line_before_print_idx = None;

    for (idx, line) in result.stdout.lines().enumerate() {
        // output by `ctx.output.print(ensured_output)`
        if line.contains("output.txt") && streaming_output_artifact_idx.is_none() {
            output_file_path = line;
            streaming_output_artifact_idx = Some(idx);
        }
        // output by `ctx.output.print("Line before streaming print")`
        if line.contains("Line before streaming print") {
            line_before_print_idx = Some(idx);
        }
    }

    let streaming_output_artifact_idx = streaming_output_artifact_idx
        .expect("Cound not find the streaming print of ensured artifact");
    let line_before_print_idx =
        line_before_print_idx.expect("Cound not find the normal ctx.output.print");

    assert!(
        streaming_output_artifact_idx < line_before_print_idx,
        "The streaming print is not before the normal print"
    );

    assert_file_content_matches(output_file_path, "hello world!")?;
    Ok(())
}

#[buck_test]
async fn test_streaming_output(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//streaming.bxl:streaming_output"]).await?;

    let mut streaming_output_idx = None;
    let mut line_before_print_idx = None;

    for (idx, line) in result.stdout.lines().enumerate() {
        if line.contains("This is the streaming output") && streaming_output_idx.is_none() {
            streaming_output_idx = Some(idx);
        }
        if line.contains("Line before streaming print") {
            line_before_print_idx = Some(idx);
        }
    }

    let streaming_output_idx = streaming_output_idx.expect("Cound not find the streaming print");
    let line_before_print_idx =
        line_before_print_idx.expect("Cound not find the normal ctx.output.print");

    assert!(
        streaming_output_idx < line_before_print_idx,
        "The streaming print is not before the normal print"
    );
    Ok(())
}

#[buck_test]
async fn test_streaming_output_without_duplicates(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//streaming.bxl:streaming_output_without_duplicates"])
        .await?;

    let streaming_output = "This is the streaming output";
    let streaming_ensured_artifact_output = "output.txt";
    let normal_output = "This is the normal output";

    let stdout = &result.stdout;
    assert_eq!(
        stdout.matches(streaming_output).count(),
        1,
        "Expected only one streaming output"
    );
    assert_eq!(
        stdout.matches(streaming_ensured_artifact_output).count(),
        1,
        "Expected only one streaming ensured artifact output"
    );
    assert_eq!(
        stdout.matches(normal_output).count(),
        1,
        "Expected only one normal output"
    );

    // call again to check when bxl key is cached

    let result = buck
        .bxl(&["//streaming.bxl:streaming_output_without_duplicates"])
        .await?;
    let stdout = &result.stdout;
    assert_eq!(
        stdout.matches(streaming_output).count(),
        1,
        "Expected only one streaming output when cached"
    );
    assert_eq!(
        stdout.matches(streaming_ensured_artifact_output).count(),
        1,
        "Expected only one streaming ensured artifact output when cached"
    );
    assert_eq!(
        stdout.matches(normal_output).count(),
        1,
        "Expected only one normal output when cached"
    );
    Ok(())
}

#[buck_test]
async fn test_streaming_output_waits_on(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//streaming.bxl:streaming_output_waits_on"])
        .await?;

    let mut file0_idx = None;
    let mut file1_idx = None;
    let mut waits_on_output_idx = None;

    for (idx, line) in result.stdout.lines().enumerate() {
        if line.contains("output0.txt") {
            file0_idx = Some(idx);
        }
        if line.contains("output1.txt") {
            file1_idx = Some(idx);
        }
        if line.contains("Waits on two file") {
            waits_on_output_idx = Some(idx);
        }
    }

    let (Some(file0_idx), Some(file1_idx), Some(waits_on_output_idx)) =
        (file0_idx, file1_idx, waits_on_output_idx)
    else {
        panic!("Cound not find the streaming print");
    };

    assert!(
        file0_idx < waits_on_output_idx || file1_idx < waits_on_output_idx,
        "wait_on should after the one of the ensured artifact streaming print"
    );
    Ok(())
}

#[buck_test]
async fn test_streaming_output_json(buck: &Buck) -> anyhow::Result<()> {
    let check_output = || async {
        let result = buck.bxl(&["//streaming.bxl:streaming_output_json"]).await?;

        let mut file0_idx = None;
        let mut file1_idx = None;
        let mut waits_on_output_idx = None;
        let mut linde_before_streaming_idx = None;

        for (idx, line) in result.stdout.lines().enumerate() {
            let mut json_line = None;
            if line.contains("output0.txt") {
                file0_idx = Some(idx);
                json_line = Some(line);
            }
            if line.contains("output1.txt") {
                file1_idx = Some(idx);
                json_line = Some(line);
            }
            if line.contains("waits_on") {
                waits_on_output_idx = Some(idx);
                json_line = Some(line);
            }
            if line.contains("Line before print streaming") {
                linde_before_streaming_idx = Some(idx);
            }
            if let Some(json_line) = json_line {
                let json_line = json_line.trim();
                assert!(
                    json_line.starts_with('{') && json_line.ends_with('}'),
                    "Expected json line"
                );
            }
        }

        let (
            Some(file0_idx),
            Some(file1_idx),
            Some(waits_on_output_idx),
            Some(linde_before_streaming_idx),
        ) = (
            file0_idx,
            file1_idx,
            waits_on_output_idx,
            linde_before_streaming_idx,
        )
        else {
            panic!("Cound not find the output");
        };

        assert!(
            file0_idx < waits_on_output_idx || file1_idx < waits_on_output_idx,
            "wait_on should after the one of the ensured artifact streaming print"
        );

        assert!(
            linde_before_streaming_idx > file0_idx.max(file1_idx).max(waits_on_output_idx),
            "The streaming print is not after the normal print"
        );
        anyhow::Ok(())
    };

    check_output().await?;
    check_output().await?;
    Ok(())
}

#[buck_test]
async fn test_stream_output_fail(buck: &Buck) {
    let check = || async {
        buck.bxl(&["//streaming.bxl:stream_output_fail"])
            .await
            .expect_failure()
            .assert_stdout_matches("Streaming output");
    };

    check().await;
    // check when cached, streaming output is still printed
    check().await;
}
