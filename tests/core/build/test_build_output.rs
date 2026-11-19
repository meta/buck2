/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use serde_json::Value;

const AGENT_MATERIALIZATION: &str = "--setting=agent_materialization.enabled=true";
const DIRECT: &str = "--agent-context=direct_call=true";
const MATERIALIZATION_NOTICE: &str = "Final-output materialization was automatically disabled";
const MATERIALIZATION_ENABLED_TAG: &str = "agent_materialization.enabled";
const MATERIALIZATION_DISABLED_TAG: &str = "agent_materialization.disabled";

async fn daemon_id(buck: &Buck) -> anyhow::Result<Value> {
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    status
        .get("daemon_constraints")
        .and_then(|constraints| constraints.get("daemon_id"))
        .cloned()
        .context("No `daemon_constraints.daemon_id` in `buck2 status`")
}

/// The tags in the invocation record of `result`.
fn tags(result: &BuckOutput) -> anyhow::Result<Vec<String>> {
    let record = result.invocation_record()?;
    record
        .get("tags")
        .and_then(Value::as_array)
        .context("No `tags` list in the invocation record")?
        .iter()
        .map(|tag| {
            tag.as_str()
                .map(str::to_owned)
                .with_context(|| format!("A tag that is not a string: {tag}"))
        })
        .collect()
}

#[buck_test]
async fn test_build_output(buck: &Buck) -> anyhow::Result<()> {
    let show_output = buck
        .build_without_report(&["root//:foo", "--show-output"])
        .await?;
    let show_full_output = buck
        .build_without_report(&["root//:foo", "--show-full-output"])
        .await?;
    let show_simple_output = buck
        .build_without_report(&["root//:foo", "--show-simple-output"])
        .await?;
    let show_json_output = buck
        .build_without_report(&["root//:foo", "--show-json-output"])
        .await?;

    let output = [
        show_output.stdout,
        show_full_output.stdout,
        show_simple_output.stdout,
        show_json_output.stdout,
    ]
    .join("\n\n");
    let output = output.replace(&*buck.cwd().to_string_lossy(), "/abs/project/root");
    let output = output.replace("\\\\", "/"); // Windows path separators in json
    let output = output.replace('\\', "/"); // Windows path separators not in json

    golden(&output, "build_output.golden");
    Ok(())
}

#[buck_test]
async fn test_build_output_on_partial_success(buck: &Buck) {
    let show_output = buck
        .build_without_report(&["root//:foo", "root//:fail", "--show-simple-output"])
        .await
        .expect_failure();
    assert_eq!(show_output.stdout.lines().count(), 1);

    let skipped_outputs = buck
        .build_without_report(&["root//:foo", "root//:fail", AGENT_MATERIALIZATION, DIRECT])
        .await
        .expect_failure();
    assert!(!skipped_outputs.stderr.contains(MATERIALIZATION_NOTICE));
}

