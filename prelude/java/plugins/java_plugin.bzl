# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//java:java_providers.bzl", "JavaPackagingDepTSet")
load(
    "@prelude//java/plugins:java_annotation_processor.bzl",
    "JavaProcessorsInfo",
    "JavaProcessorsType",
    "derive_transitive_deps",
)
load("@prelude//utils:type_defs.bzl", "is_list", "is_tuple")

PluginParams = record(
    processors = field(list[(str, cmd_args)]),
    deps = field([JavaPackagingDepTSet, None]),
)

def create_plugin_params(ctx: AnalysisContext, plugins: list[[Dependency, (Dependency, list[str] | cmd_args)]]) -> [PluginParams, None]:
    processors = []
    plugin_deps = []

    # Compiler plugin derived from `plugins` attribute
    for item in plugins:
        # Each plugin can be either a tuple of (target, arguments) or just the target
        if is_tuple(item):
            plugin = item[0]
            arguments = cmd_args(item[1]) if is_list(item[1]) else item[1]
        else:
            plugin = item
            arguments = cmd_args()

        processors_info = plugin.get(JavaProcessorsInfo)
        if processors_info != None and processors_info.type == JavaProcessorsType("plugin"):
            if len(processors_info.processors) > 1:
                fail("Only 1 java compiler plugin is expected. But received: {}".format(processors_info.processors))
            processor = processors_info.processors[0]
            if processors_info.deps:
                plugin_deps.append(processors_info.deps)

            processors.append((processor, arguments))

    if not processors:
        return None

    return PluginParams(
        processors = processors,
        deps = ctx.actions.tset(JavaPackagingDepTSet, children = plugin_deps) if plugin_deps else None,
    )

def java_plugin_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    return [
        JavaProcessorsInfo(
            deps = derive_transitive_deps(ctx, ctx.attrs.deps),
            processors = [ctx.attrs.plugin_name],
            type = JavaProcessorsType("plugin"),
        ),
        DefaultInfo(default_output = None),
    ]

java_plugin = prelude_rule(
    name = "java_plugin",
    impl = java_plugin_impl,
    docs = """
        A `java_plugin()` rule declares a `javac` plugin (identified by
        `plugin_name`) that can be referenced from `java_library()` and
        `kotlin_library()` rules to run during compilation.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "deps": attrs.list(attrs.dep(), default = []),
            "does_not_affect_abi": attrs.bool(default = False),
            "isolate_class_loader": attrs.bool(default = False),
            "plugin_name": attrs.string(default = ""),
            "supports_abi_generation_from_source": attrs.bool(default = False),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
