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

/// How Python prints the value, as the fixture's target names do.
fn python_str(value: Option<bool>) -> &'static str {
    match value {
        None => "None",
        Some(true) => "True",
        Some(false) => "False",
    }
}

async fn exec_bit_of_copied_file(
    buck: &Buck,
    executable_bit_override: Option<bool>,
    write_executable_bit: Option<bool>,
    src: Option<&str>,
    is_executable: Option<bool>,
) -> anyhow::Result<()> {
    if src.is_none() && write_executable_bit.is_none() {
        return Ok(());
    }
    if src.is_some() && write_executable_bit.is_some() {
        return Ok(());
    }

    let is_executable = executable_bit_override
        .or(write_executable_bit)
        .or(is_executable)
        .expect("every case with a `src` says whether it is executable");

    let name = format!(
        "perms_{}_{}_{}",
        python_str(executable_bit_override),
        python_str(write_executable_bit),
        src.unwrap_or("None")
    );

    let res = buck
        .build_without_report(&[
            &format!(":{name}"),
            "--out=-",
            "--local-only",
            "--no-remote-cache",
        ])
        .await?;

    let expected_val = if is_executable { b'x' } else { b'-' };

    for line in res.stdout.split_whitespace() {
        let line = line.as_bytes();
        assert_eq!(line[3], expected_val);
        assert_eq!(line[6], expected_val);
        assert_eq!(line[9], expected_val);
    }
    Ok(())
}

/// The cases of `src, is_executable` x `write_executable_bit` x `executable_bit_override`, named
/// in that order.
macro_rules! exec_bit_of_copied_file_cases {
    ($($name:ident => ($src:expr, $is_executable:expr, $write:expr, $override:expr);)*) => {
        $(
            #[buck_test(skip_for_os = ["windows"])] // Exec bit and all
            #[expect(non_snake_case, reason = "the Python harness's name for the case")]
            async fn $name(buck: &Buck) -> anyhow::Result<()> {
                exec_bit_of_copied_file(buck, $override, $write, $src, $is_executable).await
            }
        )*
    };
}

