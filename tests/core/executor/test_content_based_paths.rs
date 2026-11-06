/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Write;
use std::path::Path;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::http::StaticHttpServer;
use buck2_e2e::http::download_configs;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::random_string;
use buck2_e2e::utils::read_what_ran;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::Value;

async fn is_eligible_for_action_dedup(buck: &Buck) -> anyhow::Result<Value> {
    let mut events = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "ActionExecution",
            "eligible_for_dedupe",
        ],
    )
    .await?;

    assert!(!events.is_empty());
    Ok(events.pop().unwrap())
}

const ELIGIBLE_FOR_DEDUPE: i64 = 0;
const INELIGIBLE_INPUT: i64 = 1;
const INELIGIBLE_OUTPUT: i64 = 2;
const EXECUTION_PLATFORM_UNKNOWN_ELIGIBILITY: i64 = 3;

const EXPECTED_INELIGIBLE_FOR_DEDUPE: i64 = 1;
const UNKNOWN_ELIGIBILITY: i64 = 2;

/// The output `--show-output` printed for `target`.
fn output_path(result: &BuckOutput, target: &str) -> anyhow::Result<String> {
    result
        .target_to_build_output()?
        .remove(target)
        .with_context(|| format!("No output for `{target}`:\n{}", result.stdout))
}

async fn build_target_with_different_platforms_and_verify_output_paths_are_identical(
    buck: &Buck,
    target: &str,
    args: &[&str],
) -> anyhow::Result<()> {
    let build = |platform| {
        let mut argv = vec![target, "--target-platforms", platform, "--show-output"];
        argv.extend_from_slice(args);
        buck.build(&argv)
    };
    let result1 = build("root//:p_default").await?;
    assert_eq!(
        is_eligible_for_action_dedup(buck).await?,
        ELIGIBLE_FOR_DEDUPE
    );
    let result2 = build("root//:p_cat").await?;
    assert_eq!(
        is_eligible_for_action_dedup(buck).await?,
        ELIGIBLE_FOR_DEDUPE
    );

    let path1 = output_path(&result1, target)?;
    let path2 = output_path(&result2, target)?;

    assert!(!path1.contains("output_artifact"));
    assert_ne!(path1, path2);

    let actual1 = std::fs::canonicalize(buck.cwd().join(&path1))?;
    let actual2 = std::fs::canonicalize(buck.cwd().join(&path2))?;

    assert!(actual1.exists());
    assert!(actual2.exists());
    assert_eq!(actual1, actual2);
    Ok(())
}

async fn hg_init(cwd: &Path) -> anyhow::Result<()> {
    run_checked(
        tokio::process::Command::new("hg")
            .arg("init")
            .current_dir(cwd),
    )
    .await?;
    hg_config_reponame(cwd).await
}

async fn hg_config_reponame(cwd: &Path) -> anyhow::Result<()> {
    run_checked(
        tokio::process::Command::new("hg")
            .args(["config", "remotefilelog.reponame", "--local", "no-repo"])
            .current_dir(cwd),
    )
    .await
}

async fn run_checked(command: &mut tokio::process::Command) -> anyhow::Result<()> {
    let status = command
        .status()
        .await
        .with_context(|| format!("Running {command:?}"))?;
    anyhow::ensure!(status.success(), "{command:?} failed: {status}");
    Ok(())
}

/// A `tgz` archive of `files` (archive-relative paths and their contents). A comment changes the
/// digest of nonempty archives without changing extracted files, so a previous CAS entry cannot
/// satisfy a test intended to exercise HTTP.
fn make_tgz(files: &[(&str, &[u8])]) -> anyhow::Result<Vec<u8>> {
    let mut archive = tar::Builder::new(Vec::new());
    if !files.is_empty() {
        let comment = pax_record("comment", &random_string());
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::XGlobalHeader);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_size(comment.len().try_into()?);
        archive.append_data(&mut header, "pax_global_header", comment.as_bytes())?;
    }
    for (name, content) in files {
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_size(content.len().try_into()?);
        archive.append_data(&mut header, name, *content)?;
    }
    let mut gzip = GzEncoder::new(Vec::new(), Compression::best());
    gzip.write_all(&archive.into_inner()?)?;
    Ok(gzip.finish()?)
}

