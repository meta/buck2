/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::path::PathBuf;
use std::process::Command;

use clap::Parser;

use crate::bash::run_bash;
use crate::fish::run_fish;
use crate::zsh::run_zsh;

mod bash;
mod fish;
mod runtime;
mod zsh;

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
#[clap(rename_all = "kebab-case")]
enum Shell {
    Bash,
    Fish,
    Zsh,
}

/// The install directory of a shell that Meta's Linux build bundles with this binary. Elsewhere,
/// and in open source, there is none, and the shell on `PATH` runs.
pub(crate) fn bundled_shell(shell: &str) -> Option<PathBuf> {
    buck_resources::get(format!("buck2/shed/completion_verify/{shell}")).ok()
}

impl Shell {
    fn find(self) -> io::Result<Command> {
        match self {
            Self::Bash => Ok(Command::new("bash")),
            Self::Fish => Ok(match bundled_shell("fish") {
                Some(dir) => Command::new(dir.join("bin/fish")),
                None => Command::new("fish"),
            }),
            Self::Zsh => Ok(match bundled_shell("zsh") {
                Some(dir) => Command::new(dir.join("usr/bin/zsh")),
                None => Command::new("zsh"),
            }),
        }
    }
}

fn extract_from_outputs<S: AsRef<str>>(
    input: &str,
    raw_outs: impl IntoIterator<Item = io::Result<S>>,
) -> io::Result<Vec<String>> {
    for raw_out in raw_outs {
        if let Some(options) = extract_from_single_output(input, raw_out?.as_ref()) {
            return Ok(options);
        }
    }
    Ok(Vec::new())
}