#[buck_test(
    extra_buck_config = [
        ("buck2", "materializations", "deferred"),
        ("buck2", "defer_write_actions", "true"),
    ],
    write_invocation_record = true,
)]
async fn test_agent_materialization_activation_and_overrides(buck: &Buck) -> anyhow::Result<()> {
    let cases: [(&str, &[&str], bool); 5] = [
        ("direct", &[AGENT_MATERIALIZATION, DIRECT], false),
        ("disabled", &[DIRECT], true),
        ("tool", &[AGENT_MATERIALIZATION], true),
        (
            "explicit_all",
            &[AGENT_MATERIALIZATION, DIRECT, "--materializations=all"],
            true,
        ),
        (
            "explicit_none",
            &[
                AGENT_MATERIALIZATION,
                DIRECT,
                "--materializations=none",
                "--show-output",
            ],
            false,
        ),
    ];
    // Only builds the setting decides record its state; the rest are outside its A/B test.
    let expected_tags = BTreeMap::from([
        ("direct", MATERIALIZATION_ENABLED_TAG),
        ("disabled", MATERIALIZATION_DISABLED_TAG),
    ]);
    // Distinct outputs keep earlier materializations from masking a later skip.
    std::fs::write(
        buck.cwd().join("TARGETS.fixture"),
        cases
            .iter()
            .map(|(name, _, _)| format!("trivial_build(name=\"{name}\")"))
            .collect::<Vec<_>>()
            .join("\n"),
    )?;
    buck.audit_config(&[]).await?;
    let daemon_id_before = daemon_id(buck).await?;
    for (name, args, materialized) in cases {
        let target = format!("root//:{name}");
        let mut build_args = vec![target.as_str()];
        build_args.extend(args);
        let result = buck
            .build(&build_args)
            .env("CODING_AGENT_METADATA", "id=codex")
            .await?;
        let output = result.build_report()?.output_for_target(&target)?;
        assert_eq!(output.exists(), materialized, "{name}");
        assert_eq!(
            result.stderr.matches(MATERIALIZATION_NOTICE).count(),
            if name == "direct" { 1 } else { 0 },
            "{name}"
        );
        assert!(!result.stdout.contains(MATERIALIZATION_NOTICE));
        let tags = tags(&result)?;
        assert_eq!(
            [MATERIALIZATION_ENABLED_TAG, MATERIALIZATION_DISABLED_TAG]
                .into_iter()
                .filter(|tag| tags.iter().any(|t| t == tag))
                .collect::<BTreeSet<_>>(),
            expected_tags
                .get(name)
                .copied()
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "{name}"
        );
        assert_eq!(daemon_id(buck).await?, daemon_id_before, "{name}");
        if name == "direct" {
            for option in ["--out <path>", "--show-output", "--show-full-output"] {
                assert!(result.stderr.contains(option));
            }
        }
        if name == "direct" || name == "tool" {
            let queried = buck
                .targets(&[&target, "--show-output", AGENT_MATERIALIZATION])
                .await?;
            let path = buck.cwd().join(
                queried
                    .stdout
                    .split_whitespace()
                    .nth(1)
                    .context("No output path in `targets --show-output`")?,
            );
            assert_eq!(path, output);
            assert_eq!(path.exists(), materialized, "{name}");
            if materialized {
                assert_eq!(std::fs::read_to_string(&path)?, "abcd");
            }
        }
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_agent_materialization_tag_is_recorded_for_failed_builds(
    buck: &Buck,
) -> anyhow::Result<()> {
    // The notice only follows a successful build, but the tag is recorded before building.
    let cases: [(&[&str], &str); 2] = [
        (
            &[AGENT_MATERIALIZATION, DIRECT],
            MATERIALIZATION_ENABLED_TAG,
        ),
        (&[DIRECT], MATERIALIZATION_DISABLED_TAG),
    ];
    for (args, expected_tag) in cases {
        let mut build_args = vec!["root//:does_not_exist"];
        build_args.extend(args);
        let result = buck.build(&build_args).await.expect_failure();
        assert!(!result.stderr.contains(MATERIALIZATION_NOTICE), "{args:?}");
        assert!(
            tags(&result)?.iter().any(|tag| tag == expected_tag),
            "{args:?}"
        );
    }
    Ok(())
}

#[buck_test(
    extra_buck_config = [
        ("buck2", "materializations", "deferred"),
        ("buck2", "defer_write_actions", "true"),
    ],
)]
async fn test_agent_materialization_preserves_output_requests(buck: &Buck) -> anyhow::Result<()> {
    let cases: [&[&str]; 8] = [
        &["--show-output"],
        &["--show-full-output"],
        &["--show-simple-output"],
        &["--show-full-simple-output"],
        &["--show-json-output"],
        &["--show-full-json-output"],
        &["--out=copied.txt"],
        &["--out=-"],
    ];
    std::fs::write(
        buck.cwd().join("TARGETS.fixture"),
        (0..cases.len())
            .map(|i| format!("trivial_build(name=\"case_{i}\")"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\ntrivial_build(name=\"custom_default\")\n",
    )?;
    for (i, args) in cases.into_iter().enumerate() {
        let target = format!("root//:case_{i}");
        let mut build_args = vec![target.as_str(), AGENT_MATERIALIZATION, DIRECT];
        build_args.extend(args);
        let result = buck.build(&build_args).await?;
        assert!(!result.stderr.contains(MATERIALIZATION_NOTICE), "{args:?}");
        let output = result.build_report()?.output_for_target(&target)?;
        assert_eq!(std::fs::read_to_string(&output)?, "abcd", "{args:?}");
        if args == ["--out=copied.txt"] {
            assert_eq!(
                std::fs::read_to_string(buck.cwd().join("copied.txt"))?,
                "abcd"
            );
        } else if args == ["--out=-"] {
            assert!(result.stdout.contains("abcd"));
        }
    }

    // Output requests preserve the configured default rather than forcing materialization.
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2]\nmaterializations = deferred_skip_final_artifacts\n",
    )?;
    let target = "root//:custom_default";
    let result = buck
        .build(&[target, AGENT_MATERIALIZATION, DIRECT, "--show-output"])
        .await?;
    assert!(!result.build_report()?.output_for_target(target)?.exists());
    Ok(())
}
