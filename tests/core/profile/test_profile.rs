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

use buck2_e2e::BuckCommand;
use buck2_e2e::prelude::*;

/// Declares tests that each call `$helper` with the test's `buck` and `tmp_path` and the given
/// arguments, one test per case of the Python test's `parametrize`.
macro_rules! profiler_tests {
    ($helper:ident { $($(#[$attr:meta])* fn $name:ident($($arg:expr),*);)* }) => {
        $(
            $(#[$attr])*
            async fn $name(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
                $helper(buck, tmp_path, $($arg),*).await
            }
        )*
    };
}

async fn assertions_for_profile_without_frozen_module(
    command: BuckCommand<'_>,
    file_path: &Path,
    profiler: &str,
) -> anyhow::Result<()> {
    if profiler.ends_with("-retained") {
        command.await.expect_failure().assert_stderr_matches(
            "Retained memory profiling is available only for analysis profile",
        );
        Ok(())
    } else {
        assert_flame_outputs(command, file_path, profiler).await
    }
}

async fn assert_flame_outputs(
    command: BuckCommand<'_>,
    file_path: &Path,
    profiler: &str,
) -> anyhow::Result<()> {
    command.await?;

    assert!(file_path.exists());
    if profiler.contains("flame") {
        assert!(file_path.join("flame.src").exists());
        assert!(file_path.join("flame.svg").exists());
    } else {
        assert!(file_path.join("profile.csv").exists());
    }

    assert!(file_path.join("targets.txt").exists());
    Ok(())
}

async fn profile_analysis_for_self_transition(
    buck: &Buck,
    tmp_path: &Path,
    profiler: &str,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    buck.profile(&[
        "analysis",
        "--target-platforms=//self_transition:p",
        "--mode",
        profiler,
        "//self_transition:zzz",
        "--output",
        file_path.to_str().unwrap(),
    ])
    .await?;

    assert!(file_path.exists());
    Ok(())
}

profiler_tests!(profile_analysis_for_self_transition {
    #[buck_test] fn test_profile_analysis_for_self_transition_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_profile_analysis_for_self_transition_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_profile_analysis_for_self_transition_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_profile_analysis_for_self_transition_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_profile_analysis_for_self_transition_time_flame("time-flame");
    #[buck_test] fn test_profile_analysis_for_self_transition_statement("statement");
    #[buck_test] fn test_profile_analysis_for_self_transition_bytecode("bytecode");
    #[buck_test] fn test_profile_analysis_for_self_transition_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_profile_analysis_for_self_transition_typecheck("typecheck");
    #[buck_test] fn test_profile_analysis_for_self_transition_coverage("coverage");
});

async fn profile_analysis_last(buck: &Buck, tmp_path: &Path, profiler: &str) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    buck.profile(&[
        "analysis",
        "--mode",
        profiler,
        "//simple:test",
        "--output",
        file_path.to_str().unwrap(),
    ])
    .await?;

    assert!(file_path.exists());
    Ok(())
}

profiler_tests!(profile_analysis_last {
    #[buck_test] fn test_profile_analysis_last_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_profile_analysis_last_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_profile_analysis_last_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_profile_analysis_last_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_profile_analysis_last_time_flame("time-flame");
    #[buck_test] fn test_profile_analysis_last_statement("statement");
    #[buck_test] fn test_profile_analysis_last_bytecode("bytecode");
    #[buck_test] fn test_profile_analysis_last_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_profile_analysis_last_typecheck("typecheck");
    #[buck_test] fn test_profile_analysis_last_coverage("coverage");
});

async fn profile_analysis_recursive(
    buck: &Buck,
    tmp_path: &Path,
    profiler: &str,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let command = buck.profile(&[
        "analysis",
        "--mode",
        profiler,
        "//simple:test",
        "--output",
        file_path.to_str().unwrap(),
        "--recursive",
    ]);
    assert_flame_outputs(command, &file_path, profiler).await
}

profiler_tests!(profile_analysis_recursive {
    #[buck_test] fn test_profile_analysis_recursive_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_profile_analysis_recursive_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_profile_analysis_recursive_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_profile_analysis_recursive_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_profile_analysis_recursive_time_flame("time-flame");
    #[buck_test] fn test_profile_analysis_recursive_statement("statement");
    #[buck_test] fn test_profile_analysis_recursive_bytecode("bytecode");
    #[buck_test] fn test_profile_analysis_recursive_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_profile_analysis_recursive_typecheck("typecheck");
    #[buck_test] fn test_profile_analysis_recursive_coverage("coverage");
});

async fn profile_analysis_recursive_transition(
    buck: &Buck,
    tmp_path: &Path,
    profiler: &str,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let command = buck.profile(&[
        "analysis",
        "--mode",
        profiler,
        "//recursive_transition:ccc",
        "--output",
        file_path.to_str().unwrap(),
        "--recursive",
    ]);

    assert_flame_outputs(command, &file_path, profiler).await
}

profiler_tests!(profile_analysis_recursive_transition {
    #[buck_test] fn test_profile_analysis_recursive_transition_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_profile_analysis_recursive_transition_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_profile_analysis_recursive_transition_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_profile_analysis_recursive_transition_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_profile_analysis_recursive_transition_time_flame("time-flame");
    #[buck_test] fn test_profile_analysis_recursive_transition_statement("statement");
    #[buck_test] fn test_profile_analysis_recursive_transition_bytecode("bytecode");
    #[buck_test] fn test_profile_analysis_recursive_transition_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_profile_analysis_recursive_transition_typecheck("typecheck");
    #[buck_test] fn test_profile_analysis_recursive_transition_coverage("coverage");
});

async fn profile_loading_last(buck: &Buck, tmp_path: &Path, profiler: &str) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let command = buck.profile(&[
        "loading",
        "--mode",
        profiler,
        "//simple:",
        "--output",
        file_path.to_str().unwrap(),
    ]);

    assertions_for_profile_without_frozen_module(command, &file_path, profiler).await
}

profiler_tests!(profile_loading_last {
    #[buck_test] fn test_profile_loading_last_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_profile_loading_last_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_profile_loading_last_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_profile_loading_last_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_profile_loading_last_time_flame("time-flame");
    #[buck_test] fn test_profile_loading_last_statement("statement");
    #[buck_test] fn test_profile_loading_last_bytecode("bytecode");
    #[buck_test] fn test_profile_loading_last_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_profile_loading_last_typecheck("typecheck");
    #[buck_test] fn test_profile_loading_last_coverage("coverage");
});

async fn query_profile(buck: &Buck, tmp_path: &Path, profiler: &str) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let command = buck.cquery(&[
        "--profile-mode",
        profiler,
        "deps(//query/a:a)",
        "--profile-output",
        file_path.to_str().unwrap(),
    ]);

    assertions_for_profile_without_frozen_module(command, &file_path, profiler).await?;

    if !profiler.ends_with("-retained") {
        let targets = std::fs::read_to_string(buck.cwd().join(&file_path).join("targets.txt"))?;
        let mut lines: Vec<&str> = targets.lines().map(str::trim_end).collect();
        lines.sort();
        assert_eq!(lines, ["load/root//query/a", "load/root//query/b"]);
    } else {
        assert!(!buck.cwd().join(&file_path).exists());
    }
    Ok(())
}

profiler_tests!(query_profile {
    #[buck_test] fn test_query_profile_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_query_profile_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_query_profile_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_query_profile_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_query_profile_time_flame("time-flame");
    #[buck_test] fn test_query_profile_statement("statement");
    #[buck_test] fn test_query_profile_bytecode("bytecode");
    #[buck_test] fn test_query_profile_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_query_profile_typecheck("typecheck");
    #[buck_test] fn test_query_profile_coverage("coverage");
});

#[buck_test]
async fn test_profile_loading_last_single_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let profiler = "statement";

    let command = buck.profile(&[
        "loading",
        "--mode",
        profiler,
        "//simple:a",
        "--output",
        file_path.to_str().unwrap(),
    ]);

    assertions_for_profile_without_frozen_module(command, &file_path, profiler).await
}

