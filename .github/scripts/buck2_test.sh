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
# What failed, and the start of each failed test's output, goes on the run's
# summary page and in annotations: reading a job's log takes a GitHub login,
# those two do not.

set -uo pipefail

targets=$1
shift

# Runs `buck2 test` on the patterns in the file $1, with its output in $2 too.
run() {
  "$RUNNER_TEMP/artifacts/buck2" --client-metadata id=github_buck2_test test "@$1" "${@:3}" 2>&1 | tee "$2"
  return "${PIPESTATUS[0]}"
}

# What failed in the run with the log $1: buck2's closing counts, the test
# targets that failed and, for each of their failed cases, the start of what
# the case printed. buck2 heads a test target's output with
# `<status>: <target> (<time>)`; in that output libtest marks a failed case
# with `test <case> ... FAILED` and prints what it wrote under
# `---- <case> stdout ----`. Of that, six lines from the last `Error:` or panic
# on are kept, since a case may print a lot before it fails. Without a failed
# case to name, the end of the log has to do: it holds the error of a build
# failure and buck2's closing lines.
report() {
  awk '
    { last[NR % 40] = $0 }
    /^Tests finished: / { counts = $0 }
    {
      line = $0
      sub(/^\[[^]]*\] /, "", line)
    }
    line ~ /^[^ ]+ (Fail|Timeout|Fatal): .* \([0-9:.]+s\)$/ {
      target = line
      sub(/^[^ ]+ (Fail|Timeout|Fatal): /, "", target)
      sub(/ \([0-9:.]+s\)$/, "", target)
      sub(/^[^\/]*\/\//, "", target)
      if (!(target in seen)) {
        seen[target] = 1
        order[++targets] = target
      }
      block = ""
      next
    }
    line ~ /^test .* \.\.\. FAILED$/ {
      name = line
      sub(/^test /, "", name)
      sub(/ \.\.\. FAILED$/, "", name)
      cases[target] = cases[target] name "\n"
      failed++
      next
    }
    line ~ /^---- .* stdout ----$/ {
      block = line
      sub(/^---- /, "", block)
      sub(/ stdout ----$/, "", block)
      kept = 0
      next
    }
    line ~ /^(failures:|stack backtrace:)$/ { block = "" }
    block != "" {
      sub(/^[ \t]+/, "", line)
      sub(/^\[[^]]*\] /, "", line)
      if (line ~ /^Error: / || line ~ / panicked at /) {
        why[target, block] = ""
        kept = 0
      }
      sub(/^thread .* panicked at /, "panicked at ", line)
      sub(/^<cmd> [^ ]*\/buck2 /, "buck2 ", line)
      if (kept < 6 && line != "" && line !~ /^<(working_dir|stderr|stdout)>/ && line !~ /^(Build ID:|Buck UI:|Starting new buck2 daemon|Connected to new buck2 daemon|note: run with )/) {
        why[target, block] = why[target, block] (kept++ ? " | " : "") substr(line, 1, 200)
      }
    }
    END {
      if (failed == 0) {
        for (i = NR - 39; i <= NR; i++) if (i > 0) print last[i % 40]
        exit
      }
      print counts
      print "Failed test targets (" targets "):"
      for (i = 1; i <= targets; i++) print order[i]
      for (i = 1; i <= targets; i++) {
        print ""
        print order[i]
        n = split(cases[order[i]], names, "\n")
        for (j = 1; j < n; j++) print "  " names[j] ": " why[order[i], names[j]]
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

# Puts the text $2 on the run's summary page under the heading $1.
summarize() {
  {
    echo "### $1"
    echo '```'
    echo "$2"
    echo '```'
  } >> "$GITHUB_STEP_SUMMARY"
}

# How many annotations of each level this step has made: GitHub keeps ten of
# a level for a step.
levels=(error warning notice)
made=(0 0 0)

# Puts the text $3 in annotations of the level $1, titled $2. GitHub keeps
# about 4000 characters of an annotation, so the text goes out by whole lines
# over as many annotations as it needs, and over the levels below $1 once that
# one is used up.
annotate() {
  local chunks=() chunk="" line
  while IFS= read -r line; do
    line=${line:0:1000}
    if [[ -n $chunk && $((${#chunk} + ${#line})) -ge 3500 ]]; then
      chunks+=("$chunk")
      chunk=""
    fi
    chunk+="$line"$'\n'
  done <<< "$3"
  chunks+=("$chunk")
  local level=0
  [[ $1 == error ]] || level=1
  local i title message
  for i in "${!chunks[@]}"; do
    while [[ $level -lt 3 && ${made[$level]} -ge 10 ]]; do
      level=$((level + 1))
    done
    [[ $level -lt 3 ]] || break
    made[level]=$((made[level] + 1))
    title="$2 ($((i + 1)) of ${#chunks[@]})"
    # An annotation is one line: its newlines are escaped, and so are the
    # colons and commas of its title.
    title=${title//'%'/'%25'}
    title=${title//':'/'%3A'}
    title=${title//','/'%2C'}
    message=${chunks[$i]%$'\n'}
    message=${message//'%'/'%25'}
    message=${message//$'\r'/'%0D'}
    message=${message//$'\n'/'%0A'}
    echo "::${levels[$level]} title=$title::$message"
  done
}

# Puts the text $3 on the run's summary page and in annotations of the level
# $1, both under the heading $2.
publish() {
  summarize "$2" "$3"
  annotate "$1" "$2" "$3"
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

failed=$(report "$second")
summarize "buck2 test failed twice" "$failed"
# The flaky tests' annotation goes out first: a long report of what failed
# twice would otherwise use up the warnings.
failed_twice=$(failed_targets "$second")
if [[ -n $failed_twice ]]; then
  flaky=$(grep -vxF -f <(echo "$failed_twice") "$again")
  if [[ -n $flaky ]]; then
    publish warning "Flaky tests: failed, then passed when run again" "$flaky"
  fi
fi
annotate error "buck2 test failed twice" "$failed"
exit "$status"
