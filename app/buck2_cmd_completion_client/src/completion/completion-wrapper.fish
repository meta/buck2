# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# %INSERT_GENERATED_LINE%

# clap_complete generated content BEGINS
# %INSERT_OPTION_COMPLETION%
# clap_complete generated content ENDS

function __buck2_subcommand
    for w in $argv[2..]
        switch $w
        case --
            return 1
        case '-*'
            continue
        case '*'
            if test -n $w
                echo $w
                return
            end
        end
    end
    return 1
end

function __buck2_takes_target
    set -l cmd (commandline --current-process --tokenize --cut-at-cursor)
    if contains -- -- $cmd[..-1]
        return 1
    end
    set -l subcommand (__buck2_subcommand $cmd)
    test -n $subcommand || return

    contains $subcommand build ctargets install run targets test utargets
    return $status
end

function __buck2_add_target_completions
    set -l cur (commandline --current-token)

    string match --quiet -- '-*' $cur && return

    buck2 complete --target="$cur" 2>/dev/null
end

function __buck2_needs_flagfile
    set -l cur (commandline --current-token)
    string match --quiet -- '@*' $cur && return 0

    set -l tokens (commandline --current-process --tokenize --cut-at-cursor)
    set -l n (count $tokens)
    test $n -ge 1; and contains -- $tokens[$n] --flagfile --config-file; and return 0
    test $n -ge 2; and contains -- $tokens[(math $n - 1)] --flagfile --config-file; and return 0
    return 1
end

function __buck2_add_flagfile_completions
    set -l cur (commandline --current-token)
    buck2 complete --flagfile="$cur" 2>/dev/null
end

complete -c buck2 -n '__buck2_needs_flagfile' -f -a '(__buck2_add_flagfile_completions)'
complete -c buck2 -n '__buck2_takes_target' -f -a '(__buck2_add_target_completions)'
complete -c buck -w buck2
