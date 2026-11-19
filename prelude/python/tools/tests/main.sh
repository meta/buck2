#!/usr/bin/env bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

set -euo pipefail

TOOLS_DIR="$(dirname "$(dirname "$(realpath "$0")")")"
export PYTHONPATH="$TOOLS_DIR"
exec python3 -m unittest discover -s "$TOOLS_DIR/tests" -p '*_test.py'
