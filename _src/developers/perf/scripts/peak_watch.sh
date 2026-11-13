#!/bin/bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Run a buck2 command in the background, poll the daemon's VmRSS every
# few seconds, and trigger `debug heap-dump` whenever a new high-water
# mark is reached. The last dump approximates the peak heap profile.
# After the command exits, sleep briefly and take a final "retained"
# dump too.
#
# Usage:
#   peak_watch.sh <bin> <tag> -- <buck2 args...>
#
# Example:
#   peak_watch.sh /tmp/b2x x -- targets fbcode//... --num-threads=30 --no-cache
#
# Run this in a benchmark-only worktree — it kills and restarts the
# local daemon. See docs/developers/perf/basics.md.
#
# Outputs (default OUT_DIR=/tmp):
#   <OUT_DIR>/<tag>_peak.prof          heap-dump from the highest VmRSS observed
#   <OUT_DIR>/<tag>_aftersleep.prof    heap-dump SLEEP_S after the command exits
#   <OUT_DIR>/<tag>_aftersleep_stats.json    allocator-stats post-sleep
#   <OUT_DIR>/<tag>_watcher.log        VmRSS / VmHWM trace from the watcher
#
# Knobs (env vars):
#   OUT_DIR=/tmp        where to write outputs
#   POLL_S=5            seconds between RSS polls
#   SLEEP_S=10          seconds to wait after build exit before the "retained" dump
#
# Why this exists: heap-dump is a snapshot. By the time a build finishes,
# transient allocations have been freed and you can't see what the daemon
# was doing at peak. MALLOC_CONF=prof_gdump:true does the same thing
# automatically but produces tens of thousands of files at the default
# sample rate — see docs/developers/perf/memory.md.

set -u
OUT_DIR=${OUT_DIR:-/tmp}
POLL_S=${POLL_S:-5}
SLEEP_S=${SLEEP_S:-10}

if [ "$#" -lt 3 ] || [ "$3" != "--" ]; then
  echo "usage: $0 <bin> <tag> -- <buck2 args...>" >&2
  exit 2
fi
BIN=$1
TAG=$2
shift 3
ARGS=("$@")

LOG="$OUT_DIR/${TAG}_watcher.log"
PEAK_DUMP="$OUT_DIR/${TAG}_peak.prof"
AFTER_DUMP="$OUT_DIR/${TAG}_aftersleep.prof"
AFTER_STATS="$OUT_DIR/${TAG}_aftersleep_stats.json"
: > "$LOG"

step() { echo "[$(date +%T)] $*" | tee -a "$LOG"; }

step "launching: $BIN ${ARGS[*]}"
MALLOC_CONF=prof:true,prof_final:false,prof_active:true \
  "$BIN" "${ARGS[@]}" >/dev/null 2>"$OUT_DIR/${TAG}_cmd.txt" &
BUILDPID=$!
step "BUILDPID=$BUILDPID"

PID=""
for _i in $(seq 1 30); do
  sleep 1
  PID=$("$BIN" status 2>/dev/null \
          | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['process_info']['pid'])" 2>/dev/null \
          || true)
  [ -n "$PID" ] && break
done
[ -z "$PID" ] && { echo "could not find daemon pid"; kill -9 $BUILDPID 2>/dev/null; exit 1; }
step "daemon pid=$PID"

PEAK=0
NDUMPS=0
while kill -0 $BUILDPID 2>/dev/null; do
  sleep "$POLL_S"
  RSS=$(awk '/^VmRSS:/{print $2}' "/proc/$PID/status" 2>/dev/null || echo 0)
  [ -z "$RSS" ] && continue
  if [ "$RSS" -gt "$PEAK" ]; then
    PEAK=$RSS
    "$BIN" debug heap-dump --path "$PEAK_DUMP" 2>/dev/null
    NDUMPS=$((NDUMPS+1))
    HWM=$(awk '/^VmHWM:/{print $2}' "/proc/$PID/status" 2>/dev/null)
    step "new HWM RSS=$((RSS/1024)) MB  VmHWM=$((HWM/1024)) MB  dump#$NDUMPS"
  fi
done

wait $BUILDPID
EXIT=$?
step "build exit $EXIT, sleeping ${SLEEP_S}s..."
sleep "$SLEEP_S"
RSS=$(awk '/^VmRSS:/{print $2}' "/proc/$PID/status" 2>/dev/null || echo 0)
HWM=$(awk '/^VmHWM:/{print $2}' "/proc/$PID/status" 2>/dev/null || echo 0)
step "post-sleep RSS=$((RSS/1024)) MB  VmHWM=$((HWM/1024)) MB"
"$BIN" debug heap-dump --path "$AFTER_DUMP" 2>/dev/null
"$BIN" debug allocator-stats > "$AFTER_STATS" 2>/dev/null
step "outputs: peak=$PEAK_DUMP (ndumps=$NDUMPS) after=$AFTER_DUMP stats=$AFTER_STATS"
ls -la "$PEAK_DUMP" "$AFTER_DUMP" "$AFTER_STATS" 2>/dev/null
