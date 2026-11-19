# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":nodejs_providers.bzl", "NodejsRuntimeInfo", "NodejsRuntimeKind", "NodejsTreeInfo")
load(":nodejs_runtime.bzl", "check_node_version")

NodejsToolchainInfo = provider(
    doc = "Node.js toolchain.",
    fields = {
        # An esbuild-compatible bundler.
        "bundler": provider_field([RunInfo, None], default = None),
        # `node_modules` tree holding `jest`, for tests with `framework = "jest"`
        # and no `test_runner`, which fail when this is `None`.
        "jest": provider_field([Artifact, None], default = None),
        # Signs a Mach-O executable ad hoc.
        "macho_signer": provider_field([RunInfo, None], default = None),
        # Runs in-place binaries and tests, in the target configuration.
        "node": provider_field(RunInfo),
        # Node binary bundled into `standalone` packages, which fail when this is `None`.
        # Without `runtime`, this is also the default runtime.
        "node_dist": provider_field([Artifact, None], default = None),
        # Runs build-time Node.js actions, in the execution configuration.
        "node_exec": provider_field([RunInfo, None], default = None),
        # The default runtime for the target platform.
        "runtime": provider_field([NodejsRuntimeInfo, None], default = None),
        # Injects a single executable application blob into a copy of `node`, with postject's command line.
        "sea_injector": provider_field([RunInfo, None], default = None),
        # Targets that set `tsconfig` fail when this is `None`.
        "tsc": provider_field([RunInfo, None], default = None),
        # The Node.js version of `node` and `node_exec`, for example "24.18.1".
        "version": provider_field([str, None], default = None),
        # `node_modules` tree holding `vitest`, for tests with `framework = "vitest"`
        # and no `test_runner`, which fail when this is `None`.
        "vitest": provider_field([Artifact, None], default = None),
        # Compresses (`-o <output> <input>`) or decompresses (`-d -o <output> <input>`) zstd. Compressed output does not depend on how the input is chunked.
        "zstd": provider_field([RunInfo, None], default = None),
    },
)

def _run_info(dep: Dependency | None) -> RunInfo | None:
    return dep[RunInfo] if dep != None else None

def _nodejs_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs.node == None:
        fail("nodejs_toolchain requires the `node` attribute")
    if ctx.attrs.version != None:
        check_node_version(ctx.attrs.version, "{}: invalid `version`".format(ctx.label))
    node_dist = None
    if ctx.attrs.node_dist != None:
        outs = ctx.attrs.node_dist[DefaultInfo].default_outputs
        if len(outs) != 1:
            fail("nodejs_toolchain `node_dist` must have exactly one default output")
        node_dist = outs[0]
    runtime = None
    if ctx.attrs.runtime != None:
        runtime = ctx.attrs.runtime[NodejsRuntimeInfo]
    elif node_dist != None:
        runtime = NodejsRuntimeInfo(kind = NodejsRuntimeKind("native"), node = node_dist, version = ctx.attrs.version)
    return [
        DefaultInfo(),
        NodejsToolchainInfo(
            jest = ctx.attrs.jest[NodejsTreeInfo].node_modules if ctx.attrs.jest != None else None,
            vitest = ctx.attrs.vitest[NodejsTreeInfo].node_modules if ctx.attrs.vitest != None else None,
            node = ctx.attrs.node[RunInfo],
            node_exec = _run_info(ctx.attrs.node_exec),
            version = ctx.attrs.version,
            runtime = runtime,
            node_dist = node_dist,
            tsc = _run_info(ctx.attrs.tsc),
            bundler = _run_info(ctx.attrs.bundler),
            zstd = _run_info(ctx.attrs.zstd),
            sea_injector = _run_info(ctx.attrs.sea_injector),
            macho_signer = _run_info(ctx.attrs.macho_signer),
        ),
    ]

nodejs_toolchain = rule(
    impl = _nodejs_toolchain_impl,
    attrs = {
        "bundler": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "jest": attrs.option(attrs.dep(providers = [NodejsTreeInfo]), default = None),
        "macho_signer": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "node": attrs.option(attrs.dep(providers = [RunInfo]), default = None),
        "node_dist": attrs.option(attrs.dep(providers = [DefaultInfo]), default = None),
        "node_exec": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "runtime": attrs.option(attrs.dep(providers = [NodejsRuntimeInfo]), default = None),
        "sea_injector": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "tsc": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "version": attrs.option(attrs.string(), default = None),
        "vitest": attrs.option(attrs.dep(providers = [NodejsTreeInfo]), default = None),
        "zstd": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
    },
    is_toolchain_rule = True,
)

def _system_nodejs_toolchain_impl(_ctx: AnalysisContext) -> list[Provider]:
    node = RunInfo(args = ["node"])
    return [
        DefaultInfo(),
        NodejsToolchainInfo(
            node = node,
            node_exec = node,
            runtime = NodejsRuntimeInfo(kind = NodejsRuntimeKind("host")),
        ),
    ]

system_nodejs_toolchain = rule(
    impl = _system_nodejs_toolchain_impl,
    attrs = {},
    is_toolchain_rule = True,
)
