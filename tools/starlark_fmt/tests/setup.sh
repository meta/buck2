#!/bin/bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

set -uo pipefail

if ! [[ -n ${TESTSHELL:-} && $TESTSHELL =~ (/bash|\\bash\.exe)$ ]]; then
  echo "Bash is required. Found '$TESTSHELL' instead." >&2
  exit 1
fi

shopt -s expand_aliases

if [[ -z ${STARLARK_FMT_PATH:-} ]]; then
  echo 'Required STARLARK_FMT_PATH env var is empty or not set.' >&2
  exit 1
fi

echo '{"IsSortableListArg": {}, "SortableBlacklist": {}, "NamePriority": {}, "Overrides": [{"Files": ["*.bzl"], "SortListArgs": false, "SortKwargs": false}]}' > empty_config.json
alias starlark-fmt='"$STARLARK_FMT_PATH" --config empty_config.json fmt'
alias starlark-lint='"$STARLARK_FMT_PATH" --config empty_config.json lint'
alias starlark-diff='"$STARLARK_FMT_PATH" --config empty_config.json diff'
alias starlark-stdin='"$STARLARK_FMT_PATH" --config empty_config.json stdin'
