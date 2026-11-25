#!/usr/bin/env bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# usage: report_failure.sh <what> <command> [<argument>...]
#
# Runs the command. If it fails, why goes on the run's summary page and in
# annotations, under `<what> failed`.

set -uo pipefail

# shellcheck source=/dev/null
source "$(dirname "${BASH_SOURCE[0]}")/annotations.sh"

what=$1
shift

dir=
rc=
for sig in 1 2 3 13 15; do eval "trap 'exit $((sig + 128))' $sig"; done
trap 'rc=$?; set +e; rm -rf "$dir"; exit $rc' EXIT

# Without a place to keep the output in there is no report; the command still runs.
dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/report-failure.XXXXXX") || exec "$@"
log=$dir/log
: > "$log"

# Whether `tee` has ended, after at most $1 seconds.
tee_ended() {
  local deadline=$((SECONDS + $1))
  while [[ ! -e $dir/done && $SECONDS -lt $deadline ]]; do
    sleep 0.05
  done
  [[ -e $dir/done ]]
}

# Runs the command with `tee` beside it, not in a pipeline with it. A pipeline
# ends only when `tee` does, and `tee` reads for as long as anything holds the
# command's output open: also a process that the command left behind.
run_beside_tee() {
  exec 3> >(
    tee "$log" <&0 &
    echo $! > "$dir/tee"
    wait 2> /dev/null
    { : > "$dir/done"; } 2> /dev/null
  ) || {
    rm -rf "$dir"
    exec "$@"
  }
  "$@" >&3 2>&1 3>&-
  status=$?
  exec 3>&-

  # `tee` ends once nothing holds the output open. What the command left behind
  # may hold it for hours, so `tee` gets 5 seconds to copy what is still on its
  # way and is stopped then.
  if ! tee_ended 5 && [[ -s $dir/tee ]] && read -r tee_pid < "$dir/tee"; then
    kill "$tee_pid" 2> /dev/null
    tee_ended 2
  fi
}

# Git Bash keeps the pipeline: `tee` beside the command has not run there.
case $OSTYPE in
  msys* | cygwin*)
    "$@" 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}
    ;;
  *)
    run_beside_tee "$@"
    ;;
esac

if [[ $status -eq 0 ]]; then
  exit 0
fi

# Why the command with the output $1 failed: the group of the log it was in,
# the first action that buck2 says failed or the first error of cargo or rustc,
# with the start of what follows it, and the end of the output.
why() {
  awk '
    BEGIN { color = sprintf("%c", 27) "\\[[0-9;]*m" }
    {
      gsub(color, "")
      sub(/\r$/, "")
      line[NR] = substr($0, 1, 300)
    }
    /^::group::/ { group = substr($0, 10) }
    /^::endgroup::/ { group = "" }
    first == 0 && (/Action failed: / || /^error(\[[A-Z0-9]+\])?: /) { first = NR }
    END {
      if (group != "") print "In: " group
      start = NR - 39
      if (start < 1) start = 1
      if (first > 0 && first + 25 < start) {
        for (i = first; i < first + 25; i++) print line[i]
        print "..."
      } else if (first > 0 && first < start) {
        start = first
      }
      for (i = start; i <= NR; i++) print line[i]
    }
  ' "$1"
}

# An annotation is a command only at the start of a line.
if [[ -s $log && $(tail -c 1 "$log" | wc -l) -eq 0 ]]; then
  echo
fi
publish error "$what failed" "$(why "$log")"
exit "$status"
