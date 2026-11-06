# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Version-gated Kotlin compiler options for Android toolchain targets."""

def _leading_int(part: str) -> int:
    digits = ""
    for char in part.elems():
        if not char.isdigit():
            break
        digits += char
    return int(digits) if digits else 0

def kotlin_version_at_least(major: int, minor: int, version: str | None = None) -> bool:
    """Compare the actual compiler's major/minor, ignoring prerelease suffixes."""
    if version == None:
        version = read_root_config("kotlin", "kotlin_version", "2.3.21")
    parts = version.split(".")
    if len(parts) < 2:
        return False
    return (_leading_int(parts[0]), _leading_int(parts[1])) >= (major, minor)

def kotlin_implementation_detail_opt_in_args(version: str | None = None) -> list[str]:
    """KtFile implementations require the PSI marker starting with Kotlin 2.5."""
    if kotlin_version_at_least(2, 5, version):
        return ["-opt-in=org.jetbrains.kotlin.psi.KtImplementationDetail"]
    return []
