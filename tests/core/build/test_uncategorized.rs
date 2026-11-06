/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::json_get;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use buck2_e2e::utils::remove_output;
use serde_json::Value;

fn read_all_outputs(buck: &Buck, report: &str) -> anyhow::Result<Vec<String>> {
    let report: Value = serde_json::from_str(&std::fs::read_to_string(buck.cwd().join(report))?)?;
    let mut ret = Vec::new();
    let results = report
        .get("results")
        .and_then(Value::as_object)
        .context("No `results` in the build report")?;
    for state in results.values() {
        for key in ["outputs", "other_outputs"] {
            let outputs = state
                .get(key)
                .and_then(Value::as_object)
                .with_context(|| format!("No `{key}` in {state}"))?;
            // Only the default outputs may be missing.
            if let Some(default) = outputs.get("DEFAULT") {
                let default = default
                    .as_array()
                    .with_context(|| format!("`DEFAULT` is not a list in {state}"))?;
                ret.extend(default.iter().filter_map(|o| o.as_str().map(str::to_owned)));
            }
        }
    }
    Ok(ret)
}

/// The RE use case of the test's actions.
async fn get_buck2_re_use_case(buck: &Buck) -> anyhow::Result<String> {
    let key = "buck2_re_client.override_use_case";
    let config = buck
        .audit_config(&["--reuse-current-config", "--style=json", key])
        .await?
        .audit_config_json()?;
    if let Some(use_case) = config.get(key).and_then(Value::as_str) {
        return Ok(use_case.to_owned());
    }

    // The test harness's extra external config is part of normal Buck config
    // parsing, but `override_use_case` is filtered out of DICE-backed config
    // reads, so `audit config --reuse-current-config` may not report it. We need
    // to manually check the test config here.
    if let Some(extra_config_path) = buck.env_var("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG")
        && let Ok(extra_config) = std::fs::read_to_string(extra_config_path)
        && let Some(use_case) = override_use_case(&extra_config)
    {
        return Ok(use_case);
    }

    Ok("buck2-default".to_owned())
}

/// The last `buck2_re_client.override_use_case` in a buckconfig file.
fn override_use_case(config: &str) -> Option<String> {
    let mut section = "";
    let mut use_case = None;
    for line in config.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim();
        } else if section == "buck2_re_client"
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "override_use_case"
        {
            use_case = Some(value.trim().to_owned());
        }
    }
    use_case
}

#[buck_test(data_dir = "anon_exec_deps")]
async fn test_anon_target_exec_deps(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//tests:exec_dep_good", "--remote-only"])
        .await?;

    buck.build(&["//tests:exec_dep_bad", "--local-only"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Exec deps and the current anon target must have the same execution platform resolution",
        );

    buck.build(&["//tests:exec_dep_rejects_dep"])
        .await
        .expect_failure()
        .assert_stderr_matches("exec dep is missing the execution platform resolution");
    Ok(())
}

#[buck_test(data_dir = "args")]
async fn test_args(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:bin"]).await?;
    let output = result.build_report()?.output_for_target("//:bin")?;
    assert_eq!(
        std::fs::read_to_string(output)?.trim_end(),
        "<foo_compiler> -- <foo_compiler_flags>\nlib1 -- this is lib1\nlib2 -- this is lib2"
    );
    Ok(())
}

#[buck_test(data_dir = "prelude_import")]
async fn test_prelude_imported_once(buck: &Buck) -> anyhow::Result<()> {
    // See the comments in the relevant targets files: they explain how this
    // test works.
    buck.build(&["cell1//...", "cell2//..."]).await?;
    Ok(())
}

#[buck_test(data_dir = "build_providers")]
async fn test_build_providers(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//:target",
        "--build-default-info",
        "--skip-run-info",
        "--skip-test-info",
        "--build-report",
        "report",
    ])
    .await?;

    let outputs = read_all_outputs(buck, "report")?;
    assert!(outputs.iter().any(|o| o.contains("/build")));
    assert!(outputs.iter().all(|o| !o.contains("/run")));
    assert!(outputs.iter().all(|o| !o.contains("/test")));

    buck.build(&[
        "//:target",
        "--skip-default-info",
        "--build-run-info",
        "--skip-test-info",
        "--build-report",
        "report",
    ])
    .await?;

    let outputs = read_all_outputs(buck, "report")?;
    assert!(outputs.iter().all(|o| !o.contains("/build")));
    assert!(outputs.iter().all(|o| !o.contains("/test")));

    buck.build(&[
        "//:target",
        "--skip-default-info",
        "--skip-run-info",
        "--build-test-info",
        "--build-report",
        "report",
    ])
    .await?;

    let outputs = read_all_outputs(buck, "report")?;
    assert!(outputs.iter().all(|o| !o.contains("/build")));
    assert!(outputs.iter().all(|o| !o.contains("/run")));
    Ok(())
}

async fn test_projected_artifacts(buck: &Buck, target: &str) -> anyhow::Result<()> {
    buck.build(&[target]).await?;
    Ok(())
}

