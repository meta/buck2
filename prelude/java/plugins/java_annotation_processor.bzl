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
load("@prelude//java:java_providers.bzl", "JavaLibraryInfo", "JavaPackagingDepTSet", "JavaPackagingInfo")
load("@prelude//utils:type_defs.bzl", "is_tuple")

JavaProcessorsType = enum(
    "java_annotation_processor",
    "ksp_annotation_processor",
    "plugin",
)

JavaProcessorsInfo = provider(
    # @unsorted-dict-items
    doc = "Information about java annotation processor/ java compiler plugins and their dependencies",
    fields = {
        "affects_abi": provider_field(typing.Any, default = None),
        # Java dependencies exposed to dependent targets and supposed to be used during compilation.
        "deps": provider_field(typing.Any, default = None),  # [JavaPackagingDepTSet, None]
        "isolate_class_loader": provider_field(typing.Any, default = None),
        # Names of processors
        "processors": provider_field(typing.Any, default = None),  # ["string"]
        # Whether processor can only run on java and shall run on javac even in KotlinCD
        # (which means it will only apply on java files in kotlin module)
        "runs_on_java_only": provider_field(typing.Any, default = None),
        # Whether processor support source only abi
        "supports_source_only_abi": provider_field(typing.Any, default = None),
        # Type of processor
        "type": provider_field(typing.Any, default = None),  # "JavaProcessorsType"
    },
)

AnnotationProcessor = record(
    affects_abi = field(bool),
    supports_source_only_abi = field(bool),
    runs_on_java_only = field(bool),
    processors = field(list[str]),
    deps = field([JavaPackagingDepTSet, None]),
    isolate_class_loader = field(bool),
)

AnnotationProcessorProperties = record(
    annotation_processors = field(list[AnnotationProcessor]),
    annotation_processor_params = field(list[str]),
    source_only_abi_annotation_processor_param_overrides = field(dict[str, [str, None]], {}),
)

# Every transitive java annotation processors dependency has to be included into processor classpath for AP/Java Plugin run
def derive_transitive_deps(ctx: AnalysisContext, deps: list[Dependency]) -> [JavaPackagingDepTSet, None]:
    for dep in deps:
        if not dep[JavaLibraryInfo]:
            fail("Dependency must have a type of `java_library` or `prebuilt_jar`. Deps: {}".format(deps))

    return (
        ctx.actions.tset(
            JavaPackagingDepTSet,
            children = [dep[JavaPackagingInfo].packaging_deps for dep in deps],
        )
        if deps
        else None
    )

def create_annotation_processor_properties(
    ctx: AnalysisContext,
    plugins: list[[Dependency, (Dependency, list[str])]],
    annotation_processor_names: list[str],
    annotation_processor_params: list[str],
    annotation_processor_deps: list[Dependency],
) -> AnnotationProcessorProperties:
    annotation_processors = []

    # Extend `ap_processor_deps` with java deps from `annotation_processor_deps`
    if annotation_processor_names or annotation_processor_deps:
        for ap_dep in [_get_plugin_provider(x, JavaLibraryInfo) for x in annotation_processor_deps]:
            if not ap_dep:
                fail("Dependency must have a type of `java_library` or `prebuilt_jar`. Deps: {}".format(annotation_processor_deps))

        # "legacy" annotation processors have no mechanism for indicating if they affect abi or if they support source_only or if they run on javac for mix modules
        annotation_processors.append(
            AnnotationProcessor(
                affects_abi = True,
                supports_source_only_abi = False,
                runs_on_java_only = False,
                processors = annotation_processor_names,
                # using packaging deps to have all transitive deps collected for processors classpath
                deps = derive_transitive_deps(ctx, annotation_processor_deps),
                isolate_class_loader = False,
            )
        )

    # APs derived from `plugins` attribute
    for ap_plugin in filter(None, [_get_plugin_provider(x, JavaProcessorsInfo) for x in plugins]):
        if not ap_plugin:
            fail("Plugin must have a type of `java_annotation_processor` or `java_plugin`. Plugins: {}".format(plugins))
        if ap_plugin.type == JavaProcessorsType("java_annotation_processor"):
            annotation_processors.append(
                AnnotationProcessor(
                    affects_abi = ap_plugin.affects_abi,
                    supports_source_only_abi = ap_plugin.supports_source_only_abi,
                    runs_on_java_only = ap_plugin.runs_on_java_only,
                    processors = ap_plugin.processors,
                    deps = ap_plugin.deps,
                    isolate_class_loader = ap_plugin.isolate_class_loader,
                )
            )

    annotation_processor_params = _update_first(
        annotation_processor_params,
        "buck.current_buck_target=<will_be_replaced>",
        "buck.current_buck_target=" + str(ctx.label.raw_target()),
    )

    annotation_processor_params = _update_first(
        annotation_processor_params,
        "buck.required_for_source_only_abi=<will_be_replaced>",
        ("buck.required_for_source_only_abi=" + str(ctx.attrs.required_for_source_only_abi)) if hasattr(ctx.attrs, "required_for_source_only_abi") else None,
    )

    return AnnotationProcessorProperties(
        annotation_processors = annotation_processors,
        annotation_processor_params = annotation_processor_params,
        source_only_abi_annotation_processor_param_overrides = getattr(
            ctx.attrs,
            "source_only_abi_annotation_processor_param_overrides",
            {},
        ),
    )