/// A pax header record, `<length> <keyword>=<value>\n`, whose length counts itself.
fn pax_record(keyword: &str, value: &str) -> String {
    let rest = format!(" {keyword}={value}\n");
    let mut length = rest.len();
    while length != rest.len() + length.to_string().len() {
        length = rest.len() + length.to_string().len();
    }
    format!("{length}{rest}")
}

async fn build_output_text(buck: &Buck, target: &str, args: &[&str]) -> anyhow::Result<String> {
    let mut argv = vec![target];
    argv.extend_from_slice(args);
    argv.extend(["--show-output", "--remote-only"]);
    let result = buck.build(&argv).await?;
    let path = output_path(&result, target)?;
    Ok(std::fs::read_to_string(buck.cwd().join(path))?)
}

/// The digest of the first action `what-ran` lists.
fn what_ran_digest(what_ran: &[Value]) -> anyhow::Result<&Value> {
    what_ran[0]["reproducer"]["details"]
        .get("digest")
        .context("No `digest` in the first `what-ran` entry")
}

async fn what_ran_test_run(buck: &Buck) -> anyhow::Result<Vec<Value>> {
    Ok(read_what_ran(buck, &[])
        .await?
        .into_iter()
        .filter(|e| {
            e["identity"]
                .as_str()
                .expect("what-ran entries have an identity")
                .contains("(test_run)")
        })
        .collect())
}

/// The archive `download_with_content_based_path` fetches.
fn archive() -> anyhow::Result<Vec<u8>> {
    let files = ["foo2", "foo/bar2", "foo/bar/qux"].map(|name| (name, format!("{name}\n")));
    make_tgz(
        &files
            .each_ref()
            .map(|(name, content)| (*name, content.as_bytes())),
    )
}

#[buck_test]
async fn test_write_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:write_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_write_macro_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:write_macro_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_write_json_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:write_json_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_run_remote_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:run_remote_with_content_based_path";

    let result1 = buck
        .build(&[
            target,
            "--target-platforms",
            "root//:p_default",
            "--show-output",
            "--remote-only",
        ])
        .await?;
    let what_ran1 = read_what_ran(buck, &[]).await?;
    // Flush the local dep file cache so the second (cross-configuration) build still goes through the
    // RE path and shows up in what-ran. Otherwise it would be served by the cross-configuration local
    // action cache (this action is dedupe-eligible), and what-ran would be empty. The cross-config
    // local cache hit itself is covered by test_dep_files.py::test_dep_file_hit_across_configurations.
    buck.debug(&["flush-dep-files"]).await?;
    let result2 = buck
        .build(&[
            target,
            "--target-platforms",
            "root//:p_cat",
            "--show-output",
            "--remote-only",
        ])
        .await?;
    let what_ran2 = read_what_ran(buck, &[]).await?;

    assert_eq!(what_ran_digest(&what_ran1)?, what_ran_digest(&what_ran2)?);

    let path1 = output_path(&result1, target)?;
    let path2 = output_path(&result2, target)?;

    assert!(!path1.contains("output_artifact"));
    assert_ne!(path1, path2);

    let actual1 = std::fs::canonicalize(buck.cwd().join(&path1))?;
    let actual2 = std::fs::canonicalize(buck.cwd().join(&path2))?;

    assert!(actual1.exists());
    assert!(actual2.exists());
    assert_eq!(actual1, actual2);
    Ok(())
}

#[buck_test]
async fn test_identical_dep_file_hit_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:run_remote_with_content_based_path";

    let result1 = buck
        .build(&[target, "-c", "test.ignored_attr=run1", "--show-output"])
        .await?;
    let result2 = buck
        .build(&[target, "-c", "test.ignored_attr=run2", "--show-output"])
        .await?;
    let what_ran2 = read_what_ran(buck, &[]).await?;
    assert!(what_ran2.is_empty(), "{what_ran2:?}");

    let path1 = output_path(&result1, target)?;
    let path2 = output_path(&result2, target)?;

    assert_eq!(path1, path2);
    Ok(())
}

#[buck_test]
async fn test_run_local_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:run_local_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_copy_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:copy_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_symlink_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:symlink_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_symlink_and_copy_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:symlink_and_copy_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_copied_dir_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:copied_dir_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_symlinked_dir_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:symlinked_dir_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await
}

