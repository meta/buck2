/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::exit_result::ExitResult;
use clap::Command;
use clap::ValueEnum;
use clap_complete::generate;

// This file is the entry point for the target-completing delegate for buck2
// command line completions. Its completion commands are called from shell
// scripts which perform the actual completion logic. These shell scripts
// ignore non-zero return values and allow stderr to pass through to the
// user. As such, caution should be taken to ensure error messages are
// understandable in the context of argument completion.

#[derive(ValueEnum, Clone, Debug, Copy)]
#[clap(rename_all = "kebab-case")]
enum Shell {
    Bash,
    Fish,
    Zsh,
    Powershell,
}

#[derive(Debug, clap::Parser)]
#[clap(name = "completion", verbatim_doc_comment)]
/// Print completion configuration for shell
///
/// For a one-time setup, run the one of the following commands appropriate for the shell you're
/// using:
/// - `source <(buck2 completion bash)`
/// - `source <(buck2 completion zsh)`
/// - `source (buck2 completion fish | psub)`
/// - `buck2 completion powershell | Out-String | Invoke-Expression`
pub struct CompletionCommand {
    #[clap(
        value_enum,
        help = "shell for which to generate completion script",
        group = "operation"
    )]
    shell: Shell,

    // FIXME(JakobDegen): Remove after rollout
    #[clap(help = "Only emit completions for option flags", long, hide = true)]
    options_only: bool,
}

impl CompletionCommand {
    pub fn exec(
        self,
        command: Command,
        _matches: BuckArgMatches<'_>,
        _ctx: ClientCommandContext<'_>,
    ) -> ExitResult {
        let mut command = command;
        print_completion_script(self.shell, self.options_only, &mut command)?;
        ExitResult::success()
    }
}

// Use 'static here to avoid rust-analyzer crash when pattern matching
// on these string literals. https://github.com/rust-lang/rust-analyzer/issues/20149
#[allow(clippy::redundant_static_lifetimes)]
const GENERATED_INSERTION_POINT: &'static str = "# %INSERT_GENERATED_LINE%";
#[allow(clippy::redundant_static_lifetimes)]
const GENERATED_TAG: &'static str = concat!("@", "generated");
#[allow(clippy::redundant_static_lifetimes)]
const COMPLETION_INSERTION_POINT: &'static str = "# %INSERT_OPTION_COMPLETION%";

fn completion_wrapper(shell: Shell) -> &'static str {
    #[cfg(buck_build)]
    {
        match shell {
            Shell::Bash => completion_wrapper_bash::get(),
            Shell::Fish => completion_wrapper_fish::get(),
            Shell::Zsh => completion_wrapper_zsh::get(),
            Shell::Powershell => completion_wrapper_powershell::get(),
        }
    }
    #[cfg(not(buck_build))]
    {
        match shell {
            Shell::Bash => include_str!("completion/completion-wrapper.bash"),
            Shell::Fish => include_str!("completion/completion-wrapper.fish"),
            Shell::Zsh => include_str!("completion/completion-wrapper.zsh"),
            Shell::Powershell => include_str!("completion/completion-wrapper.ps1"),
        }
    }
}

fn options_wrapper(shell: Shell) -> &'static str {
    #[cfg(buck_build)]
    {
        match shell {
            Shell::Bash => options_wrapper_bash::get(),
            Shell::Fish => options_wrapper_fish::get(),
            Shell::Zsh => options_wrapper_zsh::get(),
            Shell::Powershell => options_wrapper_powershell::get(),
        }
    }
    #[cfg(not(buck_build))]
    {
        match shell {
            Shell::Bash => include_str!("completion/options-wrapper.bash"),
            Shell::Fish => include_str!("completion/options-wrapper.fish"),
            Shell::Zsh => include_str!("completion/options-wrapper.zsh"),
            Shell::Powershell => include_str!("completion/options-wrapper.ps1"),
        }
    }
}

fn print_completion_script(
    shell_arg: Shell,
    options_only: bool,
    cmd: &mut Command,
) -> buck2_error::Result<()> {
    let wrapper = if options_only {
        options_wrapper(shell_arg)
    } else {
        completion_wrapper(shell_arg)
    };
    let shell = match shell_arg {
        Shell::Bash => clap_complete::Shell::Bash,
        Shell::Zsh => clap_complete::Shell::Zsh,
        Shell::Fish => clap_complete::Shell::Fish,
        Shell::Powershell => clap_complete::Shell::PowerShell,
    };

    let mut wrapper_iter = wrapper.lines();
    let mut found_insertion_point = false;

    for line in wrapper_iter.by_ref() {
        match line {
            GENERATED_INSERTION_POINT => {
                buck2_client_ctx::println!(
                    "# {} by `{}`",
                    GENERATED_TAG,
                    std::env::args().collect::<Vec<String>>().join(" ")
                )?;
            }
            COMPLETION_INSERTION_POINT => {
                found_insertion_point = true;

                buck2_client_ctx::println!("{}", option_completions(shell, cmd)?)?;
            }
            s => {
                buck2_client_ctx::println!("{}", s)?;
            }
        }
    }

    if !found_insertion_point {
        Err(buck2_error::buck2_error!(
            buck2_error::ErrorTag::Tier0,
            "Failed to find {} in {:?} completion template",
            COMPLETION_INSERTION_POINT,
            shell_arg
        ))
    } else {
        Ok(())
    }
}

fn option_completions(
    shell: clap_complete::Shell,
    cmd: &mut Command,
) -> buck2_error::Result<String> {
    let mut v = Vec::new();
    // FIXME: it appears that this might silently swallow errors; would require a PR to fix
    generate(shell, cmd, cmd.get_name().to_owned(), &mut v);
    let generated = String::from_utf8(v)?;

    if shell == clap_complete::Shell::PowerShell {
        // Two PowerShell-specific fixups on clap's output:
        //
        // 1. `using namespace` directives must precede every other statement. The
        //    wrapper template declares them at the top of the file, so strip clap's
        //    copies to avoid a "using must appear first" parse error when they land in
        //    the middle of the spliced script.
        //
        // 2. Rewrite clap's `Register-ArgumentCompleter ... -ScriptBlock { ... }` into
        //    an assignment `$BuckClapStaticCompleter = { ... }`. The wrapper then owns
        //    the single native completer registration and layers dynamic (target /
        //    flagfile) completion on top of clap's static block. Capturing the block by
        //    assignment avoids shadowing the `Register-ArgumentCompleter` cmdlet and
        //    avoids a global variable (both flagged by PSScriptAnalyzer).
        let register_prefix = format!(
            "Register-ArgumentCompleter -Native -CommandName '{}' -ScriptBlock",
            cmd.get_name()
        );
        let mut rewrote_register = false;
        let script: String = generated
            .lines()
            .filter(|line| !line.trim_start().starts_with("using namespace "))
            .map(|line| {
                if line.starts_with(&register_prefix) {
                    rewrote_register = true;
                    line.replacen(&register_prefix, "$BuckClapStaticCompleter =", 1) + "\n"
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        // If clap_complete ever changes how it emits the registration line, the
        // rewrite above no-ops and the wrapper would splice a script that never
        // assigns `$BuckClapStaticCompleter`. Fail loudly instead, mirroring the
        // `found_insertion_point` check in `print_completion_script`.
        if !rewrote_register {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Failed to rewrite clap's PowerShell `Register-ArgumentCompleter` line (expected a line starting with `{}`); clap_complete's output format may have changed",
                register_prefix
            ));
        }
        return Ok(script);
    }

    Ok(generated)
}
