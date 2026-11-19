/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::GOLDEN_DIRECTORY;
use buck2_e2e::golden::sanitize_hashes;
use buck2_e2e::prelude::*;
use regex::Regex;
use serde_json::json;

fn extract_configuration(s: &str) -> Vec<String> {
    Regex::new(r"\((<modifiers>#[a-f0-9]+)\)")
        .unwrap()
        .captures_iter(s)
        .map(|c| c[1].to_owned())
        .collect()
}

/// The one configuration in `s`.
#[track_caller]
fn single_configuration(s: &str) -> String {
    match <[String; 1]>::try_from(extract_configuration(s)) {
        Ok([configuration]) => configuration,
        Err(configurations) => panic!("Expected one configuration, got {configurations:?}"),
    }
}

#[buck_test]
async fn test_bxl_cli(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_test",
            "--",
            "--int_arg",
            "1",
            "--float_arg",
            "4.3",
            "--enum_type",
            "a",
            "--list_type",
            "1",
            "2",
            "3",
            "--target",
            ":foo",
            "--sub_target",
            "cell/pkg:bar[sub]",
            "--configured_target",
            "root//:t1?root//:macos",
        ])
        .await?;

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_bxl_cli_standard.golden.txt"),
    );

    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_test",
            "--",
            // Override default bool arg with false
            "--bool_arg_with_default",
            "false",
            "--int_arg",
            "2",
            "--float_arg",
            "3.4",
            "--optional",
            "value",
            "--enum_type",
            "b",
            "--list_type",
            "1",
            "--target",
            "bar:foo",
            "--sub_target",
            "cell/pkg:bar",
            "--configured_target",
            "root//:t1?root//:macos",
        ])
        .await?;

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_bxl_cli_override_default.golden.txt"),
    );

    // multiple occurrences of a list-type argument
    // i.e., --arg 1 --arg 2 --arg 3
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_test",
            "--",
            "--int_arg",
            "1",
            "--float_arg",
            "4.3",
            "--enum_type",
            "a",
            "--list_type",
            "1",
            "--list_type",
            "2",
            "--list_type",
            "3",
            "--target",
            ":foo",
            "--sub_target",
            "cell/pkg:bar[sub]",
            "--configured_target",
            "root//:t1?root//:macos",
        ])
        .await?;

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_bxl_cli_multiple_list_occurrences.golden.txt"),
    );

    // illegal target
    buck.bxl(&[
        "//cli_args.bxl:cli_test",
        "--",
        "--int_arg",
        "2",
        "--float_arg",
        "3.4",
        "--optional",
        "value",
        "--enum_type",
        "b",
        "--list_type",
        "1",
        "--target",
        "illegal;target",
        "--sub_target",
        "cell/pkg:bar",
        "--configured_target",
        "root//:t1?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Invalid target name `illegal;target`");

    // not int
    buck.bxl(&[
        "//cli_args.bxl:cli_test",
        "--",
        "--int_arg",
        "2.0",
        "--float_arg",
        "3.4",
        "--optional",
        "value",
        "--enum_type",
        "b",
        "--list_type",
        "1",
        "--target",
        ":foo",
        "--sub_target",
        "cell/pkg:bar",
        "--configured_target",
        "root//:t1?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "invalid value '2.0' for '--int_arg <int_arg>': invalid digit found in string",
    );

    // list inner type mismatch
    buck.bxl(&[
        "//cli_args.bxl:cli_test",
        "--",
        "--int_arg",
        "2",
        "--float_arg",
        "3.4",
        "--optional",
        "value",
        "--enum_type",
        "b",
        "--list_type",
        "wrong_inner_list_type",
        "--target",
        "bar:foo",
        "--sub_target",
        "cell/pkg:bar",
        "--configured_target",
        "root//:t1?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"invalid value 'wrong_inner_list_type' for '--list_type \[<list_type>...\]': invalid digit found in string",
    );

    // not valid enum variant
    buck.bxl(&[
        "//cli_args.bxl:cli_test",
        "--",
        "--int_arg",
        "2",
        "--float_arg",
        "3.4",
        "--optional",
        "value",
        "--enum_type",
        "not_enum",
        "--list_type",
        "1",
        "--target",
        ":foo",
        "--sub_target",
        "cell/pkg:bar",
        "--configured_target",
        "root//:t1?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("invalid value 'not_enum' for '--enum_type <enum_type>'");

    // missing non-optional field
    buck.bxl(&[
        "//cli_args.bxl:cli_test",
        "--",
        "--int_arg",
        "2",
        "--optional",
        "value",
        "--enum_type",
        "a",
        "--list_type",
        "1",
        "--target",
        ":foo",
        "--sub_target",
        "cell/pkg:bar",
        "--configured_target",
        "root//:t1?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Missing cli arg from command line that isn't optional nor has any default values",
    );

    // check short args work
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_test_short",
            "--",
            "-i",
            "1",
            "-f",
            "4.3",
            "-e",
            "a",
            "-l",
            "1",
            "2",
            "3",
            "-t",
            ":foo",
            "-s",
            "default",
            "-c",
            "root//:t1?root//:macos",
        ])
        .await?;

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_bxl_cli_short.golden.txt"),
    );

    // check long args still work with short args
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_test_short",
            "--",
            "--int_arg",
            "1",
            "--float_arg",
            "4.3",
            "--enum_type",
            "a",
            "--list_type",
            "1",
            "2",
            "3",
            "--target",
            ":foo",
            "--string_arg",
            "default",
            "--configured_target",
            "root//:t1?root//:macos",
        ])
        .await?;

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_bxl_cli_short_long.golden.txt"),
    );

    // check snakecase cli_arg access from bxl context, make sure it still works with default args and shorthand args
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_test_snakecase_access",
            "--",
            "--my-arg",
            "this is my arg",
        ])
        .await?;
    assert_eq!(result.stdout, "my-arg: \"this is my arg\"\n");

    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_test_snakecase_access",
            "--",
            "-a",
            "this is my arg",
        ])
        .await?;
    assert_eq!(result.stdout, "my-arg: \"this is my arg\"\n");

    let result = buck
        .bxl(&["//cli_args.bxl:cli_test_snakecase_access"])
        .await?;
    assert_eq!(result.stdout, "my-arg: \"default\"\n");

    buck.bxl(&[
        "//cli_args_bad_case.bxl:cli_test_bad_case",
        "--",
        "--my-arg",
        "this is my arg",
    ])
    .await
    .expect_failure();
    Ok(())
}

