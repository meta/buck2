#!/usr/bin/env bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# usage: buck2_test.sh <targets file> [<buck2 test argument>...]
#
# Runs `buck2 test` on the target patterns in the file, one per line, with the
# buck2 at $RUNNER_TEMP/artifacts. Test targets that fail are run a second
# time, on their own: a test fails the job when it fails both times, and is
# reported as flaky when it passes the second time.
#
# What failed goes on the run's summary page and in an annotation: reading a
# job's log takes a GitHub login, those two do not.

set -uo pipefail

targets=$1
shift

# Runs `buck2 test` on the patterns in the file $1, with its output in $2 too.
run() {
  "$RUNNER_TEMP/artifacts/buck2" --client-metadata id=github_buck2_test test "@$1" "${@:3}" 2>&1 | tee "$2"
  return "${PIPESTATUS[0]}"
}

# What failed in the run with the log $1. buck2 heads a test target's output
# with `<status>: <target> (<time>)`, a failed case in that output is libtest's
# `test <case> ... FAILED`, and the output ends with the counts and the targets
# that did not pass. Without a failed case to name, the end of the log has to
# do: it holds the error of a build failure and those closing lines.
report() {
  awk '
    /^(\[[^]]*\] )?[^ ]+ (Fail|Timeout|Fatal): .* \([0-9:.]+s\)$/ {
      target = $0
      sub(/^.*(Fail|Timeout|Fatal): /, "", target)
      sub(/ \([0-9:.]+s\)$/, "", target)
    }
    / test .* \.\.\. FAILED$/ {
      name = $0
      sub(/^.* test /, "", name)
      sub(/ \.\.\. FAILED$/, "", name)
      cases = cases target " - " name "\n"
    }
    /^Tests finished: / { finished = 1 }
    finished { summary = summary $0 "\n" }
    { last[NR % 40] = $0 }
    END {
      if (cases != "") {
        printf "%s%s", summary, cases
      } else {
        for (i = NR - 39; i <= NR; i++) if (i > 0) print last[i % 40]
      }
    }
  ' "$1"
}

# The test targets to run again after the run with the log $1: the ones whose
# output buck2 headed as failed, timed out or fatal. Nothing unless they are
# all that went wrong, by the counts buck2 ends with: a target that did not
# build fails the same way again, and a second run must not hide any other
# kind of failure.
failed_targets() {
  awk '
    function count(line, key,    at) {
      at = index(line, key)
      return at ? substr(line, at + length(key)) + 0 : 0
    }
    /^(\[[^]]*\] )?[^ ]+ (Fail|Timeout|Fatal): .* \([0-9:.]+s\)$/ {
      target = $0
      sub(/^.*(Fail|Timeout|Fatal): /, "", target)
      sub(/ \([0-9:.]+s\)$/, "", target)
      if (!(target in seen)) {
        seen[target] = 1
        targets++
        failed = failed target "\n"
      }
    }
    /^Tests finished: / {
      tests = count($0, ". Fail ") + count($0, ". Timeout ") + count($0, ". Fatal ")
      other = count($0, "Infra Failure ") + count($0, "Build failure ") + index($0, "Listing")
      finished = 1
    }
    END { if (finished && other == 0 && targets > 0 && tests == targets) printf "%s", failed }
  ' "$1"
}

# Puts the text $3 on the run's summary page under the heading $2 and in an
# annotation of the level $1.
publish() {
  {
    echo "### $2"
    echo '```'
    echo "$3"
    echo '```'
  } >> "$GITHUB_STEP_SUMMARY"
  # An annotation's message is one line, with its newlines escaped.
  local message=${3:0:8000}
  message=${message//'%'/'%25'}
  message=${message//$'\r'/'%0D'}
  message=${message//$'\n'/'%0A'}
  echo "::$1 title=$2::$message"
}

first="$RUNNER_TEMP/buck2-test.log"
run "$targets" "$first" "$@"
status=$?
if [[ $status -eq 0 ]]; then
  exit 0
fi

again="$RUNNER_TEMP/buck2-test-again.txt"
failed_targets "$first" > "$again"
if [[ ! -s $again ]]; then
  publish error "buck2 test failed" "$(report "$first")"
  exit "$status"
fi

echo "Running the test targets that failed a second time:"
cat "$again"
second="$RUNNER_TEMP/buck2-test-again.log"
run "$again" "$second" "$@"
status=$?
if [[ $status -eq 0 ]]; then
  publish warning "Flaky tests: failed, then passed when run again" "$(report "$first")"
  exit 0
fi

publish error "buck2 test failed twice" "$(report "$second")"
exit "$status"
