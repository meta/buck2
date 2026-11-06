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

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;

/// Checks that the file is executable by owner. Windows doesn't have executable permission.
fn assert_executable(output: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(std::fs::metadata(output)?.permissions().mode() & 0o100, 0);
    }
    #[cfg(not(unix))]
    let _ = output;
    Ok(())
}

/// Checks that the file is not executable by owner.
fn assert_not_executable(output: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(output)?.permissions().mode() & 0o100, 0);
    }
    #[cfg(not(unix))]
    let _ = output;
    Ok(())
}

/// A relative path as Python prints it: with the platform's separator.
fn python_str(path: &Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.into_owned()
    }
}

fn read_text(path: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(path)?)
}

#[buck_test(data_dir = "write")]
async fn test_write_files(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "//:simple",
            "//:uses_declared_output",
            "//:uses_declared_output_as_output",
            "//:declares_output",
            "//:is_executable",
            "//:writes_array_of_commands",
            "//:writes_command_lines",
            "//:writes_frozen_command_lines",
            "//:with_inputs_and_copy",
            "//:writes_absolute",
        ])
        .await?;
    let build_report = result.build_report()?;

    let simple = match build_report
        .outputs_for_target_rel("//:simple", "DEFAULT")?
        .as_slice()
    {
        [simple] => python_str(simple),
        outputs => anyhow::bail!("`//:simple` has {} outputs", outputs.len()),
    };

    let output = build_report.output_for_target("//:uses_declared_output")?;
    assert_eq!(read_text(&output)?.trim_end(), "some content");
    assert_not_executable(&output)?;

    let output = build_report.output_for_target("//:uses_declared_output_as_output")?;
    assert_eq!(read_text(&output)?.trim_end(), "some content");
    assert_not_executable(&output)?;

    let output = build_report.output_for_target("//:declares_output")?;
    assert_eq!(read_text(&output)?.trim_end(), "some content");
    assert_not_executable(&output)?;

    let output = build_report.output_for_target("//:is_executable")?;
    assert_eq!(read_text(&output)?.trim_end(), "some content");
    assert_executable(&output)?;

    let output = build_report.output_for_target("//:writes_array_of_commands")?;
    assert_eq!(
        read_text(&output)?.trim_end(),
        format!("{simple}\nsome content")
    );
    assert_not_executable(&output)?;

    let output = build_report.output_for_target("//:writes_command_lines")?;
    assert_eq!(
        read_text(&output)?.trim_end(),
        format!("{simple}\nsome content")
    );
    assert_not_executable(&output)?;

    let output = build_report.output_for_target("//:writes_frozen_command_lines")?;
    assert_eq!(read_text(&output)?.trim_end(), simple);
    assert_not_executable(&output)?;

    let output = build_report.output_for_target("//:with_inputs_and_copy")?;
    assert_eq!(read_text(&output)?.trim_end(), "some content");

    let output = build_report.output_for_target("//:writes_absolute")?;
    assert!(Path::new(read_text(&output)?.trim()).is_absolute());
    Ok(())
}

#[buck_test(data_dir = "write")]
async fn test_shell_quoted_words_survive_bash(buck: &Buck) -> anyhow::Result<()> {
    // `cmd_args(.., quote = "shell")` written to an argsfile: bash must read every word back
    // as one word, braces included (bare braces would be brace-expanded).
    let result = buck.build(&["//:shell_quoted_roundtrip"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//:shell_quoted_roundtrip")?;
    assert_eq!(read_text(&output)?, "<a{b,c}>\n<{a..c}>\n<x y>\n");
    Ok(())
}

#[buck_test(data_dir = "write_fails")]
async fn test_write_files_fails_invalid_content(buck: &Buck) {
    buck.build(&["//:fails_on_invalid_contents"])
        .await
        .expect_failure()
        .assert_stderr_matches("Type of parameter `content`");
}

#[buck_test(data_dir = "write_fails")]
async fn test_write_files_fails_invalid_output(buck: &Buck) {
    buck.build(&["//:fails_on_invalid_output"])
        .await
        .expect_failure()
        .assert_stderr_matches("Type of parameter `output`");
}

#[buck_test(data_dir = "write")]
async fn test_output_size(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:simple"]).await?;

    let output_size = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "ActionExecution",
            "output_size",
        ],
    )
    .await?;

    assert!(!output_size.is_empty());
    assert_eq!(output_size[0], 8);
    Ok(())
}
