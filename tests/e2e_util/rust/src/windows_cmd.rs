/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Running buck2 under `cmd.exe` with options that change how it parses scripts, so that the
//! daemon a command starts runs in that state of the shell.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;

use crate::buck::BuckCommand;
use crate::buck::read_all;
use crate::output::BuckError;
use crate::output::BuckOutput;

/// An option of `cmd.exe` that changes how it parses scripts; see `setlocal /?`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsCmdOption {
    /// `/v`: delayed expansion of environment variables.
    DelayedExpansion,
    /// `/e`: command extensions.
    Extensions,
}

impl WindowsCmdOption {
    fn switch(self) -> &'static str {
        match self {
            WindowsCmdOption::DelayedExpansion => "v",
            WindowsCmdOption::Extensions => "e",
        }
    }
}

/// Runs `command` on Windows as `cmd.exe /<option>:<on|off>... /c <command>`, with `options` in
/// their order. Elsewhere, or without options, it runs as is. Under `cmd.exe` its stdin is empty.
pub async fn run_in_windows_cmd(
    command: BuckCommand<'_>,
    options: &[(WindowsCmdOption, bool)],
) -> Result<BuckOutput, BuckError> {
    if !cfg!(windows) || options.is_empty() {
        return command.await;
    }

    let prepared = command.prepare(&uuid::Uuid::new_v4().to_string());
    if let Some(parent) = prepared
        .invocation_record_path
        .as_deref()
        .and_then(Path::parent)
    {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("Could not create `{}`: {e}", parent.display()));
    }
    let argv = windows_cmd_argv(options, &prepared.argv);
    let mut child = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&prepared.cwd)
        .env_clear()
        .envs(&prepared.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|e| panic!("Could not run `cmd.exe`: {e}"));
    let stdout = read_all(child.stdout.take().expect("stdout is piped"));
    let stderr = read_all(child.stderr.take().expect("stderr is piped"));
    let status = child
        .wait()
        .await
        .unwrap_or_else(|e| panic!("Could not wait for the process: {e}"));
    let output = BuckOutput {
        stdout: String::from_utf8_lossy(&stdout.await.expect("reading stdout")).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.await.expect("reading stderr")).into_owned(),
        status,
        command: argv,
        args: prepared
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        working_dir: prepared.cwd,
        invocation_record_path: prepared.invocation_record_path,
    };
    if output.status.success() {
        Ok(output)
    } else {
        Err(BuckError(Box::new(output)))
    }
}

fn windows_cmd_argv(options: &[(WindowsCmdOption, bool)], argv: &[OsString]) -> Vec<OsString> {
    let switches = options.iter().map(|(option, on)| {
        OsString::from(format!(
            "/{}:{}",
            option.switch(),
            if *on { "on" } else { "off" }
        ))
    });
    std::iter::once(OsString::from("cmd.exe"))
        .chain(switches)
        .chain(std::iter::once(OsString::from("/c")))
        .chain(argv.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_options_come_before_the_command_in_their_order() {
        let argv = windows_cmd_argv(
            &[
                (WindowsCmdOption::DelayedExpansion, false),
                (WindowsCmdOption::Extensions, true),
            ],
            &["buck2".into(), "build".into(), "//:a".into()],
        );
        assert_eq!(
            argv,
            ["cmd.exe", "/v:off", "/e:on", "/c", "buck2", "build", "//:a"]
                .map(OsString::from)
                .to_vec()
        );
    }

    #[test]
    fn test_one_option_gives_one_switch() {
        let argv = windows_cmd_argv(&[(WindowsCmdOption::Extensions, false)], &["buck2".into()]);
        assert_eq!(
            argv,
            ["cmd.exe", "/e:off", "/c", "buck2"]
                .map(OsString::from)
                .to_vec()
        );
    }
}