#[buck_test]
async fn test_bxl_cli_json_args(buck: &Buck) -> anyhow::Result<()> {
    let json_args = json!({
        "int": 1,
        "string": "foo",
        "float": 1.0,
        "bool": true,
        "none": null,
        "list": [1, 2, 3],
        "nested": {"nested_string": "bar", "nested_int": -1},
    });

    let my_json = json_args.to_string();

    buck.bxl(&["//cli_args.bxl:cli_json_arg", "--", "--my-json", &my_json])
        .await?;

    buck.bxl(&["//cli_args.bxl:cli_json_arg", "--", "--my-json", "[1,2,3]"])
        .await
        .expect_failure()
        .assert_stderr_matches(r"Expecting json object. Got: `\[1,2,3]`");
    Ok(())
}

#[buck_test]
async fn test_bxl_cli_short_bad(buck: &Buck) {
    // duplicate "short"
    buck.bxl(&[
        "//cli_args_bad.bxl:cli_test_short_bad",
        "--",
        "-a",
        "2",
        "-a",
        "value",
        "-a",
        "a",
        "-a",
        "1",
        "-a",
        ":foo",
        "-a",
        "default",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Duplicate short args are not allowed");
}

#[buck_test]
async fn test_cli_target_pattern(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//cli_args.bxl:target_expr_test", "--", "--targets", ":t1"])
        .await?;
    assert!(result.stdout.contains("[root//:t1]"));

    let result = buck
        .bxl(&[
            "//cli_args.bxl:target_expr_test",
            "--",
            "--targets",
            "root//:",
        ])
        .await?;
    assert!(result.stdout.contains("root//:t1"));
    assert!(result.stdout.contains("root//:t2"));

    buck.bxl(&[
        "//cli_args.bxl:target_expr_test",
        "--",
        "--targets",
        ":non-existent",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Unknown target `non-existent` from package `root//`");

    buck.bxl(&[
        "//cli_args.bxl:target_expr_test",
        "--",
        "--targets",
        "invalid/...",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Directory `invalid` does not exist");
    Ok(())
}

#[buck_test]
async fn test_cli_sub_target_pattern(buck: &Buck) -> anyhow::Result<()> {
    // Tests where no sub-target is specified; should ensure functionality
    // of regular target patterns work with these subtarget patterns.
    let result = buck
        .bxl(&[
            "//cli_args.bxl:sub_target_expr_test",
            "--",
            "--sub_targets",
            ":t1",
        ])
        .await?;
    assert!(result.stdout.contains("[root//:t1]"));

    let result = buck
        .bxl(&[
            "//cli_args.bxl:sub_target_expr_test",
            "--",
            "--sub_targets",
            "root//:",
        ])
        .await?;
    assert!(result.stdout.contains("root//:t1"));
    assert!(result.stdout.contains("root//:t2"));

    // Test single sub-targets.
    let result = buck
        .bxl(&[
            "//cli_args.bxl:sub_target_expr_test",
            "--",
            "--sub_targets",
            "root//:t1[sub]",
        ])
        .await?;
    assert!(result.stdout.contains("[root//:t1[sub]]"));

    // Several subtargets / nested subtargets.
    let result = buck
        .bxl(&[
            "//cli_args.bxl:sub_target_expr_test",
            "--",
            "--sub_targets",
            "root//:t2[sub1][sub2]",
        ])
        .await?;
    assert!(result.stdout.contains("[root//:t2[sub1][sub2]]"));

    buck.bxl(&[
        "//cli_args.bxl:sub_target_expr_test",
        "--",
        "--sub_targets",
        ":fake_bin[sub]",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Unknown target `fake_bin` from package `root//`");
    Ok(())
}

#[buck_test]
async fn test_cli_target_fails_with_question_mark_modifier_syntax(buck: &Buck) {
    buck.bxl(&[
        "//cli_args.bxl:cli_test",
        "--",
        "--int_arg",
        "1",
        "--float_arg",
        "4.3",
        "--enum_type",
        "a",
        "--list_type",
        "1",
        "2",
        "3",
        "--target",
        "root//:t1?root//:macos",
        "--sub_target",
        "cell/pkg:bar[sub]",
        "--configured_target",
        "root//:t1?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(r"The \?modifier syntax is unsupported for this command");

    buck.bxl(&[
        "//cli_args.bxl:target_expr_test",
        "--",
        "--targets",
        "root//:t1?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(r"The \?modifier syntax is unsupported for this command");
}

#[buck_test]
async fn test_cli_configured_target_fails_with_global_modifiers(buck: &Buck) {
    buck.bxl(&[
        "--modifier",
        "root//:macos",
        "//cli_args.bxl:cli_configured_target",
        "--",
        "--configured_target",
        "root//:t1?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
    );

    buck.bxl(&[
        "--modifier",
        "root//:macos",
        "//cli_args.bxl:cli_configured_target",
        "--",
        "--configured_target",
        "root//:?root//:macos",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
    );
}

#[buck_test]
async fn test_cli_configured_target_pattern(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1?root//:macos",
        ])
        .await?;

    let configuration = single_configuration(&result.stdout);

    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));

    // test multiple modifiers
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1?root//:macos+root//:arm",
        ])
        .await?;

    let configuration = single_configuration(&result.stdout);

    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));
    assert!(cfg.stdout.contains("root//:arm"));

    // test order of modifiers
    // if passing in modifiers of the same constraint setting,
    // the last one should be the one that applies
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1?root//:macos+root//:linux",
        ])
        .await?;

    let configuration = single_configuration(&result.stdout);

    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:linux"));
    assert!(!cfg.stdout.contains("root//:macos"));

    // test no modifiers
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1",
        ])
        .await?;

    assert!(
        result
            .stdout
            .contains("configured_target: root//:t1 (<unspecified>)")
    );

    // test expr
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "//:t1?root//:macos",
        ])
        .await?;

    let configuration = single_configuration(&result.stdout);
    let cfg = buck.audit_configurations(&[&configuration]).await?;

    assert!(cfg.stdout.contains("root//:macos"));

    // test expr with package pattern
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "root//:?root//:macos",
        ])
        .await?;

    for configuration in extract_configuration(&result.stdout) {
        let cfg = buck.audit_configurations(&[&configuration]).await?;
        assert!(cfg.stdout.contains("root//:macos"));
    }

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_cli_configured_target_expr_package.golden.txt"),
    );

    // test expr with recursive pattern
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "root//...?root//:macos+root//:arm",
        ])
        .await?;

    for configuration in extract_configuration(&result.stdout) {
        let cfg = buck.audit_configurations(&[&configuration]).await?;
        assert!(cfg.stdout.contains("root//:macos"));
        assert!(cfg.stdout.contains("root//:arm"));
    }

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_cli_configured_target_expr_recursive.golden.txt"),
    );
    Ok(())
}

