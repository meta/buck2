# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifact_tset.bzl", "ArtifactInfoTag", "ArtifactTSet")
load(
    "@prelude//cxx:cuda.bzl",
    "CudaCompileStyle",
)
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "RuntimeDependencyHandling",
)
load(
    "@prelude//cxx:link_groups_types.bzl",
    "LinkGroupInfo",
)
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "SwiftmoduleLinkable",
)
load(
    "@prelude//linking:linkables.bzl",
    "LinkableProviders",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",
)
load("@prelude//linking:types.bzl", "LinkPreference")
load(":argsfiles.bzl", "CompileArgsfiles")
load(
    ":compile_types.bzl",
    "IndexStoreFactory",
    "UseHeaderUnitsMode",
)
load(
    ":cxx_sources.bzl",
    "CxxSrcWithFlags",
)
load(
    ":headers.bzl",
    "CxxHeadersLayout",
)
load(
    ":link_groups.bzl",
    "LinkGroupLibSpec",
)
load(
    ":linker.bzl",
    "SharedLibraryFlagOverrides",
)
load(
    ":preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorInfo",
)
load(
    ":xcode.bzl",
    "cxx_populate_xcode_attributes",
)

CxxLibraryInfo = provider(
    fields = dict(
        target = provider_field(Label),
        labels = provider_field(list[str]),
    ),
)

# Parameters to control which sub targets to define when processing Cxx rules.
# By default, generates all subtargets.
CxxRuleSubTargetParams = record(
    argsfiles = field(bool, True),
    compilation_database = field(bool, True),
    clang_remarks = field(bool, True),
    clang_llvm_statistics = field(bool, True),
    clang_traces = field(bool, True),
    headers = field(bool, True),
    link_group_map = field(bool, True),
    link_style_outputs = field(bool, True),
    xcode_data = field(bool, False),
    objects = field(bool, True),
    bitcode_bundle = field(bool, True),
    header_unit = field(bool, True),
)

def xcode_data_enabled() -> bool:
    # Project generators must manually enable xcode data because it measurably increases memory usage.
    return read_root_config("cxx", "enable_xcode_data", "false").lower() == "true"

# Parameters to control which providers to define when processing Cxx rules.
# By default, generates all providers.
CxxRuleProviderParams = record(
    compilation_database = field(bool, True),
    default = field(bool, True),
    java_packaging_info = field(bool, True),
    android_packageable_info = field(bool, True),
    java_global_code_info = field(bool, True),
    linkable_graph = field(bool, True),
    link_style_outputs = field(bool, True),
    merged_native_link_info = field(bool, True),
    omnibus_root = field(bool, True),
    preprocessors = field(bool, True),
    # Whether or not to generate a resource groups provider for raw cxx resources.
    cxx_resources_as_apple_resources = field(bool, True),
    resources = field(bool, True),
    shared_libraries = field(bool, True),
    template_placeholders = field(bool, True),
    preprocessor_for_tests = field(bool, True),
    third_party_build = field(bool, False),
    transitive_diagnostics = field(bool, False),
)

# Parameters to handle non-Clang sources, e.g Swift on Apple's platforms.
CxxRuleAdditionalParams = record(
    srcs = field(list[CxxSrcWithFlags], []),
    # Additional argsfiles to include for this rule.
    argsfiles = field(CompileArgsfiles, CompileArgsfiles()),
    # External debug info to be used when generated static output
    static_external_debug_info = field(list[ArtifactTSet], []),
    # External debug info to be used when generating shared objects
    shared_external_debug_info = field(list[ArtifactTSet], []),
    subtargets = field(dict, {}),  # [str: ["provider"]]
    # Might be used to expose additional providers to cxx layer (e.g to support #headers subtarget for Swift)
    additional_providers_factory = field([typing.Callable, None], None),  # ([CPreprocessorInfo, None]) -> ["provider"]:
    # The list of tags that should be applied to generated ArtifactTSet of debug information.
    external_debug_info_tags = field(list[ArtifactInfoTag], []),
)

