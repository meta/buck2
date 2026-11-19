# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
HIP (`.hip`) compile helpers for the C++ compile path.
"""

# Phase selectors; the last one on the command line wins.
_HOST_ONLY_FLAGS = ("--offload-host-only", "--cuda-host-only")
_DEVICE_FLAGS = (
    "--offload-device-only",
    "--cuda-device-only",
    "--offload-host-device",
    "--cuda-compile-host-device",
)

def _flag_texts(flags: list[typing.Any]) -> list[str]:
    texts = []
    for flag in flags:
        if isinstance(flag, list):
            texts.extend(_flag_texts(flag))
        elif isinstance(flag, str):
            texts.append(flag)
        elif isinstance(flag, ResolvedStringWithMacros):
            texts.append(str(flag)[1:-1])
    return texts

def hip_compile_weight(flags: list[typing.Any], source: str) -> int:
    """
    The local executor slots a HIP compile of `source` takes: the parallelism
    its `-parallel-jobs=N` asks for, or 1 when it compiles for the host only.

    `flags` are the target's and the source's own flags, in command-line
    order, which is where fbcode puts the job count; flags the toolchain adds
    through its argsfiles are not seen. `cmd_args` and artifacts are skipped.
    """
    jobs = 1
    host_only = False
    for text in _flag_texts(flags):
        if text.startswith("-parallel-jobs="):
            value = text.removeprefix("-parallel-jobs=")
            if value.isdigit():
                jobs = int(value)
        elif text in _HOST_ONLY_FLAGS:
            host_only = True
        elif text in _DEVICE_FLAGS:
            host_only = False
    if jobs == 0:
        fail("{}: `-parallel-jobs=0` is not a valid HIP job count (ROCm 7.0 clang hangs on it, newer drivers reject it)".format(source))
    return 1 if host_only else jobs