#[buck_test]
async fn test_cli_configured_target_modifiers_flag(buck: &Buck) -> anyhow::Result<()> {
    // test single modifier
    let result = buck
        .bxl(&[
            "--modifier",
            "root//:macos",
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1",
        ])
        .await?;

    let configuration = single_configuration(&result.stdout);

    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));

    // test multiple modifiers
    let result = buck
        .bxl(&[
            "--modifier",
            "root//:macos",
            "--modifier",
            "root//:arm",
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1",
        ])
        .await?;

    let configuration = single_configuration(&result.stdout);
    let cfg = buck.audit_configurations(&[&configuration]).await?;
    assert!(cfg.stdout.contains("root//:macos"));
    assert!(cfg.stdout.contains("root//:arm"));

    // test expr
    let result = buck
        .bxl(&[
            "--modifier",
            "root//:macos",
            "--modifier",
            "root//:arm",
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "//:",
        ])
        .await?;

    for configuration in extract_configuration(&result.stdout) {
        let cfg = buck.audit_configurations(&[&configuration]).await?;
        assert!(cfg.stdout.contains("root//:macos"));
        assert!(cfg.stdout.contains("root//:arm"));
    }

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_cli_configured_target_modifiers_flag_expr.golden.txt"),
    );
    Ok(())
}