async fn profile_analysis_pattern(
    buck: &Buck,
    tmp_path: &Path,
    recursive: bool,
    profiler: &str,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let mut args = vec![
        "analysis",
        "--mode",
        profiler,
        "//simple/...", // We test this.
        "--output",
        file_path.to_str().unwrap(),
    ];
    if recursive {
        args.push("--recursive");
    }
    let command = buck.profile(&args);

    assert_flame_outputs(command, &file_path, profiler).await
}

profiler_tests!(profile_analysis_pattern {
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_heap_flame_allocated(true, "heap-flame-allocated");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_heap_flame_retained(true, "heap-flame-retained");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_heap_summary_allocated(true, "heap-summary-allocated");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_heap_summary_retained(true, "heap-summary-retained");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_time_flame(true, "time-flame");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_statement(true, "statement");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_bytecode(true, "bytecode");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_bytecode_pairs(true, "bytecode-pairs");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_typecheck(true, "typecheck");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_True_coverage(true, "coverage");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_heap_flame_allocated(false, "heap-flame-allocated");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_heap_flame_retained(false, "heap-flame-retained");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_heap_summary_allocated(false, "heap-summary-allocated");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_heap_summary_retained(false, "heap-summary-retained");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_time_flame(false, "time-flame");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_statement(false, "statement");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_bytecode(false, "bytecode");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_bytecode_pairs(false, "bytecode-pairs");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_typecheck(false, "typecheck");
    #[buck_test] #[expect(non_snake_case, reason = "named like the Python test case")] fn test_profile_analysis_pattern_False_coverage(false, "coverage");
});

