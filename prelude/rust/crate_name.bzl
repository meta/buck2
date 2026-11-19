# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

CrateName = record(
    simple = field(str | ResolvedStringWithMacros),
    dynamic = field(Artifact | None),
)

def crate_name_as_cmd_arg(crate: CrateName) -> cmd_args | str | ResolvedStringWithMacros:
    if crate.dynamic:
        # TODO: consider using `cmd_args(crate.dynamic, quote = "json")` so it
        # doesn't fall apart on paths containing ')'
        return cmd_args(crate.dynamic, format = "$(cat {})")
    else:
        return crate.simple