# Parameters that allows to configure/extend generic implementation of C++ rules.
# Apple-specific rules (such as `apple_binary` and `apple_library`) and regular C++
# rules (such as `cxx_binary` and `cxx_library`) have too much in common, though
# some aspects of behavior (like layout of headers affecting inclusion of those
# or additional linking flags to support usage of platform frameworks) of are
# different and need to be specified. The following record holds the data which
# is needed to specialize user-facing rule from generic implementation.
CxxRuleConstructorParams = record(
    # Required
    # Name of the top level rule utilizing the cxx rule.
    rule_type = str,
    # Header layout to use importing headers.
    headers_layout = CxxHeadersLayout,
    # Optional
    # Whether to build an empty shared library. This is utilized for rust_python_extensions
    # so that they can link against the rust shared object.
    build_empty_so = field(bool, False),
    # If the rule is a test.
    is_test = field(bool, False),
    # Additional information used to preprocess every unit of translation in the rule.
    extra_preprocessors = field(list[CPreprocessor], []),
    extra_preprocessors_info = field(list[CPreprocessorInfo], []),
    # Additional preprocessor info to export to other rules.
    extra_exported_preprocessors = field(list[CPreprocessor], []),
    # Additional information used to link every object produced by the rule,
    # flags are _both_ exported and used to link the target itself.
    extra_exported_link_flags = field(list[typing.Any], []),
    # Additional hidden inputs for link or archive actions.
    extra_hidden = field(list[Artifact], []),
    extra_dwp_flags = field(list[typing.Any], []),
    # Additional flags used _only_ when linking the target itself.
    # These flags are _not_ propagated up the dep tree.
    extra_link_flags = field(list[typing.Any], []),
    extra_binary_link_flags = field(list[typing.Any], []),
    # Inputs that invalidate generated build info without becoming inputs to the executable link.
    generated_build_info_invalidation_inputs = field(list[typing.Any], []),
    # Additional artifacts to be linked together with the cxx compilation output.
    extra_link_input = field(list[Artifact], []),
    # If True the extra_link_input should be considered as external debug info.
    extra_link_input_has_external_debug_info = field(bool, False),
    # Additional args to be used to link the target.
    extra_link_args = field(list[LinkArgs], []),
    # The swift module linkable that should be included for linking,
    # used for link_groups and regular linking.
    swiftmodule_linkable = field([SwiftmoduleLinkable, None], None),
    # The source files to compile as part of this rule. This list can be generated
    # from ctx.attrs with the `get_srcs_with_flags` function.
    srcs = field(list[CxxSrcWithFlags]),
    additional = field(CxxRuleAdditionalParams, CxxRuleAdditionalParams()),
    # A function which enables the caller to inject subtargets into the link_style provider
    # as well as create custom providers based on the link styles.
    output_style_sub_targets_and_providers_factory = field(typing.Callable, lambda _link_style, _context, _output: ({}, [])),
    # Linker flags that tell the linker to create shared libraries, overriding the default shared library flags.
    # e.g. when building Apple tests, we want to link with `-bundle` instead of `-shared` to allow
    # linking against the bundle loader.
    shared_library_flags = field([SharedLibraryFlagOverrides, None], None),
    # Optional argument to override the default name of the shared object being produced.
    soname = field([str, None], None),
    # Optional argument to override the default name of the executable being produced.
    executable_name = field([str, None], None),
    # Optional argument to set the deffile for the windows linker on a dll
    deffile = field([Artifact, None], None),
    # If passed to cxx_executable, this field will be used to determine
    # a shared subtarget's default output should be stripped.
    strip_executable = field(bool, False),
    strip_args_factory = field(typing.Callable, lambda _: cmd_args()),
    # Whether to embed the library name as the SONAME.
    use_soname = field(bool, True),
    # Use link group's linking logic regardless whether a link group map's present.
    force_link_group_linking = field(bool, False),
    # Function to use for setting Xcode attributes for the Xcode data sub target.
    cxx_populate_xcode_attributes_func = field(typing.Callable, cxx_populate_xcode_attributes),
    # Define which sub targets to generate.
    generate_sub_targets = field(CxxRuleSubTargetParams, CxxRuleSubTargetParams()),
    # Define which providers to generate.
    generate_providers = field(CxxRuleProviderParams, CxxRuleProviderParams()),
    force_full_hybrid_if_capable = field(bool, False),
    force_remote_link_if_capable = field(bool, False),
    # Whether shared libs for executables should generate a shared lib link tree.
    exe_shared_libs_link_tree = field(bool, True),
    extra_link_deps = field(list[LinkableProviders], []),
    # Additional link roots (e.g. dlopen-able libs in a native python link),
    # other than the main executable, which can influence the C++ executable
    # result (e.g. added to link group linking or needs to be searched for
    # shared libs to include in the symlink tree).
    extra_link_roots = field(list[LinkableProviders], []),
    # Additional shared libs to "package".
    extra_shared_libs = field(list[SharedLibrary], []),
    auto_link_group_specs = field([list[LinkGroupLibSpec], None], None),
    link_group_info = field([LinkGroupInfo, None], None),
    # Whether to use pre-stripped objects when linking.
    prefer_stripped_objects = field(bool, False),
    # The category suffix to use for executables actions (e.g. linking).
    exe_category_suffix = field(str, "executable"),
    # Whether link groups liking should make `preferred_linkage = "static"` libs
    # "follow" their dependents across link group boundaries.
    link_groups_force_static_follows_dependents = field(bool, True),
    # A factory function to produce extra artifacts and output providers for a rule
    # with signature: f(ctx, ExtraLinkerOutputsCategory) -> ExtraLinkerOutputs
    extra_linker_outputs_factory = field(typing.Callable | None, None),
    # A factory function to produce linker flags for the extra linker outputs
    # returned from the extra_linker_outputs_factory. It should have the signature
    # f(ctx, ExtraLinkerOutputCategory, dict[str, Artifact]) -> list[ArgLike]
    extra_linker_outputs_flags_factory = field(typing.Callable | None, None),
    # A function to consume extra outputs produced by distributed thin-lto opt actions
    # and merge them together, as if they were produced by a local thin-lto link.
    # The signature should be:
    # f(ctx, dict[str, Artifact], list[dict[str, Artifact]])
    # The second parameter should be the artifacts field of a call to
    # extra_linker_outputs_factory(ctx, ExtraLinkerOutputsCategory("produced-during-distributed-thin-lto-opt")),
    # and the last parameter a list of matching dictionaries representing all the opt outputs
    # to be merged to bind the final outputs.
    extra_distributed_thin_lto_opt_outputs_merger = field(typing.Callable | None, None),
    # Whether to allow cache uploads for locally-executed actions (except for linking, see "exe_allow_cache_upload").
    allow_cache_upload = field(bool, False),
    # Whether to allow cache uploads for locally-linked executables.
    exe_allow_cache_upload = field(bool, False),
    # Extra shared library interfaces to propagate, eg from mixed Swift libraries.
    extra_shared_library_interfaces = field([list[Artifact], None], None),
    # Compiler flags
    compiler_flags = field(list[typing.Any], []),
    cxx_flags = field(list[Dependency], []),
    lang_compiler_flags = field(dict[typing.Any, typing.Any], {}),
    # Preprocessor flags
    preprocessor_flags = field(list[typing.Any], []),
    lang_preprocessor_flags = field(dict[typing.Any, typing.Any], {}),
    # modulename-Swift.h header for building objc targets that rely on this swift dep
    swift_objc_header = field([Artifact, None], None),
    error_handler = field([typing.Callable, None], None),
    index_store_factory = field(IndexStoreFactory | None, None),
    # Swift index stores to propagate
    index_stores = field(list[Artifact] | None, None),
    # Modularization dependency graph artifact (from Swift compilation)
    modularization_dependency_graph = field(Artifact | None, None),
    # Whether to add header units from dependencies to the command line.
    use_header_units = field(UseHeaderUnitsMode, UseHeaderUnitsMode("none")),
    # Whether to export a header unit to all dependents.
    export_header_unit = field([str, None], None),
    # Filter what headers to include in header units.
    export_header_unit_filter = field(list[str], []),
    # Additional behavior for how to handle runtime dependencies
    runtime_dependency_handling = field([RuntimeDependencyHandling, None], None),
    # Should this library only be used for build time linkage
    stub = field(bool, False),
    # The calling context is allowed to use `AnalysisActions#anon_target` API.
    # This is not allowed in the context of the `dynamic_outputs` callback.
    anon_targets_allowed = field(bool, True),
    # Any extra diagnostics to include in transitive diagnostics provider
    extra_transitive_diagnostics = field(list[Artifact], []),
    # Any extra diagnostics to include in [check] subtarget, maps from
    # identifier (usually filename) to diagnostic output.
    extra_diagnostics = field(dict[str, Artifact] | None, None),
    # Whether to use fbcc Rust wrapper
    use_fbcc_rust_wrapper = field(bool, False),
    # Precompiled header
    precompiled_header = field(Dependency | None, None),
    # Prefix header
    prefix_header = field(Artifact | None, None),
    # Store "_cxx_toolchain" as "Dependency" for use in "anon_target"
    _cxx_toolchain = field(Dependency | None, None),
    # Use content-based filepaths for artifacts
    use_content_based_paths = field(bool, False),
    # Coverage instrumentation compiler flags
    coverage_instrumentation_compiler_flags = field(list[str], []),
    # Optional profile list artifact for selective coverage via -fprofile-list
    coverage_profile_list = field(Artifact | None, None),
    coverage_profile_list_srcs = field(list[str] | None, None),
    # Separate debug info
    separate_debug_info = field(bool, False),
    # Cuda compile stype
    cuda_compile_style = field(CudaCompileStyle | None, None),
    link_preference = field(LinkPreference, LinkPreference("default")),
    supports_stripping = field(bool, True),
    # Whether to set expect_eligible_for_dedupe on compile actions.
    expect_eligible_for_dedupe = field(bool, False),
    target_stats_cycle_mode = field(str, "none"),
    target_stats_extra_srcs = field(dict[str, Artifact], {}),
    target_stats_swift_dot = field(Artifact | None, None),
    # The module the cycle tools resolve imports against. Not how the target is
    # reported -- that stays the target's label.
    target_stats_module_name = field(str | None, None),
)
