# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifact_tset.bzl", "make_artifact_tset")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_library.bzl", "cxx_compile_srcs")
load("@prelude//cxx:cxx_sources.bzl", "CxxSrcWithFlags")
load("@prelude//cxx:cxx_types.bzl", "CxxRuleConstructorParams")
load("@prelude//cxx:headers.bzl", "CxxHeadersLayout", "CxxHeadersNaming")
load("@prelude//cxx:link.bzl", "cxx_link_shared_library")
load("@prelude//cxx:link_types.bzl", "link_options")
load("@prelude//cxx:shared_library_interface.bzl", "shared_library_interface")
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference")
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "LinkInfo",
    "ObjectsLinkable",
    "SharedLibLinkable",
    "unpack_link_args_metadata",
    "wrap_with_no_as_needed_shared_libs_flags",
)
load("@prelude//linking:shared_libraries.bzl", "SharedLibrary", "create_shlib")
load("@prelude//linking:types.bzl", "Linkage")

GeneratedBuildInfo = record(
    json = Artifact,
    linker_flags = list[typing.Any],
    manifest_entries = field(Artifact | None, None),
    source = Artifact,
)

GeneratedBuildInfoData = record(
    json = Artifact,
    manifest_entries = field(Artifact | None, None),
)

GeneratedBuildInfoCompileOutput = record(
    external_debug_info = list[Artifact],
    objects = list[Artifact],
)

# Action-output-relative dir for generated build-info artifacts. Consumers use
# it to locate the linked DSO next to the binaries that link it.
GENERATED_BUILD_INFO_OUTPUT_DIR = "__generated_build_info__"

BUILD_INFO_DSO_SONAME = "libgenerated_build_info.so"

# Generated C source name stem; an output suffix (e.g. "-real" for a second
# generation in the same context) is appended as "_real".
_GENERATED_BUILD_INFO_SOURCE = "__buck2_generated_build_info"

_REQUIRED_GENERATED_BUILD_INFO_FIELDS = [
    "allow_cache_upload",
    "base_linker_flags",
    "enabled",
    "exported_symbols",
    "final_linker_flags",
    "finalize_build_info_at_link",
    "local_only",
]

GeneratedBuildInfoInvalidationInfo = provider(
    fields = {
        "inputs": provider_field(typing.Any),
    },
)

GeneratedBuildInfoSharedLibrary = record(
    json = Artifact,
    library = SharedLibrary,
    link_args = LinkArgs,
    manifest_entries = field(Artifact | None, None),
)

def compile_generated_build_info(ctx: AnalysisContext, info: GeneratedBuildInfo) -> GeneratedBuildInfoCompileOutput:
    cxx_toolchain_info = get_cxx_toolchain_info(ctx)

    # Derive the output prefix from the generated source's name: a context
    # that compiles build info twice (placeholder plus its real replacement)
    # already gives the sources distinct names (see _GENERATED_BUILD_INFO_SOURCE),
    # and the compile outputs (argsfiles, objects) must differ along with them.
    source_suffix = info.source.short_path.rsplit("/", 1)[-1].removesuffix(".c").split(_GENERATED_BUILD_INFO_SOURCE, 1)[1]
    filename_prefix = "generated_build_info" + source_suffix + "_"

    # `-fno-whole-program-vtables` is a Clang-only flag; GCC errors out on it.
    src_flags = ["-fno-lto"]
    if cxx_toolchain_info.c_compiler_info.compiler_type in ["clang", "clang_windows", "clang_cl"]:
        src_flags = ["-fno-whole-program-vtables"] + src_flags

    compiled = cxx_compile_srcs(
        actions = ctx.actions,
        target_label = ctx.label,
        cxx_toolchain_info = cxx_toolchain_info,
        impl_params = CxxRuleConstructorParams(
            rule_type = "generated_build_info",
            headers_layout = CxxHeadersLayout(
                namespace = "",
                naming = CxxHeadersNaming("regular"),
            ),
            srcs = [CxxSrcWithFlags(file = info.source, flags = src_flags)],
            _cxx_toolchain = ctx.attrs._cxx_toolchain,
        ),
        own_preprocessors = [],
        inherited_non_exported_preprocessor_infos = [],
        inherited_exported_preprocessor_infos = [],
        preferred_linkage = Linkage("shared"),
        add_coverage_instrumentation_compiler_flags = False,
        filename_prefix = filename_prefix,
    )
    return GeneratedBuildInfoCompileOutput(
        external_debug_info = (compiled.pic.external_debug_info + (compiled.pic.objects if compiled.pic.objects_have_external_debug_info else [])),
        objects = compiled.pic.objects,
    )