async fn profile_loading_recursive(
    buck: &Buck,
    tmp_path: &Path,
    profiler: &str,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let command = buck.profile(&[
        "loading",
        "--mode",
        profiler,
        "//simple:",
        "--output",
        file_path.to_str().unwrap(),
        "--recursive",
    ]);

    command
        .await
        .expect_failure()
        .assert_stderr_matches("Recursive profiling is not supported for loading profiling");
    Ok(())
}

profiler_tests!(profile_loading_recursive {
    #[buck_test] fn test_profile_loading_recursive_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_profile_loading_recursive_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_profile_loading_recursive_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_profile_loading_recursive_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_profile_loading_recursive_time_flame("time-flame");
    #[buck_test] fn test_profile_loading_recursive_statement("statement");
    #[buck_test] fn test_profile_loading_recursive_bytecode("bytecode");
    #[buck_test] fn test_profile_loading_recursive_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_profile_loading_recursive_typecheck("typecheck");
    #[buck_test] fn test_profile_loading_recursive_coverage("coverage");
});

async fn profile_bxl_with_actions(
    buck: &Buck,
    tmp_path: &Path,
    profiler: &str,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let command = buck.profile(&[
        "bxl",
        "--mode",
        profiler,
        "//bxl/profile.bxl:profile_with_actions",
        "--output",
        file_path.to_str().unwrap(),
    ]);

    assert_flame_outputs(command, &file_path, profiler).await
}

profiler_tests!(profile_bxl_with_actions {
    #[buck_test] fn test_profile_bxl_with_actions_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_profile_bxl_with_actions_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_profile_bxl_with_actions_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_profile_bxl_with_actions_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_profile_bxl_with_actions_time_flame("time-flame");
    #[buck_test] fn test_profile_bxl_with_actions_statement("statement");
    #[buck_test] fn test_profile_bxl_with_actions_bytecode("bytecode");
    #[buck_test] fn test_profile_bxl_with_actions_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_profile_bxl_with_actions_typecheck("typecheck");
    #[buck_test] fn test_profile_bxl_with_actions_coverage("coverage");
});

async fn profile_bxl_without_actions(
    buck: &Buck,
    tmp_path: &Path,
    profiler: &str,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let command = buck.profile(&[
        "bxl",
        "--mode",
        profiler,
        "//bxl/profile.bxl:profile_without_actions",
        "--output",
        file_path.to_str().unwrap(),
    ]);

    assert_flame_outputs(command, &file_path, profiler).await
}

