#compdef buck2 buck
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

compdef -d buck2

_BUCK_COMPLETE_BIN="${_BUCK_COMPLETE_BIN:-buck2}"

__buck2_takes_target()
{
    case "$1" in
    build|ctargets|install|run|targets|test|utargets)
        return 0
        ;;
    *)
        return 1
        ;;
    esac
}

__buck2_subcommand()
{
    local subcommand=
    for w in "${words[@]:1:$CURRENT - 1}"; do
        case "$w" in
        -*|@*)
            ;;
        *)
            if [[ -z $subcommand ]]; then
                subcommand="$w"
            fi
            ;;
        esac
    done
    if [[ -n $subcommand ]]; then
        echo "$subcommand"
    fi
}

__buck2_add_target_completions()
{
    local completions=()
    while read -r; do
        completions+="$REPLY"
    done < <("${_BUCK_COMPLETE_BIN[@]}" complete --target="$1" 2>/dev/null)

    # Note: `-J targets` is needed as otherwise `-o nosort` is silently ignored
    compadd -S '' -J targets -o nosort -- "${completions[@]}"
}

__buck2_add_flagfile_completions()
{
    local completions=()
    while read -r; do
        completions+="$REPLY"
    done < <("${_BUCK_COMPLETE_BIN[@]}" complete --flagfile="$1" 2>/dev/null)

    compadd -S '' -J flagfiles -o nosort -- "${completions[@]}"
}

__buck2_completions_queued()
{
    if [[ ${compstate[nmatches]} -eq 0 ]]; then
        return 255
    else
        return 0
    fi
}

__buck2_fix()
{
    for w in "${words[@]:1:$CURRENT - 1}"; do
        if [[ "$w" = '--' ]]; then
            # We're running completions after a `--` - just report file completions and otherwise
            # exit out
            _files
            return
        fi
    done

    local cur="${words[CURRENT]}"
    local prev="${words[CURRENT-1]}"
    local pprev="${words[CURRENT-2]}"

    # Flagfile / mode-file completion: an `@file` argument or the value of
    # `--flagfile`/`--config-file`.
    if [[ $cur == @* || $prev == --flagfile || $prev == --config-file ]]; then
        __buck2_add_flagfile_completions "$cur"
        compstate[insert]="automenu-unambiguous"
        return
    fi

    # Zsh treats `:` as a separate word, so we have to do some work to
    # recover a partial target name
    if [[ $cur = : ]]; then
        if [[ "${BUFFER:0:$CURRENT}" =~ .*$prev: ]]; then
            cur="$prev:"
        fi
    elif [[ $prev = : ]]; then
        if [[ "${BUFFER:0:$CURRENT}" =~ .*$prev: ]]; then
            cur="$pprev:$cur"
        else
            cur=":$cur"
        fi
    fi

    if __buck2_takes_target "$(__buck2_subcommand)"; then
        if [[ $cur =~ ^- ]]; then
            _buck2 "$@"
        else
            _buck2 "$@"
            if ! __buck2_completions_queued; then
                __buck2_add_target_completions "$cur"
            fi
        fi
    else
        _buck2 "$@"
    fi

    compstate[insert]="automenu-unambiguous"
}

compdef __buck2_fix buck buck2