# Expected `_generated_build_info_spec` shape:
# {
#     "<generator-owned-field>": <JSON-compatible value>,
# }
# Generator-owned fields remain at the top level so configured selectors are
# resolved before the specification is serialized.
def _generated_build_info_config(ctx: AnalysisContext, stable_placeholder: bool = False):
    spec = dict(getattr(ctx.attrs, "_generated_build_info_spec", {}))
    configured_enabled = getattr(ctx.attrs, "_generated_build_info_enabled", None)
    if configured_enabled != None:
        if not configured_enabled:
            return None
        if not spec:
            fail("generated build-info spec is missing")

        mode = "stable" if stable_placeholder else ctx.attrs._generated_build_info_mode
        spec["allow_cache_upload"] = mode != "full"
        spec["build_info"] = mode
        spec["enabled"] = True
        spec["final_linker_flags"] = ["--build-info={}".format(mode)]
        spec["local_only"] = mode == "full"
    elif not spec or not spec.get("enabled", False):
        return None

    missing_fields = [field for field in _REQUIRED_GENERATED_BUILD_INFO_FIELDS if field not in spec]
    if missing_fields:
        fail("generated build-info spec is missing required fields: {}".format(", ".join(missing_fields)))

    tool = getattr(ctx.attrs, "_gen_build_info", None)
    if tool == None:
        fail("_gen_build_info must be set when generated build info is enabled")
    return (spec, tool[RunInfo])

def _generate_build_info_data(
    ctx: AnalysisContext,
    spec,
    tool: RunInfo,
    generator_args: list[typing.Any] = [],
    invalidation_inputs: list[typing.Any] = [],
    output_suffix: str = "",
) -> GeneratedBuildInfoData:
    output_dir = GENERATED_BUILD_INFO_OUTPUT_DIR + output_suffix
    json = getattr(ctx.attrs, "_generated_build_info_data", None)
    if json != None and spec.get("emit_manifest_entries", False):
        fail("supplied generated build-info data does not provide manifest entries")
    manifest_entries = None
    # A caller that supplies the JSON artifact owns its invalidation edges;
    # `invalidation_inputs` apply only when this rule generates the JSON.
    if json == None:
        generator_spec = ctx.actions.write_json(
            output_dir + "/generator_spec.json",
            spec,
        )
        json = ctx.actions.declare_output(output_dir, "build_info.json")
        manifest_entries = ctx.actions.declare_output(output_dir, "manifest_entries.json") if spec.get("emit_manifest_entries", False) else None
        command = cmd_args(
            tool,
            generator_args,
            "--spec-json",
            generator_spec,
            "--output-json",
            json.as_output(),
        )
        if manifest_entries != None:
            command.add("--output-manifest-entries", manifest_entries.as_output())
        command.add(cmd_args(hidden = invalidation_inputs, experimental_salt_hidden = True))
        ctx.actions.run(
            command,
            category = "generate_build_info_json",
            env = {"LC_ALL": "C"},
            identifier = output_suffix.removeprefix("-") if output_suffix else None,
            local_only = spec["local_only"],
            allow_cache_upload = spec["allow_cache_upload"],
        )
    return GeneratedBuildInfoData(
        json = json,
        manifest_entries = manifest_entries,
    )

def generate_build_info_data(
    ctx: AnalysisContext,
    generator_args: list[typing.Any] = [],
    invalidation_inputs: list[typing.Any] = [],
    stable_placeholder: bool = False,
    output_suffix: str = "",
) -> GeneratedBuildInfoData | None:
    config = _generated_build_info_config(ctx, stable_placeholder = stable_placeholder)
    if config == None:
        return None
    spec, tool = config
    return _generate_build_info_data(
        ctx,
        spec,
        tool,
        generator_args = generator_args,
        invalidation_inputs = invalidation_inputs,
        output_suffix = output_suffix,
    )

# Expected `_generated_build_info_spec` shape:
# {
#     "<action-owned-field>": <JSON-compatible value>,
# }
# Generator arguments are supplied separately so callers can reuse existing
# configurable argument lists without expanding them into this dictionary.
def generate_build_info(
    ctx: AnalysisContext,
    invalidation_inputs: list[typing.Any] = [],
    generator_args: list[typing.Any] = [],
    output_suffix: str = "",
) -> GeneratedBuildInfo | None:
    # No stable-placeholder variant here: a statically linked object would
    # bake the placeholder values into the binary with no later swap. The
    # placeholder only exists for the shared-library form, where the link
    # consumes nothing but the DSO's interface.
    config = _generated_build_info_config(ctx)
    if config == None:
        return None
    spec, tool = config
    return _generate_build_info(ctx, spec, tool, generator_args, invalidation_inputs, output_suffix)

