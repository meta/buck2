# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @oss-disable[end= ]: load("@fbcode_macros//build_defs:platform_utils.bzl", "platform_utils")
load(":rules.bzl", "BANNED_DEP_PATHS", "BANNED_DIRECTED_DEP_PATHS", "LATE_BINDING_ONLY_CRATES", "TOP_LEVEL_ONLY_CRATES")

platform_utils = None # @oss-enable

def _dtp():
    return platform_utils.get_cxx_platform_for_base_path(package_name()).target_platform if platform_utils else None

def _check_client_to_re_path(ctx: AnalysisContext):
    path = ctx.attrs.client_to_re_path
    if len(path) != 0:
        m = "Buck2 client binary may not have a dependency on `fbcode//remote_execution/`!"
        m += "\nDependency path:"
        m += "".join(["\n" + str(t) for t in path])
        fail(m)

def _check_late_binding_only(ctx: AnalysisContext):
    for all_paths in ctx.attrs.late_binding_only_paths:
        all_paths = list(all_paths)
        target = all_paths.pop()
        all_paths.pop(0)
        if len(all_paths) != 0:
            m = "Late-binding-only crate `" + str(target.label) + "` may not be depended on by:"
            m += "".join(["\n" + str(p.label) for p in all_paths])
            fail(m)

def _check_top_level_only(ctx: AnalysisContext):
    for all_paths in ctx.attrs.top_level_only_paths:
        all_paths = list(all_paths)
        target = all_paths.pop()
        remainder = filter(lambda t: not str(t.label).startswith("fbcode//buck2/app/buck2:"), all_paths)

        if len(remainder) != 0:
            m = "Top-level-only crate `" + str(target.label) + "` may not be depended on by:"
            m += "".join(["\n" + str(p.label) for p in remainder])
            fail(m)

def _check_banned_dep_paths(ctx: AnalysisContext):
    for path in ctx.attrs.banned_dep_paths:
        if len(path) > 0:
            a = path[0].label
            b = path[-1].label
            m = str(a) + " may not depend on " + str(b) + "! Path:"
            m += "".join(["\n" + str(p.label) for p in path])
            fail(m)

def _impl(ctx: AnalysisContext):
    _check_client_to_re_path(ctx)
    _check_late_binding_only(ctx)
    _check_top_level_only(ctx)
    _check_banned_dep_paths(ctx)
    return [DefaultInfo()]

_test_buck2_dep_graph = rule(
    impl = _impl,
    attrs = {
        "banned_dep_paths": attrs.list(attrs.query()),
        "client_to_re_path": attrs.query(),
        "late_binding_only_paths": attrs.list(attrs.query()),
        "top_level_only_paths": attrs.list(attrs.query()),
    },
)

_CLIENT_BIN = "fbcode//buck2/app/buck2:buck2_client-bin"

_BUCK2_BIN = "fbcode//buck2/app/buck2:buck2-bin"

_RE_CLIENT_TARGET = "//remote_execution/client_lib/wrappers/rust:re_client_lib"

_CLIENT_TO_RE = "somepath({}, filter(fbcode//remote_execution/, deps({})) + {})".format(_CLIENT_BIN, _CLIENT_BIN, _RE_CLIENT_TARGET)

def test_buck2_dep_graph(name):
    banned_dep_paths = []
    for a, b in BANNED_DEP_PATHS:
        if a > b:
            m = "`BANNED_DEP_PATHS` entries must be sorted:\n"
            m += "    " + str(a) + "\n"
            m += "  > " + str(b)
            fail(m)

        banned_dep_paths.append("somepath({}, {})".format(a, b))
        banned_dep_paths.append("somepath({}, {})".format(b, a))

    for a, b in BANNED_DIRECTED_DEP_PATHS:
        banned_dep_paths.append("somepath({}, {})".format(a, b))

    _test_buck2_dep_graph(
        name = name,
        banned_dep_paths = banned_dep_paths,
        client_to_re_path = _CLIENT_TO_RE,
        default_target_platform = _dtp(),
        late_binding_only_paths = ["allpaths({}, {})".format(_BUCK2_BIN, c) for c in LATE_BINDING_ONLY_CRATES],
        top_level_only_paths = ["allpaths({}, {})".format(_BUCK2_BIN, c) for c in TOP_LEVEL_ONLY_CRATES],
    )
