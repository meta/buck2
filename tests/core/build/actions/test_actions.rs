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
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use serde_json::Value;
use serde_json::json;

// Taken from the ActionExecutionKind enum in data.proto.
const ACTION_EXECUTION_KIND_LOCAL: i64 = 1;
const ACTION_EXECUTION_KIND_REMOTE: i64 = 2;
const ACTION_EXECUTION_KIND_REMOTE_DEDUPLICATED: i64 = 13;

const ACTION_EXECUTION: [&str; 5] = ["Event", "data", "SpanEnd", "data", "ActionExecution"];

// In Windows, we convert all symlinks to be absolute and mostly canonical
fn get_canonicalized_for_windows(dest: &Path, relative_link: &str) -> anyhow::Result<String> {
    let parent = dest.parent().context("A symlink in a directory")?;
    let canonical = std::fs::canonicalize(parent.join(relative_link))?;
    let canonical = canonical.to_string_lossy();
    // `canonicalize` gives the `\\?\` form on Windows, which `read_link` leaves out.
    Ok(canonical
        .strip_prefix(r"\\?\")
        .unwrap_or(&canonical)
        .to_owned())
}

/// The file's text with its line endings made `\n`, as Python's `Path.read_text` reads it.
fn read_text(path: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(path)?
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

/// The symlink's target, without the `\\?\` prefix a Windows target may have or not, depending on
/// how buck2 got the project's path.
fn read_link(path: &Path) -> anyhow::Result<String> {
    let target = std::fs::read_link(path)?;
    let target = target.to_string_lossy();
    Ok(target.strip_prefix(r"\\?\").unwrap_or(&target).to_owned())
}

fn hostname() -> anyhow::Result<String> {
    Ok(hostname::get()?.to_string_lossy().into_owned())
}

fn append(path: &Path, text: &str) -> anyhow::Result<()> {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())?;
    Ok(())
}

/// The values at `ActionExecution` + `path` of the last command's events.
async fn action_execution_events(buck: &Buck, path: &[&str]) -> anyhow::Result<Vec<Value>> {
    let path: Vec<&str> = ACTION_EXECUTION.iter().chain(path).copied().collect();
    filter_events(buck, &path).await
}

#[buck_test(data_dir = "actions")]
async fn test_write_json(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["//write_json:", "-c", "write_json.content=default"])
        .await?;

    let build_report = result.build_report()?;
    let output = build_report.output_for_target("//write_json:absolute")?;
    let paths: Vec<String> = serde_json::from_str(&read_text(&output)?)?;
    for path in paths {
        assert!(Path::new(&path).is_absolute(), "{path}");
    }

    // we need to test that with_inputs properly flows input dependencies through to consumers
    buck.build(&["//write_json:with_inputs", "-c", "write_json.content=other"])
        .await?;
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_write_json_stack_overflow(buck: &Buck) {
    buck.build(&["//write_json_stack_overflow:deep_write_json"])
        .await
        .expect_failure()
        .assert_stderr_matches("stack overflow \\(internal error\\)");
    buck.build(&["//write_json_stack_overflow:deep_tset"])
        .await
        .expect_failure()
        .assert_stderr_matches("stack overflow \\(internal error\\)");
}

#[buck_test(data_dir = "actions")]
async fn test_copies_files(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&[
            "//copy:file_uses_declared_output",
            "//copy:file_uses_declared_output_as_output",
            "//copy:file_declares_output",
        ])
        .await?;
    let build_report = result.build_report()?;

    let output = build_report.output_for_target("//copy:file_uses_declared_output")?;
    assert_eq!(read_text(&output)?.trim_end(), "some file");

    let output = build_report.output_for_target("//copy:file_uses_declared_output_as_output")?;
    assert_eq!(read_text(&output)?.trim_end(), "some file");

    let output = build_report.output_for_target("//copy:file_declares_output")?;
    assert_eq!(read_text(&output)?.trim_end(), "some file");

    buck.build(&["//copy:fails_on_invalid_src"])
        .await
        .expect_failure()
        .assert_stderr_matches("Type of parameter `src`");

    buck.build(&["//copy:fails_on_invalid_dest"])
        .await
        .expect_failure()
        .assert_stderr_matches("Type of parameter `dest`");
    Ok(())
}

#[buck_test(
    data_dir = "actions",
    // Because we use eden symlink redirection on MacOS
    // this test resolves both links and points to nonexistent files,
    // hence disabling eden
    setup_eden = false,
)]
async fn test_symlink_dir(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//symlinked_dir:"]).await?;
    let build_report = result.build_report()?;
    let output = build_report.output_for_target("//symlinked_dir:out")?;

    let dest1 = output.join("dir1").join("dir1_1").join("file1.txt");
    let dest2 = output.join("dep.txt");
    let dest3 = output
        .join("subdir")
        .join("dir1")
        .join("dir1_1")
        .join("file1.txt.suffix");
    let dest4 = output.join("subdir").join("dep.txt.suffix");
    let dep_output = std::fs::canonicalize(build_report.output_for_target("//symlinked_dir:dep")?)?;
    let relpath = |start: &Path| -> anyhow::Result<String> {
        let start = start.parent().context("A symlink in a directory")?;
        Ok(pathdiff::diff_paths(&dep_output, start)
            .context("Both paths are absolute")?
            .to_string_lossy()
            .into_owned())
    };

    // Example subdir: buck-out/v2/art/root/a59b783ba97fcd85891ddb2e62fbfebb/symlinked_dir/__out__/out/dir1/dir1_1
    let mut expected_link1 = "../".repeat(10) + "symlinked_dir/dir1/dir1_1/file1.txt";
    let mut expected_link2 = relpath(&dest2)?;
    let mut expected_link3 = "../".repeat(11) + "symlinked_dir/dir1/dir1_1/file1.txt";
    let mut expected_link4 = relpath(&dest4)?;

    if cfg!(windows) {
        expected_link1 = get_canonicalized_for_windows(&dest1, &expected_link1)?;
        expected_link2 = get_canonicalized_for_windows(&dest2, &expected_link2)?;
        expected_link3 = get_canonicalized_for_windows(&dest3, &expected_link3)?;
        expected_link4 = get_canonicalized_for_windows(&dest4, &expected_link4)?;
    }

    assert!(dest1.is_symlink());
    assert!(dest2.is_symlink());
    assert!(dest3.is_symlink());
    assert!(dest4.is_symlink());

    assert_eq!(read_link(&dest1)?, expected_link1);
    assert_eq!(read_link(&dest2)?, expected_link2);
    assert_eq!(read_link(&dest3)?, expected_link3);
    assert_eq!(read_link(&dest4)?, expected_link4);

    assert_eq!(read_text(&dest1)?.trim(), "dir1_1 out contents");
    assert_eq!(read_text(&dest2)?.trim(), "dep contents");
    assert_eq!(read_text(&dest3)?.trim(), "dir1_1 out contents");
    assert_eq!(read_text(&dest4)?.trim(), "dep contents");
    Ok(())
}

