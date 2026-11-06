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

#[buck_test]
async fn test_run_with_source_macros(buck: &Buck) -> anyhow::Result<()> {
    let sep = if cfg!(windows) { "\\" } else { "/" };
    let result = buck.run(&["//source:echo_file"]).await?;
    assert!(result.stdout.ends_with(&format!("source{sep}foo.txt\n")));

    let result = buck.run(&["//source:echo_dir"]).await?;
    assert!(result.stdout.ends_with(&format!("source{sep}bar\n")));

    let result = buck.run(&["//source:cat_file"]).await?;
    assert_eq!(result.stdout, "foo file\n");

    let result = buck.run(&["//source:cat_dir"]).await?;
    assert_eq!(result.stdout, "bar file\n");
    Ok(())
}

#[buck_test]
async fn test_no_dep_in_source(buck: &Buck) {
    buck.build(&["//dep_as_source:uses_dep"])
        .await
        .expect_failure()
        .assert_stderr_matches("File not found: `root//dep_as_source/:trivial`");
}

async fn unrecognized_macro_fails_loading(
    buck: &Buck,
    macro_: &str,
    name: &str,
    arg_count: usize,
) -> anyhow::Result<()> {
    std::fs::write(
        buck.cwd().join("TARGETS.fixture"),
        format!(
            "load(\"//source:defs.bzl\", \"echo_rule\")\necho_rule(name = \"invalid\", arg = \"$({macro_})\")\n"
        ),
    )?;
    let failure = buck.uquery(&["//:invalid"]).await.expect_failure();
    failure
        .assert_exit_code(ExitCode::UserError)
        .assert_stderr_matches(&format!(
            "Unrecognized macro `{name}` \\(with {arg_count} args\\)"
        ));
    assert!(
        failure
            .stderr
            .contains(&format!("escape it as `\\$({name} ...)`"))
    );
    Ok(())
}

#[buck_test(allow_soft_errors = true)]
async fn test_unrecognized_macro_fails_loading_output_foo_yaml_output_1(
    buck: &Buck,
) -> anyhow::Result<()> {
    unrecognized_macro_fails_loading(buck, "output foo.yaml", "output", 1).await
}

#[buck_test(allow_soft_errors = true)]
async fn test_unrecognized_macro_fails_loading_find_name_foo_find_3(
    buck: &Buck,
) -> anyhow::Result<()> {
    unrecognized_macro_fails_loading(buck, "find . -name foo", "find", 3).await
}
