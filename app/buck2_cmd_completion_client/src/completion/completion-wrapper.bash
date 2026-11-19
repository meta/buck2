# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# %INSERT_GENERATED_LINE%

# clap_complete generated content BEGINS
# %INSERT_OPTION_COMPLETION%
# clap_complete generated content ENDS

complete -r buck2

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
    for w in "${COMP_WORDS[@]:1:$COMP_CWORD - 1}"; do
        case "$w" in
        --)
            # This marker should only occur after certain subcommands
            exit 1
            ;;
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
        if [[ $REPLY =~ [:]. ]]; then
            completions+=("${REPLY#*:}")
        else
            completions+=("$REPLY")
        fi
    done < <("${_BUCK_COMPLETE_BIN[@]}" complete --target="$1" 2>/dev/null)
    COMPREPLY=("${completions[@]}")
}

__buck2_add_flagfile_completions()
{
    local completions=()
    while read -r; do
        completions+=("$REPLY")
    done < <("${_BUCK_COMPLETE_BIN[@]}" complete --flagfile="$1" 2>/dev/null)
    COMPREPLY=("${completions[@]}")
}

__buck2_completions_queued()
{
    if [[ ${#COMPREPLY[@]} -eq 0 ]]; then
        return 255
    elif [[ ${#COMPREPLY[@]} -eq 1 && ${COMPREPLY[1]} = % ]]; then
        return 255
    else
        return 0
    fi
}

__buck2_fix()
{
    local cur="${COMP_WORDS[COMP_CWORD]}"
    local prev="${COMP_WORDS[COMP_CWORD-1]}"
    local pprev="${COMP_WORDS[COMP_CWORD-2]}"

    # Flagfile / mode-file completion: an `@file` argument or the value of
    # `--flagfile`/`--config-file`.
    if [[ $cur == @* || $prev == --flagfile || $prev == --config-file ]]; then
        __buck2_add_flagfile_completions "$cur"
        return
    fi

    # Bash treats `:` as a separate word, so we have to do some work to
    # recover a partial target name
    if [[ $cur = : ]]; then
        if [[ "${COMP_LINE:0:$COMP_POINT}" =~ .*$prev: ]]; then
            cur="$prev:"
        fi
    elif [[ $prev = : ]]; then
        if [[ "${COMP_LINE:0:$COMP_POINT}" =~ .*$pprev:$cur ]]; then
            cur="$pprev:$cur"
        else
            cur=":$cur"
        fi
    fi

    if __buck2_takes_target "$(__buck2_subcommand)"; then
        if [[ $cur =~ ^- ]]; then
            _buck2 "$@"
        else
            # The auto-generated completions have what is arguably a bug resulting where they don't
            # correctly fix up `$cur` in the way we do above to deal with colons. As a result, skip
            # flag completions if there's a colon in the current word - that wasn't going to be
            # useful anyway.
            if [[ ! $cur == *:* ]]; then
                _buck2 "$@"
            fi
            if ! __buck2_completions_queued; then
                __buck2_add_target_completions "$cur"
            fi
        fi
    else
        _buck2 "$@"
    fi
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F __buck2_fix -o nosort -o bashdefault -o default -o nospace buck
    complete -F __buck2_fix -o nosort -o bashdefault -o default -o nospace buck2
else
    complete -F __buck2_fix -o bashdefault -o default -o nospace buck
    complete -F __buck2_fix -o bashdefault -o default -o nospace buck2
fi