// Check building the whole thing
#[buck_test(data_dir = "projected_artifacts")]
async fn test_projected_artifacts_case(buck: &Buck) -> anyhow::Result<()> {
    test_projected_artifacts(buck, "//...").await
}

// Check building just one target, which may reveal bugs if things are
// materialized differently when a projected target uses them.
#[buck_test(data_dir = "projected_artifacts")]
async fn test_projected_artifacts_check_c_b_local(buck: &Buck) -> anyhow::Result<()> {
    test_projected_artifacts(buck, "//:check_c_b_local").await
}

#[buck_test(data_dir = "upload_all_actions")]
async fn test_upload_all_actions(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join("src"), random_string())?;

    // This action includes `src` and is forced to run locally. This means RE
    // can never have seen it (and we'll check that later by asserting there is
    // only 1 cache query, excluding local actions).
    buck.build(&["//:cp", "--upload-all-actions"]).await?;

    let what_ran =
        read_what_ran(buck, &["--emit-cache-queries", "--skip-local-executions"]).await?;
    assert_eq!(what_ran.len(), 1);

    // Now, download the action. This will succeed only if we uploaded it.
    let digest = what_ran[0]["reproducer"]["details"]["digest"]
        .as_str()
        .expect("a digest");
    let use_case = get_buck2_re_use_case(buck).await?;
    let status = tokio::process::Command::new("dotslash")
        .arg(std::env::var_os("RECLI").expect("the test target sets `RECLI`"))
        .args(["--use-case", &use_case, "cas", "download-action", digest])
        .status()
        .await?;
    assert!(status.success(), "Downloading the action failed: {status}");
    Ok(())
}

#[buck_test(data_dir = "buckroot")]
async fn test_buckroot(buck: &Buck) -> anyhow::Result<()> {
    // Test that .buckroot files work
    buck.build(&[":inner"]).rel_cwd("rooted/cell").await?;
    Ok(())
}

/// This is a regression test for https://github.com/facebook/buck2/pull/43,
/// including the similar issue with directories that was fixed first.
#[buck_test(data_dir = "cell_delete")]
async fn test_cell_deletion(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?;
    std::fs::create_dir(buck.cwd().join("hello"))?;
    buck.targets(&[":"]).await?;
    std::fs::remove_dir(buck.cwd().join("hello"))?;
    buck.targets(&[":"]).await?;
    Ok(())
}

#[buck_test(data_dir = "concurrency")]
async fn test_concurrency(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//:weight", "--local-only", "--no-remote-cache"])
        .await?;

    // Now, since our commands request 20% of resources, check that a no point
    // we had more than 5 running commands. Also check that we found the right
    // amount of commands.
    let log = buck.log(&["show"]).await?;

    let mut running_execs = HashMap::new();
    let mut execs_done = 0;

    for line in log.stdout.trim().lines() {
        let event: Value = serde_json::from_str(line)?;
        let id = event["Event"]["span_id"].to_string();

        let is_end = json_get(&event, &["Event", "data", "SpanEnd"]).is_some();

        if is_end {
            if running_execs.remove(&id).is_some() {
                execs_done += 1;
            }

            continue;
        }

        let is_local_exec = json_get(
            &event,
            &[
                "Event",
                "data",
                "SpanStart",
                "data",
                "ExecutorStage",
                "stage",
                "Local",
                "stage",
                "Execute",
            ],
        )
        .is_some();

        if is_local_exec {
            running_execs.insert(id, true);
        }

        // Check that concurrently running local commands
        // don't exceed 5.
        assert!(running_execs.len() <= 5);
    }

    assert_eq!(execs_done, 10);
    Ok(())
}

#[buck_test(data_dir = "fail_fast")]
async fn test_fail_fast(buck: &Buck) {
    let exc = buck
        .build(&[
            "root//:mixed",
            "root//:slow",
            "--local-only",
            "--no-remote-cache",
        ])
        .await
        .expect_failure();

    assert!(exc.stderr.contains("fast_default_output"));
    assert!(exc.stderr.contains("slow_default_output"));
    assert!(exc.stderr.contains("slow_other_output"));

    let exc = buck
        .build(&[
            "root//:mixed",
            "root//:slow",
            "--local-only",
            "--no-remote-cache",
            "--fail-fast",
        ])
        .await
        .expect_failure();

    assert!(exc.stderr.contains("fast_default_output"));
    assert!(!exc.stderr.contains("slow_default_output"));
    assert!(!exc.stderr.contains("slow_other_output"));
}

#[buck_test(data_dir = "keep_going_build")]
async fn test_keep_going(buck: &Buck) -> anyhow::Result<()> {
    let exc = buck
        .build(&["root//:top", "--local-only", "--no-remote-cache"])
        .await
        .expect_failure();

    assert!(exc.stderr.contains("fast_action"));
    assert!(!exc.stderr.contains("slow_action"));

    // Dont want to re-attach to the ongoing evaluation for slow_action.
    // Normally that gets cancelled, but even so that's still a race.
    buck.kill().await?;

    let exc = buck
        .build(&[
            "root//:top",
            "--local-only",
            "--no-remote-cache",
            "--keep-going",
        ])
        .await
        .expect_failure();

    assert!(exc.stderr.contains("fast_action"));
    assert!(exc.stderr.contains("slow_action"));
    Ok(())
}

