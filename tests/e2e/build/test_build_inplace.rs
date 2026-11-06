/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::PathBuf;

use anyhow::Context;
use buck2_e2e::mode::Mode;
use buck2_e2e::mode::get_mode_from_platform;
use buck2_e2e::prelude::*;
use serde_json::Value;

// Only the disabled tests below use it.
#[cfg(any())]
async fn read_io_provider_for_last_build(buck: &Buck) -> anyhow::Result<Value> {
    let log = buck.log(&["show"]).await?.stdout;
    for line in log.lines() {
        let event: Value = serde_json::from_str(line)?;
        let io_provider = buck2_e2e::utils::json_get(
            &event,
            &[
                "Event",
                "data",
                "SpanStart",
                "data",
                "Command",
                "metadata",
                "io_provider",
            ],
        );
        if let Some(io_provider) = io_provider.filter(|p| p.as_str() != Some("")) {
            return Ok(io_provider.clone());
        }
    }

    anyhow::bail!("Could not find io_provider")
}

#[buck_test(inplace = true)]
async fn test_sh_binary_no_append_extension(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/rules/shell:no_extension";
    let args = [
        target,
        "--show-full-output",
        get_mode_from_platform(Mode::Dev),
    ];
    let result = buck.build(&args).await?;
    let output_dict = result.target_to_build_output()?;
    let output = PathBuf::from(
        output_dict
            .get(target)
            .with_context(|| format!("No output for `{target}`"))?,
    );

    // Verify that we created the script symlink without an extension
    assert!(
        output
            .parent()
            .context("The output has no parent")?
            .join("resources")
            .join("no_extension")
            .is_symlink()
    );

    // And that we're calling it without an extension as well
    let script = std::fs::read_to_string(&output)?;
    let last_script_line = script.lines().last().context("The script is empty")?;
    if cfg!(windows) {
        assert!(last_script_line.contains("%BUCK_PROJECT_ROOT%\\no_extension %*"));
    } else {
        assert!(last_script_line.contains("\"$BUCK_PROJECT_ROOT/no_extension\" \"$@\""));
    }
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_build_test_dependencies(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/rules/sh_test:test_with_env";
    let build = buck
        .build(&[target, "--build-test-info", "--build-report", "-"])
        .await?;
    let build_report = build.build_report()?;

    let path = ["results", target, "other_outputs"];
    let mut report = build_report.json();
    for p in path {
        report = report
            .get(p)
            .with_context(|| format!("No `{p}` in the build report"))?;
    }

    let has_file = report
        .as_object()
        .context("`other_outputs` is not an object")?
        .keys()
        .any(|artifact| artifact.contains("__file__"));

    assert!(!has_file);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_missing_outputs_error(buck: &Buck) {
    // Check that we a) say what went wrong, b) show the command
    buck.build(&[
        "fbcode//buck2/tests/targets/rules/genrule/bad:my_genrule_bad",
        // We really should make this an isolated test to avoid having to set this.
        "-c",
        "build.use_limited_hybrid=True",
        "--remote-only",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "(Action failed to produce output.*frecli|frecli.*OUTMISS|does not exist in the artifact)",
    );

    // Same, but locally.
    buck.build(&["fbcode//buck2/tests/targets/rules/genrule/bad:my_genrule_bad_local"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"(Action failed to produce outputs.*Stdout:\nHELLO_STDOUT.*Stderr:\nHELLO_STDERR|does not exist in the artifact)",
        );
}

#[buck_test(inplace = true)]
async fn test_local_execution(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/rules/genrule:echo_pythonpath";

    buck.kill().await?;
    let res = buck.build(&[target]).env("PYTHONPATH", "foobar").await?;

    let build_report = res.build_report()?;
    let output = build_report.output_for_target(target)?;
    assert_eq!(std::fs::read_to_string(output)?.trim_end(), "");
    Ok(())
}

// In case of timeouts and failures, best would be to just disable this test.
#[buck_test(inplace = true, skip_for_os = ["windows"])]
async fn test_asic_platforms(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/asic_platforms:uses_asic_tool";
    let result = buck.build(&[target, "--show-full-output"]).await?;
    let output = result
        .target_to_build_output()?
        .remove(target)
        .with_context(|| format!("No output for `{target}`"))?;
    let s = std::fs::read_to_string(&output)?;
    assert!(
        s.contains("facebook.com"),
        "expected 'facebook.com' in output: `{output}`"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_genrule_with_remote_execution_dependencies(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            get_mode_from_platform(Mode::Dev),
            "fbcode//buck2/tests/targets/rules/genrule/re_dependencies:remote_execution_dependencies",
            "--config",
            "build.default_remote_execution_use_case=buck2-testing",
            "--no-remote-cache",
            "--remote-only",
            "--show-full-output",
        ])
        .await?;
    let output_dict = result.target_to_build_output()?;
    for output in output_dict.values() {
        let deps: Value = serde_json::from_str(&std::fs::read_to_string(output)?)?;
        let deps = deps.as_array().context("The dependencies are not a list")?;
        assert_eq!(deps.len(), 1);
        let field = |key: &str| {
            deps[0]
                .get(key)
                .with_context(|| format!("No `{key}` in {}", deps[0]))
        };
        assert_eq!(field("smc_tier")?, "noop");
        assert_eq!(field("id")?, "foo");
        // reservation_id is a random string which is 20 characters long
        assert_eq!(
            field("reservation_id")?
                .as_str()
                .context("`reservation_id` is not a string")?
                .chars()
                .count(),
            20
        );
    }
    Ok(())
}

// FIXME(JakobDegen): This test is flakey due to something in the apple toolchain that I don't
// understand. The flakeyness needs to be fixed, or better yet, this needs to be made isolated so
// that people don't have to learn things about apple toolchains to debug it
//
// `cfg(any())` never holds: the tests below are disabled.

#[cfg(any())]
fn sort_by_identity(mut what_ran: Vec<Value>) -> Vec<Value> {
    what_ran.sort_by_key(|entry| {
        entry["identity"]
            .as_str()
            .expect("an action identity")
            .to_owned()
    });
    what_ran
}

#[cfg(any())]
#[buck_test(
    inplace = true,
    skip_for_os = ["windows"],
    extra_buck_config = [
        ("buck2", "allow_eden_io", "false"),
        ("buck2", "digest_algorithms", "BLAKE3-KEYED"),
        ("buck2", "source_digest_algorithm", "BLAKE3-KEYED"),
    ],
)]
async fn test_source_hashing_blake3_only(buck: &Buck) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/rules/rust/hello_world:welcome";

    buck.build(&[target, "--no-remote-cache", "--remote-only"])
        .await?;
    let run1 = buck2_e2e::utils::read_what_ran(buck, &[]).await?;

    let io_provider = read_io_provider_for_last_build(buck).await?;
    assert_eq!(io_provider, "fs");

    buck.kill().await?;
    buck.build(&[target, "--no-remote-cache", "--remote-only"])
        .env("BUCK2_DISABLE_FILE_ATTR", "true")
        .await?;
    let run2 = buck2_e2e::utils::read_what_ran(buck, &[]).await?;

    assert_eq!(sort_by_identity(run1), sort_by_identity(run2));
    Ok(())
}

#[cfg(any())]
#[buck_test(
    inplace = true,
    skip_for_os = ["windows"],
    extra_buck_config = [
        ("buck2", "allow_eden_io", "true"),
        ("buck2", "digest_algorithms", "BLAKE3-KEYED"),
        ("buck2", "source_digest_algorithm", "BLAKE3-KEYED"),
    ],
)]
async fn test_source_hashing_eden_blake3_only(buck: &Buck) -> anyhow::Result<()> {
    if !buck.cwd().join(".eden").exists() {
        eprintln!("This test is meaningless if not using Eden");
        return Ok(());
    }

    // Check we have Eden I/O
    buck.build(&[]).await?;
    let io_provider = read_io_provider_for_last_build(buck).await?;

    // If our test didn't use Eden then that means the current host's Eden is too old.
    // Skip in this case, unless

    if io_provider != "eden" && std::env::var_os("SANDCASTLE").is_none() {
        eprintln!("Unsupported Eden version");
        return Ok(());
    }

    // On Sandcastle we'll assert we *are* using Eden to make sure this test
    // isn't just always skipping.
    assert_eq!(io_provider, "eden");

    let target = "fbcode//buck2/tests/targets/rules/rust/hello_world:welcome";

    buck.build(&[target, "--no-remote-cache", "--remote-only"])
        .await?;
    let run1 = buck2_e2e::utils::read_what_ran(buck, &[]).await?;

    {
        use std::io::Write;

        let mut f = std::fs::OpenOptions::new().append(true).open(
            buck.env_var("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG")
                .context("No `BUCK2_TEST_EXTRA_EXTERNAL_CONFIG`")?,
        )?;
        f.write_all(b"[buck2]\n")?;
        f.write_all(b"allow_eden_io = false")?;
    }

    buck.kill().await?;
    buck.build(&[target, "--no-remote-cache", "--remote-only"])
        .env("BUCK2_DISABLE_FILE_ATTR", "true")
        .await?;
    let run2 = buck2_e2e::utils::read_what_ran(buck, &[]).await?;

    let io_provider = read_io_provider_for_last_build(buck).await?;
    assert_eq!(io_provider, "fs");

    assert_eq!(sort_by_identity(run1), sort_by_identity(run2));
    Ok(())
}

#[cfg(any())]
#[buck_test(
    inplace = true,
    skip_for_os = ["windows"],
    extra_buck_config = [
        ("buck2", "allow_eden_io", "false"),
        ("buck2", "digest_algorithms", "BLAKE3-KEYED,SHA1"),
        ("buck2", "source_digest_algorithm", "SHA1"),
    ],
)]
async fn test_source_hashing(buck: &Buck) -> anyhow::Result<()> {
    if !buck.cwd().join(".eden").exists() {
        eprintln!("This test is meaningless if not using Eden");
        return Ok(());
    }

    let target = "fbcode//buck2/tests/targets/rules/rust/hello_world:welcome";

    buck.build(&[target, "--no-remote-cache", "--remote-only"])
        .await?;
    let run1 = buck2_e2e::utils::read_what_ran(buck, &[]).await?;

    buck.kill().await?;
    buck.build(&[target, "--no-remote-cache", "--remote-only"])
        .env("BUCK2_DISABLE_FILE_ATTR", "true")
        .await?;
    let run2 = buck2_e2e::utils::read_what_ran(buck, &[]).await?;

    assert_eq!(sort_by_identity(run1), sort_by_identity(run2));
    Ok(())
}
