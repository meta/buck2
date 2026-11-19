# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:platform_exe_constants.bzl", "SUPPORTED_ARCHS")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

def resolve_platform_exe(
    exe: Artifact | Dependency | RunInfo | None,
    platform_exe: dict[str, Dependency | dict[str, Dependency]],
    target_os: OsLookup,
) -> RunInfo | dict[str, RunInfo | dict[str, RunInfo]]:
    """Select executable dependencies without changing their configurations.

    fat_linux keeps both Linux architectures for runtime selection via uname -m.
    fat_mac_linux selects its macOS or Linux executable at runtime via uname:
    Darwin selects macOS; Linux selects Linux. Both OSes use the same configured
    target architecture for nested maps; the runtime host architecture is not used.
    """
    has_arch_maps = False
    for os_name, entry in platform_exe.items():
        if isinstance(entry, dict):
            has_arch_maps = True
            if len(entry) == 0:
                fail("platform_exe[{}] arch map must not be empty".format(repr(os_name)))
            for arch in entry:
                if arch not in SUPPORTED_ARCHS:
                    fail("platform_exe[{}]: unsupported arch {}; expected {}".format(repr(os_name), repr(arch), SUPPORTED_ARCHS))

    if target_os.os == Os("fat_linux"):
        linux = platform_exe.get("linux")
        if not isinstance(linux, dict):
            fail("fat_linux requires platform_exe['linux'] to map each supported arch {} to an executable".format(SUPPORTED_ARCHS))
        for arch in SUPPORTED_ARCHS:
            if arch not in linux:
                fail("fat_linux requires platform_exe['linux'][{}]".format(repr(arch)))
        return {"Linux": {arch: _run_info(linux[arch]) for arch in SUPPORTED_ARCHS}}

    if target_os.os == Os("fat_mac_linux") and platform_exe:
        return {
            "Darwin": _os_base(exe, platform_exe, "macos", target_os.cpu, has_arch_maps = has_arch_maps),
            "Linux": _os_base(exe, platform_exe, "linux", target_os.cpu, has_arch_maps = has_arch_maps),
        }
    return _os_base(exe, platform_exe, target_os.os.value, target_os.cpu, has_arch_maps = has_arch_maps)

def _os_base(exe, platform_exe, os: str, arch: str | None, *, has_arch_maps: bool) -> RunInfo:
    value = platform_exe.get(os, exe)
    # Flat maps allow args-only commands when no OS entry matches.
    if value == None and has_arch_maps:
        fail("platform_exe has no entry for OS {} and no exe fallback".format(repr(os)))
    if isinstance(value, dict):
        if arch not in SUPPORTED_ARCHS:
            fail("platform_exe[{}] needs a supported target arch {}; got {}".format(repr(os), SUPPORTED_ARCHS, repr(arch)))
        if arch not in value:
            fail("platform_exe[{}] is missing an executable for target arch {}; add an entry for that arch to the map".format(repr(os), repr(arch)))
        value = value[arch]
    return _run_info(value)

def _run_info(exe: Artifact | Dependency | RunInfo | None) -> RunInfo:
    if exe == None:
        return RunInfo()
    if isinstance(exe, RunInfo):
        return exe
    if isinstance(exe, Artifact):
        return RunInfo(args = cmd_args(exe))
    run_info = exe.get(RunInfo)
    if run_info == None:
        run_info = RunInfo(args = exe[DefaultInfo].default_outputs)
    return run_info
