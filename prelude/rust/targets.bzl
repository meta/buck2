# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:common.bzl", "TargetCpuType")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

_cpu = enum(*TargetCpuType)

_OS_TRIPLES = {
    (Os("linux"), _cpu("arm64")): "aarch64-unknown-linux-gnu",
    (Os("linux"), _cpu("x86_64")): "x86_64-unknown-linux-gnu",
    (Os("linux"), _cpu("riscv64")): "riscv64gc-unknown-linux-gnu",
    (Os("macos"), _cpu("arm64")): "aarch64-apple-darwin",
    (Os("macos"), _cpu("x86_64")): "x86_64-apple-darwin",
    (Os("windows"), _cpu("arm64")): "aarch64-pc-windows-msvc",
    (Os("windows"), _cpu("x86_64")): "x86_64-pc-windows-msvc",
}

def _exec_triple(ctx: AnalysisContext) -> str | None:
    exec_os = ctx.attrs._exec_os_type[OsLookup]
    if exec_os.cpu:
        return _OS_TRIPLES.get((exec_os.os, _cpu(exec_os.cpu)))
    else:
        return None

targets = struct(
    exec_triple = _exec_triple,
)
