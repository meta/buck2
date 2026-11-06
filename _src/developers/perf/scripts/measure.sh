#!/bin/bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Warm the local daemon, kill it for a clean retained-memory baseline,
# run a measurement build, and capture VmHWM, allocator-stats, and a
# heap profile.
#
# Usage:
#   measure.sh <bin> <tag> -- <buck2 args...>
#
# Example:
#   measure.sh /tmp/b2a a -- build //some/target -v0 --console=none
#
# Run this in a benchmark-only worktree — it kills and restarts the
# local daemon, which would interrupt any real work in the same
# checkout. See docs/developers/perf/basics.md.
#
# Outputs (in /tmp by default; override with OUT_DIR):
#   <OUT_DIR>/<tag>_warmup.txt       client stderr from the warmup build
#   <OUT_DIR>/<tag>_measure.txt      client stderr from the measurement build (with /usr/bin/time -v)
#   <OUT_DIR>/<tag>_rss.txt          /proc/<pid>/status excerpt for the daemon
#   <OUT_DIR>/<tag>_stats.json       buck2 debug allocator-stats
#   <OUT_DIR>/<tag>_heap.prof        buck2 debug heap-dump

set -u
OUT_DIR=${OUT_DIR:-/tmp}

if [ "$#" -lt 3 ] || [ "$3" != "--" ]; then
  echo "usage: $0 <bin> <tag> -- <buck2 args...>" >&2
  exit 2
fi
BIN=$1
TAG=$2
shift 3
ARGS=("$@")

step() { echo "[$(date +%T)] $*"; }

step "killing local daemon (if any)"
"$BIN" kill 2>&1 | tail -1 || true

step "warmup build (populates caches)"
MALLOC_CONF=prof:true,prof_final:false,prof_active:true \
  "$BIN" "${ARGS[@]}" 1>/dev/null 2>"$OUT_DIR/${TAG}_warmup.txt"
echo "warmup exit $?"

step "killing daemon for fresh DICE"
"$BIN" kill 2>&1 | tail -1 || true

step "measurement build"
MALLOC_CONF=prof:true,prof_final:false,prof_active:true \
  /usr/bin/time -v "$BIN" "${ARGS[@]}" 1>/dev/null 2>"$OUT_DIR/${TAG}_measure.txt"
echo "measure exit $?"

PID=$("$BIN" status 2>/dev/null \
        | python3 -c "import sys,json; print(json.load(sys.stdin)['process_info']['pid'])")
step "daemon pid: $PID"

step "/proc/<pid>/status (daemon, peak/current RSS)"
grep -E '^(VmPeak|VmSize|VmHWM|VmRSS|RssAnon|RssFile|VmData):' \
  "/proc/$PID/status" | tee "$OUT_DIR/${TAG}_rss.txt"

step "/usr/bin/time output (CLIENT only — see docs)"
grep -E 'Maximum resident|Elapsed|page faults|context switches' "$OUT_DIR/${TAG}_measure.txt" | sed 's/^/  /'

step "heap-dump + allocator-stats"
"$BIN" debug heap-dump --path "$OUT_DIR/${TAG}_heap.prof" 2>/dev/null
"$BIN" debug allocator-stats > "$OUT_DIR/${TAG}_stats.json" 2>/dev/null

step "outputs:"
ls -la "$OUT_DIR/${TAG}_heap.prof" "$OUT_DIR/${TAG}_stats.json" "$OUT_DIR/${TAG}_rss.txt" 2>/dev/null