#[buck_test(data_dir = "cleanup")]
async fn test_cleanup(buck: &Buck) -> anyhow::Result<()> {
    // Test for T85589819 - broken cleanup
    let target_pattern = "//:cleanup";
    let result = buck.build(&[target_pattern]).await?;
    let output = result.build_report()?.output_for_target(target_pattern)?;
    let parent = output.parent().unwrap();

    // The output should be something like path/__cleanup__/out/dir1/dir2/output.txt
    // We want to ensure that if we make a file dir1 or dir1/dir2, cleanup still works
    remove_output(&output)?;
    std::fs::remove_dir(parent)?;
    std::fs::write(parent, "File that must be deleted")?;
    buck.kill().await?;
    buck.build(&[target_pattern]).await?;

    remove_output(&output)?;
    std::fs::remove_dir(parent)?;
    std::fs::remove_dir(parent.parent().unwrap())?;
    std::fs::write(parent.parent().unwrap(), "File that must be deleted")?;
    buck.build(&[target_pattern]).await?;
    Ok(())
}

#[buck_test(data_dir = "log_action_keys")]
async fn test_log_action_keys(buck: &Buck) -> anyhow::Result<()> {
    async fn read_action_keys(buck: &Buck) -> anyhow::Result<Vec<(String, String)>> {
        let out = read_what_ran(buck, &[]).await?;
        Ok(out
            .iter()
            .map(|line| {
                (
                    line["reproducer"]["executor"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    line["reproducer"]["details"]["action_key"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )
            })
            .collect())
    }

    let seed = random_string();
    let action_key = "executor root//:test (<unspecified>) touch";

    // Run on RE
    buck.build(&[
        ":test",
        "-c",
        &format!("test.seed={seed}"),
        "-c",
        "buck2.log_action_keys=true",
    ])
    .await?;
    assert_eq!(
        read_action_keys(buck).await?,
        [("Re".to_owned(), action_key.to_owned())]
    );

    buck.kill().await?;

    // Run on RE again, get a cache hit this time
    buck.build(&[
        ":test",
        "-c",
        &format!("test.seed={seed}"),
        "-c",
        "buck2.log_action_keys=true",
    ])
    .await?;

    assert_eq!(
        read_action_keys(buck).await?,
        [("Cache".to_owned(), action_key.to_owned())]
    );
    Ok(())
}

#[buck_test(data_dir = "roots")]
async fn test_roots(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.build(&["root//:test", "other//:test"]).await?;

    let platformify = |path: &str| {
        if cfg!(windows) {
            path.replace('/', "\\")
        } else {
            path.to_owned()
        }
    };
    let read_json = |path: &Path| -> anyhow::Result<Value> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    };
    let field = |j: &Value, key: &str| -> anyhow::Result<String> {
        Ok(j.get(key)
            .and_then(Value::as_str)
            .with_context(|| format!("No string `{key}` in {j}"))?
            .to_owned())
    };

    let output = res.build_report()?.output_for_target("root//:test")?;
    let j = read_json(&output)?;
    eprintln!("{j}");
    assert!(
        buck.cwd()
            .join(field(&j, "fixture_relative_to_cell")?)
            .exists()
    );
    assert!(
        buck.cwd()
            .join(field(&j, "fixture_relative_to_project")?)
            .exists()
    );

    assert_eq!(
        field(&j, "cell_relative_to_fixture")?,
        platformify("../../../../../../..")
    );
    assert_eq!(
        field(&j, "project_relative_to_fixture")?,
        platformify("../../../../../../..")
    );

    let output = res.build_report()?.output_for_target("other//:test")?;
    let j = read_json(&output)?;
    assert!(
        buck.cwd()
            .join("other")
            .join(field(&j, "fixture_relative_to_cell")?)
            .exists()
    );
    assert!(
        buck.cwd()
            .join(field(&j, "fixture_relative_to_project")?)
            .exists()
    );

    assert_eq!(
        field(&j, "cell_relative_to_fixture")?,
        platformify("../../../../../../../other")
    );
    assert_eq!(
        field(&j, "project_relative_to_fixture")?,
        platformify("../../../../../../..")
    );
    Ok(())
}

#[buck_test(data_dir = "tmpdir")]
async fn test_tmpdir(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:"]).await?;
    Ok(())
}

#[buck_test(data_dir = "artifact_consistency")]
async fn test_artifact_consistency(buck: &Buck) -> anyhow::Result<()> {
    let out = buck
        .build_without_report(&[":gen[file3]", "--local-only", "--out=-"])
        .await?;

    assert_eq!(out.stdout, "This is file3");

    let out = buck
        .build_without_report(&["-c", "gen.idx=2", ":gen[file3]", "--local-only", "--out=-"])
        .await?;
    assert_eq!(out.stdout, "This is file3");
    Ok(())
}
