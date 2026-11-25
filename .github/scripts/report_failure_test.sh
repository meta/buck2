#!/usr/bin/env bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# usage: report_failure_test.sh
#
# Runs report_failure.sh with stand-in commands and checks what it does with
# them. What is wrong goes on the run's summary page and in annotations.

set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=/dev/null
source "$here/annotations.sh"

tmp=
rc=
for sig in 1 2 3 13 15; do eval "trap 'exit $((sig + 128))' $sig"; done
trap 'rc=$?; set +e; rm -rf "$tmp"; exit $rc' EXIT
tmp=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/report-failure-test.XXXXXX") || exit 1

wrong=""

# Notes that the case $1 is wrong: $2 says how.
fail() {
  wrong+="$1: $2"$'\n'
}

# Runs report_failure.sh with the command $2... as the case $1. Its output is
# then in $out, its report in $report, its exit status in $status, and the
# seconds it took in $took.
run() {
  local name=$1
  shift
  out=$tmp/$name.out
  report=$tmp/$name.report
  mkdir "$tmp/$name"
  local start=$SECONDS
  RUNNER_TEMP=$tmp/$name GITHUB_STEP_SUMMARY=$report \
    bash "$here/report_failure.sh" "$name" "$@" > "$out" 2>&1
  status=$?
  took=$((SECONDS - start))
  local left
  left=$(ls -A "$tmp/$name")
  if [[ -n $left ]]; then
    fail "$name" "left $left in RUNNER_TEMP"
  fi
}

run passes bash -c 'echo to stdout; echo to stderr >&2'
if [[ $status -ne 0 ]]; then
  fail passes "exit status $status, not 0"
fi
if ! grep -qx 'to stdout' "$out" || ! grep -qx 'to stderr' "$out"; then
  fail passes "the command's output is not passed on"
fi
if [[ -s $report ]]; then
  fail passes "a report for a command that passed"
fi

run fails bash -c 'echo "error: boom"; exit 3'
if [[ $status -ne 3 ]]; then
  fail fails "exit status $status, not 3"
fi
if ! grep -qx 'error: boom' "$report"; then
  fail fails "the report lacks the error"
fi
if ! grep -q '^::error title=fails failed' "$out"; then
  fail fails "no annotation"
fi

run no-newline bash -c 'printf "error: cut"; exit 1'
if ! grep -q '^::error ' "$out"; then
  fail no-newline "the annotation is not at the start of a line"
fi

# Not in Git Bash, where report_failure.sh keeps `tee` in a pipeline with the
# command and so waits for what the command left behind.
case $OSTYPE in
  msys* | cygwin*) ;;
  *)
    # shellcheck disable=SC2016
    run leftover bash -c 'sleep 120 & echo $! > "$0"; echo "error: left"; exit 4' "$tmp/leftover.pid"
    kill "$(cat "$tmp/leftover.pid")" 2> /dev/null
    if [[ $status -ne 4 ]]; then
      fail leftover "exit status $status, not 4"
    fi
    if [[ $took -ge 60 ]]; then
      fail leftover "waited $took seconds for the process that the command left behind"
    fi
    if ! grep -qx 'error: left' "$report"; then
      fail leftover "the report lacks the error"
    fi
    ;;
esac

if [[ -n $wrong ]]; then
  wrong+=$'\n'"The start of each case's output:"$'\n'"$(head -n 20 "$tmp"/*.out)"
  echo "$wrong"
  if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then
    publish error "report_failure.sh is wrong" "$wrong"
  fi
  exit 1
fi
echo "report_failure.sh does what it should."