#[buck_test]
async fn test_assembled_dir_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:assembled_dir_with_content_based_path";
    build_target_with_different_platforms_and_verify_output_paths_are_identical(buck, target, &[])
        .await?;

    // Entry modes must be honored: `assembled_dir.copy` entries are laid out
    // as real bytes, `assembled_dir.symlink` entries as symlinks.
    let result = buck
        .build(&[
            target,
            "--target-platforms",
            "root//:p_default",
            "--show-output",
        ])
        .await?;
    let path = output_path(&result, target)?;
    let out = buck.cwd().join(path);
    assert!(out.is_dir());
    for name in ["copied", "copied_dep"] {
        assert!(out.join(name).is_file());
        assert!(!out.join(name).is_symlink());
    }
    for name in ["symlinked", "symlinked_dep"] {
        assert!(out.join(name).is_symlink());
    }
    Ok(())
}

#[buck_test]
async fn test_cas_artifact_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    build_target_with_different_platforms_and_verify_output_paths_are_identical(
        buck,
        "root//:empty_cas_artifact_with_content_based_path",
        &[],
    )
    .await
}

#[buck_test]
async fn test_download_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let content = archive()?;
    let server = StaticHttpServer::start([("/archive", content.clone())]).await?;
    let config = download_configs(&server.url("/archive"), &content);
    let config: Vec<&str> = config.iter().map(String::as_str).collect();
    build_target_with_different_platforms_and_verify_output_paths_are_identical(
        buck,
        "root//:download_with_content_based_path",
        &config,
    )
    .await
}

