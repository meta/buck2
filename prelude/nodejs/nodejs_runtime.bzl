# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load(":nodejs_providers.bzl", "NodejsRuntimeInfo", "NodejsRuntimeKind", "check_relative_path")

def _is_node_version(version: str) -> bool:
    parts = version.split("-", 1)[0].split(".")
    if len(parts) != 3:
        return False
    for part in parts:
        if not part.isdigit():
            return False
    return True

def check_node_version(version: str, error_prefix: str):
    if not _is_node_version(version):
        fail("{} '{}': must be a Node.js version such as '24.18.1'".format(error_prefix, version))

def _check_staged_paths(label: Label, paths: list[(str, str)]):
    """Fails if two of `paths`, each an attribute and a path staged on one platform, are the same or nest.

    Case is ignored, as Windows and macOS file systems ignore it.
    """
    staged = {}
    for attr, path in paths:
        what = "{} '{}'".format(attr, path)
        key = path.lower()
        if key in staged:
            fail("{}: {} collides with {}".format(label, what, staged[key]))
        staged[key] = what
    for key, what in staged.items():
        segments = key.split("/")
        for i in range(1, len(segments)):
            parent = staged.get("/".join(segments[:i]))
            if parent != None:
                fail("{}: {} is inside {}".format(label, what, parent))

def nodejs_runtime_impl(ctx: AnalysisContext) -> list[Provider]:
    node = ctx.attrs.node
    files = ctx.attrs.files
    posix_exe = ctx.attrs.posix_exe
    windows_exe = ctx.attrs.windows_exe
    check_relative_path(posix_exe, "{}: invalid `posix_exe`".format(ctx.label))
    check_relative_path(windows_exe, "{}: invalid `windows_exe`".format(ctx.label))
    for path in files:
        check_relative_path(path, "{}: invalid `files` path".format(ctx.label))
    if ctx.attrs.version != None:
        check_node_version(ctx.attrs.version, "{}: invalid `version`".format(ctx.label))

    if not ctx.attrs.target_independent:
        kind = NodejsRuntimeKind("native")
        node_paths = [("`posix_exe`", posix_exe), ("`windows_exe`", windows_exe)] if node != None else []
    elif node != None:
        kind = NodejsRuntimeKind("pointer")
        node_paths = [("`posix_exe`", posix_exe)]
        if windows_exe not in files:
            fail(
                "{}: `windows_exe` '{}' must name one of `files`, because a target-independent runtime stages `node` at `posix_exe` on every platform".format(
                    ctx.label, windows_exe
                )
            )
    else:
        kind = NodejsRuntimeKind("host")
        node_paths = []
        if files:
            fail("{}: a host runtime (`target_independent` without `node`) bundles nothing, so it cannot have `files`".format(ctx.label))
    file_paths = [("`files` path", path) for path in files]

    # Each platform stages `node` at one of `node_paths`, beside `files`.
    for staged in [[node_path] + file_paths for node_path in node_paths] or [file_paths]:
        _check_staged_paths(ctx.label, staged)

    return [
        DefaultInfo(default_outputs = ([node] if node != None else []) + files.values()),
        NodejsRuntimeInfo(
            kind = kind,
            node = node,
            files = files,
            posix_exe = posix_exe,
            windows_exe = windows_exe,
            version = ctx.attrs.version,
            node_args = ctx.attrs.node_args,
            env = ctx.attrs.env,
            exe = ctx.attrs.exe[RunInfo] if ctx.attrs.exe != None else None,
        ),
    ]

nodejs_runtime = prelude_rule(
    name = "nodejs_runtime",
    impl = nodejs_runtime_impl,
    docs = """A Node.js runtime that an app runs on or ships with,
    provided as `NodejsRuntimeInfo`. It has no toolchain attribute, so a
    nodejs toolchain can name one as its default `runtime`. Its kind is
    `native` when `node` is a Node.js binary for the target platform
    (typically picked by a `select`); `pointer` when
    `target_independent = True` and `node` is one file that runs Node.js
    on every platform, such as a DotSlash file; and `host` when
    `target_independent = True` and there is no `node`: nothing is
    bundled, and Node.js comes from the host, for example from `PATH`.
    `prelude//nodejs/runtimes:host` is a ready-made host runtime. A
    native runtime without `node`, as when a `select` falls back to
    `None`, has no binary for the target platform.
    `files` are staged next to `node`, keyed by their path relative to it
    (relative paths with `/` separators, no `:` and no `.` or `..`
    segments, as are `posix_exe` and `windows_exe`).
    `posix_exe` (default `"node"`) and `windows_exe` (default
    `"node.exe"`) are where the executable to run sits on POSIX and on
    Windows. A native runtime stages `node` at `windows_exe` on Windows
    and at `posix_exe` elsewhere. A pointer runtime stages `node` at
    `posix_exe` on every platform, and its `windows_exe` must name one of
    `files`, such as a `node.bat` that runs `node`. No two paths staged
    on a platform may be the same or nest, ignoring case as Windows and
    macOS do. A host runtime has no `files`.
    `version` is the Node.js version, for example `"24.18.1"`.
    `node_args` and `env` are the flags and environment variables to run
    `node` with. `exe`, when set, runs the runtime in place, for example
    a wrapper that adds flags.""",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "node": attrs.option(attrs.source(allow_directory = False), default = None),
            "files": attrs.dict(attrs.string(), attrs.source(), default = {}),
            "posix_exe": attrs.string(default = "node"),
            "windows_exe": attrs.string(default = "node.exe"),
            "version": attrs.option(attrs.string(), default = None),
            "target_independent": attrs.bool(default = False),
            "node_args": attrs.list(attrs.string(), default = []),
            "env": attrs.dict(attrs.string(), attrs.string(), default = {}),
            "exe": attrs.option(attrs.dep(providers = [RunInfo]), default = None),
        }
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