#[buck_test]
async fn test_cli_configured_target_platform(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "--target-platforms",
            "root//:p",
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1",
        ])
        .await?;

    assert!(
        sanitize_hashes(&result.stdout).contains("configured_target: root//:t1 (root//:p#<HASH>)")
    );

    let result = buck
        .bxl(&[
            "--target-platforms",
            "root//:p",
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "//:",
        ])
        .await?;

    golden(
        &sanitize_hashes(&result.stdout),
        &format!("{GOLDEN_DIRECTORY}test_cli_configured_target_platform_expr.golden.txt"),
    );
    Ok(())
}

#[buck_test]
async fn test_cli_json_file(buck: &Buck) -> anyhow::Result<()> {
    let json_file_path = buck.cwd().join("a.json");

    buck.bxl(&[
        "//cli_args.bxl:cli_json_file",
        "--",
        "--json-file",
        &json_file_path.to_string_lossy(),
    ])
    .await?;

    buck.bxl(&[
        "//cli_args.bxl:cli_json_file",
        "--",
        "--json-file",
        "a.json",
    ])
    .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_cli_option_default_is_used(buck: &Buck) -> anyhow::Result<()> {
    // `cli_args.option(T, default = X)` gives X when the flag is absent, and None when no
    // default was declared.
    let result = buck.bxl(&["//cli_args.bxl:cli_option_default"]).await?;
    assert!(result.stdout.contains("opt_int: 5\n"));
    assert!(result.stdout.contains("opt_str: \"consumer\"\n"));
    assert!(result.stdout.contains("opt_bool: True\n"));
    assert!(result.stdout.contains("opt_list: None\n"));

    // A given flag wins.
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_option_default",
            "--",
            "--opt_int",
            "7",
            "--opt_bool",
            "false",
        ])
        .await?;
    assert!(result.stdout.contains("opt_int: 7\n"));
    assert!(result.stdout.contains("opt_bool: False\n"));

    // A bare `--opt_list` gives [] like a plain list arg does.
    let result = buck
        .bxl(&["//cli_args.bxl:cli_option_default", "--", "--opt_list"])
        .await?;
    assert!(result.stdout.contains("opt_list: []\n"));
    Ok(())
}

#[buck_test]
async fn test_bxl_cli_negative_int_is_accepted(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//cli_args.bxl:cli_int", "--", "--n", "-5"])
        .await?;
    assert!(result.stdout.contains("n: -5\n"));
    let result = buck
        .bxl(&[
            "//cli_args.bxl:cli_int",
            "--",
            "--n",
            "1",
            "--ns",
            "1",
            "-2",
        ])
        .await?;
    assert!(result.stdout.contains("ns: [1, -2]\n"));
    let result = buck
        .bxl(&["//cli_args.bxl:cli_int", "--", "--n=-5"])
        .await?;
    assert!(result.stdout.contains("n: -5\n"));
    Ok(())
}