#[buck_test]
async fn test_download_with_content_based_path_and_no_metadata(buck: &Buck) {
    buck.build(&[
        "root//:download_with_content_based_path_and_no_metadata",
        "-c",
        "test.url=http://localhost:1/archive",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Downloads using content-based path .* must supply metadata \(usually in the form of a sha1\)!",
    );
}

#[buck_test]
async fn test_offline_cas_artifact_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    hg_init(buck.cwd()).await?;

    buck.debug(&["trace-io", "enable"]).await?;
    let target = "root//:empty_cas_artifact_with_content_based_path";
    buck.build(&[
        target,
        "--target-platforms",
        "root//:p_default",
        "--show-output",
    ])
    .await?;

    buck.debug(&["trace-io", "export-manifest"]).await?;
    buck.kill().await?;

    buck.build(&[
        target,
        "--target-platforms",
        "root//:p_default",
        "--show-output",
        "-c",
        "buck2.use_network_action_output_cache=true",
        "--no-remote-cache",
        "--local-only",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_offline_download_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    let content = archive()?;
    let server = StaticHttpServer::start([("/archive", content.clone())]).await?;
    let config = download_configs(&server.url("/archive"), &content);
    let config: Vec<&str> = config.iter().map(String::as_str).collect();
    hg_init(buck.cwd()).await?;

    buck.debug(&["trace-io", "enable"]).await?;
    let target = "root//:download_with_content_based_path";
    let mut args = config.clone();
    args.extend([
        target,
        "--target-platforms",
        "root//:p_default",
        "--show-output",
    ]);
    buck.build(&args).await?;

    buck.debug(&["trace-io", "export-manifest"]).await?;
    buck.kill().await?;
    assert!(server.count("GET", "/archive") > 0);
    drop(server);

    let mut args = config;
    args.extend([
        target,
        "--target-platforms",
        "root//:p_default",
        "--show-output",
        "-c",
        "buck2.use_network_action_output_cache=true",
        "--no-remote-cache",
        "--local-only",
    ]);
    buck.build(&args).await?;
    Ok(())
}

#[buck_test]
async fn test_validation_with_content_based_path(buck: &Buck) {
    buck.build(&[
        "root//:failing_validation_with_content_based_path",
        "--target-platforms",
        "root//:p_default",
        "--show-output",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("This is a failing validation");
}

#[buck_test]
async fn test_dynamic_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    build_target_with_different_platforms_and_verify_output_paths_are_identical(
        buck,
        "root//:dynamic_with_content_based_path",
        &[],
    )
    .await
}

#[buck_test]
async fn test_dynamic_new_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    build_target_with_different_platforms_and_verify_output_paths_are_identical(
        buck,
        "root//:dynamic_new_with_content_based_path",
        &[],
    )
    .await
}

#[buck_test]
async fn test_projection_with_content_based_path(buck: &Buck) -> anyhow::Result<()> {
    build_target_with_different_platforms_and_verify_output_paths_are_identical(
        buck,
        "root//:use_projection_with_content_based_path",
        &[],
    )
    .await
}

#[buck_test]
async fn test_ignore_artifacts_content_based_input_is_not_materialized_but_path_is_real(
    buck: &Buck,
) -> anyhow::Result<()> {
    let target = "root//:ignore_artifacts_content_based_artifact";
    let result = buck
        .build(&[target, "--show-output", "--remote-only"])
        .await?;
    let path = output_path(&result, target)?;
    // The remote action asserts the stamp artifact is absent from its input tree;
    // its output records the rendered path.
    let out = std::fs::read_to_string(buck.cwd().join(path))?;
    assert!(out.contains("stamp.txt"), "{out}");
    // The rendered path embeds the real content hash, not a placeholder.
    assert!(!out.contains("output_artifact"), "{out}");
    assert!(!out.contains("dep_files_placeholder"), "{out}");
    Ok(())
}

#[buck_test]
async fn test_ignore_artifacts_content_based_input_content_change_reruns_consumer(
    buck: &Buck,
) -> anyhow::Result<()> {
    let target = "root//:ignore_artifacts_content_based_artifact";
    let result1 = buck
        .build(&[
            target,
            "-c",
            "test.stamp_data=one",
            "--show-output",
            "--remote-only",
        ])
        .await?;
    let path1 = output_path(&result1, target)?;
    let out1 = std::fs::read_to_string(buck.cwd().join(path1))?;
    let result2 = buck
        .build(&[
            target,
            "-c",
            "test.stamp_data=two",
            "--show-output",
            "--remote-only",
        ])
        .await?;
    let path2 = output_path(&result2, target)?;
    let out2 = std::fs::read_to_string(buck.cwd().join(path2))?;
    // The stamp is not an input, but its content reaches the command line via the
    // rendered path, so changing it must re-run the consumer.
    assert_ne!(out1, out2);
    Ok(())
}

#[buck_test]
async fn test_ignore_artifacts_rejects_hidden_content_based_artifact(buck: &Buck) {
    // Nothing renders a hidden artifact, so nothing could carry a content-based
    // one's content to the action: analysis rejects it rather than dropping it.
    buck.build(&[
        "root//:ignore_artifacts_rejects_hidden_content_based_artifact",
        "--target-platforms",
        "root//:p_default",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Artifact\(s\).*stamp.txt.*content-based and `hidden` under `ignore_artifacts`",
    );
}

#[buck_test]
async fn test_ignore_artifacts_nested_under_hidden_is_not_a_dependency(
    buck: &Buck,
) -> anyhow::Result<()> {
    // The content-based artifact's producer fails, but nothing renders it, so it
    // is not a dependency: the consumer builds without running the producer.
    buck.build(&[
        "root//:ignore_artifacts_nested_under_hidden",
        "--target-platforms",
        "root//:p_default",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_ignore_artifacts_does_not_depend_on_static_path_artifacts(
    buck: &Buck,
) -> anyhow::Result<()> {
    // The referenced artifact's producer fails, but a static path needs no
    // dependency at all, so the consumer builds without running it.
    buck.build(&[
        "root//:ignore_artifacts_skips_failing_static_path_dep",
        "--target-platforms",
        "root//:p_default",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_salt_hidden_is_a_content_dependency(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:salt_hidden_dependency";
    // The remote action asserts the hidden stamp is absent from its input tree.
    build_output_text(buck, target, &["-c", "test.stamp_data=one"]).await?;
    let what_ran1 = what_ran_test_run(buck).await?;
    assert_eq!(what_ran1.len(), 1, "{what_ran1:?}");

    // Nothing on the command line or in the input tree mentions the stamp's
    // content; the action must still re-run, under a different digest, when it changes.
    build_output_text(buck, target, &["-c", "test.stamp_data=two"]).await?;
    let what_ran2 = what_ran_test_run(buck).await?;
    assert_eq!(what_ran2.len(), 1, "{what_ran2:?}");
    assert_ne!(
        what_ran1[0]["reproducer"]["details"]["digest"],
        what_ran2[0]["reproducer"]["details"]["digest"]
    );
    Ok(())
}

#[buck_test]
async fn test_local_actions_do_not_overwrite_each_other(buck: &Buck) -> anyhow::Result<()> {
    let target1 = "root//:uses_slow_running_local_action_with_content_based_path1";
    let target2 = "root//:uses_slow_running_local_action_with_content_based_path2";
    let result = buck.build(&[target1, target2, "--show-output"]).await?;

    let path1 = output_path(&result, target1)?;
    let path2 = output_path(&result, target2)?;

    let path1 = path1.replace(
        "uses_slow_running_local_action_with_content_based_path1",
        "uses_slow_running_local_action_with_content_based_path2",
    );

    assert_ne!(path1, path2);
    Ok(())
}

#[buck_test]
async fn test_uses_relative_to(buck: &Buck) -> anyhow::Result<()> {
    build_target_with_different_platforms_and_verify_output_paths_are_identical(
        buck,
        "root//:uses_relative_to",
        &[],
    )
    .await
}

#[buck_test]
async fn test_sets_inconsistent_params(buck: &Buck) {
    buck.build(&[
        "root//:sets_inconsistent_params",
        "--target-platforms",
        "root//:p_default",
        "--show-output",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Artifact `out` was declared with `has_content_based_path = true`, but is now being used with `has_content_based_path = false`",
    );
}

#[buck_test]
async fn test_local_action_inputs_have_configuration_path_symlinks(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.build(&["root//:run_local_with_dep_on_run_remote"])
        .await?;
    let materialized_out = buck.log(&["what-materialized", "--format", "json"]).await?;
    let materialized = materialized_out
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    let run_remote_output_symlink_materialized: Vec<&Value> = materialized
        .iter()
        .filter(|entry| {
            let path = entry["path"]
                .as_str()
                .expect("what-materialized entries have a path");
            path.contains("run_remote_with_content_based_path")
                && path.ends_with("out")
                && entry["method"] == "copy"
                && entry["file_count"] == 0
        })
        .collect();
    assert_eq!(
        run_remote_output_symlink_materialized.len(),
        1,
        "{materialized:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_output_symlink_is_updated(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:run_remote_with_content_based_path";

    let result1 = buck
        .build(&[
            target,
            "-c",
            "test.data_string=hello world",
            "--show-output",
        ])
        .await?;
    let path1 = output_path(&result1, target)?;

    let actual1 = std::fs::canonicalize(buck.cwd().join(&path1))?;
    assert!(actual1.exists());
    assert_eq!(std::fs::read_to_string(&actual1)?, "hello world");

    let result2 = buck
        .build(&[
            target,
            "-c",
            "test.data_string=goodbye world",
            "--show-output",
        ])
        .await?;
    let path2 = output_path(&result2, target)?;

    assert_eq!(path2, path1);

    let actual2 = std::fs::canonicalize(buck.cwd().join(&path2))?;
    assert!(actual2.exists());
    assert_ne!(actual2, actual1);
    assert_eq!(std::fs::read_to_string(&actual2)?, "goodbye world");
    Ok(())
}

#[buck_test]
async fn test_argsfile_with_incorrectly_declared_output(buck: &Buck) {
    let target = "root//:argsfile_with_incorrectly_declared_output";
    buck.build(&[target])
        .await
        .expect_failure()
        .assert_stderr_matches("error: Artifact must be bound by now");
}

#[buck_test]
async fn test_run_action_with_incremental_metadata(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//:incremental_action";

    buck.build(&[
        target,
        "--target-platforms",
        "root//:p_default",
        "--show-output",
        "--remote-only",
    ])
    .await?;
    let what_ran1 = read_what_ran(buck, &[]).await?;
    // Flush the local dep file cache so the second (cross-configuration) build still goes through the
    // RE path and shows up in what-ran. Otherwise it would be served by the cross-configuration local
    // action cache (this action is dedupe-eligible), and what-ran would be empty. The cross-config
    // local cache hit itself is covered by test_dep_files.py::test_dep_file_hit_across_configurations.
    buck.debug(&["flush-dep-files"]).await?;
    buck.build(&[
        target,
        "--target-platforms",
        "root//:p_cat",
        "--show-output",
        "--remote-only",
    ])
    .await?;
    let what_ran2 = read_what_ran(buck, &[]).await?;

    assert_eq!(what_ran_digest(&what_ran1)?, what_ran_digest(&what_ran2)?);
    Ok(())
}

#[buck_test]
async fn test_resolve_promise_artifact(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//:resolve_promise_artifact",
        "-c",
        "test.artifact_has_content_based_path=true",
        "-c",
        "test.assert_promised_artifact_has_content_based_path=false",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Artifact promise resolved to artifact that uses content based paths. Call `actions.assert_has_content_based_path` on the promised artifact to assert that.",
    );

    buck.build(&[
        "root//:resolve_promise_artifact",
        "-c",
        "test.artifact_has_content_based_path=false",
        "-c",
        "test.assert_promised_artifact_has_content_based_path=true",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Artifact promise resolved to artifact that does not use content based paths. Remove the `actions.assert_has_content_based_path` on the promised artifact.",
    );

    buck.build(&[
        "root//:resolve_promise_artifact",
        "-c",
        "test.artifact_has_content_based_path=true",
        "-c",
        "test.assert_promised_artifact_has_content_based_path=true",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_pass_cbp_promise_artifact_to_anon_target(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:pass_cbp_promise_to_anon_target"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_run_with_anon_non_cbp_dep_eligible_for_dedupe(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//:run_with_anon_non_cbp_dep",
        "--target-platforms",
        "root//:p_default",
    ])
    .await?;
    assert_eq!(
        is_eligible_for_action_dedup(buck).await?,
        ELIGIBLE_FOR_DEDUPE
    );
    Ok(())
}

#[buck_test]
async fn test_run_with_symlink_to_non_content_based_input_eligible_for_dedupe_bug(
    buck: &Buck,
) -> anyhow::Result<()> {
    // TODO(T276504188): This test documents a BUG and asserts the buggy
    // behavior, so it must be updated once the task is fixed.
    //
    // The `run_with_symlink_to_non_cbp_input` target's consuming `run` action
    // depends on a symlink whose own path is content-based, but which points at
    // a non-content-based (configuration-based) artifact. Because it is a `run`,
    // the symlink (and therefore its target) is actually materialized as an
    // input, so the action's resolved inputs still embed a configuration hash.
    // The action is therefore NOT actually eligible for dedupe and should be
    // reported as INELIGIBLE_INPUT.
    //
    // Instead it is wrongly reported as ELIGIBLE_FOR_DEDUPE: the eligibility
    // check (`ArtifactGroup::is_eligible_for_dedupe`) only inspects each input
    // artifact's own path and never follows a symlink to its target, so it
    // misses the configuration-based target.
    //
    // When T276504188 is fixed, the consuming `run` action should instead be
    // reported as INELIGIBLE_INPUT and this assertion must be flipped.
    buck.build(&[
        "root//:run_with_symlink_to_non_cbp_input",
        "--target-platforms",
        "root//:p_default",
    ])
    .await?;

    // The consuming run action executes last (it depends on every other action
    // in the rule), so its eligibility verdict is the last event.
    assert_eq!(
        is_eligible_for_action_dedup(buck).await?,
        ELIGIBLE_FOR_DEDUPE
    );
    Ok(())
}

#[buck_test]
async fn test_not_eligible_for_dedupe(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "root//:not_eligible_for_dedupe",
        "--target-platforms",
        "root//:p_default",
        "-c",
        "test.expect_eligible_for_dedupe=false",
    ])
    .await?;

    let eligible_for_dedupe_events = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "ActionExecution",
            "eligible_for_dedupe",
        ],
    )
    .await?;

    assert_eq!(
        eligible_for_dedupe_events,
        [INELIGIBLE_OUTPUT, INELIGIBLE_INPUT]
    );

    let expected_eligible_for_dedupe_events = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "ActionExecution",
            "expected_eligible_for_dedupe",
        ],
    )
    .await?;

    assert_eq!(
        expected_eligible_for_dedupe_events,
        [UNKNOWN_ELIGIBILITY, EXPECTED_INELIGIBLE_FOR_DEDUPE]
    );
    Ok(())
}

#[buck_test]
async fn test_expect_eligible_for_dedupe_ineligible_input(buck: &Buck) {
    buck.build(&[
        "root//:not_eligible_for_dedupe",
        "--target-platforms",
        "root//:p_default",
        "-c",
        "test.expect_eligible_for_dedupe=true",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Action is marked with `expect_eligible_for_dedupe` but input.*script.py.*is not eligible for dedupe",
    );
}

#[buck_test]
async fn test_expect_eligible_for_dedupe_ineligible_input_for_execution_platform(buck: &Buck) {
    // The command is never awaited, so it never runs.
    drop(buck.build(&[
        "root//:not_eligible_for_dedupe",
        "--target-platforms",
        "root//platforms:default",
        "-c",
        "test.expect_eligible_for_dedupe=true",
    ]));
}

#[buck_test]
async fn test_expect_eligible_for_dedupe_ineligible_output(buck: &Buck) {
    buck.build(&[
        "root//:not_eligible_for_dedupe",
        "--target-platforms",
        "root//:p_default",
        "-c",
        "test.expect_eligible_for_dedupe=true",
        "-c",
        "test.run_action_output_has_content_based_path=false",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Action is marked with `expect_eligible_for_dedupe` but output `out` is not content-based",
    );
}

#[buck_test]
async fn test_execution_platform_returns_unknown_eligibility(buck: &Buck) -> anyhow::Result<()> {
    // When an action's owner is configured for an execution platform (i.e. via
    // exec_dep), eligible_for_dedupe will return EXECUTION_PLATFORM_UNKNOWN_ELIGIBILITY
    // if an input is configured for the same platform as the action itself.
    buck.build(&[
        "root//:uses_exec_dep",
        "--target-platforms",
        "root//:p_default",
    ])
    .await?;

    let eligible_for_dedupe_events = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanEnd",
            "data",
            "ActionExecution",
            "eligible_for_dedupe",
        ],
    )
    .await?;

    // The exec dep (non_content_based_exec_dep) has a single run action that hits this.
    assert_eq!(
        eligible_for_dedupe_events
            .iter()
            .filter(|e| **e == EXECUTION_PLATFORM_UNKNOWN_ELIGIBILITY)
            .count(),
        1,
        "{eligible_for_dedupe_events:?}"
    );
    Ok(())
}

#[buck_test]
async fn test_failing_run_with_run_info(buck: &Buck) {
    let failure = buck
        .build(&[
            "root//:failing_run_with_content_based_path",
            "--target-platforms",
            "root//:p_default",
            "--show-output",
        ])
        .await
        .expect_failure();
    failure
        .assert_exit_code(ExitCode::UserError)
        .assert_stderr_matches("Remote command returned non-zero exit code 1");
    assert!(
        !failure.stderr.contains(
            "Tried to resolve a content-based path out without providing the content hash!"
        ),
        "{}",
        failure.stderr
    );
}

/// Regression test for a writer-vs-writer race on shared content-based paths.
///
/// Mirrors the pattern from prelude/rust/build.bzl
/// _create_transitive_dependency_symlinks: a write_json output with
/// has_content_based_path = True whose content does not depend on
/// configuration. A single producer target is analyzed under N different
/// platforms (via N consumer targets with distinct default_target_platform),
/// so every analysis's write_json resolves to the SAME
/// __<target>__/<content_hash>/artifacts.json on-disk path. Concurrently
/// running those N writes exercises the cleanup_path -> write_file
/// sequence in immediate::write_to_disk; before the fix, two writers
/// would race on the same path and one's remove would return ENOENT,
/// failing the build with:
///     Action failed: ... (write_json artifacts.json)
///     remove_file(.../<target>/<content_hash>/artifacts.json):
///     No such file or directory (os error 2)
#[buck_test]
async fn test_shared_content_hash_race(buck: &Buck) -> anyhow::Result<()> {
    const N: usize = 200;
    let targets: Vec<String> = (0..N)
        .map(|i| format!("root//:rust_pattern_consumer_{i}"))
        .collect();
    // Embed a distinct seed per iteration via buckconfig so every iteration
    // writes different JSON content and therefore misses the RE action cache
    // and the local materializer cache. All N consumers within an iteration
    // share the same seed, so they share the same content-hash path.
    let base_seed = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    for i in 0..3 {
        let seed = format!("cbp_race.seed={}", base_seed + i);
        let mut args = vec!["-c", seed.as_str()];
        args.extend(targets.iter().map(String::as_str));
        buck.build(&args).await?;
        buck.clean(&[]).await?;
    }
    Ok(())
}