/// Accepts an output like `% buck2 targets` or `% buck2\ntargets   test` and returns
/// the possible completions
fn extract_from_single_output(input: &str, raw_out: &str) -> Option<Vec<String>> {
    if let Some((_, rest)) = raw_out.split_once('\n') {
        // Multiple lines of output indicates there is more than one option. Just naively splitting
        // the output by whitespace is unfortunate wrong in hypothetical cases of completions with
        // spaces, but those should be uncommon so this is fine.
        Some(
            rest.lines()
                // Fish can redraw the command line on another terminal row while rendering the
                // completion menu. Do not treat redrawn prompt lines as completion candidates.
                .filter(|line| !line.starts_with("% "))
                .flat_map(str::split_ascii_whitespace)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    } else {
        let raw_out = raw_out.strip_prefix("% ").unwrap_or(raw_out);

        // No outputed completions
        if raw_out == input || raw_out.is_empty() {
            return None;
        }

        if !raw_out.ends_with(|c: char| c.is_ascii_whitespace()) {
            // Output does not end with whitespace. This means that the output is a partial
            // completion, and so we'll return `None` to indicate that the completion should be
            // retried with an additional tab
            return None;
        }

        // Find the first changed word and copy everything beginning there
        let mut last_equal = 0;
        for (i, c) in raw_out.char_indices() {
            if c.is_ascii_whitespace() && input.len() > i {
                // Include this character in the comparison
                let i = i + 1;
                if raw_out.as_bytes()[..i] == input.as_bytes()[..i] {
                    last_equal = i;
                } else {
                    break;
                }
            }
        }
        Some(vec![raw_out[last_equal..].trim_end().to_owned()])
    }
}

fn run(
    completion_name: &str,
    script: &str,
    input: &str,
    tempdir: &Option<String>,
    shell: Shell,
) -> io::Result<Vec<String>> {
    #[cfg(unix)]
    {
        use nix::sys::resource;
        use nix::sys::resource::Resource;

        let (_, hard_limit) = resource::getrlimit(Resource::RLIMIT_NOFILE)?;
        if hard_limit >= 100_000 {
            // `ptyprocess`, which we depend on, does a fairly clowny thing of attempting to close
            // *all* file descriptors. When there's too many of them, that takes forever, so limit
            // the number to something more sensible
            resource::setrlimit(Resource::RLIMIT_NOFILE, 100_000, 100_000)?;
        }
    }

    let real_tempdir;
    let tempdir = match tempdir {
        Some(tempdir) => tempdir.as_ref(),
        None => {
            real_tempdir = tempfile::tempdir()?;
            real_tempdir.path()
        }
    };

    match shell {
        Shell::Bash => run_bash(completion_name, script, input, &tempdir),
        Shell::Fish => run_fish(completion_name, script, input, &tempdir),
        Shell::Zsh => run_zsh(completion_name, script, input, &tempdir),
    }
}

/// Helper binary used to test CLI completions.
///
/// Other than the args, it accepts a single line of input containing a partial command invocation
/// to be completed and outputs the possible completions, newline delimited.
///
/// PTY-based completion checking for Bash and Zsh is fundamentally racey. To help guard against
/// this, the test environment sets `COMPLETION_VERIFY_LOCKFILE` to a path when invoking the
/// completion script. A backing completion impl can create the file while it is still executing.
#[derive(Debug, clap::Parser)]
#[clap(name = "completion-verify")]
struct CompletionVerify {
    /// The path to a directory to use as a tempdir
    ///
    /// Must be empty prior to each invocation of this binary
    #[clap(long, value_name = "DIR")]
    tempdir: Option<String>,
    /// The command we complete
    #[clap(long, value_name = "COMMAND", default_value = "buck2")]
    name: String,
    /// The shell to test with
    shell: Shell,
    /// The path of the completion script to load
    script: String,
}

fn main() -> io::Result<()> {
    let args = CompletionVerify::parse();

    let script = std::fs::read_to_string(&args.script)?;
    let input = std::io::read_to_string(io::stdin())?;

    for option in run(&args.name, &script, &input, &args.tempdir, args.shell)? {
        println!("{option}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::Shell;
    use crate::extract_from_single_output;
    use crate::run;

    const BASH_SCRIPT: &str = "complete -W 'car1 cat2' find";

    // Note: fish requires the command to actually exist
    const FISH_SCRIPT: &str = "complete -c find -a 'car1 cat2'";

    const ZSH_SCRIPT: &str = "\
#compdef find
_impl()
{
    compadd car1 cat2
}
compdef _impl find
";

    fn test_complete(input: &str, expected: &[&'static str]) {
        check_shell_available(Shell::Bash);
        let actual = run(
            "find",
            BASH_SCRIPT,
            &format!("find {input}"),
            &None,
            Shell::Bash,
        )
        .unwrap();
        assert_eq!(actual, expected, "testing bash");

        if cfg!(target_os = "linux") {
            check_shell_available(Shell::Fish);
            let actual = run(
                "find",
                FISH_SCRIPT,
                &format!("find {input}"),
                &None,
                Shell::Fish,
            )
            .unwrap();
            assert_eq!(actual, expected, "testing fish");
        }

        check_shell_available(Shell::Zsh);
        let actual = run(
            "find",
            ZSH_SCRIPT,
            &format!("find {input}"),
            &None,
            Shell::Zsh,
        )
        .unwrap();
        assert_eq!(actual, expected, "testing zsh");
    }

    fn check_shell_available(shell: Shell) {
        #[allow(clippy::expect_fun_call)]
        let output = shell
            .find()
            .unwrap()
            .arg("--version")
            .output()
            .expect(format!("Failed to run {shell:?}").as_str());
        assert!(
            output.status.success(),
            "checking that `{shell:?}` is available",
        );
    }

    #[test]
    fn test_extract_from_single_output_with_redrawn_prompt() {
        let input = "buck2 abcdefghijkl";
        let output = "\
% buck2 abcdefghijkl
% buck2 abcdefghijkl
% buck2 abcdefghijkl0
abcdefghijkl0  abcdefghijkl1
";

        assert_eq!(
            extract_from_single_output(input, output),
            Some(vec!["abcdefghijkl0".to_owned(), "abcdefghijkl1".to_owned(),])
        );
    }

    #[test]
    fn test_zero() {
        test_complete("camp", &[]);
    }

    #[test]
    fn test_one() {
        test_complete("car", &["car1"]);
        test_complete("car1", &["car1"]);
    }

    #[test]
    fn test_two() {
        test_complete("ca", &["car1", "cat2"]);
        test_complete("c", &["car1", "cat2"]);
    }

    #[test]
    fn test_long_completion() {
        let arg1 = "abcdefghijkl0";
        let arg2 = "abcdefghijkl1";
        let script: &str = &format!("complete -c buck2 -a '{arg1} {arg2}'");

        if cfg!(target_os = "linux") {
            check_shell_available(Shell::Fish);
            let actual = run("buck2", script, "buck2 abcdefghijkl", &None, Shell::Fish).unwrap();
            assert_eq!(
                actual,
                vec![arg1.to_owned(), arg2.to_owned()],
                "testing fish"
            );
        }
    }
}
