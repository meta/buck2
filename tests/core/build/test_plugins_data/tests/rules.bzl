# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @lint-ignore-every FBCODEBZLADDLOADS

RustProcMacroMarker = provider(fields = ["target"])

RustProcMacroInfo = provider(fields = ["data"])

RustLibraryInfo = provider(fields = ["proc_macros"])

RustProcMacro = plugins.kind()

def _proc_macro_alias_impl(ctx):
    # Test that the `actual` attribute correctly resolved to a target label
    if not isinstance(ctx.attrs.actual, TargetLabel):
        fail("Actual is not a target label: " + type(ctx.attrs.actual))
    return [DefaultInfo(), RustProcMacroMarker(target = ctx.attrs.actual)]

_rust_proc_macro_alias = rule(
    impl = _proc_macro_alias_impl,
    attrs = {
        "actual": attrs.plugin_dep(kind = RustProcMacro),
    },
)

def _rust_rule_impl(ctx):
    def gather_proc_macros(deps):
        direct = []
        indirect = []
        for d in deps:
            if RustProcMacroMarker in d:
                direct.append(d[RustProcMacroMarker].target)
            else:
                indirect.extend(d[RustLibraryInfo].proc_macros)
        return (direct, indirect)

    direct, indirect = gather_proc_macros(ctx.attrs.deps)
    direct_doc, indirect_doc = gather_proc_macros(ctx.attrs.doc_deps)

    proc_macros = {}
    for prov in ctx.plugins[RustProcMacro]:
        proc_macros[str(prov[RustProcMacroMarker].target)] = prov[RustProcMacroInfo].data

    get_data = lambda x: proc_macros[str(x)]
    out = {
        "direct": map(get_data, direct),
        "direct_doc": map(get_data, direct_doc),
        "indirect": map(get_data, indirect),
        "indirect_doc": map(get_data, indirect_doc),
    }
    out_art = ctx.actions.write_json("out.json", out, has_content_based_path = False)
    return (out_art, dedupe(sorted(direct + indirect)))

def _rust_library_impl(ctx):
    out_art, proc_macros = _rust_rule_impl(ctx)
    if ctx.attrs.proc_macro:
        return [DefaultInfo(default_output = out_art), RustProcMacroInfo(data = ctx.attrs.data), RustProcMacroMarker(target = ctx.label.raw_target())]
    else:
        return [DefaultInfo(default_output = out_art), RustLibraryInfo(proc_macros = proc_macros)]

rust_library = rule(
    impl = _rust_library_impl,
    attrs = {
        # Just a dummy value that allows us to pass some extra information around in tests
        "data": attrs.string(default = ""),
        "deps": attrs.list(
            attrs.dep(pulls_and_pushes_plugins = [RustProcMacro]),
            default = [],
        ),
        "doc_deps": attrs.list(
            attrs.dep(pulls_plugins = [RustProcMacro]),
            default = [],
        ),
        "proc_macro": attrs.bool(default = False),
    },
    uses_plugins = [RustProcMacro],
)

def _rust_binary_impl(ctx):
    out_art, _proc_macros = _rust_rule_impl(ctx)
    return [DefaultInfo(default_output = out_art)]

rust_binary = rule(
    impl = _rust_binary_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(pulls_and_pushes_plugins = [RustProcMacro])),
        "doc_deps": attrs.list(attrs.dep(pulls_plugins = [RustProcMacro])),
    },
    uses_plugins = [RustProcMacro],
)

def rust_proc_macro(name, **kwargs):
    rust_library(name = name + "_REAL", proc_macro = True, **kwargs)

    _rust_proc_macro_alias(
        name = name,
        actual = ":" + name + "_REAL",
    )

def _alias_impl(ctx):
    return ctx.attrs.actual.providers

alias = rule(
    impl = _alias_impl,
    attrs = {
        "actual": attrs.dep(pulls_and_pushes_plugins = plugins.All),
    },
)
