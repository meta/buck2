# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

using namespace System.Management.Automation
using namespace System.Management.Automation.Language

# %INSERT_GENERATED_LINE%

# The generated block below is clap's static (flag/subcommand) completer. Its
# `Register-ArgumentCompleter` call is rewritten at generation time into an
# assignment to `$BuckClapStaticCompleter` so that we own the single native
# completer registration (PowerShell allows only one per command) and can layer
# dynamic target completion on top of clap's static completions.

# clap_complete generated content BEGINS
# %INSERT_OPTION_COMPLETION%
# clap_complete generated content ENDS

$BuckCompleter = {
    param($wordToComplete, $commandAst, $cursorPosition)

    $completeBin = if ($env:_BUCK_COMPLETE_BIN) { $env:_BUCK_COMPLETE_BIN } else { 'buck2' }
    $targetSubcommands = @('build', 'ctargets', 'install', 'run', 'targets', 'test', 'utargets')

    $elements = $commandAst.CommandElements

    # The subcommand is the first bareword after argv[0] that is neither a flag nor an
    # `@flagfile`, stopping at a `--` separator.
    $subcommand = $null
    for ($i = 1; $i -lt $elements.Count; $i++) {
        $value = $elements[$i].Extent.Text
        if ($value -eq '--') { break }
        if ($value.StartsWith('-') -or $value.StartsWith('@')) { continue }
        $subcommand = $value
        break
    }

    # The token immediately before the one being completed (used to detect `--flagfile`
    # <value> and `--config-file` <value>).
    $prev = ''
    if ($elements.Count -ge 1) {
        $last = $elements[$elements.Count - 1].Extent.Text
        if (($last -eq $wordToComplete) -and ($elements.Count -ge 2)) {
            $prev = $elements[$elements.Count - 2].Extent.Text
        } elseif ($last -ne $wordToComplete) {
            $prev = $last
        }
    }

    # Flagfile / mode-file completion: an `@file` argument or the value of
    # `--flagfile`/`--config-file`.
    if ($wordToComplete.StartsWith('@') -or $prev -eq '--flagfile' -or $prev -eq '--config-file') {
        $flagfiles = @(& $completeBin complete --flagfile="$wordToComplete" 2>$null)
        return $flagfiles | ForEach-Object {
            [CompletionResult]::new($_, $_, [CompletionResultType]::ParameterValue, $_)
        }
    }

    # Dynamic target completion for target-taking subcommands. Skipped when the current
    # word is a flag. Unlike bash, PowerShell keeps the whole `//pkg:target` token
    # together, so the completion delegate's output is used verbatim.
    if (($subcommand -in $targetSubcommands) -and (-not $wordToComplete.StartsWith('-'))) {
        $targets = @(& $completeBin complete --target="$wordToComplete" 2>$null)
        if ($targets.Count -gt 0) {
            return $targets | ForEach-Object {
                [CompletionResult]::new($_, $_, [CompletionResultType]::ParameterValue, $_)
            }
        }
    }

    # Fall back to clap's static flag/subcommand completions.
    if ($BuckClapStaticCompleter) {
        return (& $BuckClapStaticCompleter $wordToComplete $commandAst $cursorPosition)
    }
}.GetNewClosure()

Register-ArgumentCompleter -Native -CommandName 'buck', 'buck2' -ScriptBlock $BuckCompleter