def _update_first(iterable, old_value, new_value):
    """
    Replaces the first occurrence of old_value with new_value.
    If new_value is None, removes the first occurrence instead.
    Returns the original iterable unchanged if old_value is not found.
    """
    if old_value not in iterable:
        return iterable
    result = list(iterable)
    idx = result.index(old_value)
    if new_value == None:
        result.pop(idx)
    else:
        result[idx] = new_value
    return result

def create_ksp_annotation_processor_properties(plugins: list[[Dependency, (Dependency, list[str])]]) -> AnnotationProcessorProperties:
    annotation_processors = []

    # APs derived from `plugins` attribute
    for ap_plugin in filter(None, [_get_plugin_provider(x, JavaProcessorsInfo) for x in plugins]):
        if not ap_plugin:
            fail("Plugin must have a type of `java_annotation_processor` or `java_plugin`. Plugins: {}".format(plugins))
        if ap_plugin.type == JavaProcessorsType("ksp_annotation_processor"):
            annotation_processors.append(
                AnnotationProcessor(
                    affects_abi = ap_plugin.affects_abi,
                    supports_source_only_abi = ap_plugin.supports_source_only_abi,
                    runs_on_java_only = ap_plugin.runs_on_java_only,
                    processors = ap_plugin.processors,
                    deps = ap_plugin.deps,
                    isolate_class_loader = ap_plugin.isolate_class_loader,
                )
            )

    return AnnotationProcessorProperties(
        annotation_processors = annotation_processors,
        annotation_processor_params = [],
    )

def _get_processor_type(processor_class: str) -> JavaProcessorsType:
    if processor_class.startswith("KSP:"):
        return JavaProcessorsType("ksp_annotation_processor")

    return JavaProcessorsType("java_annotation_processor")

def _get_plugin_provider(plugin: [Dependency, (Dependency, list[str])], provider: typing.Callable[[], Provider]) -> [Provider, None]:
    return (plugin[0] if is_tuple(plugin) else plugin).get(provider)

def java_annotation_processor_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    transitive_deps = derive_transitive_deps(ctx, ctx.attrs.deps)

    return [
        JavaProcessorsInfo(
            deps = transitive_deps,
            processors = [ctx.attrs.processor_class],
            type = _get_processor_type(ctx.attrs.processor_class),
            affects_abi = not ctx.attrs.does_not_affect_abi,
            supports_source_only_abi = ctx.attrs.supports_abi_generation_from_source,
            runs_on_java_only = ctx.attrs.runs_on_java_only,
            isolate_class_loader = ctx.attrs.isolate_class_loader,
        ),
        DefaultInfo(default_output = None, other_outputs = [transitive_deps.project_as_args("full_jar_args")]),
    ]

java_annotation_processor = prelude_rule(
    name = "java_annotation_processor",
    impl = java_annotation_processor_impl,
    docs = """
        A `java_annotation_processor()` rule declares a Java annotation
        processor (identified by `processor_class`) that can be referenced
        from `java_library()` and `kotlin_library()` rules to run during
        compilation.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "deps": attrs.list(attrs.dep(), default = []),
            "does_not_affect_abi": attrs.bool(default = False),
            "isolate_class_loader": attrs.bool(default = False),
            "processor_class": attrs.string(default = ""),
            "supports_abi_generation_from_source": attrs.bool(default = False),
            "runs_on_java_only": attrs.bool(default = False),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
