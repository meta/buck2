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

use buck2_e2e::BuckOutput;
use buck2_e2e::mode::Mode;
use buck2_e2e::mode::get_mode_from_platform;
use buck2_e2e::prelude::*;

/// The outputs `targets --show-output` (or `--show-full-output`) printed, as absolute paths.
fn show_output_outputs(
    targets_result: &BuckOutput,
    root: &Path,
) -> anyhow::Result<Vec<(String, PathBuf)>> {
    Ok(targets_result
        .target_to_build_output()?
        .into_iter()
        .map(|(target, output)| (target, root.join(output)))
        .collect())
}

#[buck_test(inplace = true)]
async fn test_targets(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .targets(&["fbcode//buck2/tests/targets/commands:"])
        .await?;

    let targets = [
        "fbcode//buck2/tests/targets/commands:dynamic",
        "fbcode//buck2/tests/targets/commands:exported",
        "fbcode//buck2/tests/targets/commands:lib",
    ];

    for target in targets {
        assert!(result.stdout.contains(target));
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_targets_errors(buck: &Buck) {
    buck.targets(&[
        "fbcode//buck2/tests/targets/commands:",
        "fbcode//buck2/tests/targets/non_existent_path:",
    ])
    .await
    .expect_failure()
    .assert_exit_code(ExitCode::UserError);
}

#[buck_test(inplace = true)]
async fn test_explicit_targets_errors(buck: &Buck) {
    buck.targets(&["fbcode//buck2/tests/targets/commands:notarealtarget"])
        .await
        .expect_failure()
        .assert_exit_code(ExitCode::UserError)
        .assert_stderr_matches(
            "Unknown target `notarealtarget` from package `fbcode//buck2/tests/targets/commands`",
        );
}

#[buck_test(inplace = true)]
async fn test_targets_with_config_value(buck: &Buck) -> anyhow::Result<()> {
    let targets_enabled_result = buck
        .targets(&[
            "--config",
            "user.targets_enabled=true",
            "fbcode//buck2/tests/targets/commands:",
        ])
        .await?;
    assert!(
        targets_enabled_result
            .stdout
            .contains("fbcode//buck2/tests/targets/commands:config_defined_target")
    );

    let targets_disabled_result = buck
        .targets(&[
            "--config",
            "user.targets_enabled=false",
            "fbcode//buck2/tests/targets/commands:",
        ])
        .await?;
    assert!(
        !targets_disabled_result
            .stdout
            .contains("fbcode//buck2/tests/targets/commands:config_defined_target")
    );

    let targets_cell_rel_result = buck
        .targets(&[
            "--config",
            "fbsource//user.targets_enabled=true",
            "fbcode//buck2/tests/targets/commands:",
        ])
        .await?;
    assert_eq!(
        targets_cell_rel_result.stdout,
        targets_disabled_result.stdout
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_targets_root_relative_from_fbcode(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.targets(&["//buck2/tests/targets/commands:"]).await?;

    let targets = [
        "fbcode//buck2/tests/targets/commands:dynamic",
        "fbcode//buck2/tests/targets/commands:exported",
        "fbcode//buck2/tests/targets/commands:lib",
    ];

    for target in targets {
        assert!(result.stdout.contains(target));
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_targets_show_output(buck: &Buck) -> anyhow::Result<()> {
    for target in [
        "fbcode//buck2/tests/targets/rules/genrule:executable_helper",
        "fbcode//buck2/tests/targets/rules/export_file:exported.txt",
    ] {
        let build_result = buck.build(&[target, "--show-output"]).await?;
        let targets_result = buck.targets(&[target, "--show-output"]).await?;

        let build_report = build_result.build_report()?;
        let build_report_outputs: Vec<(String, PathBuf)> = build_report
            .outputs_for_target(target, "DEFAULT")?
            .into_iter()
            .map(|output| (target.to_owned(), output))
            .collect();

        assert_eq!(
            show_output_outputs(&targets_result, build_report.root())?,
            build_report_outputs
        );
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_targets_show_output_subtargets(buck: &Buck) -> anyhow::Result<()> {
    const TARGET: &str = "fbcode//buck2/tests/targets/rules/cxx:my_cpp1";
    const SUBTARGET: &str = "compilation-database";
    const TARGET_WITH_SUBTARGET: &str =
        "fbcode//buck2/tests/targets/rules/cxx:my_cpp1[compilation-database]";

    let build_result = buck
        .build(&[
            TARGET_WITH_SUBTARGET,
            "--show-output",
            get_mode_from_platform(Mode::Dev),
        ])
        .await?;
    let targets_result = buck
        .targets(&[
            TARGET_WITH_SUBTARGET,
            "--show-output",
            get_mode_from_platform(Mode::Dev),
        ])
        .await?;

    let build_report = build_result.build_report()?;
    let build_report_outputs: Vec<(String, PathBuf)> = build_report
        .outputs_for_target(TARGET, SUBTARGET)?
        .into_iter()
        .map(|output| (TARGET_WITH_SUBTARGET.to_owned(), output))
        .collect();

    assert_eq!(
        show_output_outputs(&targets_result, build_report.root())?,
        build_report_outputs
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_targets_show_full_output(buck: &Buck) -> anyhow::Result<()> {
    for target in [
        "fbcode//buck2/tests/targets/rules/genrule:executable_helper",
        "fbcode//buck2/tests/targets/rules/export_file:exported.txt",
    ] {
        let build_result = buck.build(&[target, "--show-full-output"]).await?;
        let targets_result = buck.targets(&[target, "--show-full-output"]).await?;

        let build_report = build_result.build_report()?;
        let build_report_outputs: Vec<(String, PathBuf)> = build_report
            .outputs_for_target(target, "DEFAULT")?
            .into_iter()
            .map(|output| (target.to_owned(), output))
            .collect();

        assert_eq!(
            show_output_outputs(&targets_result, build_report.root())?,
            build_report_outputs
        );
    }
    Ok(())
}
