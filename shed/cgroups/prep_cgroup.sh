#!/bin/bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# `cat x | cut` is stylistically fine
# shellcheck disable=SC2002

set -e
set -o pipefail

# `argv[1]` to specify the time until the scope should be automatically cleaned up
scope_ttl=${1:-10m}

# Spawn a systemd service unit that sleeps for the specified time; once that process exits, systemd
# will clean up everything else in the cgroup tree, so nothing can leak
unit=testpcg$RANDOM
systemd-run \
    --user \
    --quiet \
    --unit $unit \
    --property=Delegate=yes \
    --slice-inherit \
    --expand-environment=no \
    --property=MemoryAccounting=yes \
    -- \
    bash -c "sleep $scope_ttl"
keep_alive_pid=$(systemctl show --user --property MainPID $unit | cut -d= -f2)
echo "Keep alive PID: $keep_alive_pid (kill to clean up)" >&2
cgd=/sys/fs/cgroup$(cat /proc/"$keep_alive_pid"/cgroup | cut -c '4-')
mkdir "$cgd"/_keep_alive
echo "$keep_alive_pid" > "$cgd"/_keep_alive/cgroup.procs
mkdir "$cgd"/action
echo +memory > "$cgd"/cgroup.subtree_control
echo +cpu > "$cgd"/cgroup.subtree_control
echo +pids > "$cgd"/cgroup.subtree_control
echo "$cgd"/action
