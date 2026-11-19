# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""The configuration transition of the Lean rules.

Lean builds for Windows with MinGW, as upstream does: the runtime includes
POSIX headers that the MSVC toolchain does not have. `lean_library`,
`lean_binary` and `lean_test` apply this transition, so a Windows
configuration gets the `gnu` ABI, which selects the MinGW cxx toolchain, and
their dependencies get it with them. A configuration that has the `gnu` ABI
already, or another OS, passes through unchanged.
"""

def _gnu_abi_on_windows(platform: PlatformInfo, refs: struct) -> PlatformInfo:
    constraints = platform.configuration.constraints
    windows = refs.os_windows[ConstraintValueInfo]
    if constraints.get(windows.setting.label) != windows:
        return platform
    gnu = refs.abi_gnu[ConstraintValueInfo]
    if constraints.get(gnu.setting.label) == gnu:
        return platform
    constraints[gnu.setting.label] = gnu
    return PlatformInfo(
        label = platform.label,
        configuration = ConfigurationInfo(
            constraints = constraints,
            values = platform.configuration.values,
        ),
    )

lean_transition = transition(
    impl = _gnu_abi_on_windows,
    refs = {
        "abi_gnu": "prelude//abi/constraints:gnu",
        "os_windows": "config//os/constraints:windows",
    },
)