#[buck_test(
    data_dir = "actions",
    // See note on test_symlink_dir
    setup_eden = false,
)]
async fn test_symlink_dir_associated_artifacts(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["//symlinked_dir:symlinked_transitive_files_target"])
        .await?;
    let build_report = result.build_report()?;
    let output =
        build_report.output_for_target("//symlinked_dir:symlinked_transitive_files_target")?;

    // This is set up in symlinked_dir:target_with_tdep
    let dest = output.join("out_file");

    // The direct src of the symlink is handled properly
    assert!(dest.is_symlink());

    // The transitive dependency of the symlink is not handled properly
    assert!(!output.join("tdep1").exists());
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_simple_run(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//run:runs_simple_script"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//run:runs_simple_script")?;
    if cfg!(windows) {
        assert_eq!(read_text(&output)?, "foo\nrun\\src.txt\nbar\n");
    } else {
        assert_eq!(read_text(&output)?, "foo\nrun/src.txt\nbar\n");
    }

    let result = buck.build(&["//run:runs_simple_script_as_exe"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//run:runs_simple_script_as_exe")?;
    if cfg!(windows) {
        assert_eq!(read_text(&output)?, "foo\nrun\\src.txt\nbar\n");
    } else {
        assert_eq!(read_text(&output)?, "foo\nrun/src.txt\nbar\n");
    }

    let result = buck.build(&["//run:runs_script_locally"]).await?;
    let output = result
        .build_report()?
        .output_for_target("//run:runs_script_locally")?;
    assert_eq!(read_text(&output)?.trim(), hostname()?);

    let result = buck
        .build(&["//run:runs_script_locally_outputs_symlink"])
        .await?;
    let output = result
        .build_report()?
        .output_for_target("//run:runs_script_locally_outputs_symlink")?;
    assert!(output.is_symlink());

    buck.build(&["//run:rejects_zero_outputs"])
        .await
        .expect_failure()
        .assert_stderr_matches("expected at least one output artifact");

    buck.build(&["//run:rejects_bad_args"])
        .await
        .expect_failure()
        .assert_stderr_matches("Type of parameter `arguments` doesn't match");
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_local_action_records_hostname(buck: &Buck) -> anyhow::Result<()> {
    // A locally-executed action always runs on the buck2 daemon's host, so its
    // ActionExecutionEnd event must carry that hostname -- including on the
    // success path, which previously left it unset.
    buck.build(&["//run:runs_script_locally"]).await?;

    let actions = action_execution_events(buck, &[]).await?;
    let local: Vec<&Value> = actions
        .iter()
        .filter(|a| a["execution_kind"] == ACTION_EXECUTION_KIND_LOCAL)
        .collect();
    assert_eq!(local.len(), 1, "{actions:?}");
    assert_eq!(local[0]["hostname"], hostname()?);
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_remote_action_records_no_hostname(buck: &Buck) -> anyhow::Result<()> {
    // A remotely-executed action does not run on the daemon host, so its
    // ActionExecutionEnd event must not carry a hostname on the success path.
    buck.build(&[
        "//run:runs_simple_script_remote",
        "--remote-only",
        "--no-remote-cache",
    ])
    .await?;

    let actions = action_execution_events(buck, &[]).await?;
    let remote: Vec<&Value> = actions
        .iter()
        .filter(|a| {
            a["execution_kind"] == ACTION_EXECUTION_KIND_REMOTE
                || a["execution_kind"] == ACTION_EXECUTION_KIND_REMOTE_DEDUPLICATED
        })
        .collect();
    assert_eq!(remote.len(), 1, "{actions:?}");
    assert!(remote[0]["hostname"].is_null());
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_anon_targets(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//anon:"]).await?;

    buck.build(&["//anon_invalid_defaults/source:default_source_fails"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Anon targets do not support default values for `attrs.source\\(\\)`, specify `source_attr` explicitly",
        );

    buck.build(&["//anon_invalid_defaults/dep:default_dep_fails"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Anon targets do not support default values for `attrs.dep\\(\\)`, specify `dep_attr` explicitly",
        );

    buck.build(&["//anon_invalid_defaults/arg:default_arg_fails"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Anon targets do not support default values for `attrs.arg\\(\\)`, specify `arg_attr` explicitly",
        );

    buck.build(&["//anon_invalid_defaults/arg:arg_not_compatible"])
        .await
        .expect_failure()
        .assert_stderr_matches("Arg attribute must have `anon_target_compatible` set to `True`");

    buck.build(&["//anon_invalid_defaults/promise_artifact:bad_short_path"])
        .await
        .expect_failure()
        .assert_stderr_matches("assert_short_path\\(\\) was called with `short_path = WRONG_PATH`");

    buck.build(&["//anon_invalid_defaults/anon_rule:bad_anon_rule"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Attr type `attrs.plugin_dep\\(\\)` is not supported for anon rules",
        );
    Ok(())
}

// The missing use case is served from the fallback, which is the
// `cas_artifact_canonical_check_failed` soft error.
#[buck_test(data_dir = "actions", allow_soft_errors = true)]
async fn test_cas_artifact(buck: &Buck) -> anyhow::Result<()> {
    // The digests in `//cas_artifact:` require the buckconfig.
    // NB: cannot use `extra_buck_config` attrib of `#[buck_test]`
    append(
        &buck.cwd().join(".buckconfig"),
        "[buck2]\ndigest_algorithms = BLAKE3-KEYED,SHA1\n",
    )?;

    // The override is the daemon's canonical use case: a file artifact from another use case is
    // copied into it when that works and served from its own use case otherwise, and directory
    // and tree artifacts are never copied. Either way the build must succeed.
    let result = buck
        .build(&[
            "//cas_artifact:",
            "-c",
            "buck2_re_client.override_use_case=missing_usecase",
        ])
        .await?;

    let empty = result
        .build_report()?
        .output_for_target("//cas_artifact:empty")?;
    assert_eq!(read_text(&empty)?, "");

    let tree = result
        .build_report()?
        .output_for_target("//cas_artifact:tree")?;
    let entries = std::fs::read_dir(&tree)?
        .map(|entry| Ok(entry?.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(entries, [tree.join("b")]);
    assert_eq!(read_text(&tree.join("b"))?, "b\n");

    let tree = result
        .build_report()?
        .output_for_target("//cas_artifact:dir")?;
    let entries = std::fs::read_dir(&tree)?
        .map(|entry| Ok(entry?.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(entries, [tree.join("y")]);
    assert_eq!(read_text(&tree.join("y"))?, "hi\n");
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_invalid_command(buck: &Buck) {
    buck.build(&["//run_bad:run_invalid_command_local"])
        .await
        .expect_failure()
        .assert_stderr_matches("non-zero exit code.*no exit code");
    let expected_error = if cfg!(target_os = "linux") {
        "non-zero exit code"
    } else {
        "cannot find binary path"
    };
    buck.build(&["//run_bad:run_invalid_command_remote"])
        .await
        .expect_failure()
        .assert_stderr_matches(expected_error);
}

#[buck_test(data_dir = "actions")]
async fn test_exit_code(buck: &Buck) {
    buck.build(&["//run_bad:run_odd_exit_code"])
        .await
        .expect_failure()
        .assert_stderr_matches("non-zero exit code 45");
    // Linux does not allow negative exit codes
    if cfg!(windows) {
        buck.build(&["//run_bad:run_negative_exit_code"])
            .await
            .expect_failure()
            .assert_stderr_matches("non-zero exit code -65");
    }
}

#[buck_test(data_dir = "actions")]
async fn test_artifact_cycle(buck: &Buck) {
    buck.build(&["//run_invalid:artifact_cycle"])
        .await
        .expect_failure()
        .assert_stderr_matches("Recursion limit exceeded");
}

#[buck_test(data_dir = "actions")]
async fn test_associated_artifacts(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//associated_artifacts:check_artifacts"])
        .await?;
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_associated_artifacts_transitive_dep(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//associated_artifacts:check_dropped_artifacts"])
        .await?;
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_failure_has_wall_time(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//run_bad:run_odd_exit_code"])
        .await
        .expect_failure()
        .assert_stderr_matches("non-zero exit code 45");

    let wall_time = action_execution_events(buck, &["wall_time_us"]).await?;

    assert!(!wall_time.is_empty());
    eprintln!("{wall_time:?}");
    eprintln!("{:?}", action_execution_events(buck, &[]).await?);
    for time in wall_time {
        assert!(time.as_u64().context("A wall time in microseconds")? > 0);
    }
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_local_action_has_input_size(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//run:runs_script_locally"]).await?;
    let input_size = action_execution_events(buck, &["input_files_bytes"]).await?;

    assert!(!input_size.is_empty());

    if cfg!(windows) {
        assert_eq!(input_size[0], 448);
    } else {
        assert_eq!(input_size[0], 416);
    }
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_remote_action_has_input_size(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//run:runs_simple_script_remote"]).await?;
    let input_size = action_execution_events(buck, &["input_files_bytes"]).await?;

    assert!(!input_size.is_empty());

    if cfg!(windows) {
        assert_eq!(input_size[0], 448);
    } else {
        assert_eq!(input_size[0], 416);
    }
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_action_invalidation_tracking(buck: &Buck) -> anyhow::Result<()> {
    append(
        &buck.cwd().join(".buckconfig"),
        "[buck2]\ninvalidation_tracking_enabled = true\n[buck2]\ninvalidation_tracking_enabled = true\n",
    )?;

    buck.build(&["//run:runs_simple_script"]).await?;
    let invalidation_info = action_execution_events(buck, &["invalidation_info"]).await?;

    assert!(!invalidation_info.is_empty());
    assert_eq!(invalidation_info[0].get("changed_file"), Some(&Value::Null));

    append(&buck.cwd().join("run").join("src.txt"), "more data\n")?;

    buck.build(&["//run:runs_simple_script"]).await?;
    let invalidation_info = action_execution_events(buck, &["invalidation_info"]).await?;

    assert!(!invalidation_info.is_empty());
    assert_eq!(invalidation_info[0]["changed_file"], json!({}));
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_target_rule_type_name(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&[
        "//run:runs_simple_script",
        "//copy:file_uses_declared_output",
    ])
    .await?;

    let target_rule_type_name = action_execution_events(buck, &["target_rule_type_name"]).await?;

    assert_eq!(target_rule_type_name.len(), 2);
    assert!(target_rule_type_name.contains(&json!("copy_file")));
    assert!(target_rule_type_name.contains(&json!("run_command")));

    buck.build(&["//run_bad:run_odd_exit_code"])
        .await
        .expect_failure()
        .assert_stderr_matches("non-zero exit code 45");

    let target_rule_type_name = action_execution_events(buck, &["target_rule_type_name"]).await?;

    assert_eq!(target_rule_type_name.len(), 1);
    assert!(target_rule_type_name.contains(&json!("run_odd_exit_code")));
    Ok(())
}

#[buck_test(data_dir = "actions")]
async fn test_anon_target_rule_type_name(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["//anon:build"]).await?;

    let actions = action_execution_events(buck, &[]).await?;
    let anon_actions: Vec<&Value> = actions
        .iter()
        .filter(|action| action["key"]["owner"].get("AnonTarget").is_some())
        .collect();

    assert!(!anon_actions.is_empty(), "{actions:?}");
    assert_eq!(
        anon_actions
            .iter()
            .map(|action| action["target_rule_type_name"].as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([Some("_builder")])
    );
    Ok(())
}
