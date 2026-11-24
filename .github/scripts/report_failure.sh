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

# Without a file to keep the output in there is no report; the command still runs.
log=$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/report-failure.XXXXXX") || exec "$@"
"$@" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
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

publish error "$what failed" "$(why "$log")"
exit "$status"
