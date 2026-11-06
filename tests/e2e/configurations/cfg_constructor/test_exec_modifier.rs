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

const EXEC_MARKER_CONFLICT: &str = concat!(
    r"cannot change exec marker .* from .*is_exec_platform\[true\].*",
    r"to .*is_exec_platform\[false\]",
);

#[buck_test(inplace = false)]
async fn test_exec_modifier(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[
        "//test_exec_modifier.bxl:main",
        "-c",
        "buck2.preserve_exec_platform_marker=true",
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_exec_modifier_with_constructor_dropping_marker(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[
        "//test_exec_modifier.bxl:main",
        "-c",
        "test.drop_constructor_marker=true",
        "-c",
        "buck2.preserve_exec_platform_marker=true",
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_exec_modifier_rejects_conflicting_transition_marker(buck: &Buck) {
    for transition in ["single", "split"] {
        buck.cquery(&[
            &format!("deps(//:binary_with_{transition}_transition_exec_dep)"),
            "-c",
            "test.replace_transition_marker=true",
            "-c",
            "buck2.preserve_exec_platform_marker=true",
        ])
        .await
        .expect_failure()
        .assert_stderr_matches(EXEC_MARKER_CONFLICT);
    }
}

#[buck_test(inplace = false)]
async fn test_exec_modifier_rejects_conflicting_constructor_marker(buck: &Buck) {
    buck.cquery(&[
        "deps(//:binary)",
        "-c",
        "test.replace_constructor_marker=true",
        "-c",
        "buck2.preserve_exec_platform_marker=true",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(EXEC_MARKER_CONFLICT);
}

#[buck_test(inplace = false)]
async fn test_exec_modifier_legacy(buck: &Buck) -> anyhow::Result<()> {
    for config in [
        &[][..],
        &["-c", "buck2.preserve_exec_platform_marker=false"][..],
    ] {
        buck.bxl(&[&["//test_exec_modifier.bxl:legacy"], config].concat())
            .await?;
        buck.cquery(&[&["deps(//:binary_with_single_transition_exec_dep)"], config].concat())
            .await
            .expect_failure()
            .assert_stderr_matches("did not produce identical `PlatformInfo`");
        for (mutation, entrypoint) in [
            ("test.replace_transition_marker=true", "legacy_marker"),
            (
                "test.drop_constructor_marker=true",
                "legacy_constructor_marker",
            ),
            (
                "test.replace_constructor_marker=true",
                "legacy_constructor_marker",
            ),
        ] {
            let target = format!("//test_exec_modifier.bxl:{entrypoint}");
            buck.bxl(&[&[target.as_str()], config, &["-c", mutation]].concat())
                .await?;
        }
    }
    Ok(())
}

#[buck_test(inplace = false)]
async fn test_exec_marker_flag_changes_in_one_daemon(buck: &Buck) -> anyhow::Result<()> {
    for enabled in [true, false, true] {
        let entrypoint = if enabled {
            "preserved_marker"
        } else {
            "legacy_marker"
        };
        buck.bxl(&[
            &format!("//test_exec_modifier.bxl:{entrypoint}"),
            "-c",
            &format!("buck2.preserve_exec_platform_marker={enabled}"),
        ])
        .await?;
    }
    Ok(())
}