def _generate_build_info(
    ctx: AnalysisContext,
    spec,
    tool: RunInfo,
    generator_args: list[typing.Any] = [],
    invalidation_inputs: list[typing.Any] = [],
    output_suffix: str = "",
) -> GeneratedBuildInfo:
    data = _generate_build_info_data(ctx, spec, tool, generator_args, invalidation_inputs, output_suffix = output_suffix)

    output_dir = GENERATED_BUILD_INFO_OUTPUT_DIR + output_suffix
    # The object output is named after the source basename, so a second
    # generation in the same context (a placeholder inside a link plus its
    # real replacement) needs a distinct source name.
    source = ctx.actions.declare_output(output_dir, "{}{}.c".format(_GENERATED_BUILD_INFO_SOURCE, output_suffix.replace("-", "_")))

    ctx.actions.run(
        cmd_args(
            tool,
            "--input-json",
            data.json,
            "--output-source",
            source.as_output(),
        ),
        category = "generate_build_info_source",
        identifier = output_suffix.removeprefix("-") if output_suffix else None,
    )

    json_linker_flags = (
        (spec.get("final_linker_flags", []) + [cmd_args("--build-info-json=", data.json, delimiter = "")])
        if spec.get("finalize_build_info_at_link", True)
        else []
    )
    return GeneratedBuildInfo(
        json = data.json,
        linker_flags = json_linker_flags + spec["base_linker_flags"],
        manifest_entries = data.manifest_entries,
        source = source,
    )

def generate_build_info_shared_library(
    ctx: AnalysisContext,
    metadata_link_args: LinkArgs | list[LinkArgs],
    generator_args: list[typing.Any] = [],
    invalidation_inputs: list[typing.Any] = [],
    stable_placeholder: bool = False,
    output_suffix: str = "",
) -> GeneratedBuildInfoSharedLibrary | None:
    config = _generated_build_info_config(ctx, stable_placeholder = stable_placeholder)
    if config == None:
        return None
    spec, _tool = config
    tool = _tool
    if stable_placeholder:
        # Stable generation ignores per-build `-c build_info.*` overrides,
        # so the config-baked tool would re-run on every host change with
        # identical outputs. The bare tool keeps the action host-independent.
        stable_tool = getattr(ctx.attrs, "_gen_build_info_no_config", None)
        if stable_tool != None:
            tool = stable_tool[RunInfo]

    # Callers that regenerate the DSO outside the original link (e.g. the
    # python_binary rule regenerating it outside its anon native-linking
    # target) only have the executable's combined link args, not the
    # dep-only `LinkArgs` the link itself used.
    if type(metadata_link_args) != type([]):
        metadata_link_args = [metadata_link_args]
    metadata_args = cmd_args(
        [unpack_link_args_metadata(args) for args in metadata_link_args],
    )

    info = _generate_build_info(
        ctx,
        spec,
        tool,
        output_suffix = output_suffix,
        generator_args = generator_args
        + [
            cmd_args(
                metadata_args,
                format = "--build-info-link-metadata={}",
            ),
        ],
        invalidation_inputs = invalidation_inputs,
    )

    linker_info = get_cxx_toolchain_info(ctx).linker_info
    soname = BUILD_INFO_DSO_SONAME
    compile_output = compile_generated_build_info(ctx, info)
    exported_symbols = spec["exported_symbols"]
    links = [
        LinkArgs(
            infos = [
                LinkInfo(
                    external_debug_info = make_artifact_tset(
                        actions = ctx.actions,
                        label = ctx.label,
                        artifacts = compile_output.external_debug_info,
                    ),
                    pre_flags = info.linker_flags + ["-Wl,--export-dynamic-symbol={}".format(symbol) for symbol in exported_symbols],
                    linkables = [
                        ObjectsLinkable(
                            objects = compile_output.objects,
                            linker_type = linker_info.type,
                            link_whole = True,
                        ),
                    ],
                ),
            ],
        ),
        LinkArgs(
            flags = cmd_args(
                metadata_args,
                format = "--build-info-link-metadata={}",
            ),
        ),
    ]
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = "{}/{}".format(GENERATED_BUILD_INFO_OUTPUT_DIR + output_suffix, soname),
        name = soname,
        opts = link_options(
            links = links,
            link_execution_preference = LinkExecutionPreference("any"),
            link_weight = linker_info.link_weight,
            identifier = "generated_build_info_shared_library{}".format(output_suffix) if output_suffix else None,
        ),
    )
    interface = shared_library_interface(ctx, link_result.linked_object.output)
    link_info = wrap_with_no_as_needed_shared_libs_flags(
        linker_info.type,
        LinkInfo(linkables = [SharedLibLinkable(lib = interface)]),
    )
    return GeneratedBuildInfoSharedLibrary(
        json = info.json,
        library = create_shlib(
            label = ctx.label,
            lib = link_result.linked_object,
            soname = soname,
        ),
        link_args = LinkArgs(infos = [link_info]),
        manifest_entries = info.manifest_entries,
    )

def generated_build_info_is_shared_library(ctx: AnalysisContext) -> bool:
    if not getattr(ctx.attrs, "build_info_as_shared_library", True):
        return False
    spec = getattr(ctx.attrs, "_generated_build_info_spec", {})
    configured_enabled = getattr(ctx.attrs, "_generated_build_info_enabled", None)
    enabled = configured_enabled if configured_enabled != None else spec.get("enabled", False)
    return bool(enabled and spec.get("link_as_shared_library", False))