profiler_tests!(profile_bxl_without_actions {
    #[buck_test] fn test_profile_bxl_without_actions_heap_flame_allocated("heap-flame-allocated");
    #[buck_test] fn test_profile_bxl_without_actions_heap_flame_retained("heap-flame-retained");
    #[buck_test] fn test_profile_bxl_without_actions_heap_summary_allocated("heap-summary-allocated");
    #[buck_test] fn test_profile_bxl_without_actions_heap_summary_retained("heap-summary-retained");
    #[buck_test] fn test_profile_bxl_without_actions_time_flame("time-flame");
    #[buck_test] fn test_profile_bxl_without_actions_statement("statement");
    #[buck_test] fn test_profile_bxl_without_actions_bytecode("bytecode");
    #[buck_test] fn test_profile_bxl_without_actions_bytecode_pairs("bytecode-pairs");
    #[buck_test] fn test_profile_bxl_without_actions_typecheck("typecheck");
    #[buck_test] fn test_profile_bxl_without_actions_coverage("coverage");
});

#[buck_test(setup_eden = true)]
async fn test_profile_no_buckd(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");

    let command = buck
        .profile(&[
            "loading",
            "--mode",
            "statement",
            "//simple:",
            "--output",
            file_path.to_str().unwrap(),
            "--no-buckd",
        ])
        .await?;

    assert!(
        command.stdout.contains("Total retained bytes:"),
        "{}",
        command.stdout
    );
    assert!(file_path.exists());
    Ok(())
}

#[buck_test]
async fn test_profile_loading_recursive_target_pattern(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_path = tmp_path.join("profile");
    let profiler = "time-flame";

    let command = buck.profile(&[
        "loading",
        "--mode",
        profiler,
        "//simple/...",
        "--output",
        file_path.to_str().unwrap(),
    ]);

    assertions_for_profile_without_frozen_module(command, &file_path, profiler).await
}

/// The files under `dir`, recursively, relative to it.
fn walk_files(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            walk_files(&entry.path(), files)?;
        } else {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// Each file `--profile-patterns-output` wrote to `tmp_path`, as `file: True/False`, where the
/// bool indicates a non-zero size.
fn profile_pattern_files(tmp_path: &Path) -> anyhow::Result<String> {
    // Use paths relative to tmpdir instead of just filenames
    let mut files = Vec::new();
    walk_files(tmp_path, &mut files)?;
    let mut files_with_sizes = Vec::new();
    for fpath in files {
        let size = match std::fs::metadata(&fpath) {
            Ok(metadata) => metadata.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let rel_path = fpath.strip_prefix(tmp_path)?;

        // Drop the first path component from rel_path before storing (it's nondeterministic of the format <timestamp>-<builduuid>)
        let mut components = rel_path.components();
        assert!(components.clone().count() > 1);
        components.next();
        let rel_path = components.as_path();

        files_with_sizes.push((rel_path.to_string_lossy().into_owned(), size));
    }

    // Sort by filename
    files_with_sizes.sort_by(|a, b| a.0.cmp(&b.0));

    // Format as "file: True/False" where bool indicates non-zero size
    Ok(files_with_sizes
        .iter()
        .map(|(fname, size)| format!("{fname}: {}", if *size > 0 { "True" } else { "False" }))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_profile_patterns(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.build(&[
        "//simple/...",
        "--profile-patterns=.*",
        "--profile-patterns-mode=statement",
        &format!("--profile-patterns-output={}", tmp_path.display()),
    ])
    .await?;

    golden(&profile_pattern_files(tmp_path)?, "profile_patterns.golden");
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_profile_patterns_flame(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    buck.build(&[
        "//simple/...",
        "--profile-patterns=.*",
        "--profile-patterns-mode=time-flame",
        &format!("--profile-patterns-output={}", tmp_path.display()),
    ])
    .await?;

    golden(
        &profile_pattern_files(tmp_path)?,
        "profile_patterns_flame.golden",
    );
    Ok(())
}
