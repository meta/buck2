/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A test per shell, named `<scenario>_<shell>`: bash, fish and zsh on Linux, bash and zsh
//! elsewhere (downloading fish on Mac is not straightforward). A scenario with one shell is a
//! single test named after the scenario alone, so a scenario can carry its shell's name itself
//! (`test_x_bash`) and have one expectation for bash and another for the rest.

use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

use buck2_e2e::prelude::*;
use tokio::io::AsyncWriteExt;

enum Expected {
    /// These completions, in order.
    Lines(&'static [&'static str]),
    /// Completions this accepts.
    Matches(fn(&[String]) -> bool),
}

struct Case {
    input: &'static str,
    expected: Expected,
    options_only: bool,
    cwd: &'static str,
    bin: &'static str,
}

const DEFAULT: Case = Case {
    input: "",
    expected: Expected::Lines(&[]),
    options_only: false,
    cwd: "",
    bin: "buck2",
};

async fn completion_test(buck: &Buck, shell: &str, case: Case) -> anyhow::Result<()> {
    let Case {
        input,
        expected,
        options_only,
        cwd,
        bin,
    } = case;
    let tmp_path = buck
        .cwd()
        .parent()
        .expect("the project is in a temporary directory")
        .join("tmp");
    std::fs::create_dir_all(&tmp_path)?;

    let verify_bin = std::path::absolute(
        std::env::var_os("BUCK2_COMPLETION_VERIFY")
            .expect("the test's `BUCK` sets `BUCK2_COMPLETION_VERIFY`"),
    )?;

    let options: &[&str] = if options_only {
        &["--options-only"]
    } else {
        &[]
    };
    let get_completions = buck.completion(&[&[shell], options].concat()).await?;
    let completions_path = std::path::absolute(tmp_path.join(format!("completion.{shell}")))?;
    std::fs::write(&completions_path, &get_completions.stdout)?;

    let shell_home = std::path::absolute(tmp_path.join(format!("{shell}_tmp")))?;
    std::fs::create_dir_all(&shell_home)?;

    let bin_dir = std::path::absolute(
        buck.executable()
            .parent()
            .expect("the executable is in a directory"),
    )?;
    let shell_home = shell_home.display();
    // Write this to a script to make it easier to debug with `BUCK_E2E_KEEP_TMP=1`
    let script = [
        "#!/bin/bash".to_owned(),
        "shopt -s dotglob".to_owned(),
        format!("export PATH=\"{}:$PATH\"", bin_dir.display()),
        "export BUCK2_COMPLETION_TIMEOUT=30000".to_owned(),
        format!("if [ -n \"$( ls -A '{shell_home}' )\" ]; then"),
        format!("    rm -r -- {shell_home}/*"),
        "fi".to_owned(),
        format!(
            "{} --tempdir {shell_home} --name {bin} {shell} {}",
            verify_bin.display(),
            completions_path.display()
        ),
    ]
    .join("\n");
    let script_path = std::path::absolute(tmp_path.join(format!("test_{shell}.sh")))?;
    std::fs::write(&script_path, script)?;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;

    // Bash and Zsh infer completion through PTY idle periods, so avoid including daemon
    // startup in that timing window. Fish also uses the warmed daemon for parity.
    buck.uquery(&["//..."]).await?;

    // The generated completion script invokes Buck recursively. It must inherit the same
    // external config and file watcher settings as the warmed daemon.
    let mut child = tokio::process::Command::new(&script_path)
        .current_dir(buck.cwd().join(cwd))
        .env_clear()
        .envs(buck.envs())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin.write_all(format!("{bin} {input}").as_bytes()).await?;
    drop(stdin);
    let output = child.wait_with_output().await?;
    assert!(
        output.status.success(),
        "`{}` failed: {}",
        script_path.display(),
        output.status
    );
    let actual: Vec<String> = String::from_utf8(output.stdout)?
        .lines()
        .map(str::to_owned)
        .collect();
    match expected {
        Expected::Lines(expected) => {
            let expected: Vec<String> = if shell == "bash" {
                // Bash treats `:` as a separator for the purpose of
                // completions as long as it's not at the end, so adjust for
                // that
                expected
                    .iter()
                    .map(|i| match i.strip_suffix(':') {
                        Some(before) => {
                            format!("{}:", before.rsplit(':').next().unwrap_or_default())
                        }
                        None => i.rsplit(':').next().unwrap_or_default().to_owned(),
                    })
                    .collect()
            } else {
                expected.iter().map(|i| (*i).to_owned()).collect()
            };
            assert_eq!(actual, expected, "testing shell: {shell}");
        }
        Expected::Matches(expected) => {
            assert!(expected(&actual), "testing shell: {shell}: {actual:?}");
        }
    }
    Ok(())
}

