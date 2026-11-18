# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _internal_tool(default: str) -> Attr:
    return attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = default))

# Factored out of prelude//toolchains/rust.bzl to keep only the user-facing
# configurable attributes there. This list of internal tools is distracting and
# expected to grow.
_internal_tool_attrs = {
    "cd_run": _internal_tool("prelude//rust/tools:cd_run"),
    "extract_link_action": _internal_tool("prelude//rust/tools:extract_link_action"),
    "failure_filter_action": _internal_tool("prelude//rust/tools:failure_filter_action"),
    "from_any_dir": _internal_tool("prelude//rust/tools:from_any_dir"),
    "redirect_stdout": _internal_tool("prelude//rust/tools:redirect_stdout"),
    "rustc_action": _internal_tool("prelude//rust/tools:rustc_action"),
    "rustdoc_coverage": _internal_tool("prelude//rust/tools:rustdoc_coverage"),
    "rustdoc_test_with_resources": _internal_tool("prelude//rust/tools:rustdoc_test_with_resources"),
    "shared_libraries_symlink_tree": _internal_tool("prelude//rust/tools:shared_libraries_symlink_tree"),
    "symlink_only_dir_entry": _internal_tool("prelude//rust/tools:symlink_only_dir_entry"),
    "transitive_dependency_symlinks_tool": _internal_tool("prelude//rust/tools:transitive_dependency_symlinks"),
    "windows_export_def": _internal_tool("prelude//rust/tools:windows_export_def"),
}

RustInternalToolsInfo = provider(fields = {tool: RunInfo for tool in _internal_tool_attrs.keys()})

def _impl(ctx: AnalysisContext) -> list[Provider]:
    info = RustInternalToolsInfo(**{tool: getattr(ctx.attrs, tool)[RunInfo] for tool in _internal_tool_attrs.keys()})
    return [DefaultInfo(), info]

rust_internal_tools_toolchain = rule(
    impl = _impl,
    attrs = _internal_tool_attrs,
    is_toolchain_rule = True,
)