exec_bit_of_copied_file_cases! {
    test_exec_bit_of_copied_file_None_None_None_None => (None, None, None, None);
    test_exec_bit_of_copied_file_None_None_None_True => (None, None, None, Some(true));
    test_exec_bit_of_copied_file_None_None_None_False => (None, None, None, Some(false));
    test_exec_bit_of_copied_file_None_None_True_None => (None, None, Some(true), None);
    test_exec_bit_of_copied_file_None_None_True_True => (None, None, Some(true), Some(true));
    test_exec_bit_of_copied_file_None_None_True_False => (None, None, Some(true), Some(false));
    test_exec_bit_of_copied_file_None_None_False_None => (None, None, Some(false), None);
    test_exec_bit_of_copied_file_None_None_False_True => (None, None, Some(false), Some(true));
    test_exec_bit_of_copied_file_None_None_False_False => (None, None, Some(false), Some(false));
    test_exec_bit_of_copied_file_files_is_executable_sh_True_None_None => (Some("files/is_executable.sh"), Some(true), None, None);
    test_exec_bit_of_copied_file_files_is_executable_sh_True_None_True => (Some("files/is_executable.sh"), Some(true), None, Some(true));
    test_exec_bit_of_copied_file_files_is_executable_sh_True_None_False => (Some("files/is_executable.sh"), Some(true), None, Some(false));
    test_exec_bit_of_copied_file_files_is_executable_sh_True_True_None => (Some("files/is_executable.sh"), Some(true), Some(true), None);
    test_exec_bit_of_copied_file_files_is_executable_sh_True_True_True => (Some("files/is_executable.sh"), Some(true), Some(true), Some(true));
    test_exec_bit_of_copied_file_files_is_executable_sh_True_True_False => (Some("files/is_executable.sh"), Some(true), Some(true), Some(false));
    test_exec_bit_of_copied_file_files_is_executable_sh_True_False_None => (Some("files/is_executable.sh"), Some(true), Some(false), None);
    test_exec_bit_of_copied_file_files_is_executable_sh_True_False_True => (Some("files/is_executable.sh"), Some(true), Some(false), Some(true));
    test_exec_bit_of_copied_file_files_is_executable_sh_True_False_False => (Some("files/is_executable.sh"), Some(true), Some(false), Some(false));
    test_exec_bit_of_copied_file_files_not_executable_sh_False_None_None => (Some("files/not_executable.sh"), Some(false), None, None);
    test_exec_bit_of_copied_file_files_not_executable_sh_False_None_True => (Some("files/not_executable.sh"), Some(false), None, Some(true));
    test_exec_bit_of_copied_file_files_not_executable_sh_False_None_False => (Some("files/not_executable.sh"), Some(false), None, Some(false));
    test_exec_bit_of_copied_file_files_not_executable_sh_False_True_None => (Some("files/not_executable.sh"), Some(false), Some(true), None);
    test_exec_bit_of_copied_file_files_not_executable_sh_False_True_True => (Some("files/not_executable.sh"), Some(false), Some(true), Some(true));
    test_exec_bit_of_copied_file_files_not_executable_sh_False_True_False => (Some("files/not_executable.sh"), Some(false), Some(true), Some(false));
    test_exec_bit_of_copied_file_files_not_executable_sh_False_False_None => (Some("files/not_executable.sh"), Some(false), Some(false), None);
    test_exec_bit_of_copied_file_files_not_executable_sh_False_False_True => (Some("files/not_executable.sh"), Some(false), Some(false), Some(true));
    test_exec_bit_of_copied_file_files_not_executable_sh_False_False_False => (Some("files/not_executable.sh"), Some(false), Some(false), Some(false));
    test_exec_bit_of_copied_file_files_executable_scripts_True_None_None => (Some("files/executable_scripts"), Some(true), None, None);
    test_exec_bit_of_copied_file_files_executable_scripts_True_None_True => (Some("files/executable_scripts"), Some(true), None, Some(true));
    test_exec_bit_of_copied_file_files_executable_scripts_True_None_False => (Some("files/executable_scripts"), Some(true), None, Some(false));
    test_exec_bit_of_copied_file_files_executable_scripts_True_True_None => (Some("files/executable_scripts"), Some(true), Some(true), None);
    test_exec_bit_of_copied_file_files_executable_scripts_True_True_True => (Some("files/executable_scripts"), Some(true), Some(true), Some(true));
    test_exec_bit_of_copied_file_files_executable_scripts_True_True_False => (Some("files/executable_scripts"), Some(true), Some(true), Some(false));
    test_exec_bit_of_copied_file_files_executable_scripts_True_False_None => (Some("files/executable_scripts"), Some(true), Some(false), None);
    test_exec_bit_of_copied_file_files_executable_scripts_True_False_True => (Some("files/executable_scripts"), Some(true), Some(false), Some(true));
    test_exec_bit_of_copied_file_files_executable_scripts_True_False_False => (Some("files/executable_scripts"), Some(true), Some(false), Some(false));
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_None_None => (Some("files/not_executable_scripts"), Some(false), None, None);
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_None_True => (Some("files/not_executable_scripts"), Some(false), None, Some(true));
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_None_False => (Some("files/not_executable_scripts"), Some(false), None, Some(false));
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_True_None => (Some("files/not_executable_scripts"), Some(false), Some(true), None);
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_True_True => (Some("files/not_executable_scripts"), Some(false), Some(true), Some(true));
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_True_False => (Some("files/not_executable_scripts"), Some(false), Some(true), Some(false));
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_False_None => (Some("files/not_executable_scripts"), Some(false), Some(false), None);
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_False_True => (Some("files/not_executable_scripts"), Some(false), Some(false), Some(true));
    test_exec_bit_of_copied_file_files_not_executable_scripts_False_False_False => (Some("files/not_executable_scripts"), Some(false), Some(false), Some(false));
}

#[buck_test] // Make sure there's at least one test defined
async fn test_dummy(_buck: &Buck) {}
