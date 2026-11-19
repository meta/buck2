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

# shellcheck source=/dev/null
source "$(dirname "${BASH_SOURCE[0]}")/annotations.sh"

targets=$1
shift

# Runs `buck2 test` on the patterns in the file $1, with its output in $2 too.
run() {
  "$RUNNER_TEMP/artifacts/buck2" --client-metadata id=github_buck2_test test "@$1" "${@:3}" 2>&1 | tee "$2"
  return "${PIPESTATUS[0]}"
}

# What failed in the run with the log $1: buck2's closing counts, the test
# targets that failed and, for each of their failed cases, why. buck2 heads a
# test target's output with `<status>: <target> (<time>)`; in that output
# libtest marks a failed case with `test <case> ... FAILED` and prints what it
# wrote under `---- <case> stdout ----`, and either names the case: the start
# of a long output may be missing. Of what it wrote, the lines from the last
# `Error:` or panic on are kept, without a backtrace and what a daemon logs as
# it starts: the first 12 and the last 9 when there are more, since a chain of
# errors names its cause near one end. Cases of a target that failed the same
# way are listed together, above what they printed. A target without a failed
# case died some other way, and gets the end of its output. Without any failed
# target to name, the end of the log has to do: it holds the error of a build
# failure and buck2's closing lines.
report() {
  awk '
    function fail(name) {
      if (!((target, name) in failed)) {
        failed[target, name] = 1
        cases[target] = cases[target] name "\n"
      }
    }
    function keep(text,    n) {
      n = ++count[target, block]
      if (n <= leading) {
        first[target, block, n] = text
      } else {
        rest[target, block, (n - leading) % trailing] = text
      }
    }
    function why(t, c,    n, i, text) {
      n = count[t, c]
      text = ""
      for (i = 1; i <= n; i++) {
        if (n > leading + trailing && i == leading + 1) {
          text = text "    ...\n"
          i = n - trailing + 1
        }
        if (i <= leading) {
          text = text "    " first[t, c, i] "\n"
        } else {
          text = text "    " rest[t, c, (i - leading) % trailing] "\n"
        }
      }
      return text
    }
    BEGIN {
      color = sprintf("%c", 27) "\\[[0-9;]*m"
      leading = 12
      trailing = 9
    }
    { last[NR % 40] = $0 }
    /^Tests finished: / { counts = $0 }
    {
      line = $0
      gsub(color, "", line)
      sub(/^\[[0-9][-0-9T:.+Z]*\] ?/, "", line)
    }
    # A result of the run: of a failed target, whose output follows, or of
    # another, which ends the output of the one before. What a case printed can
    # hold the results of a `buck2 test` that it ran. There, only a line that
    # names a target in the cell of the tests counts as a result of the run.
    line ~ /^[^ ]+ [A-Z][A-Za-z ]*: .* \([0-9:.]+s\)$/ {
      label = line
      sub(/^[^ ]+ [A-Z][A-Za-z ]*: /, "", label)
      sub(/ \([0-9:.]+s\)$/, "", label)
      from = label
      sub(/\/\/.*$/, "", from)
      if (block == "" && cell == "") cell = from
      if (block == "" || from == cell) {
        block = ""
        target = ""
        if (line ~ /^[^ ]+ (Fail|Timeout|Fatal): /) {
          target = label
          sub(/^[^\/]*\/\//, "", target)
          if (!(target in seen)) {
            seen[target] = 1
            order[++targets] = target
          }
          ended[target] = 0
        }
        next
      }
    }
    block == "" && line ~ /^Tests finished: / { target = "" }
    target != "" && line != "" { tail[target, ++ended[target]] = line }
    line ~ /^test .* \.\.\. FAILED$/ {
      name = line
      sub(/^test /, "", name)
      sub(/ \.\.\. FAILED$/, "", name)
      fail(name)
      next
    }
    line ~ /^---- .* stdout ----$/ {
      block = line
      sub(/^---- /, "", block)
      sub(/ stdout ----$/, "", block)
      fail(block)
      count[target, block] = 0
      backtrace = 0
      next
    }
    block != "" {
      if (line == "failures:") {
        block = ""
        next
      }
      sub(/^[ \t]+/, "", line)
      sub(/^\[[0-9][-0-9T:.+Z]*\] ?/, "", line)
      if (line ~ /^Error: / || line ~ / panicked at /) {
        count[target, block] = 0
        backtrace = 0
      }
      if (line == "stack backtrace:") backtrace = 1
      if (backtrace) next
      sub(/^thread .* panicked at /, "panicked at ", line)
      sub(/^<cmd> [^ ]*\/buck2 /, "buck2 ", line)
      if (line == "" || line ~ /^<\/?(working_dir|stderr|stdout)>/) next
      if (line ~ /^(Build ID:|Buck UI:|Starting new buck2 daemon|Connected to new buck2 daemon|note: run with )/) next
      if (line ~ /^(Daemon process info from |daemon dir: |pid: |endpoint: |version: |pid present on system: |Try running `rm -rf )/) next
      if (line ~ /^[0-9-]+T[0-9:.]+Z +(INFO|DEBUG|TRACE) /) next
      keep(substr(line, 1, 200))
      next
    }
    END {
      if (targets == 0) {
        for (i = NR - 39; i <= NR; i++) if (i > 0) print last[i % 40]
        exit
      }
      print counts
      print "Failed test targets (" targets "):"
      for (i = 1; i <= targets; i++) print order[i]
      for (i = 1; i <= targets; i++) {
        t = order[i]
        print ""
        print t
        n = split(cases[t], names, "\n")
        if (n < 2) {
          print "  No failed test case. Its output ends:"
          for (j = ended[t] - 11; j <= ended[t]; j++) if (j > 0) print "  " substr(tail[t, j], 1, 200)
          continue
        }
        for (j = 1; j < n; j++) {
          if (names[j] in listed) continue
          print "  " names[j]
          for (k = j + 1; k < n; k++) {
            if (!(names[k] in listed) && why(t, names[k]) == why(t, names[j])) {
              listed[names[k]] = 1
              print "  " names[k]
            }
          }
          printf "%s", why(t, names[j])
        }
        for (name in listed) delete listed[name]
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
