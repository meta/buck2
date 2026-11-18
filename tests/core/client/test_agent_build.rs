/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;

const ENABLED: &str = "--setting=agent_build.enabled=true";
const DIRECT: &str = "--agent-context=direct_call=true";
const ACK: &str = "--agent-context=advice_ack=build,reason=Check code generation";
const ADVICE: &str = r"Buck advice \[build\]";

fn tags(record: &Value) -> anyhow::Result<Vec<&str>> {
    record
        .get("tags")
        .and_then(Value::as_array)
        .context("No `tags` in the invocation record")?
        .iter()
        .map(|tag| tag.as_str().context("A tag that is not a string"))
        .collect()
}

/// The `agent_interactive.build.*` tags of an invocation record.
fn build_tags(record: &Value) -> anyhow::Result<BTreeSet<String>> {
    Ok(tags(record)?
        .into_iter()
        .filter(|tag| tag.starts_with("agent_interactive.build."))
        .map(str::to_owned)
        .collect())
}

fn set<S: Into<String>>(tags: impl IntoIterator<Item = S>) -> BTreeSet<String> {
    tags.into_iter().map(Into::into).collect()
}

#[buck_test(write_invocation_record = true)]
async fn test_language_messages_and_rule_tags(buck: &Buck) -> anyhow::Result<()> {
    for (target, rule_type, language, message, absent) in [
        (
            "rust",
            "rust_binary",
            "rust",
            "arc rust-check root//:rust",
            "C++",
        ),
        (
            "rust_library",
            "rust_library",
            "rust",
            "arc rust-check root//:rust_library",
            "Rust binary",
        ),
        (
            "cpp",
            "cxx_library",
            "cxx",
            "arc cpp-check root//:cpp",
            "arc rust-check",
        ),
        (
            "cpp_binary",
            "cxx_binary",
            "cxx",
            "arc cpp-check root//:cpp_binary",
            "arc rust-check",
        ),
        (
            "rust_test",
            "rust_test",
            "rust",
            "arc rust-check root//:rust_test",
            "arc cpp-check",
        ),
        (
            "cpp_test",
            "cxx_test",
            "cxx",
            "arc cpp-check root//:cpp_test",
            "arc rust-check",
        ),
        (
            "rust_custom",
            "rust_custom",
            "rust",
            "arc rust-check root//:rust_custom",
            "arc cpp-check",
        ),
        (
            "cpp_custom",
            "cxx_custom",
            "cxx",
            "arc cpp-check root//:cpp_custom",
            "arc rust-check",
        ),
    ] {
        let label = format!("root//:{target}");
        let result = buck
            .build(&[&label, ENABLED, DIRECT])
            .await
            .expect_failure();
        result.assert_stderr_matches(ADVICE);
        assert!(result.stderr.contains(&format!("rule type `{rule_type}`")));
        assert!(result.stderr.contains(message));
        assert!(!result.stderr.contains(absent));
        assert!(!result.stderr.contains("[check]"));
        assert!(result.stderr.contains("If your goal is to verify"));
        assert!(
            result
                .stderr
                .contains("If your goal is to run tests, use `buck2 test` directly")
        );
        assert!(
            result
                .stderr
                .contains("a separate build step is unnecessary")
        );
        let record = result.invocation_record()?;
        assert_eq!(
            build_tags(&record)?,
            set([
                "agent_interactive.build.enabled".to_owned(),
                format!("agent_interactive.build.{language}.enabled"),
                format!("agent_interactive.build.{rule_type}.enabled"),
                "agent_interactive.build.block".to_owned(),
                format!("agent_interactive.build.{language}.block"),
                format!("agent_interactive.build.{rule_type}.block"),
            ])
        );
        assert!(tags(record.single_error())?.contains(&"AGENT_ADVICE_BLOCKED"));
        let acknowledged = buck.build(&[&label, ENABLED, DIRECT, ACK]).await?;
        assert_eq!(
            build_tags(&*acknowledged.invocation_record()?)?,
            set([
                "agent_interactive.build.enabled".to_owned(),
                format!("agent_interactive.build.{language}.enabled"),
                format!("agent_interactive.build.{rule_type}.enabled"),
                "agent_interactive.build.ack".to_owned(),
                format!("agent_interactive.build.{language}.ack"),
                format!("agent_interactive.build.{rule_type}.ack"),
            ])
        );
        let disabled = buck
            .build(&[
                &label,
                ENABLED,
                "--setting=agent_build.enabled=false",
                DIRECT,
                ACK,
            ])
            .await?;
        assert_eq!(
            build_tags(&*disabled.invocation_record()?)?,
            set([
                "agent_interactive.build.disabled".to_owned(),
                format!("agent_interactive.build.{language}.disabled"),
                format!("agent_interactive.build.{rule_type}.disabled"),
            ])
        );
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_blocks_before_build_actions(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:fails", ENABLED, DIRECT])
        .await
        .expect_failure()
        .assert_stderr_matches(ADVICE);
    buck.build(&["root//:fails", ENABLED, DIRECT, ACK])
        .await
        .expect_failure()
        .assert_stderr_matches("build action executed");
    let disabled = buck.build(&["root//:fails", DIRECT]).await.expect_failure();
    disabled.assert_stderr_matches("build action executed");
    assert!(!disabled.stderr.contains("Buck advice"));
    assert_eq!(
        build_tags(&*disabled.invocation_record()?)?,
        set([
            "agent_interactive.build.disabled",
            "agent_interactive.build.rust.disabled",
            "agent_interactive.build.rust_binary.disabled",
        ])
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_unrelated_rules_and_subtargets_proceed(buck: &Buck) -> anyhow::Result<()> {
    for target in [
        "plain",
        "rust_library[check]",
        "rust[check]",
        "cpp[check]",
        "cpp[custom]",
    ] {
        let label = format!("root//:{target}");
        for settings in [&[ENABLED][..], &[]] {
            let result = buck
                .build(&[&[label.as_str()][..], settings, &[DIRECT]].concat())
                .await?;
            assert!(!result.stderr.contains("Buck advice"));
            assert!(build_tags(&*result.invocation_record()?)?.is_empty());
        }
    }
    Ok(())
}

#[buck_test]
async fn test_rule_filter_applies_in_target_universe(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//:cpp",
        "--target-universe=root//:cpp",
        ENABLED,
        DIRECT,
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("arc cpp-check root//:cpp");
    let result = buck
        .build(&[
            "root//:plain",
            "--target-universe=root//:plain",
            ENABLED,
            DIRECT,
        ])
        .await?;
    assert!(!result.stderr.contains("Buck advice"));
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_acknowledgement_applies_to_one_build(buck: &Buck) -> anyhow::Result<()> {
    for context in [
        "advice_ack=build_intent,reason=Check code generation",
        "advice_ack=clean,reason=Check code generation",
        "advice_ack=build",
        "advice_ack=build,reason=   ",
        "advice_ack=build,reason=old,reason=",
        "reason=Check code generation",
    ] {
        buck.build(&[
            "root//:rust",
            ENABLED,
            DIRECT,
            &format!("--agent-context={context}"),
        ])
        .await
        .expect_failure()
        .assert_stderr_matches(ADVICE);
    }
    for target in ["rust", "rust_library"] {
        let label = format!("root//:{target}");
        let result = buck.build(&[&label, ENABLED, DIRECT, ACK]).await?;
        assert!(tags(&*result.invocation_record()?)?.contains(&"agent_interactive.build.ack"));
        buck.build(&[&label, ENABLED, DIRECT])
            .await
            .expect_failure()
            .assert_stderr_matches(ADVICE);
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_mixed_targets_only_block_matching_rules(buck: &Buck) -> anyhow::Result<()> {
    let report_path = buck.cwd().join("report.json");
    let result = buck
        .build(&[
            "root//:cpp",
            "root//:rust_test",
            "root//:plain",
            ENABLED,
            DIRECT,
            "--build-report",
            report_path.to_str().context("A non-UTF-8 path")?,
        ])
        .await
        .expect_failure();
    result.assert_stderr_matches(ADVICE);
    let report: Value = serde_json::from_str(&std::fs::read_to_string(&report_path)?)?;
    let results = report
        .get("results")
        .context("No `results` in the build report")?;
    assert_eq!(results["root//:cpp"]["success"], "FAIL");
    assert_eq!(results["root//:rust_test"]["success"], "FAIL");
    assert_eq!(results["root//:plain"]["success"], "SUCCESS");
    assert_eq!(
        build_tags(&*result.invocation_record()?)?,
        set([
            "agent_interactive.build.enabled",
            "agent_interactive.build.cxx.enabled",
            "agent_interactive.build.rust.enabled",
            "agent_interactive.build.cxx_library.enabled",
            "agent_interactive.build.rust_test.enabled",
            "agent_interactive.build.block",
            "agent_interactive.build.cxx.block",
            "agent_interactive.build.rust.block",
            "agent_interactive.build.cxx_library.block",
            "agent_interactive.build.rust_test.block",
        ])
    );
    let disabled = buck
        .build(&["root//:cpp", "root//:rust_test", "root//:plain", DIRECT])
        .await?;
    assert_eq!(
        build_tags(&*disabled.invocation_record()?)?,
        set([
            "agent_interactive.build.disabled",
            "agent_interactive.build.cxx.disabled",
            "agent_interactive.build.rust.disabled",
            "agent_interactive.build.cxx_library.disabled",
            "agent_interactive.build.rust_test.disabled",
        ])
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_explicit_artifact_requests_proceed(buck: &Buck) -> anyhow::Result<()> {
    let output = buck.cwd().join("output.txt");
    for args in [
        &["--show-output"][..],
        &["--show-full-output"],
        &["--show-json-output"],
        &["--show-full-json-output"],
        &["--materializations=all"],
        &["--out", output.to_str().context("A non-UTF-8 path")?],
    ] {
        for settings in [&[ENABLED][..], &[]] {
            let result = buck
                .build(&[&["root//:rust"][..], settings, &[DIRECT], args].concat())
                .await?;
            assert!(!result.stderr.contains("Buck advice"));
            assert!(build_tags(&*result.invocation_record()?)?.is_empty());
        }
    }
    assert_eq!(std::fs::read_to_string(&output)?, "pass");
    buck.build(&["root//:rust", ENABLED, DIRECT, "--materializations=none"])
        .await
        .expect_failure()
        .assert_stderr_matches(ADVICE);
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_disabled_and_non_direct_builds_proceed(buck: &Buck) -> anyhow::Result<()> {
    for args in [
        &[DIRECT][..],
        &["--setting=agent_interactive.enabled=true", DIRECT],
        &[ENABLED, "--setting=agent_build.enabled=false", DIRECT],
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_build.enabled=false",
            DIRECT,
        ],
    ] {
        let result = buck.build(&[&["root//:rust"][..], args].concat()).await?;
        assert!(!result.stderr.contains("Buck advice"));
        assert_eq!(
            build_tags(&*result.invocation_record()?)?,
            set([
                "agent_interactive.build.disabled",
                "agent_interactive.build.rust.disabled",
                "agent_interactive.build.rust_binary.disabled",
            ])
        );
    }
    for args in [
        &[ENABLED][..],
        &[ENABLED, DIRECT, "--agent-context=direct_call=false"],
        &[DIRECT, "--agent-context=direct_call=false"],
    ] {
        let result = buck.build(&[&["root//:rust"][..], args].concat()).await?;
        assert!(!result.stderr.contains("Buck advice"));
        assert!(build_tags(&*result.invocation_record()?)?.is_empty());
    }
    let result = buck.build(&[ENABLED, DIRECT]).await?;
    assert!(result.stderr.contains("NO BUILD TARGET PATTERNS SPECIFIED"));
    assert!(!result.stderr.contains("Buck advice"));
    assert!(build_tags(&*result.invocation_record()?)?.is_empty());
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_build_setting_is_independent(buck: &Buck) -> anyhow::Result<()> {
    for interactive in ["false", "true"] {
        let result = buck
            .build(&[
                "root//:rust",
                ENABLED,
                DIRECT,
                &format!("--setting=agent_interactive.enabled={interactive}"),
            ])
            .await
            .expect_failure();
        result.assert_stderr_matches(ADVICE);
        assert!(
            tags(&*result.invocation_record()?)?.contains(&"agent_interactive.build.rust.enabled")
        );
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_testing_and_running_do_not_require_build_ack(buck: &Buck) -> anyhow::Result<()> {
    let tested = buck
        .test(&["root//:rust_test", "root//:cpp_test", ENABLED, DIRECT])
        .await?;
    assert!(!tested.stderr.contains("Buck advice"));
    let ran = buck.run(&["root//:rust", ENABLED, DIRECT]).await?;
    assert!(!ran.stderr.contains("Buck advice"));
    for result in [&tested, &ran] {
        assert!(build_tags(&*result.invocation_record()?)?.is_empty());
    }
    Ok(())
}