/// A `#[buck_test]` running `completion_test` for one shell.
macro_rules! shell_test {
    ($(#[$attr:meta])* $name:ident, $shell:literal, $case:expr) => {
        $(#[$attr])*
        #[buck_test]
        async fn $name(buck: &Buck) -> anyhow::Result<()> {
            completion_test(buck, $shell, $case).await
        }
    };
}

/// The tests of a scenario for every shell.
macro_rules! all_shells_test {
    ($bash:ident, $fish:ident, $zsh:ident, $case:expr) => {
        shell_test!($bash, "bash", $case);
        shell_test!(
            #[cfg(target_os = "linux")]
            $fish,
            "fish",
            $case
        );
        shell_test!($zsh, "zsh", $case);
    };
}

/// The tests of a scenario for zsh and fish: one for zsh alone, named after the scenario, where
/// there is no fish.
macro_rules! zsh_and_fish_test {
    ($scenario:ident, $zsh:ident, $fish:ident, $case:expr) => {
        shell_test!(
            #[cfg(target_os = "linux")]
            $zsh,
            "zsh",
            $case
        );
        shell_test!(
            #[cfg(target_os = "linux")]
            $fish,
            "fish",
            $case
        );
        shell_test!(
            #[cfg(not(target_os = "linux"))]
            $scenario,
            "zsh",
            $case
        );
    };
}

shell_test!(
    test_command_name_bash,
    "bash",
    Case {
        input: "t",
        // FIXME(JakobDegen): Should probably not be inconsistent
        expected: Expected::Lines(if cfg!(target_os = "linux") {
            &["test", "targets"]
        } else {
            &["targets", "test"]
        }),
        options_only: true,
        // Skip this on zsh and fish because they have fancy formatting with help messages for commands
        ..DEFAULT
    }
);

fn prefers_local_and_remote(actual: &[String]) -> bool {
    actual.iter().any(|a| a == "--prefer-local") && actual.iter().any(|a| a == "--prefer-remote")
}

all_shells_test!(
    test_build_flags_bash,
    test_build_flags_fish,
    test_build_flags_zsh,
    Case {
        // Use `--p` so that we don't get too many outputs, which the test framework doesn't handle well
        // on zsh
        input: "build --p",
        options_only: true,
        expected: Expected::Matches(prefers_local_and_remote),
        ..DEFAULT
    }
);

all_shells_test!(
    test_build_flags_buck_bin_bash,
    test_build_flags_buck_bin_fish,
    test_build_flags_buck_bin_zsh,
    Case {
        // Use `--p` so that we don't get too many outputs, which the test framework doesn't handle well
        // on zsh
        input: "build --p",
        options_only: true,
        expected: Expected::Matches(prefers_local_and_remote),
        bin: "buck",
        ..DEFAULT
    }
);

zsh_and_fish_test!(
    test_completes_simple_partial_directory,
    test_completes_simple_partial_directory_zsh,
    test_completes_simple_partial_directory_fish,
    Case {
        input: "build d",
        expected: Expected::Lines(&["dir1/", "dir1:", "dir2/"]),
        ..DEFAULT
    }
);

shell_test!(
    test_completes_simple_partial_directory_bash,
    "bash",
    Case {
        input: "build d",
        // On bash & mac, there's no way to disable sorting
        expected: Expected::Lines(if cfg!(target_os = "linux") {
            &["dir1/", "dir1:", "dir2/"]
        } else {
            &["dir1:", "dir1/", "dir2/"]
        }),
        ..DEFAULT
    }
);

zsh_and_fish_test!(
    test_completes_simple_directory,
    test_completes_simple_directory_zsh,
    test_completes_simple_directory_fish,
    Case {
        input: "build dir",
        expected: Expected::Lines(&["dir1/", "dir1:", "dir2/"]),
        ..DEFAULT
    }
);

shell_test!(
    test_completes_simple_directory_bash,
    "bash",
    Case {
        input: "build dir",
        // On bash & mac, there's no way to disable sorting
        expected: Expected::Lines(if cfg!(target_os = "linux") {
            &["dir1/", "dir1:", "dir2/"]
        } else {
            &["dir1:", "dir1/", "dir2/"]
        }),
        ..DEFAULT
    }
);

zsh_and_fish_test!(
    test_completes_dir_from_trailing_slash,
    test_completes_dir_from_trailing_slash_zsh,
    test_completes_dir_from_trailing_slash_fish,
    Case {
        input: "build other/",
        expected: Expected::Lines(&["other/far/", "other/far:", "other/foo/", "other/foo:"]),
        ..DEFAULT
    }
);

shell_test!(
    test_completes_dir_from_trailing_slash_bash,
    "bash",
    Case {
        input: "build other/",
        // On bash & mac, there's no way to disable sorting
        expected: Expected::Lines(if cfg!(target_os = "linux") {
            &["other/far/", "other/far:", "other/foo/", "other/foo:"]
        } else {
            &["other/far:", "other/far/", "other/foo:", "other/foo/"]
        }),
        ..DEFAULT
    }
);

all_shells_test!(
    test_completes_simple_cells_bash,
    test_completes_simple_cells_fish,
    test_completes_simple_cells_zsh,
    Case {
        input: "build cell",
        expected: Expected::Lines(&["cell2a//", "cell2a//:", "cell3//", "cell3//:"]),
        ..DEFAULT
    }
);

all_shells_test!(
    test_completes_rule_bash,
    test_completes_rule_fish,
    test_completes_rule_zsh,
    Case {
        input: "build dir1:target1",
        expected: Expected::Lines(&["dir1:target1a", "dir1:target1b"]),
        ..DEFAULT
    }
);

all_shells_test!(
    test_starts_with_colon_bash,
    test_starts_with_colon_fish,
    test_starts_with_colon_zsh,
    Case {
        input: "build :tar",
        expected: Expected::Lines(&[":target1a", ":target1b"]),
        cwd: "dir1",
        ..DEFAULT
    }
);

all_shells_test!(
    test_colon_only_arg_bash,
    test_colon_only_arg_fish,
    test_colon_only_arg_zsh,
    Case {
        input: "build :",
        expected: Expected::Lines(&[":target1a", ":target1b"]),
        cwd: "dir1",
        ..DEFAULT
    }
);

zsh_and_fish_test!(
    test_does_not_complete_files_in_target_position,
    test_does_not_complete_files_in_target_position_zsh,
    test_does_not_complete_files_in_target_position_fish,
    Case {
        input: "build TARG",
        expected: Expected::Lines(&[]),
        cwd: "dir1",
        ..DEFAULT
    }
);

shell_test!(
    test_does_not_complete_files_in_target_position_bash,
    "bash",
    Case {
        input: "build TARG",
        // FIXME(JakobDegen): Bug: expected=[],
        expected: Expected::Lines(&["TARGETS.fixture"]),
        cwd: "dir1",
        ..DEFAULT
    }
);

all_shells_test!(
    test_no_targets_for_required_option_param_bash,
    test_no_targets_for_required_option_param_fish,
    test_no_targets_for_required_option_param_zsh,
    Case {
        input: "build --console ",
        expected: Expected::Lines(&[
            "auto",
            "none",
            "simple",
            "simplenotty",
            "simpletty",
            "super",
        ]),
        ..DEFAULT
    }
);

all_shells_test!(
    test_default_completions_after_run_dash_dash_bash,
    test_default_completions_after_run_dash_dash_fish,
    test_default_completions_after_run_dash_dash_zsh,
    Case {
        input: "run :sometarget -- d",
        expected: Expected::Lines(&["dir1/", "dir2/"]),
        ..DEFAULT
    }
);

shell_test!(
    test_generated_target_handling_bash,
    "bash",
    Case {
        input: "build //generated:t_",
        expected: Expected::Lines(&[
            "//generated:t_no_labels",
            "//generated:t_not_generated",
            "//generated:t_not_generated2",
        ]),
        ..DEFAULT
    }
);

shell_test!(
    test_generated_target_handling_zsh,
    "zsh",
    Case {
        input: "build //generated:t_",
        expected: Expected::Lines(&[
            "//generated:t_no_labels",
            "//generated:t_not_generated",
            "//generated:t_not_generated2",
        ]),
        ..DEFAULT
    }
);

shell_test!(
    // FIXME(JakobDegen): Why is this different from above?
    #[cfg(target_os = "linux")]
    test_generated_target_handling_fish,
    "fish",
    Case {
        input: "build //generated:t_",
        expected: Expected::Lines(&[
            "//generated:t_not_generated",
            "//generated:t_not_generated2",
            "//generated:t_no_labels",
        ]),
        ..DEFAULT
    }
);
