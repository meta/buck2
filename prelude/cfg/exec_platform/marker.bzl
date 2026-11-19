# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Default exec platform marker constraint value.
# This is the constraint value that gets injected into all execution platforms.
_DEFAULT_EXEC_PLATFORM_MARKER = "prelude//cfg/exec_platform/marker:is_exec_platform[true]"

def get_exec_platform_marker() -> str:
    """
    Get the exec platform marker constraint value target.

    This reads from buckconfig `[build].exec_platform_marker` and defaults to
    `prelude//cfg/exec_platform/marker:is_exec_platform[true]`.

    Returns:
        The target label string for the exec platform marker constraint value,
        unless the buckconfig doesn't exist, then `_DEFAULT_EXEC_PLATFORM_MARKER`.
    """
    return read_root_config("build", "exec_platform_marker", _DEFAULT_EXEC_PLATFORM_MARKER)
