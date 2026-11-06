# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//android:android_providers.bzl",
    "AndroidBinaryNativeLibsInfo",
    "AndroidPackageableInfo",
    "ExopackageNativeInfo",
    "PrebuiltNativeLibraryDir",  # @unused Used as a type
)
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:cpu_filters.bzl", "CPU_FILTER_FOR_PRIMARY_PLATFORM", "CPU_FILTER_TO_ABI_DIRECTORY")
load(
    "@prelude//android:native_build_commands.bzl",
    "EMIT_NATIVE_BUILD_COMMANDS",
    "GATORADE_PHASE_SUBTARGETS",
    "NATIVE_BUILD_COMMAND_KINDS",
    "native_build_command_entry",
    "record_link_command",
)
load("@prelude//android:relinker_linker_outputs.bzl", "get_extra_relinker_args")
load("@prelude//android:util.bzl", "EnhancementContext", "merge_extra_linker_args")
load("@prelude//android:voltron.bzl", "ROOT_MODULE", "all_targets_in_root_module", "get_apk_module_graph_info", "is_root_module")
# @oss-disable[end= ]: load(
    # @oss-disable[end= ]: "@prelude//android/meta_only:gatorade.bzl",
    # @oss-disable[end= ]: "add_gatorade_relinker_args",
    # @oss-disable[end= ]: "early_gatorade_libraries",
    # @oss-disable[end= ]: "gatorade_deferred_libs",
    # @oss-disable[end= ]: "gatorade_libraries",
    # @oss-disable[end= ]: "is_late_gatorade_enabled",
    # @oss-disable[end= ]: "is_middle_gatorade_enabled",
    # @oss-disable[end= ]: "is_monogatorade_enabled",
    # @oss-disable[end= ]: "middle_gatorade_merge_args",
    # @oss-disable[end= ]: "monogatorade_whole_app",
    # @oss-disable[end= ]: "relink_for_native_libs",
# @oss-disable[end= ]: )
# @oss-disable[end= ]: load(
    # @oss-disable[end= ]: "@prelude//android/meta_only:native_link_diagnostics.bzl",
    # @oss-disable[end= ]: "declare_native_link_diagnostics",
    # @oss-disable[end= ]: "materialize_native_link_diagnostics",
    # @oss-disable[end= ]: "native_link_diagnostic_reports",
# @oss-disable[end= ]: )
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo", "PicBehavior", "ShlibInterfacesMode")
load(
    "@prelude//cxx:link.bzl",
    "cxx_link_shared_library",
)
load("@prelude//cxx:link_types.bzl", "link_options")
load(
    "@prelude//cxx:symbols.bzl",
    "extract_defined_syms",
    "extract_undefined_syms",
)
load("@prelude//java:java_library.bzl", "compile_to_jar")  # @unused
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference", "get_action_execution_attributes")
load(
    "@prelude//linking:link_info.bzl",
    "ArchiveLinkable",
    "LibOutputStyle",
    "LinkArgs",
    "LinkInfo",
    "LinkOrdering",
    "LinkedObject",
    "ObjectsLinkable",
    "SharedLibLinkable",
    "get_lib_output_style",
    "set_link_info_link_whole",
    "unpack_link_args",
    "wrap_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",  # @unused Used as a type
    "LinkableNode",  # @unused Used as a type
    "create_linkable_graph",
    "get_linkable_graph_node_map_func",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",  # @unused Used as a type
    "SharedLibraryInfo",  # @unused Used as a type
    "create_shlib",
    "get_strip_non_global_flags",
    "merge_shared_libraries",
    "traverse_shared_library_info",
    "with_unique_str_sonames",
)
load("@prelude//linking:strip.bzl", "strip_object")
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//utils:argfile.bzl", "argfile")
load("@prelude//utils:arglike.bzl", "ArgLike")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:graph_utils.bzl", "post_order_traversal", "pre_order_traversal", "rust_matching_topological_traversal")
load("@prelude//utils:utils.bzl", "dedupe_by_value")

_GATORADE_PHASE_ORDER = ["early", "middle", "late"]

# Native libraries on Android are built for a particular Application Binary Interface (ABI). We
# package native libraries for one (or more, for multi-arch builds) ABIs into an Android APK.
#
# Our native libraries come from two sources:
# 1. "Prebuilt native library dirs", which are directory artifacts whose sub-directories are ABIs,
#    and those ABI subdirectories contain native libraries. These come from `android_prebuilt_aar`s
#    and `prebuilt_native_library`s, for example.
# 2. "Native linkables". These are each a single shared library - `.so`s for one particular ABI.
#
# Native libraries can be packaged into Android APKs in two ways.
# 1. As native libraries. This means that they are passed to the APK builder as native libraries,
#    and the APK builder will package `<ABI>/library.so` into the APK at `libs/<ABI>/library.so`.
# 2. As assets. These are passed to the APK build as assets, and are stored at
#    `assets/lib/<ABI>/library.so` In the root module, we only package a native library as an
#    asset if it is eligible to be an asset (e.g. `can_be_asset` on a `cxx_library`), and
#    `package_asset_libraries` is set to True for the APK. We create a metadata file at
#    `assets/libs/metadata.txt` that has a single line entry for each packaged asset consisting of
#    '<ABI/library_name> <file_size> <sha256>'.
#
#    Native libraries that are not part of the root module (i.e. it is part of some other Voltron
#    module) are controlled by `package_voltron_asset_libraries` and `can_be_asset` flags.
#    Similarly, the metadata for each module is stored at `assets/<module_name>/libs.txt`.

def _merged_lib_provided_by_apk_under_test(merged_lib, shared_libraries_to_exclude: set) -> bool:
    # True when every primary constituent of a merged library is provided by the
    # apk-under-test, so the instrumentation APK can drop its own copy and resolve
    # the library at runtime from the apk-under-test instead.
    if not shared_libraries_to_exclude or not merged_lib.primary_constituents:
        return False
    for constituent in merged_lib.primary_constituents:
        if constituent.raw_target() not in shared_libraries_to_exclude:
            return False
    return True

# [relinked_libs] and [relinked_libs_manifest] when no relink is deferred: there is nothing to swap
# in after packaging, so the step that does the swap (relinked_libs_combine_rule) copies its input.
def _register_empty_relinked_libs(enhance_ctx: EnhancementContext) -> None:
    actions = enhance_ctx.ctx.actions
    enhance_ctx.debug_output("relinked_libs", actions.symlinked_dir("relinked_libs", {}, has_content_based_path = False))
    enhance_ctx.debug_output("relinked_libs_manifest", actions.write_json("relinked_libs_manifest.json", [], has_content_based_path = False))

def _register_gatorade_phase_evidence(
    enhance_ctx: EnhancementContext,
    platforms: list[str],
    native_library_merge_dir: Artifact | None,
    middle_gatorade_products: Artifact | None,
    unstripped_native_libraries_files: Artifact,
    relinked_libs_output: Artifact | None,
    defer_relink: bool,
) -> None:
    ctx = enhance_ctx.ctx
    configured = getattr(ctx.attrs, "gatorade_phases", [])
    configured_phases = [phase for phase in _GATORADE_PHASE_ORDER if phase in configured]
    phase_outputs = {}
    phase_dependencies = {}
    if "early" in configured_phases and native_library_merge_dir != None:
        phase_outputs["early"] = native_library_merge_dir
        phase_dependencies["early"] = "merge_sequence"
    if "middle" in configured_phases and middle_gatorade_products != None:
        phase_outputs["middle"] = middle_gatorade_products
        phase_dependencies["middle"] = "middle_products"
    if "late" in configured_phases:
        phase_outputs["late"] = relinked_libs_output if defer_relink else unstripped_native_libraries_files
        phase_dependencies["late"] = "relinked_libraries" if defer_relink else "final_unstripped_libraries"
    if not phase_outputs:
        return

    evidence = ctx.actions.declare_output(
        "gatorade_phase_evidence.json",
        has_content_based_path = False,
    )
    # Preserve the associated inputs so building this manifest also materializes
    # the phase products it certifies.
    evidence_inputs = ctx.actions.write_json(
        evidence,
        {
            "configured_phases": configured_phases,
            "defer_relink": defer_relink,
            "engaged_phases": [phase for phase in _GATORADE_PHASE_ORDER if phase in phase_outputs],
            "phase_dependencies": phase_dependencies,
            "phase_outputs": phase_outputs,
            "platforms": sorted(platforms),
            "schema_version": 1,
            "target": str(ctx.label.raw_target()),
        },
        pretty = True,
        with_inputs = True,
    )
    enhance_ctx.debug_output(
        "gatorade_phase_evidence",
        evidence,
        other_outputs = [evidence_inputs],
    )

def _materialize_middle_gatorade_products(
    ctx: AnalysisContext,
    output,
    products_by_platform: dict[str, Artifact],
    relinked_libraries_by_platform: dict[str, dict[str, SharedLibrary]],
) -> None:
    entries = {}
    for platform in sorted(products_by_platform):
        entries["{}/products".format(platform)] = products_by_platform[platform]
        relinked_libraries = relinked_libraries_by_platform[platform]
        for soname in sorted(relinked_libraries):
            entries["{}/relinked/{}".format(platform, soname)] = relinked_libraries[soname].lib.output
    ctx.actions.symlinked_dir(output, entries)

def get_android_binary_native_library_info(
    enhance_ctx: EnhancementContext,
    android_packageable_info: AndroidPackageableInfo,
    deps_by_platform: dict[str, list[Dependency]],
    apk_module_graph_file: Artifact | None = None,
    prebuilt_native_library_dirs_to_exclude: set[TargetLabel] = set(),
    shared_libraries_to_exclude: set[TargetLabel] = set(),
    native_library_merge_sequence: list | None = None,
    native_library_merge_glue: dict | None = None,
    native_library_merge_map: dict | None = None,
    native_library_merge_non_asset_libs: bool = False,
    native_library_merge_linker_args: dict | None = None,
    native_library_merge_linker_args_all: list | None = None,
    native_library_merge_code_generator: Dependency | None = None,
    native_library_merge_sequence_blocklist: list | None = None,
) -> AndroidBinaryNativeLibsInfo:
    ctx = enhance_ctx.ctx

    traversed_prebuilt_native_library_dirs = (
        android_packageable_info.prebuilt_native_library_dirs.traverse() if android_packageable_info.prebuilt_native_library_dirs else []
    )
    all_prebuilt_native_library_dirs = [
        native_lib for native_lib in traversed_prebuilt_native_library_dirs if native_lib.raw_target not in prebuilt_native_library_dirs_to_exclude
    ]

    included_shared_lib_targets = []
    original_shared_libs_by_platform = {}  # dict[str, dict[str (soname), list[SharedLibrary]]]
    for platform, deps in deps_by_platform.items():
        if platform == CPU_FILTER_FOR_PRIMARY_PLATFORM and platform not in ctx.attrs.cpu_filters and len(ctx.attrs.cpu_filters) != 0:
            continue

        shared_libs = get_default_shared_libs(ctx, deps, shared_libraries_to_exclude)
        included_shared_lib_targets.extend([lib.label.raw_target() for lib in shared_libs.values()])
        original_shared_libs_by_platform[platform] = shared_libs

    if not all_prebuilt_native_library_dirs and not included_shared_lib_targets:
        enhance_ctx.debug_output("native_libs", ctx.actions.write("native_libs", [], has_content_based_path = False))
        enhance_ctx.debug_output("linker_argsfiles", ctx.actions.symlinked_dir("linker_argsfiles", {}, has_content_based_path = False))
        enhance_ctx.debug_output("linker_commands", ctx.actions.write("linker_commands", [], has_content_based_path = False))
        enhance_ctx.debug_output(
            "native_build_commands",
            ctx.actions.write_json("native_build_commands", [], has_content_based_path = False),
            # Expose the same empty per-kind filter sub_targets as the main path so
            # TARGET[native_build_commands][<kind>] resolves uniformly (to empty JSON) even on
            # apps with no native libraries.
            sub_targets = {
                kind: [DefaultInfo(default_outputs = [ctx.actions.write_json("native_build_commands.{}.json".format(kind), [], has_content_based_path = False)])]
                for kind in NATIVE_BUILD_COMMAND_KINDS
            },
        )
        # The top-level Gatorade-phase sub-targets output that phase's produced artifacts (an empty
        # dir here: an app with no native libraries runs no Gatorade phase), so they resolve
        # uniformly. See the main path for the populated case.
        for gatorade_phase in GATORADE_PHASE_SUBTARGETS:
            enhance_ctx.debug_output(gatorade_phase, ctx.actions.symlinked_dir(gatorade_phase, {}, has_content_based_path = False))
        enhance_ctx.debug_output("unstripped_native_libraries", ctx.actions.write("unstripped_native_libraries", [], has_content_based_path = False))
        enhance_ctx.debug_output(
            "unstripped_native_libraries_json", ctx.actions.write_json("unstripped_native_libraries_json", {}, has_content_based_path = False)
        )
        enhance_ctx.debug_output(
            "unstripped_native_libraries_files", ctx.actions.symlinked_dir("unstripped_native_libraries_files", {}, has_content_based_path = False)
        )
        enhance_ctx.debug_output("relinker_extra_outputs", ctx.actions.symlinked_dir("relinker_extra_outputs", {}, has_content_based_path = False))
        _register_empty_relinked_libs(enhance_ctx)
        return AndroidBinaryNativeLibsInfo(
            prebuilt_native_library_dirs = [],
            shared_libraries = [],
            native_libs_for_primary_apk = [],
            exopackage_info = None,
            root_module_native_lib_assets = [],
            non_root_module_native_lib_assets = [],
            generated_java_code = [],
            unstripped_shared_libraries = None,
            validation_outputs = [],
        )

    native_libs = ctx.actions.declare_output("native_libs_symlink", has_content_based_path = False)
    native_libs_metadata = ctx.actions.declare_output("native_libs_metadata_symlink", has_content_based_path = False)
    native_libs_always_in_primary_apk = ctx.actions.declare_output("native_libs_always_in_primary_apk_symlink", has_content_based_path = False)
    native_lib_assets_for_primary_apk = ctx.actions.declare_output("native_lib_assets_for_primary_apk_symlink", has_content_based_path = False)
    stripped_native_linkable_assets_for_primary_apk = ctx.actions.declare_output(
        "stripped_native_linkable_assets_for_primary_apk_symlink", has_content_based_path = False
    )
    root_module_metadata_assets = ctx.actions.declare_output("root_module_metadata_assets_symlink", has_content_based_path = False)
    non_root_module_metadata_assets = ctx.actions.declare_output("non_root_module_metadata_assets_symlink", has_content_based_path = False)
    non_root_module_lib_assets = ctx.actions.declare_output("non_root_module_lib_assets_symlink", has_content_based_path = False)

    linker_argsfiles = ctx.actions.declare_output("linker_argsfiles", dir = True, has_content_based_path = False)
    linker_commands = ctx.actions.declare_output("linker_commands", has_content_based_path = False)
    unstripped_native_libraries = ctx.actions.declare_output("unstripped_native_libraries", has_content_based_path = False)
    unstripped_native_libraries_json = ctx.actions.declare_output("unstripped_native_libraries_json", has_content_based_path = False)
    unstripped_native_libraries_files = ctx.actions.declare_output("unstripped_native_libraries.links", dir = True, has_content_based_path = False)

    dynamic_outputs = [
        native_libs,
        native_libs_metadata,
        native_libs_always_in_primary_apk,
        native_lib_assets_for_primary_apk,
        linker_argsfiles,
        linker_commands,
        unstripped_native_libraries,
        unstripped_native_libraries_json,
        unstripped_native_libraries_files,
        stripped_native_linkable_assets_for_primary_apk,
        root_module_metadata_assets,
        non_root_module_metadata_assets,
        non_root_module_lib_assets,
    ]

    # [native_build_commands] outputs. `native_build_commands` (full) + the per-kind filters are
    # produced by a final combine action (see below) that merges two fragments: the "base" fragment
    # (all commands known by the end of the outer dynamic lambda) and the late-Gatorade "codegen"
    # fragment. The codegen commands (clang --codegen-library) are constructed inside a NESTED
    # Gatorade dynamic that runs after the outer lambda, so they cannot be folded in by the outer
    # lambda directly and instead flow through this fragment + combine.
    native_build_commands = ctx.actions.declare_output("native_build_commands.json", has_content_based_path = False)
    native_build_commands_by_kind = {
        kind: ctx.actions.declare_output("native_build_commands.{}.json".format(kind), has_content_based_path = False) for kind in NATIVE_BUILD_COMMAND_KINDS
    }
    native_build_commands_base = ctx.actions.declare_output("native_build_commands.base.json", has_content_based_path = False)
    native_build_commands_codegen = ctx.actions.declare_output("native_build_commands.late_gatorade_codegen.json", has_content_based_path = False)
    dynamic_outputs.append(native_build_commands_base)

    fake_input = ctx.actions.write("dynamic.trigger", "", has_content_based_path = False)

    # some cases don't actually need to use a dynamic_output, but it's simplest to consistently use it anyway. we need some fake input to allow that.
    dynamic_inputs = [fake_input]
    if apk_module_graph_file:
        dynamic_inputs.append(apk_module_graph_file)
    split_groups_map = None
    native_library_merge_dir = None
    native_merge_debug = None
    generated_java_code = []

    glue_linkables = None
    if native_library_merge_glue:
        glue_linkables = {}
        for platform, glue in native_library_merge_glue.items():
            glue_link_graph = glue.get(LinkableGraph)
            expect(glue_link_graph != None, "native_library_merge_glue (`{}`) should be a linkable target", glue.label)
            glue_linkable = glue_link_graph.nodes.value.linkable
            expect(glue_linkable != None, "native_library_merge_glue (`{}`) should be a linkable target", glue.label)
            expect(glue_linkable.preferred_linkage == Linkage("static"), "buck2 currently only supports preferred_linkage='static' native_library_merge_glue")
            glue_linkables[platform] = (glue.label, glue_linkable.link_infos[LibOutputStyle("pic_archive")].default)

    linkable_nodes_by_platform = {}

    # Analysis-scope [native_build_commands] entries (e.g. compute_mergemap) that run before the
    # dynamic lambda. The lambda seeds its own list from a copy of this, then appends the
    # in-lambda link/bolt commands. Entries created inside the frozen lambda cannot be appended
    # here, so anything produced at analysis time is collected into this list instead.
    native_build_command_entries = []

    # TARGET[<phase>] Gatorade-phase product sub-targets. Each outputs the full set of artifacts that
    # phase's gatorade invocation(s) produce, as a symlinked_dir (like [native_libs]).
    #  - early runs at analysis scope: early_gatorade_libraries fills this {relpath: artifact} map,
    #    and early_gatorade_products is built from it below.
    #  - middle reuses middle_gatorade_products, declared below and materialized inside the dynamic
    #    lambda when the middle phase runs.
    #  - late runs inside the nested Gatorade dynamics, so its dir is declared here, appended to
    #    dynamic_outputs, and bound inside (empty when the phase is off).
    early_gatorade_product_mapping = {}
    late_gatorade_products = ctx.actions.declare_output("late_gatorade_products", dir = True, has_content_based_path = False)
    dynamic_outputs.append(late_gatorade_products)

    has_native_merging = native_library_merge_sequence or native_library_merge_map
    enable_relinker = getattr(ctx.attrs, "enable_relinker", False)
    # An exopackage build is installed as this binary, which no combine step follows, and the
    # combine step would also replace a library a pick-first prebuilt shadows in the APK.
    defer_relink = (
        getattr(ctx.attrs, "defer_relink", False)
        and enable_relinker
        and not getattr(ctx.attrs, "exopackage_modes", [])
        and not getattr(ctx.attrs, "native_library_pick_first", [])
    )

    middle_gatorade_products = None
    if enable_relinker and has_native_merging and "middle" in getattr(ctx.attrs, "gatorade_phases", []):
        middle_gatorade_products = ctx.actions.declare_output(
            "middle_gatorade_products",
            dir = True,
            has_content_based_path = False,
        )
        dynamic_outputs.append(middle_gatorade_products)

    # The outer lambda owns the codegen fragment. With late Gatorade, the nested Gatorade dynamic
    # (gatorade_libraries or gatorade_deferred_libs) binds it through the passed OutputArtifact;
    # otherwise the outer lambda binds it with an empty list, so the combine always has both inputs.
    # @oss-disable[end= ]: will_capture_late_gatorade_codegen = is_late_gatorade_enabled(ctx)
    will_capture_late_gatorade_codegen = False # @oss-enable
    dynamic_outputs.append(native_build_commands_codegen)

    if has_native_merging or enable_relinker:
        native_merge_debug = ctx.actions.declare_output("native_merge_debug", dir = True, has_content_based_path = False)
        dynamic_outputs.append(native_merge_debug)

    # Argsfiles referenced by [native_build_commands][compile] entries, attached as other_outputs
    # below. Keyed by the Artifact, not short_path: short_path is only the extension-based filename
    # (e.g. `.cpp.cxx_compile_argsfile`), identical across every library and platform.
    compile_argsfiles = {}

    # EMIT_NATIVE_BUILD_COMMANDS also needs the node map (compile entries read each node's
    # compile_cmds), so compile capture works on apps that neither merge nor relink.
    if has_native_merging or enable_relinker or EMIT_NATIVE_BUILD_COMMANDS:
        # We serialize info about the linkable graph and the apk module mapping and pass that to an
        # external subcommand to apply a merge sequence algorithm and return us the merge mapping.
        for platform, deps in deps_by_platform.items():
            if platform == CPU_FILTER_FOR_PRIMARY_PLATFORM and platform not in ctx.attrs.cpu_filters and len(ctx.attrs.cpu_filters) != 0:
                continue
            linkable_graph = create_linkable_graph(ctx, deps = deps)
            graph_node_map = get_linkable_graph_node_map_func(linkable_graph)()
            linkables_debug = ctx.actions.write("linkables." + platform, list(graph_node_map.keys()), has_content_based_path = False)
            enhance_ctx.debug_output("linkables." + platform, linkables_debug)
            linkable_nodes_by_platform[platform] = graph_node_map
            if EMIT_NATIVE_BUILD_COMMANDS:
                for node in graph_node_map.values():
                    for cc in node.compile_cmds:
                        compile_argsfiles[cc.cxx_compile_cmd.argsfile.file] = None

    jni_onload_check_report = None
    if has_native_merging and ctx.attrs._android_toolchain[AndroidToolchainInfo].jni_onload_check:
        jni_onload_check_report = ctx.actions.declare_output("jni_onload_check.json", has_content_based_path = False)
        dynamic_outputs.append(jni_onload_check_report)

    middle_relink_check_report = None

    if False: # @oss-enable
    # @oss-disable[end= ]: if enable_relinker and is_middle_gatorade_enabled(ctx):
        middle_relink_check_report = ctx.actions.declare_output("middle_relink_check.json", has_content_based_path = False)
        dynamic_outputs.append(middle_relink_check_report)

    relinked_libs_output = None
    relinked_libs_manifest = None
    # Under defer_relink, the packaging inputs built from the relinked libraries, which the
    # binary's own package is built from (see AndroidBinaryNativeLibsInfo.shipped).
    shipped_packaging_outputs = {}
    if defer_relink:
        relinked_libs_output = ctx.actions.declare_output("relinked_libs", dir = True)
        relinked_libs_manifest = ctx.actions.declare_output("relinked_libs_manifest.json")
        dynamic_outputs.append(relinked_libs_output)
        dynamic_outputs.append(relinked_libs_manifest)
        for key in _PACKAGING_INPUTS:
            shipped_packaging_outputs[key] = ctx.actions.declare_output("shipped_{}_symlink".format(key), has_content_based_path = False)
        dynamic_outputs.extend(shipped_packaging_outputs.values())

    # Aggregate of every native library before the relinker's dead-code-stripping
    # pass, exposed as the [unrelinked_libs] sub-target (the pre-relink counterpart
    # of [relinked_libs]). Only the relinker produces an unrelinked vs relinked
    # distinction, so this only exists when relinking is enabled.
    unrelinked_libs_output = None
    if enable_relinker:
        unrelinked_libs_output = ctx.actions.declare_output("unrelinked_libs", dir = True)
        dynamic_outputs.append(unrelinked_libs_output)

    # The `extra_relinker_outputs` of every relinked library, as
    # <type>/<abi>/<soname>/<file>, exposed as the [relinker_extra_outputs] sub-target so
    # they can be built (and so put on disk) without knowing their paths. Always
    # present; empty when nothing is configured or nothing is relinked.
    relinker_extra_outputs = ctx.actions.declare_output("relinker_extra_outputs", dir = True, has_content_based_path = False)
    dynamic_outputs.append(relinker_extra_outputs)

    lib_outputs_by_platform = _declare_library_subtargets(
        ctx, dynamic_outputs, original_shared_libs_by_platform, native_library_merge_map, native_library_merge_sequence, enable_relinker
    )
    # @oss-disable[end= ]: link_diagnostics = declare_native_link_diagnostics(
        # @oss-disable[end= ]: enhance_ctx,
        # @oss-disable[end= ]: dynamic_outputs,
        # @oss-disable[end= ]: {platform: sorted(outputs.keys()) for platform, outputs in lib_outputs_by_platform.items()},
        # @oss-disable[end= ]: enable_relinker,
    # @oss-disable[end= ]: )

    monogatorade_products = None
    if native_library_merge_sequence:
        native_library_merge_input_file = ctx.actions.write_json(
            "mergemap.input",
            {
                "linkable_graphs_by_platform": encode_linkable_graph_for_mergemap(linkable_nodes_by_platform),
                "native_library_merge_sequence": native_library_merge_sequence,
                "native_library_merge_sequence_blocklist": native_library_merge_sequence_blocklist or [],
            },
            has_content_based_path = False,
        )

        if False: # @oss-enable
        # @oss-disable[end= ]: if is_monogatorade_enabled(ctx) and enable_relinker:
            # Monogatorade's whole-app step decides the merges itself, so
            # neither Early Gatorade nor the mergemap tool runs.
            native_library_merge_dir = ctx.actions.declare_output("merge_sequence_output", dir = True, has_content_based_path = False)
            native_library_merge_map = native_library_merge_dir.project("merge.map")
            split_groups_map = native_library_merge_dir.project("split_groups.map")
            # @oss-disable[end= ]: monogatorade_products = monogatorade_whole_app(
                # @oss-disable[end= ]: ctx,
                # @oss-disable[end= ]: linkable_nodes_by_platform,
                # @oss-disable[end= ]: glue_linkables,
                # @oss-disable[end= ]: native_library_merge_input_file,
                # @oss-disable[end= ]: None,
                # @oss-disable[end= ]: apk_module_graph_file,
                # @oss-disable[end= ]: native_library_merge_non_asset_libs,
                # @oss-disable[end= ]: native_library_merge_linker_args_all or [],
                # @oss-disable[end= ]: native_library_merge_linker_args or {},
                # @oss-disable[end= ]: native_library_merge_dir,
                # @oss-disable[end= ]: native_build_command_entries,
            # @oss-disable[end= ]: )
        elif not "early" in getattr(ctx.attrs, "gatorade_phases", []):
            mergemap_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].mergemap_tool)
            mergemap_cmd.add(cmd_args(native_library_merge_input_file, format = "--mergemap-input={}"))
            if apk_module_graph_file:
                mergemap_cmd.add(cmd_args(apk_module_graph_file, format = "--apk-module-graph={}"))
            if native_library_merge_non_asset_libs:
                mergemap_cmd.add(cmd_args("--merge-non-asset-libs"))
            native_library_merge_dir = ctx.actions.declare_output("merge_sequence_output", has_content_based_path = False)
            native_library_merge_map = native_library_merge_dir.project("merge.map")
            split_groups_map = native_library_merge_dir.project("split_groups.map")

            # Record the mergemap command before adding the output as an `as_output()` (which
            # cannot be re-serialized by write_json); reference the plain output artifact instead.
            native_build_command_entries.append(
                native_build_command_entry(
                    kind = "mergemap",
                    category = "compute_mergemap",
                    arch = None,
                    soname = None,
                    output = native_library_merge_dir.short_path,
                    argv = cmd_args(mergemap_cmd, cmd_args(native_library_merge_dir, format = "--output={}")),
                )
            )

            ctx.actions.run(
                cmd_args(mergemap_cmd, cmd_args(native_library_merge_dir.as_output(), format = "--output={}")),
                category = "compute_mergemap",
                allow_cache_upload = True,
            )
        else:
            native_library_merge_dir = ctx.actions.declare_output("merge_sequence_output", dir = True, has_content_based_path = False)
            native_library_merge_map = native_library_merge_dir.project("merge.map")
            split_groups_map = native_library_merge_dir.project("split_groups.map")

            # Pass all merge sequence arguments to early_gatorade_libraries
            # which will handle running either Python or C++ implementation
            # Prevent Buildifier from moving comments in a way that breaks things.
            args = [
                ctx,
                original_shared_libs_by_platform,
                linkable_nodes_by_platform,
                native_library_merge_input_file,
                apk_module_graph_file,
                native_library_merge_non_asset_libs,
                native_library_merge_dir,
                # @oss-disable[end= ]: native_build_command_entries,
                # @oss-disable[end= ]: early_gatorade_product_mapping,
            ]
            # @oss-disable[end= ]: early_gatorade_libraries(*args)

        enhance_ctx.debug_output("compute_merge_sequence", native_library_merge_dir)

        dynamic_inputs.append(native_library_merge_map)
        dynamic_inputs.append(split_groups_map)

    elif False: # @oss-enable
    # @oss-disable[end= ]: elif native_library_merge_map and is_monogatorade_enabled(ctx) and enable_relinker:
        # A regex merge map is decided here, so the whole-app step takes the
        # decision instead of making it.
        given_merge_map = ctx.actions.write_json(
            "monogatorade-merge-map.json",
            _merge_map_from_regexes(native_library_merge_map, linkable_nodes_by_platform),
            pretty = True,
            has_content_based_path = False,
        )
        # @oss-disable[end= ]: monogatorade_products = monogatorade_whole_app(
            # @oss-disable[end= ]: ctx,
            # @oss-disable[end= ]: linkable_nodes_by_platform,
            # @oss-disable[end= ]: glue_linkables,
            # @oss-disable[end= ]: None,
            # @oss-disable[end= ]: given_merge_map,
            # @oss-disable[end= ]: None,
            # @oss-disable[end= ]: False,
            # @oss-disable[end= ]: native_library_merge_linker_args_all or [],
            # @oss-disable[end= ]: native_library_merge_linker_args or {},
            # @oss-disable[end= ]: None,
            # @oss-disable[end= ]: native_build_command_entries,
        # @oss-disable[end= ]: )

    mergemap_gencode_jar = None

    if has_native_merging and native_library_merge_code_generator:
        mergemap_gencode_jar = ctx.actions.declare_output("MergedLibraryMapping.jar", has_content_based_path = False)
        dynamic_outputs.append(mergemap_gencode_jar)
        generated_java_code.append(mergemap_gencode_jar)

    def dynamic_native_libs_info(ctx: AnalysisContext, artifacts, outputs):
        middle_gatorade_outputs_by_platform = {}
        middle_gatorade_relinked_libraries_by_platform = {}
        # Seed with the analysis-scope entries (frozen list, read-only copy), then append the
        # in-lambda link/relink/merge/bolt commands as the pipeline produces them.
        native_cmd_entries = list(native_build_command_entries)
        get_module_from_target = all_targets_in_root_module
        get_module_tdeps = all_targets_in_root_module
        get_calculated_module_deps = all_targets_in_root_module
        get_deps_debug_data = None
        if apk_module_graph_file:
            apk_module_graph = get_apk_module_graph_info(ctx, apk_module_graph_file, artifacts)
            get_module_from_target = apk_module_graph.target_to_module_mapping_function
            get_module_tdeps = apk_module_graph.transitive_module_deps_function
            get_calculated_module_deps = apk_module_graph.calculated_deps_function
            get_deps_debug_data = apk_module_graph.get_deps_debug_data

        split_groups = None
        merged_shared_lib_targets_by_platform = {}  # dict[str, dict[Label, str]]
        excluded_shared_libs_by_platform = {}
        if has_native_merging:
            native_library_merge_debug_outputs = {}

            # When changing this dynamic_output, the workflow is a lot better if you compute the module graph once and
            # then set it as the binary's precomputed_apk_module_graph attr.
            if native_library_merge_sequence:
                merge_map_by_platform = artifacts[native_library_merge_map].read_json()
                split_groups = artifacts[split_groups_map].read_json()
                native_library_merge_debug_outputs["merge_sequence_output"] = native_library_merge_dir
            elif native_library_merge_map:
                merge_map_by_platform = _merge_map_from_regexes(native_library_merge_map, linkable_nodes_by_platform)
                merge_map = ctx.actions.write_json("merge.map", merge_map_by_platform, pretty = True, has_content_based_path = False)
                native_library_merge_debug_outputs["merge_map_output"] = merge_map
            else:
                fail("unreachable")

            shared_object_targets = {}
            debug_info_by_platform = {}  # dict[str, MergedLinkablesDebugInfo]
            merged_shared_libs_by_platform = {}  # dict[str, dict[str, MergedSharedLibrary]]
            jni_onload_check_inputs = [] if jni_onload_check_report else None
            for platform in original_shared_libs_by_platform:
                merged_shared_libs, debug_info = _get_merged_linkables_for_platform(
                    ctx = ctx,
                    cxx_toolchain = ctx.attrs._cxx_toolchain[platform][CxxToolchainInfo],
                    platform = platform if len(original_shared_libs_by_platform) > 1 else None,
                    native_library_merge_linker_args_all = native_library_merge_linker_args_all,
                    glue_linkable = glue_linkables[platform] if glue_linkables else None,
                    default_shared_libs = original_shared_libs_by_platform[platform],
                    linkable_nodes = linkable_nodes_by_platform[platform],
                    merge_map = merge_map_by_platform[platform],
                    merge_linker_args = native_library_merge_linker_args or {},
                    apk_module_graph = get_module_from_target,
                    jni_onload_check_inputs = jni_onload_check_inputs,
                )
                debug_info_by_platform[platform] = debug_info
                merged_shared_libs_by_platform[platform] = merged_shared_libs
                merged_shared_lib_targets = {}
                for soname, lib in merged_shared_libs.items():
                    shared_object_targets[soname] = [str(target.raw_target()) for target in lib.primary_constituents]

                    for target in lib.primary_constituents:
                        merged_shared_lib_targets[target] = soname
                merged_shared_lib_targets_by_platform[platform] = merged_shared_lib_targets

            debug_data_json = ctx.actions.write_json("native_merge_debug.json", debug_info_by_platform, pretty = True, has_content_based_path = False)
            native_library_merge_debug_outputs["native_merge_debug.json"] = debug_data_json

            shared_object_targets_lines = ""
            for soname, targets in shared_object_targets.items():
                shared_object_targets_lines += soname + " " + " ".join(targets) + "\n"
            shared_object_targets_txt = ctx.actions.write("shared_object_targets.txt", shared_object_targets_lines, has_content_based_path = False)
            native_library_merge_debug_outputs["shared_object_targets.txt"] = shared_object_targets_txt

            if mergemap_gencode_jar:
                merged_library_map = write_merged_library_map(ctx, merged_shared_libs_by_platform)
                mergemap_gencode = run_mergemap_codegen(ctx, native_library_merge_code_generator, merged_library_map)
                compile_to_jar(ctx, [mergemap_gencode], output = outputs[mergemap_gencode_jar])
                native_library_merge_debug_outputs["NativeLibraryMergeGeneratedCode.java"] = mergemap_gencode
                native_library_merge_debug_outputs["merged_library_map.txt"] = merged_library_map
                native_library_merge_debug_outputs["mergemap_gencode.jar"] = mergemap_gencode_jar

                # Write the JNI_OnLoad mappings file (merged SO name -> constituent targets)
                jni_on_load_mappings = write_jni_on_load_mappings(ctx, merged_shared_libs_by_platform)
                native_library_merge_debug_outputs["jni_on_load_mappings.txt"] = jni_on_load_mappings

            ctx.actions.symlinked_dir(outputs[native_merge_debug], native_library_merge_debug_outputs)

            if jni_onload_check_report:
                # nm is the same across the android platforms in a binary, so
                # any of them serves for a symbol-table read.
                check_platform = list(original_shared_libs_by_platform.keys())[0]
                _run_jni_onload_check(
                    ctx,
                    ctx.attrs._cxx_toolchain[check_platform][CxxToolchainInfo],
                    jni_onload_check_inputs,
                    outputs[jni_onload_check_report],
                )

            # Merged libraries are assembled from the full linkable graph, so unlike the
            # non-merging path (which filters via get_default_shared_libs) they do not
            # otherwise honor shared_libraries_to_exclude. Drop any merged library whose
            # primary constituents are all provided by the apk-under-test: re-emitting it
            # would ship a duplicate merged .so that loads alongside the apk-under-test's
            # copy in the instrumentation process and re-runs its link_whole static
            # initializers. shared_libraries_to_exclude is empty for non-instrumentation
            # binaries, so this is a no-op there.
            final_shared_libs_by_platform = {
                platform: {
                    soname: d.lib for soname, d in merged_shared_libs.items() if not _merged_lib_provided_by_apk_under_test(d, shared_libraries_to_exclude)
                }
                for platform, merged_shared_libs in merged_shared_libs_by_platform.items()
            }

            # The libraries that ship still link against a dropped library, and
            # monogatorade's whole-app step linked it like any other.
            excluded_shared_libs_by_platform = {
                platform: {soname: d.lib for soname, d in merged_shared_libs.items() if _merged_lib_provided_by_apk_under_test(d, shared_libraries_to_exclude)}
                for platform, merged_shared_libs in merged_shared_libs_by_platform.items()
            }
        elif enable_relinker:
            final_shared_libs_by_platform, native_library_merge_debug_outputs = _create_all_relinkable_links(
                ctx,
                original_shared_libs_by_platform,
                linkable_nodes_by_platform,
            )
            ctx.actions.symlinked_dir(outputs[native_merge_debug], native_library_merge_debug_outputs)

        else:
            final_shared_libs_by_platform = original_shared_libs_by_platform

        relinked_libs_for_extra_outputs = {}
        final_link_diagnostic_libs_by_platform = None

        # [native_build_commands][merge]: record every per-soname merge link here, once, before any
        # relink reassigns final_shared_libs_by_platform. This is the single capture point for merge
        # links across all paths (inline relink, deferred relink, and late-Gatorade), which is why
        # the finalization step below never labels anything kind=merge.
        if has_native_merging:
            for platform, libs in final_shared_libs_by_platform.items():
                for soname, lib in libs.items():
                    record_link_command(native_cmd_entries, "merge", platform, soname, lib.lib)

        # [native_build_commands][compile]: emit one entry per source in every linkable node that
        # carries compile commands (populated only when `-c cxx.emit_native_build_commands=true`, so
        # this whole block is skipped otherwise). Attribute each to the FINAL merged soname via the
        # merge map (falling back to the node's own pre-merge soname), keeping the per-library soname
        # in pre_merge_soname. The soname is best-effort: split groups, static-only nodes, and
        # late-gatorade code motion make it approximate — consumers should cross-reference merge.map.
        if EMIT_NATIVE_BUILD_COMMANDS:
            for platform, node_map in linkable_nodes_by_platform.items():
                merged_by_label = merged_shared_lib_targets_by_platform.get(platform, {})
                for node_label, node in node_map.items():
                    pre_merge_soname = node.default_soname
                    for cc in node.compile_cmds:
                        # A source compiled more than once in the same node (different flavors/flags)
                        # carries a non-None index; append it so those entries get distinct identifiers.
                        identifier = "{}/{}/{}".format(platform, node_label.raw_target(), cc.src.short_path)
                        if cc.index != None:
                            identifier += "/{}".format(cc.index)
                        native_cmd_entries.append(
                            native_build_command_entry(
                                kind = "compile",
                                category = cc.cxx_compile_cmd.category,
                                arch = platform,
                                soname = merged_by_label.get(node_label, pre_merge_soname),
                                pre_merge_soname = pre_merge_soname,
                                output = cc.src.short_path,
                                argv = cmd_args(cc.cxx_compile_cmd.base_compile_cmd, cc.cxx_compile_cmd.argsfile.cmd_form, cc.args),
                                argsfile = cc.cxx_compile_cmd.argsfile.file,
                                identifier = identifier,
                            )
                        )

        shipped_lib_artifacts = {
            "linker_argsfiles": linker_argsfiles,
            "linker_commands": linker_commands,
            "unstripped_native_libraries": unstripped_native_libraries,
            "unstripped_native_libraries_files": unstripped_native_libraries_files,
            "unstripped_native_libraries_json": unstripped_native_libraries_json,
        }
        shipped_lib_outputs = {name: outputs[artifact] for name, artifact in shipped_lib_artifacts.items()}

        # Under defer_relink, the relinked libraries that ship, which the library subtargets show.
        deferred_shipped_libs_by_platform = None

        if enable_relinker and not defer_relink:
            unrelinked_shared_libs_by_platform = final_shared_libs_by_platform
            final_shared_libs_by_platform, middle_gatorade_outputs_by_platform = _relink_for_native_libs(
                ctx,
                final_shared_libs_by_platform,
                native_cmd_entries,
                outputs[middle_relink_check_report] if middle_relink_check_report else None,
                monogatorade_products,
                excluded_shared_libs_by_platform,
            )
            middle_gatorade_relinked_libraries_by_platform = final_shared_libs_by_platform
            relinked_libs_for_extra_outputs = final_shared_libs_by_platform
            _link_library_subtargets(
                ctx,
                outputs,
                lib_outputs_by_platform,
                original_shared_libs_by_platform,
                unrelinked_shared_libs_by_platform,
                merged_shared_lib_targets_by_platform,
                split_groups,
                native_merge_debug,
                unrelinked = True,
            )
            _write_native_libs_dir_and_manifest(
                ctx,
                unrelinked_shared_libs_by_platform,
                outputs[unrelinked_libs_output],
                None,
                stripped = False,
            )
            # @oss-disable[end= ]: materialize_native_link_diagnostics(
                # @oss-disable[end= ]: ctx,
                # @oss-disable[end= ]: outputs,
                # @oss-disable[end= ]: link_diagnostics,
                # @oss-disable[end= ]: unrelinked_shared_libs_by_platform,
                # @oss-disable[end= ]: unrelinked = True,
            # @oss-disable[end= ]: )

        if defer_relink:
            # Packaging honors strip_libraries; the combine must ship what packaging would have.
            strip_libraries = getattr(ctx.attrs, "strip_libraries", True)

            # Run relinking as separate actions that can execute in parallel with other build steps.
            # The relinked libs are exposed as [relinked_libs] sub-target for the combine genrule.
            # A JSON manifest listing the <abi>/<soname> entries is produced alongside so that
            # the combine script knows exactly which libraries to replace without guessing.
            relinked_libs_by_platform, middle_gatorade_outputs_by_platform = _relink_for_native_libs(
                ctx,
                final_shared_libs_by_platform,
                native_cmd_entries,
                outputs[middle_relink_check_report] if middle_relink_check_report else None,
                monogatorade_products,
                excluded_shared_libs_by_platform,
            )
            # This branch deliberately leaves final_shared_libs_by_platform unrelinked for the
            # [unrelinked] subtargets below. Final diagnostic reports must instead reuse the
            # deferred relink actions' arguments, including their version scripts and extra flags.
            final_link_diagnostic_libs_by_platform = relinked_libs_by_platform
            middle_gatorade_relinked_libraries_by_platform = relinked_libs_by_platform
            relinked_libs_for_extra_outputs = relinked_libs_by_platform

            if False: # @oss-enable
            # @oss-disable[end= ]: if is_late_gatorade_enabled(ctx):
                # Run Gatorade cross-library optimization + codegen + re-link on
                # the deferred relinked libs. The callback finalizes the
                # Gatorade-processed libs and writes them to the relinked_libs_output artifacts.
                def deferred_gatorade_output(ctx, gatorade_libs_by_platform, dyn_outputs, codegen_entries):
                    for platform, libs in gatorade_libs_by_platform.items():
                        added = sorted([soname for soname in libs if soname not in relinked_libs_by_platform[platform]])
                        removed = sorted([soname for soname in relinked_libs_by_platform[platform] if soname not in libs])
                        if added or removed:
                            fail(
                                "Late Gatorade changed the {} library set (added {}, removed {}), but under defer_relink the APK already packages the libraries before Late Gatorade, and the combine step can only replace them".format(
                                    platform, added, removed
                                )
                            )
                    shipped_libs_by_platform = _finalize_shipped_libs(
                        ctx,
                        gatorade_libs_by_platform,
                        native_cmd_entries = codegen_entries,
                        **{name: dyn_outputs[artifact] for name, artifact in shipped_lib_artifacts.items()},
                    )
                    _write_native_libs_dir_and_manifest(
                        ctx, shipped_libs_by_platform, dyn_outputs[relinked_libs_output], dyn_outputs[relinked_libs_manifest], stripped = strip_libraries
                    )
                    _post_native_lib_graph_finalization_steps(
                        ctx,
                        shipped_libs_by_platform,
                        all_prebuilt_native_library_dirs,
                        get_module_from_target,
                        name_prefix = "shipped/",
                        **{key: dyn_outputs[artifact] for key, artifact in shipped_packaging_outputs.items()},
                    )

                deferred_outputs = (
                    [outputs[relinked_libs_output], outputs[relinked_libs_manifest]]
                    + shipped_lib_outputs.values()
                    + [outputs[artifact] for artifact in shipped_packaging_outputs.values()]
                )
                # @oss-disable[end= ]: gatorade_deferred_libs(
                    # @oss-disable[end= ]: ctx,
                    # @oss-disable[end= ]: relinked_libs_by_platform,
                    # @oss-disable[end= ]: deferred_gatorade_output,
                    # @oss-disable[end= ]: deferred_outputs,
                    # @oss-disable[end= ]: native_cmd_entries,
                    # @oss-disable[end= ]: outputs[native_build_commands_codegen],
                    # @oss-disable[end= ]: outputs[late_gatorade_products],
                    # @oss-disable[end= ]: link_diagnostics,
                    # @oss-disable[end= ]: native_link_diagnostic_reports(link_diagnostics, outputs = outputs),
                # @oss-disable[end= ]: )
                # As without defer_relink (see gatorade_libraries), the library subtargets show the
                # relinked libraries: Late Gatorade can change the library boundaries they are keyed by.
                deferred_shipped_libs_by_platform = relinked_libs_by_platform
            else:
                deferred_shipped_libs_by_platform = _finalize_shipped_libs(
                    ctx, relinked_libs_by_platform, native_cmd_entries = native_cmd_entries, **shipped_lib_outputs
                )
                _write_native_libs_dir_and_manifest(
                    ctx, deferred_shipped_libs_by_platform, outputs[relinked_libs_output], outputs[relinked_libs_manifest], stripped = strip_libraries
                )
                _post_native_lib_graph_finalization_steps(
                    ctx,
                    deferred_shipped_libs_by_platform,
                    all_prebuilt_native_library_dirs,
                    get_module_from_target,
                    name_prefix = "shipped/",
                    **{key: outputs[artifact] for key, artifact in shipped_packaging_outputs.items()},
                )

            # Bind unrelinked subtarget outputs (same as final since we skipped inline relinking)
            _link_library_subtargets(
                ctx,
                outputs,
                lib_outputs_by_platform,
                original_shared_libs_by_platform,
                final_shared_libs_by_platform,
                merged_shared_lib_targets_by_platform,
                split_groups,
                native_merge_debug,
                unrelinked = True,
            )
            _write_native_libs_dir_and_manifest(
                ctx,
                final_shared_libs_by_platform,
                outputs[unrelinked_libs_output],
                None,
                stripped = False,
            )
            # @oss-disable[end= ]: materialize_native_link_diagnostics(
                # @oss-disable[end= ]: ctx,
                # @oss-disable[end= ]: outputs,
                # @oss-disable[end= ]: link_diagnostics,
                # @oss-disable[end= ]: final_shared_libs_by_platform,
                # @oss-disable[end= ]: unrelinked = True,
            # @oss-disable[end= ]: )

        if middle_gatorade_products != None:
            _materialize_middle_gatorade_products(
                ctx,
                outputs[middle_gatorade_products],
                middle_gatorade_outputs_by_platform,
                middle_gatorade_relinked_libraries_by_platform,
            )

        if ctx.attrs._android_toolchain[AndroidToolchainInfo].cross_module_native_deps_check and (has_native_merging or enable_relinker):
            # note: can only detect these if linkable_nodes_by_platform is created, ie. if using relinker or merging.
            # EMIT_NATIVE_BUILD_COMMANDS also builds it on apps that do neither; those apps never ran
            # this check, so it stays gated on merging/relinking.
            cross_module_link_errors = []
            for linkable_nodes in linkable_nodes_by_platform.values():
                for target, node in linkable_nodes.items():
                    node_target = str(target.raw_target())
                    node_module = get_module_from_target(node_target)
                    for dep in node.deps:
                        dep_target = str(dep.raw_target())
                        dep_module = get_module_from_target(dep_target)
                        if (
                            not is_root_module(dep_module)
                            and node_module != dep_module
                            and dep_module not in get_module_tdeps(node_module)
                            and dep_module not in get_calculated_module_deps(node_module)
                        ):
                            cross_module_link_errors.append("{} (module: {}) -> {} (module: {}) ".format(node_target, node_module, dep_target, dep_module))

            if cross_module_link_errors:
                cross_module_link_errors.append(get_deps_debug_data())
                fail(
                    "Native libraries in modules should only depend on libraries in the same module or the root. Remove these deps:\n"
                    + "\n".join(cross_module_link_errors)
                )

        _write_relinker_extra_outputs_dir(ctx, relinked_libs_for_extra_outputs, outputs[relinker_extra_outputs])

        native_lib_dynamic_outputs = {
            "native_lib_assets_for_primary_apk": outputs[native_lib_assets_for_primary_apk],
            "native_libs": outputs[native_libs],
            "native_libs_always_in_primary_apk": outputs[native_libs_always_in_primary_apk],
            "native_libs_metadata": outputs[native_libs_metadata],
            "non_root_module_lib_assets": outputs[non_root_module_lib_assets],
            "non_root_module_metadata_assets": outputs[non_root_module_metadata_assets],
            "root_module_metadata_assets": outputs[root_module_metadata_assets],
            "stripped_native_linkable_assets_for_primary_apk": outputs[stripped_native_linkable_assets_for_primary_apk],
        }
        if not defer_relink:
            native_lib_dynamic_outputs.update(shipped_lib_outputs)

        if False: # @oss-enable
        # @oss-disable[end= ]: if is_late_gatorade_enabled(ctx) and not defer_relink:
            # Prevent Buildifier from moving comments in a way that breaks things.
            # When defer_relink is True, Gatorade runs in the deferred pipeline via
            # gatorade_deferred_libs() above.
            args = [
                ctx,
                final_shared_libs_by_platform,
                _post_native_lib_graph_finalization_steps,
                all_prebuilt_native_library_dirs,
                get_module_from_target,
                native_lib_dynamic_outputs,
                # @oss-disable[end= ]: native_cmd_entries,
                # @oss-disable[end= ]: outputs[native_build_commands_codegen],
                # @oss-disable[end= ]: outputs[late_gatorade_products],
                # @oss-disable[end= ]: link_diagnostics,
                # @oss-disable[end= ]: native_link_diagnostic_reports(link_diagnostics, outputs = outputs),
            ]
            # @oss-disable[end= ]: subtarget_shared_libs_by_platform = gatorade_libraries(*args)
        else:
            subtarget_shared_libs_by_platform = _post_native_lib_graph_finalization_steps(
                ctx,
                final_shared_libs_by_platform,
                all_prebuilt_native_library_dirs,
                get_module_from_target,
                native_cmd_entries = native_cmd_entries,
                # Only record the libs finalized here as plain kind=link when the app neither merges
                # nor relinks; otherwise merge/relink links are already captured at their own sites.
                record_final_link = not has_native_merging and not enable_relinker,
                **native_lib_dynamic_outputs,
            )
        if deferred_shipped_libs_by_platform != None:
            subtarget_shared_libs_by_platform = deferred_shipped_libs_by_platform

        # Subtargets can't be created or changed from within dynamic actions, so the individual
        # library subtargets can't reflect any changes to the set of libraries made by a dynamic
        # action. All other logic should be in _post_native_lib_graph_finalization_steps, which
        # receives the final set of native libraries.
        _link_library_subtargets(
            ctx,
            outputs,
            lib_outputs_by_platform,
            original_shared_libs_by_platform,
            subtarget_shared_libs_by_platform,
            merged_shared_lib_targets_by_platform,
            split_groups,
            native_merge_debug,
        )
        # @oss-disable[end= ]: if not is_late_gatorade_enabled(ctx):
            # @oss-disable[end= ]: if final_link_diagnostic_libs_by_platform == None:
                # @oss-disable[end= ]: final_link_diagnostic_libs_by_platform = subtarget_shared_libs_by_platform
            # @oss-disable[end= ]: materialize_native_link_diagnostics(
                # @oss-disable[end= ]: ctx,
                # @oss-disable[end= ]: outputs,
                # @oss-disable[end= ]: link_diagnostics,
                # @oss-disable[end= ]: final_link_diagnostic_libs_by_platform,
            # @oss-disable[end= ]: )

        # Write the "base" fragment: every native-build command known by the end of this lambda.
        # with_inputs = False embeds each argv/argsfile as text without materializing the produced
        # .o/.so, matching the linker_commands writer. The final [native_build_commands] JSON and its
        # per-kind filters are produced by the combine action below (which folds in the late-Gatorade
        # codegen fragment).
        ctx.actions.write_json(outputs[native_build_commands_base], native_cmd_entries, with_inputs = False)

        # On paths that don't capture late-Gatorade codegen, bind its fragment here (empty) so the
        # combine action always has both inputs. On the capturing path the nested Gatorade dynamic
        # binds it instead.
        if not will_capture_late_gatorade_codegen:
            ctx.actions.write_json(outputs[native_build_commands_codegen], [], with_inputs = False)

        # TARGET[late_gatorade] products are bound inside the nested Gatorade dynamic (which knows the
        # output library graph); on every non-late path the outer lambda binds it empty here, mirroring
        # the native_build_commands_codegen fragment's ownership.
        if not will_capture_late_gatorade_codegen:
            ctx.actions.symlinked_dir(outputs[late_gatorade_products], {})

    ctx.actions.dynamic_output(dynamic = dynamic_inputs, inputs = [], outputs = [o.as_output() for o in dynamic_outputs], f = dynamic_native_libs_info)

    # Combine the base fragment with the late-Gatorade codegen fragment into the final
    # [native_build_commands] JSON and its per-kind filters. Both fragments were written with
    # with_inputs = False, so their entries are already-rendered plain JSON; read them back and
    # re-emit. This runs after the (possibly nested) writers of both fragments complete.
    def combine_native_build_commands(ctx: AnalysisContext, artifacts, outputs):
        entries = artifacts[native_build_commands_base].read_json() + artifacts[native_build_commands_codegen].read_json()
        ctx.actions.write_json(outputs[native_build_commands], entries)
        for kind in NATIVE_BUILD_COMMAND_KINDS:
            ctx.actions.write_json(outputs[native_build_commands_by_kind[kind]], [entry for entry in entries if entry["kind"] == kind])

    ctx.actions.dynamic_output(
        dynamic = [native_build_commands_base, native_build_commands_codegen],
        inputs = [],
        outputs = [native_build_commands.as_output()] + [native_build_commands_by_kind[kind].as_output() for kind in NATIVE_BUILD_COMMAND_KINDS],
        f = combine_native_build_commands,
    )
    combined_asset_libs = ctx.actions.declare_output("combined_asset_libs", dir = True, has_content_based_path = False)
    ctx.actions.run(
        cmd_args([
            ctx.attrs._android_toolchain[AndroidToolchainInfo].combine_native_library_dirs[RunInfo],
            "--output-dir",
            combined_asset_libs.as_output(),
            "--library-dirs",
            native_lib_assets_for_primary_apk,
            stripped_native_linkable_assets_for_primary_apk,
            "--subdir",
            "assets/lib",
        ]),
        category = "combine_native_asset_libs",
        allow_cache_upload = True,
    )
    all_native_libs = ctx.actions.symlinked_dir(
        "debug_all_native_libs",
        {
            "assets": combined_asset_libs,
            "others": native_libs,
            "primary": native_libs_always_in_primary_apk,
        },
        has_content_based_path = False,
    )

    lib_subtargets = _create_library_subtargets(
        lib_outputs_by_platform,
        native_libs,
        # @oss-disable[end= ]: create_default_outputs = not is_late_gatorade_enabled(ctx),
        create_default_outputs = True, # @oss-enable
    )
    enhance_ctx.debug_output("native_libs", all_native_libs, sub_targets = lib_subtargets)
    if native_merge_debug:
        enhance_ctx.debug_output("native_merge_debug", native_merge_debug)

    if jni_onload_check_report:
        # Buildable on its own so the check can be run, and its report read,
        # without building the whole apk.
        enhance_ctx.debug_output("jni_onload_check", jni_onload_check_report)

    if middle_relink_check_report:
        # Readable on its own: which relinked libraries' imports were checked,
        # and any import whose provider's relink does not export it.
        enhance_ctx.debug_output("middle_relink_check", middle_relink_check_report)

    enhance_ctx.debug_output("linker_argsfiles", linker_argsfiles)
    enhance_ctx.debug_output("linker_commands", linker_commands)
    # Materialize the argsfiles a consumer needs to expand an entry's `@argsfile` on disk:
    #  - link-family kinds (merge/link/relink/bolt) -> the linker_argsfiles symlinked_dir built in
    #    finalization from every final lib's linker_argsfile;
    #  - compile -> the per-source compile argsfiles collected at analysis time above.
    # mergemap has no argsfile; Gatorade argsfiles are recorded by path only (see
    # native_build_commands.bzl `argsfile` note), so those kinds get no extra outputs here.
    compile_argsfiles_outputs = list(compile_argsfiles.keys())
    kind_argsfile_outputs = {kind: [linker_argsfiles] for kind in ["merge", "link", "relink", "bolt"]}
    kind_argsfile_outputs["compile"] = compile_argsfiles_outputs
    enhance_ctx.debug_output(
        "native_build_commands",
        native_build_commands,
        other_outputs = [linker_argsfiles] + compile_argsfiles_outputs,
        sub_targets = {
            kind: [
                DefaultInfo(
                    default_outputs = [native_build_commands_by_kind[kind]],
                    other_outputs = kind_argsfile_outputs.get(kind, []),
                )
            ]
            for kind in NATIVE_BUILD_COMMAND_KINDS
        },
    )
    # TARGET[<phase>] Gatorade-phase product sub-targets: building one runs that phase and outputs
    # the artifacts its gatorade invocation(s) produce (a symlinked_dir, like [native_libs]). Early's
    # products were collected at analysis scope; middle/late were bound inside the dynamic lambda.
    # middle_gatorade_products is only declared when the middle phase runs, so otherwise the
    # sub-target resolves to an empty dir.
    early_gatorade_products = ctx.actions.symlinked_dir("early_gatorade_products", early_gatorade_product_mapping, has_content_based_path = False)
    gatorade_phase_products = {
        "early_gatorade": early_gatorade_products,
        "late_gatorade": late_gatorade_products,
        "middle_gatorade": middle_gatorade_products or ctx.actions.symlinked_dir("middle_gatorade_products", {}, has_content_based_path = False),
    }
    expect(
        sorted(gatorade_phase_products.keys()) == sorted(GATORADE_PHASE_SUBTARGETS),
        "gatorade_phase_products must cover exactly GATORADE_PHASE_SUBTARGETS",
    )
    for gatorade_phase, products in gatorade_phase_products.items():
        enhance_ctx.debug_output(gatorade_phase, products)
    enhance_ctx.debug_output("unstripped_native_libraries", unstripped_native_libraries, other_outputs = [unstripped_native_libraries_files])
    enhance_ctx.debug_output("unstripped_native_libraries_json", unstripped_native_libraries_json, other_outputs = [unstripped_native_libraries_files])
    enhance_ctx.debug_output("unstripped_native_libraries_files", unstripped_native_libraries_files)
    if relinked_libs_output:
        # The combine genrule reads this through `$(location)`, which also
        # builds other_outputs, so a failing middle_relink_check fails it.
        enhance_ctx.debug_output(
            "relinked_libs",
            relinked_libs_output,
            other_outputs = [middle_relink_check_report] if middle_relink_check_report else [],
        )
    if relinked_libs_manifest:
        enhance_ctx.debug_output("relinked_libs_manifest", relinked_libs_manifest)
    else:
        _register_empty_relinked_libs(enhance_ctx)
    if unrelinked_libs_output:
        enhance_ctx.debug_output("unrelinked_libs", unrelinked_libs_output)
    enhance_ctx.debug_output("relinker_extra_outputs", relinker_extra_outputs)

    _register_gatorade_phase_evidence(
        enhance_ctx,
        original_shared_libs_by_platform.keys(),
        native_library_merge_dir,
        middle_gatorade_products,
        unstripped_native_libraries_files,
        relinked_libs_output,
        defer_relink,
    )

    native_libs_for_primary_apk, exopackage_info = _get_exopackage_info(ctx, native_libs_always_in_primary_apk, native_libs, native_libs_metadata)
    shipped = None
    if shipped_packaging_outputs:
        # defer_relink rules out exopackage, so these mirror the non-exopackage inputs below.
        outs = shipped_packaging_outputs
        shipped = AndroidBinaryNativeLibsInfo(
            prebuilt_native_library_dirs = all_prebuilt_native_library_dirs,
            shared_libraries = included_shared_lib_targets,
            native_libs_for_primary_apk = [outs["native_libs"], outs["native_libs_always_in_primary_apk"]],
            exopackage_info = None,
            root_module_native_lib_assets = [
                outs["native_lib_assets_for_primary_apk"],
                outs["stripped_native_linkable_assets_for_primary_apk"],
                outs["root_module_metadata_assets"],
            ],
            non_root_module_native_lib_assets = [outs["non_root_module_metadata_assets"], outs["non_root_module_lib_assets"]],
            generated_java_code = generated_java_code,
            unstripped_shared_libraries = unstripped_native_libraries_files,
            # The package of the relinked libraries carries the relink check.
            validation_outputs = [report for report in [jni_onload_check_report, middle_relink_check_report] if report],
        )
    return AndroidBinaryNativeLibsInfo(
        prebuilt_native_library_dirs = all_prebuilt_native_library_dirs,
        shared_libraries = included_shared_lib_targets,
        native_libs_for_primary_apk = native_libs_for_primary_apk,
        exopackage_info = exopackage_info,
        root_module_native_lib_assets = [native_lib_assets_for_primary_apk, stripped_native_linkable_assets_for_primary_apk, root_module_metadata_assets],
        non_root_module_native_lib_assets = [non_root_module_metadata_assets, non_root_module_lib_assets],
        generated_java_code = generated_java_code,
        unstripped_shared_libraries = unstripped_native_libraries_files,
        # Packaging adds validation_outputs as hidden inputs. A deferred relink's
        # check must not become one, or the package Redex starts from would wait
        # for every relink; [relinked_libs] carries it to the combine step.
        validation_outputs = [report for report in [jni_onload_check_report, None if defer_relink else middle_relink_check_report] if report],
        shipped = shipped,
    )

# The packaging inputs _post_native_lib_graph_finalization_steps writes, by keyword.
_PACKAGING_INPUTS = [
    "native_libs",
    "native_libs_metadata",
    "native_libs_always_in_primary_apk",
    "native_lib_assets_for_primary_apk",
    "stripped_native_linkable_assets_for_primary_apk",
    "root_module_metadata_assets",
    "non_root_module_metadata_assets",
    "non_root_module_lib_assets",
]

_NativeLibSubtargetArtifacts = record(
    default = Artifact,
    unrelinked = Artifact | None,
    linker_command = Artifact | None,
    linker_argsfile = Artifact | None,
    # Pre-inline module IR (.ll) emitted by the relink when the `preinline-ir`
    # extra relinker output is enabled; a comment-only `.ll` otherwise. Surfaced
    # as the `[preinline_ir]` sub-target so tooling (evt) can fetch it.
    preinline_ir = Artifact | None,
)

# Writes a directory of <abi>/<soname> shared libraries, and optionally a JSON
# manifest listing those entries. Shared by the [relinked_libs] and
# [unrelinked_libs] sub-targets. [relinked_libs] feeds the combine genrule and
# wants the packaged (stripped) form plus the manifest (so the combine step
# knows which libraries to replace); [unrelinked_libs] mirrors the per-library
# [unrelinked] sub-target, exposing the unstripped linker output, and needs no
# manifest (pass out_manifest = None).
def _write_relinker_extra_outputs_dir(ctx: AnalysisContext, libs_by_platform: dict[str, dict[str, SharedLibrary]], out_dir: Artifact):
    # Only the configured types: the relinker records other things under
    # `extra_outputs` as well, and they are not for building on their own.
    output_types = getattr(ctx.attrs, "extra_relinker_outputs", [])
    files = {}
    for platform, libs in libs_by_platform.items():
        abi_directory = CPU_FILTER_TO_ABI_DIRECTORY[platform]
        for soname, lib in libs.items():
            for output_type in output_types:
                for info in lib.extra_outputs.get(output_type, []):
                    for artifact in info.default_outputs:
                        # Key by soname as well: two libraries in the same
                        # abi/type can emit outputs sharing a basename, which
                        # would otherwise collide and drop one silently.
                        files["{}/{}/{}/{}".format(output_type, abi_directory, soname, artifact.basename)] = artifact
    ctx.actions.symlinked_dir(out_dir, files)

def _write_native_libs_dir_and_manifest(ctx, libs_by_platform, out_dir, out_manifest, stripped):
    lib_files = {}
    for platform in libs_by_platform:
        libs = libs_by_platform[platform]
        abi_directory = CPU_FILTER_TO_ABI_DIRECTORY[platform]
        for soname in libs:
            shlib = libs[soname]
            lib_files["{}/{}".format(abi_directory, soname)] = (shlib.stripped_lib or shlib.lib.output) if stripped else shlib.lib.output
    ctx.actions.symlinked_dir(out_dir, lib_files)
    if out_manifest != None:
        ctx.actions.write_json(out_manifest, sorted(lib_files.keys()))

# The outputs that describe the libraries the app ships. Under defer_relink those are the relinked
# libraries, which are finalized apart from the unrelinked ones packaged for Redex.
def _post_native_lib_graph_finalization_steps(
    ctx: AnalysisContext,
    final_shared_libs_by_platform: dict[str, dict[str, SharedLibrary]],
    all_prebuilt_native_library_dirs: list[PrebuiltNativeLibraryDir],
    get_module_from_target: typing.Callable,
    native_libs: Artifact,
    native_libs_metadata: Artifact,
    native_libs_always_in_primary_apk: Artifact,
    native_lib_assets_for_primary_apk: Artifact,
    stripped_native_linkable_assets_for_primary_apk: Artifact,
    root_module_metadata_assets: Artifact,
    non_root_module_metadata_assets: Artifact,
    non_root_module_lib_assets: Artifact,
    native_cmd_entries = None,
    # True only on apps that neither merge nor relink: then the libs finalized here ARE the plain
    # per-library links, recorded kind=link. When merging/relinking, those commands are captured at
    # their own sites (kind=merge / kind=relink) and this must stay False to avoid double-recording.
    record_final_link: bool = False,
    # Prefixes the outputs and actions declared here (see _get_native_libs_and_assets).
    name_prefix: str = "",
    # _finalize_shipped_libs's outputs, when the libraries packaged here are the ones that ship.
    **shipped_lib_outputs,
) -> dict[str, dict[str, SharedLibrary]]:
    if shipped_lib_outputs:
        final_shared_libs_by_platform = _finalize_shipped_libs(
            ctx, final_shared_libs_by_platform, native_cmd_entries = native_cmd_entries, record_final_link = record_final_link, **shipped_lib_outputs
        )

    dynamic_info = _get_native_libs_and_assets(
        ctx,
        get_module_from_target,
        all_prebuilt_native_library_dirs,
        final_shared_libs_by_platform,
        name_prefix,
    )

    # Since we are using a dynamic action, we need to declare the outputs in advance.
    # Rather than passing the created outputs into `_get_native_libs_and_assets`, we
    # just symlink to the outputs that function produces.
    ctx.actions.symlink_file(native_libs, dynamic_info.native_libs)
    ctx.actions.symlink_file(native_libs_metadata, dynamic_info.native_libs_metadata)
    ctx.actions.symlink_file(native_libs_always_in_primary_apk, dynamic_info.native_libs_always_in_primary_apk)
    ctx.actions.symlink_file(
        native_lib_assets_for_primary_apk,
        dynamic_info.native_lib_assets_for_primary_apk
        if dynamic_info.native_lib_assets_for_primary_apk
        else ctx.actions.symlinked_dir(name_prefix + "empty_native_lib_assets", {}, has_content_based_path = False),
    )
    ctx.actions.symlink_file(
        stripped_native_linkable_assets_for_primary_apk,
        dynamic_info.stripped_native_linkable_assets_for_primary_apk
        if dynamic_info.stripped_native_linkable_assets_for_primary_apk
        else ctx.actions.symlinked_dir(name_prefix + "empty_stripped_native_linkable_assets", {}, has_content_based_path = False),
    )
    ctx.actions.symlink_file(root_module_metadata_assets, dynamic_info.root_module_metadata_assets)
    ctx.actions.symlink_file(non_root_module_metadata_assets, dynamic_info.non_root_module_metadata_assets)
    ctx.actions.symlink_file(
        non_root_module_lib_assets,
        dynamic_info.non_root_module_lib_assets
        if dynamic_info.non_root_module_lib_assets
        else ctx.actions.symlinked_dir(name_prefix + "empty_non_root_module_lib_assets", {}, has_content_based_path = False),
    )

    return final_shared_libs_by_platform

# BOLTs the libraries the app ships and writes the outputs that describe them. Returns the
# libraries to ship.
def _finalize_shipped_libs(
    ctx: AnalysisContext,
    final_shared_libs_by_platform: dict[str, dict[str, SharedLibrary]],
    linker_argsfiles: Artifact,
    linker_commands: Artifact,
    unstripped_native_libraries: Artifact,
    unstripped_native_libraries_json: Artifact,
    unstripped_native_libraries_files: Artifact,
    native_cmd_entries = None,
    record_final_link: bool = False,
) -> dict[str, dict[str, SharedLibrary]]:
    bolt_args = getattr(ctx.attrs, "native_library_bolt_args", None)
    pre_bolt_libs_by_platform = {}
    if bolt_args and len(bolt_args) != 0:
        final_shared_libs_by_platform, pre_bolt_libs_by_platform = _bolt_libraries(
            ctx, final_shared_libs_by_platform, bolt_args, native_cmd_entries, record_final_link
        )

    unstripped_libs = {}
    linker_argsfiles_list = []
    linker_commands_json = []
    for platform, libs in final_shared_libs_by_platform.items() + pre_bolt_libs_by_platform.items():
        for soname, lib in libs.items():
            unstripped_libs[lib.lib.output] = platform
            if lib.lib.linker_argsfile:
                linker_argsfiles_list.append(lib.lib)
            linker_commands_json.append({
                "argsfile": lib.lib.linker_argsfile,
                "command": lib.lib.linker_command,
                "filename": lib.lib.output.short_path,
            })

            # Record the plain per-library link for [native_build_commands][link], but only on apps
            # that neither merge nor relink (record_final_link). Merge and relink links are captured
            # at their own sites, and on those paths the libs seen here are either already-recorded
            # (merge/relink outputs) or command-less (late-gatorade dummies), so recording here would
            # double-count or add null entries.
            if record_final_link:
                record_link_command(native_cmd_entries, "link", platform, soname, lib.lib)

    ctx.actions.symlinked_dir(linker_argsfiles, {"{}".format(lib.output.basename): lib.linker_argsfile for lib in linker_argsfiles_list})
    ctx.actions.write_json(linker_commands, linker_commands_json, with_inputs = False)
    ctx.actions.write(unstripped_native_libraries, unstripped_libs.keys())
    ctx.actions.write_json(unstripped_native_libraries_json, unstripped_libs)
    ctx.actions.symlinked_dir(unstripped_native_libraries_files, {"{}/{}".format(platform, lib.basename): lib for lib, platform in unstripped_libs.items()})

    return final_shared_libs_by_platform

# Merged libraries are dynamic dependencies, but outputs need to be declared in advance to be used by subtargets.
# This means we have to declare outputs for all possible merged libs (every merged name and every unmerged library name).
def _declare_library_subtargets(
    ctx: AnalysisContext,
    dynamic_outputs: list[Artifact],
    original_shared_libs_by_platform: dict[str, dict[str, SharedLibrary]],
    native_library_merge_map,
    native_library_merge_sequence,
    enable_relinker: bool,
) -> dict[str, dict[str, _NativeLibSubtargetArtifacts]]:
    lib_outputs_by_platform = {}
    for platform, original_shared_libs in original_shared_libs_by_platform.items():
        sonames = set()
        sonames.update(original_shared_libs.keys())
        if native_library_merge_map:
            sonames.update(native_library_merge_map.keys())
        elif native_library_merge_sequence:
            for entry in native_library_merge_sequence:
                if type(entry) == "list":
                    sonames.update([soname for (soname, _) in entry])
                else:
                    (soname, _) = entry
                    sonames.add(soname)

        lib_outputs = {}
        for soname in sonames:
            output_path = _platform_output_path(soname, platform if len(original_shared_libs_by_platform) > 1 else None)
            lib_output = ctx.actions.declare_output(output_path, dir = True, has_content_based_path = False)
            dynamic_outputs.append(lib_output)

            if enable_relinker:
                linker_command_output = ctx.actions.declare_output(output_path + ".linker_command", has_content_based_path = False)
                dynamic_outputs.append(linker_command_output)
                linker_argsfile_output = ctx.actions.declare_output(output_path + ".linker_argsfile", has_content_based_path = False)
                dynamic_outputs.append(linker_argsfile_output)
                preinline_ir_output = ctx.actions.declare_output(output_path + ".preinline.ll", has_content_based_path = False)
                dynamic_outputs.append(preinline_ir_output)

                output_path = output_path + ".unrelinked"
                unrelinked_lib_output = ctx.actions.declare_output(output_path, dir = True, has_content_based_path = False)
                dynamic_outputs.append(unrelinked_lib_output)
                lib_outputs[soname] = _NativeLibSubtargetArtifacts(
                    default = lib_output,
                    unrelinked = unrelinked_lib_output,
                    linker_command = linker_command_output,
                    linker_argsfile = linker_argsfile_output,
                    preinline_ir = preinline_ir_output,
                )
            else:
                lib_outputs[soname] = _NativeLibSubtargetArtifacts(
                    default = lib_output,
                    unrelinked = None,
                    linker_command = None,
                    linker_argsfile = None,
                    preinline_ir = None,
                )

        lib_outputs_by_platform[platform] = lib_outputs
    return lib_outputs_by_platform

# Bind debug library subtarget outputs to actual outputs.
# For individual libraries, link to either the unmerged or merged output.
# For merged libraries, link to either the merged output, or a symlinked dir of all merged split group outputs.
def _link_library_subtargets(
    ctx: AnalysisContext,
    outputs,  # IndexSet[OutputArtifact]
    lib_outputs_by_platform: dict[str, dict[str, _NativeLibSubtargetArtifacts]],  # dict[platform, dict[soname,  _NativeLibSubtargetArtifacts]]
    original_shared_libs_by_platform: dict[str, dict[str, SharedLibrary]],
    final_shared_libs_by_platform: dict[str, dict[str, SharedLibrary]],
    merged_shared_lib_targets_by_platform: dict[str, dict[Label, str]],
    split_groups: dict[str, str] | None,
    native_merge_debug,
    unrelinked: bool = False,
):
    for platform, final_shared_libs in final_shared_libs_by_platform.items():
        merged_lib_outputs = {}
        linker_commands_by_soname = {}
        linker_argsfiles_by_soname = {}
        preinline_ir_by_soname = {}

        for soname, lib in final_shared_libs.items():
            base_soname = soname
            if split_groups and soname in split_groups:
                base_soname = split_groups[soname]

            group_outputs = merged_lib_outputs.setdefault(base_soname, {})
            group_outputs[soname] = lib.lib.output

            if not unrelinked:
                if lib.lib.linker_command:
                    linker_commands_by_soname[soname] = {
                        "argsfile": lib.lib.linker_argsfile,
                        "command": lib.lib.linker_command,
                        "filename": lib.lib.output.short_path,
                    }
                if lib.lib.linker_argsfile:
                    linker_argsfiles_by_soname[soname] = lib.lib.linker_argsfile
                preinline_ir = lib.extra_outputs.get("preinline-ir") if lib.extra_outputs else None
                if preinline_ir:
                    preinline_ir_outputs = preinline_ir[0].default_outputs
                    if preinline_ir_outputs:
                        preinline_ir_by_soname[soname] = preinline_ir_outputs[0]

        for soname, lib_outputs in lib_outputs_by_platform[platform].items():
            if soname in merged_lib_outputs:
                group_outputs = merged_lib_outputs[soname]
            elif soname in original_shared_libs_by_platform[platform]:
                # link unmerged soname to merged output
                original_shared_lib = original_shared_libs_by_platform[platform][soname]
                merged_soname = merged_shared_lib_targets_by_platform[platform][original_shared_lib.label]
                if split_groups and merged_soname in split_groups:
                    merged_soname = split_groups[merged_soname]
                group_outputs = merged_lib_outputs[merged_soname]
            else:
                # merged group name has no constituents, link to debug output
                group_outputs = {soname: native_merge_debug}

            output = lib_outputs.default
            if unrelinked:
                output = lib_outputs.unrelinked
            ctx.actions.symlinked_dir(outputs[output], group_outputs)

            if not unrelinked and lib_outputs.linker_command:
                if soname in linker_commands_by_soname:
                    ctx.actions.write_json(outputs[lib_outputs.linker_command], linker_commands_by_soname[soname])
                else:
                    ctx.actions.write_json(outputs[lib_outputs.linker_command], {})

                if soname in linker_argsfiles_by_soname:
                    ctx.actions.symlink_file(outputs[lib_outputs.linker_argsfile], linker_argsfiles_by_soname[soname])
                else:
                    ctx.actions.write_json(outputs[lib_outputs.linker_argsfile], {})

                if lib_outputs.preinline_ir:
                    if soname in preinline_ir_by_soname:
                        ctx.actions.symlink_file(outputs[lib_outputs.preinline_ir], preinline_ir_by_soname[soname])
                    else:
                        # `;` is an LLVM IR line comment, so a consumer that parses this
                        # unconditionally reads an empty module rather than choking on a
                        # placeholder that is not IR at all.
                        ctx.actions.write(
                            outputs[lib_outputs.preinline_ir],
                            "; no pre-inline IR for {}: the `preinline_ir` relinker output was not enabled for this library (constraint off, or soname absent from `gator.preinline_ir_sonames`)".format(
                                soname
                            ),
                        )

def _create_library_subtargets(
    lib_outputs_by_platform: dict[str, dict[str, _NativeLibSubtargetArtifacts]], native_libs: Artifact, create_default_outputs: bool
):
    def create_library_subtarget(output: _NativeLibSubtargetArtifacts, create_default_outputs: bool):
        if not create_default_outputs and not output.unrelinked:
            fail("create_default_outputs can only be False when relinking")

        sub_targets = {}
        if output.linker_command:
            sub_targets["linker_command"] = [DefaultInfo(default_outputs = [output.linker_command])]
        if output.linker_argsfile:
            sub_targets["linker_argsfile"] = [DefaultInfo(default_outputs = [output.linker_argsfile])]
        if output.preinline_ir:
            sub_targets["preinline_ir"] = [DefaultInfo(default_outputs = [output.preinline_ir])]

        if output.unrelinked:
            sub_targets["unrelinked"] = [DefaultInfo(default_outputs = [output.unrelinked])]
            if create_default_outputs:
                default_outputs = [output.default]
            else:
                sub_targets["relinked"] = [DefaultInfo(default_outputs = [output.default])]
                default_outputs = []
            return [DefaultInfo(default_outputs = default_outputs, sub_targets = sub_targets)]
        return [DefaultInfo(default_outputs = [output.default], sub_targets = sub_targets)]

    if len(lib_outputs_by_platform) > 1:
        return {
            platform: [
                DefaultInfo(
                    default_outputs = [native_libs],
                    sub_targets = {soname: create_library_subtarget(output, create_default_outputs) for soname, output in lib_outputs.items()},
                )
            ]
            for platform, lib_outputs in lib_outputs_by_platform.items()
        }
    elif len(lib_outputs_by_platform) == 1:
        lib_outputs = list(lib_outputs_by_platform.values())[0]
        return {soname: create_library_subtarget(output, create_default_outputs) for soname, output in lib_outputs.items()}
    else:
        # TODO(ctolliday) at this point we should have thrown an error earlier if no libraries matched cpu_filters
        # (or returned earlier if there are no native library deps)
        return {}

# We could just return two artifacts of libs (one for the primary APK, one which can go
# either into the primary APK or be exopackaged), and one artifact of assets,
# but we'd need an extra action in order to combine them (we can't use `symlinked_dir` since
# the paths overlap) so it's easier to just be explicit about exactly what we produce.
_NativeLibsAndAssetsInfo = record(
    native_libs = Artifact,
    native_libs_metadata = Artifact,
    native_libs_always_in_primary_apk = Artifact,
    native_lib_assets_for_primary_apk = Artifact | None,
    stripped_native_linkable_assets_for_primary_apk = Artifact | None,
    root_module_metadata_assets = Artifact,
    non_root_module_metadata_assets = Artifact,
    non_root_module_lib_assets = [Artifact, None],
)

def _get_exopackage_info(
    ctx: AnalysisContext, native_libs_always_in_primary_apk: Artifact, native_libs: Artifact, native_libs_metadata: Artifact
) -> (list[Artifact], [ExopackageNativeInfo, None]):
    is_exopackage_enabled_for_native_libs = "native_library" in getattr(ctx.attrs, "exopackage_modes", [])
    if is_exopackage_enabled_for_native_libs:
        return [native_libs_always_in_primary_apk], ExopackageNativeInfo(directory = native_libs, metadata = native_libs_metadata)
    else:
        return [native_libs, native_libs_always_in_primary_apk], None

def _get_native_libs_and_assets(
    ctx: AnalysisContext,
    get_module_from_target: typing.Callable,
    all_prebuilt_native_library_dirs: list[PrebuiltNativeLibraryDir],
    platform_to_native_linkables: dict[str, dict[str, SharedLibrary]],
    # Prefixes every output and action identifier, so one analysis can package two sets of
    # libraries (defer_relink packages the unrelinked libraries and the relinked ones).
    name_prefix: str = "",
) -> _NativeLibsAndAssetsInfo:
    is_packaging_native_libs_as_assets_supported = getattr(ctx.attrs, "package_asset_libraries", False)
    is_voltron_packaging_native_libs_as_assets_supported = getattr(ctx.attrs, "package_voltron_asset_libraries", False)

    prebuilt_native_library_dirs = []
    prebuilt_native_library_dirs_always_in_primary_apk = []
    prebuilt_native_library_dir_assets_for_primary_apk = []
    prebuilt_native_library_dir_module_assets_map = {}
    prebuilt_native_library_dir_module_libs_map = {}
    for native_lib in all_prebuilt_native_library_dirs:
        native_lib_target = str(native_lib.raw_target)
        module = get_module_from_target(native_lib_target)
        expect(
            not native_lib.for_primary_apk or is_root_module(module),
            "{} which is marked as needing to be in the primary APK cannot be included in non-root-module {}".format(native_lib_target, module),
        )
        expect(
            not native_lib.for_primary_apk or not native_lib.is_asset,
            "{} which is marked as needing to be in the primary APK cannot be an asset".format(native_lib_target),
        )
        if not is_root_module(module):
            if native_lib.is_asset and is_voltron_packaging_native_libs_as_assets_supported:
                prebuilt_native_library_dir_module_assets_map.setdefault(module, []).append(native_lib)
            else:
                prebuilt_native_library_dir_module_libs_map.setdefault(module, []).append(native_lib)
        elif native_lib.is_asset and is_packaging_native_libs_as_assets_supported:
            expect(not native_lib.for_primary_apk, "{} which is marked as needing to be in the primary APK cannot be an asset".format(native_lib_target))
            prebuilt_native_library_dir_assets_for_primary_apk.append(native_lib)
        elif native_lib.for_primary_apk:
            prebuilt_native_library_dirs_always_in_primary_apk.append(native_lib)
        else:
            prebuilt_native_library_dirs.append(native_lib)

    native_libs = _filter_prebuilt_native_library_dir(
        ctx,
        prebuilt_native_library_dirs,
        name_prefix + "native_libs",
    )
    native_libs_always_in_primary_apk = _filter_prebuilt_native_library_dir(
        ctx,
        prebuilt_native_library_dirs_always_in_primary_apk,
        name_prefix + "native_libs_always_in_primary_apk",
    )
    native_lib_assets_for_primary_apk = (
        _filter_prebuilt_native_library_dir(
            ctx,
            prebuilt_native_library_dir_assets_for_primary_apk,
            name_prefix + "native_lib_assets_for_primary_apk",
            package_as_assets = True,
            module = ROOT_MODULE,
        )
        if prebuilt_native_library_dir_assets_for_primary_apk
        else None
    )
    native_lib_module_assets_map = {}
    for module, native_lib_dir in prebuilt_native_library_dir_module_assets_map.items():
        native_lib_module_assets_map.setdefault(module, []).append(
            _filter_prebuilt_native_library_dir(
                ctx,
                native_lib_dir,
                name_prefix + "native_lib_assets_for_module_{}".format(module),
                package_as_assets = True,
                module = module,
            )
        )
    for module, native_lib_dir in prebuilt_native_library_dir_module_libs_map.items():
        native_lib_module_assets_map.setdefault(module, []).append(
            _filter_prebuilt_native_library_dir(
                ctx,
                native_lib_dir,
                name_prefix + "native_lib_libs_for_module_{}".format(module),
                package_as_assets = False,
                module = module,
            )
        )

    stripped_linkables = _get_native_linkables(
        ctx,
        platform_to_native_linkables,
        get_module_from_target,
        is_packaging_native_libs_as_assets_supported,
        is_voltron_packaging_native_libs_as_assets_supported,
        name_prefix,
    )
    for module, native_linkable_assets in stripped_linkables.linkable_module_assets_map.items():
        native_lib_module_assets_map.setdefault(module, []).append(native_linkable_assets)

    root_module_metadata_srcs = {}
    non_root_module_metadata_srcs = {}
    non_root_module_libs_srcs = []
    assets_for_primary_apk = filter(None, [native_lib_assets_for_primary_apk, stripped_linkables.linkable_assets_for_primary_apk])
    stripped_linkable_assets_for_primary_apk = stripped_linkables.linkable_assets_for_primary_apk
    if assets_for_primary_apk:
        metadata_file = _get_native_libs_as_assets_metadata(ctx, assets_for_primary_apk, ROOT_MODULE, name_prefix)
        root_module_metadata_srcs[paths.join(_get_native_libs_as_assets_dir(ROOT_MODULE), "metadata.txt")] = metadata_file

    for module, native_lib_assets in native_lib_module_assets_map.items():
        metadata_file = _get_native_libs_as_assets_metadata(ctx, native_lib_assets, module, name_prefix)
        libs_metadata_path = paths.join("assets", "libs.txt")
        non_root_module_metadata_srcs[paths.join(_get_native_libs_as_assets_dir(module), libs_metadata_path)] = metadata_file
        non_root_module_libs_srcs.extend(native_lib_assets)

    non_root_module_libs = None
    if non_root_module_libs_srcs:
        non_root_module_libs = ctx.actions.declare_output(name_prefix + "non_root_module_libs", has_content_based_path = False)
        ctx.actions.run(
            cmd_args(
                [
                    ctx.attrs._android_toolchain[AndroidToolchainInfo].combine_native_library_dirs[RunInfo],
                    "--output-dir",
                    non_root_module_libs.as_output(),
                    "--library-dirs",
                ]
                + non_root_module_libs_srcs
            ),
            category = "combine_non_root_module_native_libs",
            identifier = name_prefix + "non_root_module_libs",
            allow_cache_upload = True,
        )

    combined_native_libs = ctx.actions.declare_output(name_prefix + "combined_native_libs", dir = True, has_content_based_path = False)
    native_libs_metadata = ctx.actions.declare_output(name_prefix + "native_libs_metadata.txt", has_content_based_path = False)
    native_library_pick_first = getattr(ctx.attrs, "native_library_pick_first", [])
    native_library_pick_first_arg = ["--pick-first"] + native_library_pick_first if native_library_pick_first else []
    ctx.actions.run(
        cmd_args(
            [
                ctx.attrs._android_toolchain[AndroidToolchainInfo].combine_native_library_dirs[RunInfo],
                "--output-dir",
                combined_native_libs.as_output(),
                "--library-dirs",
                native_libs,
                stripped_linkables.linkables,
                "--metadata-file",
                native_libs_metadata.as_output(),
            ]
            + native_library_pick_first_arg
        ),
        category = "combine_native_libs",
        identifier = name_prefix + "native_libs",
        allow_cache_upload = True,
    )

    combined_native_libs_always_in_primary_apk = ctx.actions.declare_output(
        name_prefix + "combined_native_libs_always_in_primary_apk", dir = True, has_content_based_path = False
    )
    ctx.actions.run(
        cmd_args([
            ctx.attrs._android_toolchain[AndroidToolchainInfo].combine_native_library_dirs[RunInfo],
            "--output-dir",
            combined_native_libs_always_in_primary_apk.as_output(),
            "--library-dirs",
            native_libs_always_in_primary_apk,
            stripped_linkables.linkables_always_in_primary_apk,
        ]),
        category = "combine_native_libs_always_in_primary_apk",
        identifier = name_prefix + "native_libs_always_in_primary_apk",
        allow_cache_upload = True,
    )

    return _NativeLibsAndAssetsInfo(
        native_libs = combined_native_libs,
        native_libs_metadata = native_libs_metadata,
        native_libs_always_in_primary_apk = combined_native_libs_always_in_primary_apk,
        native_lib_assets_for_primary_apk = native_lib_assets_for_primary_apk,
        stripped_native_linkable_assets_for_primary_apk = stripped_linkable_assets_for_primary_apk,
        root_module_metadata_assets = ctx.actions.symlinked_dir(
            name_prefix + "root_module_metadata_assets", root_module_metadata_srcs, has_content_based_path = False
        ),
        non_root_module_metadata_assets = ctx.actions.symlinked_dir(
            name_prefix + "non_root_module_metadata_assets", non_root_module_metadata_srcs, has_content_based_path = False
        ),
        non_root_module_lib_assets = non_root_module_libs,
    )

def _filter_prebuilt_native_library_dir(
    ctx: AnalysisContext, native_libs: list[PrebuiltNativeLibraryDir], identifier: str, package_as_assets: bool = False, module: str = ROOT_MODULE
) -> Artifact:
    cpu_filters = ctx.attrs.cpu_filters or CPU_FILTER_TO_ABI_DIRECTORY.keys()
    abis = [CPU_FILTER_TO_ABI_DIRECTORY[cpu] for cpu in cpu_filters]
    filter_tool = ctx.attrs._android_toolchain[AndroidToolchainInfo].filter_prebuilt_native_library_dir[RunInfo]
    native_libs_dirs_file = argfile(actions = ctx.actions, name = "{}_list.txt".format(identifier), args = [native_lib.dir for native_lib in native_libs])
    base_output_dir = ctx.actions.declare_output(identifier, dir = True, has_content_based_path = False)
    if module == ROOT_MODULE:
        output_dir = base_output_dir.project(_get_native_libs_as_assets_dir(module)) if package_as_assets else base_output_dir
    elif package_as_assets:
        output_dir = base_output_dir.project(paths.join(_get_native_libs_as_assets_dir(module), "assets"))
    else:
        output_dir = base_output_dir.project(paths.join(_get_native_libs_as_assets_dir(module), "lib"))
    ctx.actions.run(
        cmd_args([filter_tool, native_libs_dirs_file, output_dir.as_output(), "--abis"] + abis),
        category = "filter_prebuilt_native_library_dir",
        identifier = identifier,
        allow_cache_upload = True,
    )

    return base_output_dir

_StrippedNativeLinkables = record(
    linkables = Artifact,
    linkables_always_in_primary_apk = Artifact,
    linkable_assets_for_primary_apk = Artifact | None,
    linkable_module_assets_map = dict[str, Artifact],
)

def _get_native_linkables(
    ctx: AnalysisContext,
    platform_to_native_linkables: dict[str, dict[str, SharedLibrary]],
    get_module_from_target: typing.Callable,
    package_native_libs_as_assets_enabled: bool,
    package_voltron_native_libs_as_assets_enabled: bool,
    name_prefix: str = "",
) -> _StrippedNativeLinkables:
    stripped_native_linkables_srcs = {}
    stripped_native_linkables_always_in_primary_apk_srcs = {}
    stripped_native_linkable_assets_for_primary_apk_srcs = {}
    stripped_native_linkable_module_assets_srcs = {}
    strip_libraries = getattr(ctx.attrs, "strip_libraries", True)

    cpu_filters = ctx.attrs.cpu_filters
    for platform, native_linkables in platform_to_native_linkables.items():
        if cpu_filters and platform not in cpu_filters and platform != CPU_FILTER_FOR_PRIMARY_PLATFORM:
            fail("Platform `{}` is not in the CPU filters `{}`".format(platform, cpu_filters))

        abi_directory = CPU_FILTER_TO_ABI_DIRECTORY[platform]
        for so_name, native_linkable in native_linkables.items():
            native_linkable_target = str(native_linkable.label.raw_target())
            module = get_module_from_target(native_linkable_target)
            lib = native_linkable.stripped_lib if strip_libraries else native_linkable.lib.output

            expect(
                not native_linkable.for_primary_apk or is_root_module(module),
                "{} which is marked as needing to be in the primary APK cannot be included in non-root-module {}".format(native_linkable_target, module),
            )
            expect(
                not native_linkable.for_primary_apk or not native_linkable.can_be_asset,
                "{} which is marked as needing to be in the primary APK cannot be an asset".format(native_linkable_target),
            )

            if is_root_module(module):
                if native_linkable.can_be_asset and package_native_libs_as_assets_enabled:
                    native_libs_assets_dir = paths.join(_get_native_libs_as_assets_dir(module))
                    so_name_path = paths.join(native_libs_assets_dir, abi_directory, so_name)
                    stripped_native_linkable_assets_for_primary_apk_srcs[so_name_path] = lib
                else:
                    so_name_path = paths.join(abi_directory, so_name)
                    if native_linkable.for_primary_apk:
                        stripped_native_linkables_always_in_primary_apk_srcs[so_name_path] = lib
                    else:
                        stripped_native_linkables_srcs[so_name_path] = lib
            else:
                module_dir = "assets" if native_linkable.can_be_asset and package_voltron_native_libs_as_assets_enabled else "lib"
                so_name_path = paths.join(_get_native_libs_as_assets_dir(module), module_dir, abi_directory, so_name)
                stripped_native_linkable_module_assets_srcs.setdefault(module, {})[so_name_path] = lib

    stripped_native_linkables = ctx.actions.symlinked_dir(
        name_prefix + "stripped_native_linkables",
        stripped_native_linkables_srcs,
        has_content_based_path = False,
    )
    stripped_native_linkables_always_in_primary_apk = ctx.actions.symlinked_dir(
        name_prefix + "stripped_native_linkables_always_in_primary_apk",
        stripped_native_linkables_always_in_primary_apk_srcs,
        has_content_based_path = False,
    )
    stripped_native_linkable_assets_for_primary_apk = (
        ctx.actions.symlinked_dir(
            name_prefix + "stripped_native_linkables_assets_for_primary_apk",
            stripped_native_linkable_assets_for_primary_apk_srcs,
            has_content_based_path = False,
        )
        if stripped_native_linkable_assets_for_primary_apk_srcs
        else None
    )
    stripped_native_linkable_module_assets_map = {}
    for module, srcs in stripped_native_linkable_module_assets_srcs.items():
        stripped_native_linkable_module_assets_map[module] = ctx.actions.symlinked_dir(
            name_prefix + "stripped_native_linkable_assets_for_module_{}".format(module),
            srcs,
            has_content_based_path = False,
        )

    return _StrippedNativeLinkables(
        linkables = stripped_native_linkables,
        linkables_always_in_primary_apk = stripped_native_linkables_always_in_primary_apk,
        linkable_assets_for_primary_apk = stripped_native_linkable_assets_for_primary_apk,
        linkable_module_assets_map = stripped_native_linkable_module_assets_map,
    )

def _get_native_libs_as_assets_metadata(ctx: AnalysisContext, native_lib_assets: list[Artifact], module: str, name_prefix: str = "") -> Artifact:
    native_lib_assets_file = argfile(
        actions = ctx.actions,
        name = "{}{}/native_lib_assets".format(name_prefix, module),
        args = [cmd_args([native_lib_asset, _get_native_libs_as_assets_dir(module)], delimiter = "/") for native_lib_asset in native_lib_assets],
    )
    metadata_output = ctx.actions.declare_output("{}{}/native_libs_as_assets_metadata.txt".format(name_prefix, module), has_content_based_path = False)
    metadata_cmd = cmd_args([
        ctx.attrs._android_toolchain[AndroidToolchainInfo].native_libs_as_assets_metadata[RunInfo],
        "--native-library-dirs",
        native_lib_assets_file,
        "--metadata-output",
        metadata_output.as_output(),
    ])
    ctx.actions.run(metadata_cmd, category = "get_native_libs_as_assets_metadata", identifier = name_prefix + module, allow_cache_upload = True)
    return metadata_output

def _get_native_libs_as_assets_dir(module: str) -> str:
    return "assets/{}".format("lib" if is_root_module(module) else module)

def get_default_shared_libs(ctx: AnalysisContext, deps: list[Dependency], shared_libraries_to_exclude: set) -> dict[str, SharedLibrary]:
    shared_library_info = merge_shared_libraries(
        ctx.actions,
        deps = filter(None, [x.get(SharedLibraryInfo) for x in deps]),
    )

    # Filter out shared libraries whose label.raw_target() is in shared_libraries_to_exclude
    # Do it before calling with_unique_str_sonames to skip duplication check for excluded libs
    shared_libraries = [
        shared_lib
        for shared_lib in traverse_shared_library_info(shared_library_info, transformation_provider = None)
        if (shared_lib.label.raw_target() not in shared_libraries_to_exclude)
    ]
    return {soname: shared_lib for soname, shared_lib in with_unique_str_sonames(shared_libraries).items()}

_LinkableSharedNode = record(
    raw_target = field(str),
    soname = field(str | None),
    labels = field(list[str], []),
    # Linkable deps of this target.
    deps = field(list[Label], []),
    can_be_asset = field(bool),
    force_static = field(bool),
)

def encode_linkable_graph_for_mergemap(graph_node_map_by_platform: dict[str, dict[Label, LinkableNode]]) -> dict[str, dict[Label, _LinkableSharedNode]]:
    return {
        platform: {
            target: _LinkableSharedNode(
                raw_target = str(target.raw_target()),
                soname = node.default_soname,
                labels = node.labels,
                deps = node.deps + node.exported_deps,
                can_be_asset = node.can_be_asset,  # and not node.exclude_from_android_merge
                force_static = node.preferred_linkage == Linkage("static"),
            )
            for target, node in graph_node_map.items()
        }
        for platform, graph_node_map in graph_node_map_by_platform.items()
    }

# Debugging info about the linkables merge process. All of this will be written in one of the outputs of
# the `[native_merge_debug]` subtarget.
MergedLinkablesDebugInfo = record(
    unmerged_statics = list[str],
    group_debug = dict[str, typing.Any],
    with_default_soname = list[typing.Any],
    missing_default_solibs = list[Label],
)

# As shared lib output of the linkables merge process. This is not necessarily an actually merged node (there
# may be just one constituent)
MergedSharedLibrary = record(
    soname = str,
    lib = SharedLibrary,
    apk_module = str,
    # this only includes solib constituents that are included in the android merge map
    solib_constituents = list[str],
    # targets that correspond to solib_constituents (i.e., targets with include_in_android_mergemap = True)
    solib_constituent_targets = list[Label],
    is_actually_merged = bool,
    primary_constituents = list[Label],
)

# information about a link group derived from the merge mapping
LinkGroupData = record(
    group_name = [str, Label],
    constituents = list[Label],
    apk_module = str,
)

# Lookup key for somerge groups, either the soname for shared libraries or the target name for unmerged statics
GroupLabel = str

# Represents the primary constituents and deps of primary constituents used to create a LinkGroupLinkableNode for a non-prebuilt shared library.
LinkGroupMergeInfo = record(
    label = GroupLabel,
    deps = list[GroupLabel],
    exported_deps = list[GroupLabel],
    constituent_link_infos = list[LinkInfo],
)

# Represents a node in the final merged linkable map. Most of these will be shared libraries, either prebuilt shared libs or
# libraries that are created below for a node in the link_groups_graph. The exception is for non-merged static-only nodes.
LinkGroupLinkableNode = record(
    # The LinkInfo to add to the link line for a node that links against this.
    link = LinkInfo,
    deps = list[GroupLabel],
    exported_deps = list[GroupLabel],
    shared_lib = [SharedLibrary, None],
    # linker flags to be exported by any node that links against this. This can only be non-None for non-merged static only nodes (as we don't
    # propagate exported linker flags through transitive shared lib deps).
    exported_linker_flags = [(list[typing.Any], list[typing.Any]), None],
)

def write_merged_library_map(ctx: AnalysisContext, shared_libs_by_platform: dict[str, dict[str, MergedSharedLibrary]]) -> Artifact:
    """
    Writes the "merged library map". This is a map of original soname to final soname of the form:

    ```
    original_soname1 final_soname1
    original_soname2 final_soname1
    original_soname3 final_soname2
    ...
    ```
    """
    solib_map = {}  # dict[final_soname, set[original_soname]]
    for _, shared_libs in shared_libs_by_platform.items():
        for soname in shared_libs.keys():
            merged_shared_lib = shared_libs[soname]
            if merged_shared_lib.is_actually_merged:
                solib_map.setdefault(soname, set()).update(merged_shared_lib.solib_constituents)

    lines = []
    for final_soname in sorted(solib_map.keys()):
        for original_soname in solib_map[final_soname]:
            lines.append("{} {}".format(original_soname, final_soname))

    # we wanted it sorted by original_soname
    return ctx.actions.write("merged_library_map.txt", sorted(lines), has_content_based_path = False)

def run_mergemap_codegen(ctx: AnalysisContext, merge_generator: Dependency, merged_library_map: Artifact) -> Artifact:
    mapping_java = ctx.actions.declare_output("MergedLibraryMapping.java", has_content_based_path = False)
    args = cmd_args(merge_generator[RunInfo])
    args.add([merged_library_map, mapping_java.as_output()])
    ctx.actions.run(args, category = "mergemap_codegen", allow_cache_upload = True)
    return mapping_java

def write_jni_on_load_mappings(ctx: AnalysisContext, shared_libs_by_platform: dict[str, dict[str, MergedSharedLibrary]]) -> Artifact:
    """
    Writes a mapping from merged library soname to the targets that are included in the android merge map.

    This only includes targets with include_in_android_mergemap = True, which are the same targets
    that appear in merged_library_map.txt.

    The output format matches the merge sequence format:
    ```
    (
        "libappdrivenaudiomerged.so",
        [
            "fbandroid/java/com/facebook/rsys/appdrivenaudio:appdrivenaudio-jni",
            "fbandroid/java/com/facebook/rsys/audio/frame:frame-jni",
        ],
    ),
    ```
    """
    solib_to_targets = {}  # dict[final_soname, list[str]]
    for shared_libs in shared_libs_by_platform.values():
        for soname, merged_shared_lib in shared_libs.items():
            if not merged_shared_lib.is_actually_merged:
                continue

            # Only use solib_constituent_targets, which are targets with include_in_android_mergemap = True
            solib_to_targets.setdefault(soname, set()).update(merged_shared_lib.solib_constituent_targets)

    lines = []
    for soname, targets in sorted(solib_to_targets.items()):
        sorted_targets = sorted([t.raw_target() for t in targets])

        # Format targets as a list with proper indentation
        if len(sorted_targets) == 0:
            targets_str = "[]"
        elif len(sorted_targets) == 1:
            targets_str = '["{}"]'.format(sorted_targets[0])
        else:
            target_lines = ",\n            ".join(['"{}"'.format(t) for t in sorted_targets])
            targets_str = "[\n            {},\n        ]".format(target_lines)
        lines.append('    (\n        "{}",\n        {},\n    ),'.format(soname, targets_str))

    return ctx.actions.write("jni_on_load_mappings.txt", "\n".join(lines), has_content_based_path = False)

# We can't merge a prebuilt shared (that has no archive) and must use it's original info.
# Ideally this would probably be structured info on the linkablenode.
def _is_prebuilt_shared(node_data: LinkableNode) -> bool:
    shared_link_info = node_data.link_infos.get(LibOutputStyle("shared_lib"), None)
    if not shared_link_info or not shared_link_info.default.linkables:
        return False
    pic_archive_info = node_data.link_infos.get(LibOutputStyle("pic_archive"), None)
    if not pic_archive_info or not pic_archive_info.default.linkables:
        return True
    return False

def _has_linkable(node_data: LinkableNode) -> bool:
    for _, output in node_data.link_infos.items():
        if output.default.linkables:
            return True
    return False

def _platform_output_path(path: str, platform: [str, None] = None):
    if platform:
        return platform + "/" + path
    return path

def _transitive_has_non_prebuilt_linkable_deps(target: Label, linkable_nodes: dict[Label, LinkableNode], transitive_linkable_cache: dict[Label, bool]) -> bool:
    if target in transitive_linkable_cache:
        return transitive_linkable_cache[target]

    target_node = linkable_nodes.get(target)
    for dep in target_node.deps:
        dep_node = linkable_nodes.get(dep)
        if (_has_linkable(dep_node) and not _is_prebuilt_shared(dep_node)) or _transitive_has_non_prebuilt_linkable_deps(
            dep, linkable_nodes, transitive_linkable_cache
        ):
            transitive_linkable_cache[target] = True
            return True
    for dep in target_node.exported_deps:
        dep_node = linkable_nodes.get(dep)
        if (_has_linkable(dep_node) and not _is_prebuilt_shared(dep_node)) or _transitive_has_non_prebuilt_linkable_deps(
            dep, linkable_nodes, transitive_linkable_cache
        ):
            transitive_linkable_cache[target] = True
            return True

    transitive_linkable_cache[target] = False
    return False

def _shared_lib_for_prebuilt_shared(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    target: Label,
    node_data: LinkableNode,
    linkable_nodes: dict[Label, LinkableNode],
    transitive_linkable_cache: dict[Label, bool],
    platform: [str, None] = None,
) -> SharedLibrary:
    expect(
        len(node_data.shared_libs.libraries) == 1,
        "unexpected shared_libs length for somerge of {} ({})".format(target, node_data.shared_libs),
    )

    # TODO(cjhopman): We don't currently support prebuilt shared libs with deps on other libs because
    # we don't compute the shared lib deps of prebuilt shared libs here. That
    # shouldn't be too hard, but we haven't needed it.
    expect(
        not _transitive_has_non_prebuilt_linkable_deps(target, linkable_nodes, transitive_linkable_cache),
        "prebuilt shared library `{}` with non-prebuilt linkable deps not supported by somerge".format(target),
    )

    shlib = node_data.shared_libs.libraries[0]
    soname = shlib.soname.ensure_str()
    shlib = shlib.lib
    output_path = _platform_output_path(soname, platform)
    return create_shlib(
        lib = shlib,
        stripped_lib = strip_lib(ctx, cxx_toolchain, shlib.output, output_path),
        link_args = None,
        shlib_deps = None,
        can_be_asset = node_data.can_be_asset,
        for_primary_apk = False,
        soname = soname,
        label = target,
    )

def _run_jni_onload_check(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    check_inputs: list,
    output: Artifact,
):
    """Verify that no input to a merged library defines JNI_OnLoad.

    A constituent with allow_jni_merging = True has its JNI_OnLoad renamed by
    jni_lib_merge.h, and the JNI_OnLoad symbol in the merged library is
    manufactured by -Wl,--defsym. --defsym silently overrides a constituent's
    own definition rather than colliding with it, so a constituent that has a
    JNI_OnLoad but did not opt in has it discarded with no diagnostic and is
    never registered on MergedSoMapping$Invoke_JNI_OnLoad -- an
    UnsatisfiedLinkError at runtime and nothing at build time.

    This has to inspect the link *inputs*: by the time the merged library
    exists, --defsym has erased the evidence.
    """
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]

    # write_json renders each artifact as its path and, with with_inputs, makes
    # the objects inputs of the action so they are materialized for the scan.
    manifest = ctx.actions.write_json(
        "jni_onload_check_inputs.json",
        [{"artifact": obj, "merged_lib": soname, "target": target} for soname, target, obj in check_inputs],
        with_inputs = True,
        has_content_based_path = False,
    )
    ctx.actions.run(
        cmd_args(
            android_toolchain.jni_onload_check[RunInfo],
            "--inputs",
            manifest,
            "--nm",
            cxx_toolchain.binary_utilities_info.nm,
            "--out",
            output.as_output(),
        ),
        category = "jni_onload_check",
        # Reads every object that goes into every merged library; running it
        # remotely avoids shipping them back for a local scan.
        prefer_remote = True,
    )

def _link_info_object_artifacts(link_info: LinkInfo) -> list[Artifact]:
    """The compiled inputs a LinkInfo contributes, for symbol inspection.

    Thin archives are represented by their member objects, which sidesteps the
    fact that a thin archive's member paths are relative to the archive itself
    and so cannot be read from an arbitrary working directory.
    """
    artifacts = []
    for linkable in link_info.linkables:
        if isinstance(linkable, ArchiveLinkable):
            if linkable.archive.external_objects:
                artifacts.extend(linkable.archive.external_objects)
            else:
                artifacts.append(linkable.archive.artifact)
        elif isinstance(linkable, ObjectsLinkable):
            artifacts.extend(linkable.objects or [])
    return artifacts

def _merge_map_from_regexes(native_library_merge_map: dict, linkable_nodes_by_platform: dict[str, dict[Label, LinkableNode]]) -> dict[str, dict[str, str]]:
    # Each target whose raw label one of a library's patterns matches merges
    # into that library, the first library in the map winning.
    merge_map_by_platform = {}
    merge_lib_to_fancy_regexes = {merge_lib: [regex(pattern, fancy = True) for pattern in patterns] for merge_lib, patterns in native_library_merge_map.items()}
    for platform, linkable_nodes in linkable_nodes_by_platform.items():
        merge_map = merge_map_by_platform.setdefault(platform, {})
        for target, _node in linkable_nodes.items():
            raw_target = str(target.raw_target())
            merge_result = None
            for merge_lib, fancy_regexes in merge_lib_to_fancy_regexes.items():
                for fancy_regex in fancy_regexes:
                    if fancy_regex.match(raw_target):
                        merge_result = merge_lib
                        break
                if merge_result:
                    break
            if merge_result:
                merge_map[str(target)] = merge_result
    return merge_map_by_platform

def _get_merged_linkables_for_platform(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    native_library_merge_linker_args_all: list,
    platform: str | None,
    glue_linkable: [(Label, LinkInfo), None],
    default_shared_libs: dict[str, SharedLibrary],
    linkable_nodes: dict[Label, LinkableNode],
    merge_map: dict[str, [str, None]],
    merge_linker_args: dict[str, typing.Any],
    apk_module_graph: typing.Callable,
    jni_onload_check_inputs: list | None = None,
) -> (dict[str, MergedSharedLibrary], MergedLinkablesDebugInfo):
    """
    This takes the merge mapping and constructs the resulting merged shared libraries.

    This is similar to link_groups used by ios and cxx_binary, but is sufficiently different that it needs its
    own implementation. Potentially we could find a generalization of them that covers both.

    An overview of how this works:

    The input includes a mapping of target -> link group. This mapping does not need to be comprehensive, a target
    not in that mapping is assigned to its own link group. The input also contains a mapping of target -> apk module,
    and this must have a mapping for every target. A link group cannot contain targets in different apk modules.

    Once we have the mapping of link groups, we construct an analog of the LinkableGraph on the new link group graph.
    This new graph consists of LinkGroupLinkableNodes (in some sense, the merged equivalent of LinkableNode).
    We traverse this graph from the bottom up, producing link info for each node.

    First, there are some special cases:

    If a target is "not actually merged" (i.e. it's in a link group where it is the only constituent) then it may get
    special handling if:
        (1) it is lib with static preferred linkage or it contains no linkables (i.e. no code)
        (2) a prebuild shared lib
    For both these cases we will produce a LinkGroupLinkableNode that basically reuses their original information. For
    every other case we will produce a node that represents a shared library that we define.

    When constructing a LinkableNode for a link group, we will be traversing sort of a hybrid graph, as we will traverse
    the primary constituents of the link group itself in the original LinkableGraph, but dependencies outside of the link
    group will be mapped to the corresponding LinkGroupLinkableNode (and potentially then further traversing that node's
    deps).

    The merge mapping input determines the "primary constituents" of each link group. The "real constituents" of that link
    group will include all those primary constituents, all of the transitive non-shared lib deps in the linkgroup linkable
    graph, and then all the shared lib dependencies of all of them (that is describing a traversal similar to the
    normal link strategies).

    There are some subtle differences in the handling of primary constituents and the statically linked non-primary
    constituents:
    1. all primary constituents are public nodes, non primary ones are only public if they are transitively exported_deps
    of a primary constituent. A public node is linked via "link whole".
    2. linker_flags of primary constituents are included in the link, for non primary they are not
    """
    debug_info = MergedLinkablesDebugInfo(
        unmerged_statics = [],
        group_debug = {},
        with_default_soname = [],
        missing_default_solibs = [],
    )

    linkable_nodes_graph = {k: dedupe(v.deps + v.exported_deps) for k, v in linkable_nodes.items()}
    topo_sorted_targets = pre_order_traversal(linkable_nodes_graph)

    # first we collect basic information about each link group, this will populate the fields in LinkGroupData and
    # map target labels to their link group name.
    link_groups = {}
    target_to_link_group = {}

    # Because we cannot attach this to the LinkableNode after the fact, declare a cache for each platform
    transitive_linkable_cache = {}

    for target in topo_sorted_targets:
        expect(target not in target_to_link_group, "prelude internal error, target seen twice?")
        target_apk_module = apk_module_graph(str(target.raw_target()))

        link_group = merge_map.get(str(target), None)
        if not link_group:
            link_group = str(target)
            link_groups[link_group] = LinkGroupData(
                group_name = target,
                constituents = [target],
                apk_module = target_apk_module,
            )
        elif link_group in link_groups:
            link_group_data = link_groups[link_group]

            # TODO(cjhopman): buck1 provides a more useful error here in that it lists the module mappings for all
            # constituents of the merge group (rather than just one conflict). That allows users to resolve all the
            # issues at once. With merge sequence merging (the replacement for merge map), this error shouldn't ever be hit
            # and so maybe it's not necessary to improve it.
            expect(
                link_group_data.apk_module == target_apk_module,
                "Native library merge of {} has inconsistent application module mappings:\n{} is in module {}\n{} is in module {}",
                link_group_data.group_name,
                target,
                target_apk_module,
                link_group_data.constituents[0],
                link_group_data.apk_module,
            )
            link_groups[link_group].constituents.append(target)
        else:
            link_groups[link_group] = LinkGroupData(
                group_name = link_group,
                constituents = [target],
                apk_module = target_apk_module,
            )

        target_to_link_group[target] = link_group

    # Now that all targets are assigned to a link group, build up the link group graph.
    link_groups_graph_builder = {}
    for target in topo_sorted_targets:
        target_group = target_to_link_group[target]
        group_deps = link_groups_graph_builder.setdefault(target_group, {})
        for dep in linkable_nodes_graph[target]:
            dep_group = target_to_link_group[dep]
            if target_group != dep_group and dep_group not in group_deps:
                # Store one example of why target_group depends on dep_group
                group_deps[dep_group] = (target, dep)
    link_groups_graph = {k: list(v.keys()) for k, v in link_groups_graph_builder.items()}

    archive_output_style = LibOutputStyle("pic_archive")
    shlib_output_style = LibOutputStyle("shared_lib")

    link_group_linkable_nodes = {}
    group_shared_libs = {}
    included_default_solibs = {}

    def edge_explainer(src_group, dest_group):
        """Explains in an error why src_group has a dependency on dest_group"""
        if src_group not in link_groups_graph_builder or dest_group not in link_groups_graph_builder[src_group]:
            return ["Unknown"]

        src_target, dest_target = link_groups_graph_builder[src_group][dest_group]
        return ["   " + str(src_target), "-> " + str(dest_target)]

    # Now we will traverse from the leaves up the graph (the link groups graph). As we traverse, we will produce
    # a link group linkablenode for each group.
    for group in post_order_traversal(link_groups_graph, edge_explainer = edge_explainer):
        group_data = link_groups[group]
        is_actually_merged = len(group_data.constituents) > 1

        non_asset_constituents = [target for target in group_data.constituents if not linkable_nodes[target].can_be_asset]
        can_be_asset = len(non_asset_constituents) == 0

        if not is_actually_merged:
            target = group_data.constituents[0]
            node_data = linkable_nodes[target]

            if node_data.preferred_linkage == Linkage("static") or not _has_linkable(node_data):
                debug_info.unmerged_statics.append(target)
                link_group_linkable_nodes[group] = LinkGroupLinkableNode(
                    link = node_data.link_infos[archive_output_style].default,
                    deps = dedupe_by_value([target_to_link_group[t] for t in node_data.deps]),
                    exported_deps = dedupe_by_value([target_to_link_group[t] for t in node_data.exported_deps]),
                    shared_lib = None,
                    exported_linker_flags = (node_data.linker_flags.exported_flags, node_data.linker_flags.exported_post_flags),
                )
                continue

            if _is_prebuilt_shared(node_data):
                shared_lib = _shared_lib_for_prebuilt_shared(
                    ctx,
                    cxx_toolchain,
                    target,
                    node_data,
                    linkable_nodes,
                    transitive_linkable_cache,
                    platform,
                )
                link_group_linkable_nodes[group] = LinkGroupLinkableNode(
                    link = node_data.link_infos[shlib_output_style].default,
                    deps = [],
                    exported_deps = [],
                    shared_lib = shared_lib,
                    # exported linker flags for shared libs are in their linkinfo itself and are not exported from dependents
                    exported_linker_flags = None,
                )
                group_shared_libs[shared_lib.soname.ensure_str()] = MergedSharedLibrary(
                    soname = shared_lib.soname.ensure_str(),
                    lib = shared_lib,
                    apk_module = group_data.apk_module,
                    solib_constituents = [],
                    solib_constituent_targets = [],
                    is_actually_merged = False,
                    primary_constituents = [target],
                )
                continue

        exported_linker_flags = []
        exported_linker_post_flags = []
        links = []

        if is_actually_merged and glue_linkable:
            links.append(set_link_info_link_whole(glue_linkable[1]))

        solib_constituents = []
        solib_constituent_targets = []
        group_deps = []
        group_exported_deps = []
        # (target, object) pairs for the JNI_OnLoad check. Collected here
        # because the constituents' link infos are not retained anywhere else,
        # and associated with the soname below once it is known.
        check_objects = []
        for key in group_data.constituents:
            expect(target_to_link_group[key] == group)
            node = linkable_nodes[key]

            default_solibs = list([shlib.soname.ensure_str() for shlib in node.shared_libs.libraries])
            if not default_solibs and node.preferred_linkage == Linkage("static"):
                default_solibs = [node.default_soname]

            for soname in default_solibs:
                included_default_solibs[soname] = True
                if node.include_in_android_mergemap:
                    solib_constituents.append(soname)
                    solib_constituent_targets.append(key)

            node = linkable_nodes[key]
            link_info = node.link_infos[archive_output_style].default

            if jni_onload_check_inputs != None:
                target_str = str(key.raw_target())
                for obj in _link_info_object_artifacts(link_info):
                    check_objects.append((target_str, obj))

            # the propagated link info should already be wrapped with exported flags.
            link_info = wrap_link_info(
                link_info,
                pre_flags = node.linker_flags.flags,
                post_flags = node.linker_flags.post_flags,
            )
            exported_linker_flags.extend(node.linker_flags.exported_flags)
            exported_linker_post_flags.extend(node.linker_flags.exported_post_flags)
            links.append(set_link_info_link_whole(link_info))

            dep_groups = [target_to_link_group[dep] for dep in node.deps]
            group_deps.extend([dep_group for dep_group in dep_groups if dep_group != group])

            exported_dep_groups = [target_to_link_group[dep] for dep in node.exported_deps]
            group_exported_deps.extend([dep_group for dep_group in exported_dep_groups if dep_group != group])

        soname = group
        if not is_actually_merged:
            soname = linkable_nodes[group_data.constituents[0]].default_soname
            debug_info.with_default_soname.append((soname, group_data.constituents[0]))

        # Only groups that are actually merged are checked. A group of one is a
        # normal standalone library, where defining JNI_OnLoad is correct.
        if jni_onload_check_inputs != None and is_actually_merged:
            for target_str, obj in check_objects:
                jni_onload_check_inputs.append((soname, target_str, obj))

        output_path = _platform_output_path(soname, platform)

        link_merge_info = LinkGroupMergeInfo(
            label = group,
            deps = dedupe_by_value(group_deps),
            exported_deps = dedupe_by_value(group_exported_deps),
            constituent_link_infos = links,
        )
        link_args, shlib_deps, link_deps_graph = _create_merged_link_args(
            root_target = link_merge_info,
            linkable_nodes = link_group_linkable_nodes,
            cxx_toolchain = cxx_toolchain,
        )
        link_args = [link_args]
        if native_library_merge_linker_args_all:
            link_args += [LinkArgs(flags = native_library_merge_linker_args_all)]
        if soname in merge_linker_args:
            link_args += [LinkArgs(flags = merge_linker_args[soname])]

        supports_shlib_interfaces = is_actually_merged or linkable_nodes[group_data.constituents[0]].supports_shlib_interfaces
        use_shlib_interface = supports_shlib_interfaces and cxx_toolchain.linker_info.shlib_interfaces == ShlibInterfacesMode("stub_from_library")
        # Gatorade tracks original shared-library artifacts by identity.
        # @oss-disable[end= ]: use_shlib_interface = use_shlib_interface and not getattr(ctx.attrs, "gatorade_phases", [])

        # Emit the Middle Gatorade container from the merge link; {} when off.
        # @oss-disable[end= ]: mg_args = middle_gatorade_merge_args(ctx, output_path, cxx_toolchain)
        shared_lib_result = _create_shared_lib(
            ctx,
            output_path = output_path,
            soname = soname,
            link_args = link_args,
            cxx_toolchain = cxx_toolchain,
            shared_lib_deps = [link_group_linkable_nodes[label].shared_lib.soname.ensure_str() for label in shlib_deps],
            label = group_data.constituents[0],
            can_be_asset = can_be_asset,
            produce_shared_library_interface = use_shlib_interface,
            # @oss-disable[end= ]: **mg_args,
        )
        shared_lib = shared_lib_result.shared_library

        link_group_linkable_nodes[group] = LinkGroupLinkableNode(
            link = LinkInfo(
                name = soname,
                pre_flags = exported_linker_flags,
                linkables = [
                    SharedLibLinkable(
                        lib = shared_lib_result.shared_library_interface or shared_lib.lib.output,
                    )
                ],
                post_flags = exported_linker_post_flags,
            ),
            deps = link_merge_info.deps,
            exported_deps = link_merge_info.exported_deps,
            shared_lib = shared_lib,
            # exported linker flags for shared libs are in their linkinfo itself and are not exported from dependents
            exported_linker_flags = None,
        )
        group_shared_libs[soname] = MergedSharedLibrary(
            soname = soname,
            lib = shared_lib,
            apk_module = group_data.apk_module,
            solib_constituents = solib_constituents,
            solib_constituent_targets = solib_constituent_targets,
            is_actually_merged = is_actually_merged,
            primary_constituents = group_data.constituents,
        )

        debug_info.group_debug.setdefault(
            group,
            struct(
                soname = soname,
                merged = is_actually_merged,
                primary_constituents = group_data.constituents,
                real_constituents = link_deps_graph.keys(),
                shlib_deps = shlib_deps,
                exported_linker_flags = exported_linker_flags,
                exported_linker_post_flags = exported_linker_post_flags,
                non_asset_constituents = non_asset_constituents,
                uses_shlib_interface = use_shlib_interface,
            ),
        )

    debug_info.missing_default_solibs.extend([d for d in default_shared_libs if d not in included_default_solibs])

    return group_shared_libs, debug_info

# The current default link strategies don't produce enough information in the
# SharedLibrary objects to perform relinking. To do that best, linking should be based on
# the LinkableGraph rather than the current approach with MergedLinkInfo.
# The overall plan for linking is to move to linkable graph-based linking, but for now
# we can do it just for the case that we need it.
def _create_all_relinkable_links(
    ctx: AnalysisContext,
    platform_to_original_native_linkables: dict[str, dict[str, SharedLibrary]],
    graph_node_map_by_platform: dict[str, dict[Label, LinkableNode]],
) -> (dict[str, dict[str, SharedLibrary]], dict[str, typing.Any]):
    final_platform_to_native_linkables = {}
    link_graphs_by_platform = {}
    for platform in platform_to_original_native_linkables:
        linkables, link_graphs = _create_relinkable_links(
            ctx,
            cxx_toolchain = ctx.attrs._cxx_toolchain[platform][CxxToolchainInfo],
            linkable_nodes = graph_node_map_by_platform[platform],
            platform = platform,
        )
        link_graphs_by_platform[platform] = link_graphs
        final_platform_to_native_linkables[platform] = linkables

    # sanity check that we produce the same list of linkables that are produced by standard linking.
    original_sonames = sorted(platform_to_original_native_linkables.keys())
    final_sonames = sorted(final_platform_to_native_linkables.keys())
    expect(original_sonames == final_sonames, "Unexpected differences in final sonames! {} {}".format(original_sonames, final_sonames))

    debug_outputs = {}

    # The biggest issue we could run into here is that we produce different link args than the original, so let's make that easy to debug.
    for platform in platform_to_original_native_linkables:
        for soname, lib in platform_to_original_native_linkables[platform].items():
            final = final_platform_to_native_linkables[platform][soname]
            original_args, _ = ctx.actions.write(
                "{}/{}/original.args".format(platform, soname),
                [unpack_link_args(args, LinkOrdering("topological")) for args in lib.link_args] if lib.link_args else "",
                allow_args = True,
                has_content_based_path = False,
            )
            final_args, _ = ctx.actions.write(
                "{}/{}/final.args".format(platform, soname),
                [unpack_link_args(args, LinkOrdering("topological")) for args in final.link_args] if final.link_args else "",
                allow_args = True,
                has_content_based_path = False,
            )
            debug_outputs["{}/{}/original.args".format(platform, soname)] = original_args
            debug_outputs["{}/{}/final.args".format(platform, soname)] = final_args

            if lib.label in link_graphs_by_platform[platform]:
                link_graph = ctx.actions.write_json(
                    "{}/{}/link.graph".format(platform, soname),
                    link_graphs_by_platform[platform][lib.label],
                    pretty = True,
                    has_content_based_path = False,
                )
                debug_outputs["{}/{}/link.graph".format(platform, soname)] = link_graph

            # TODO(cjhopman): should we also just produce a diff here? We could also consider creating sort of a merged diff or a list
            # of the differing argsfiles.
            # We can't compare them eagerly because the link args have large tsets that we don't want to traverse at analysis time.

    return final_platform_to_native_linkables, debug_outputs

def _create_relinkable_links(
    ctx: AnalysisContext, *, cxx_toolchain: CxxToolchainInfo, linkable_nodes: dict[Label, LinkableNode], platform: str
) -> (dict[str, SharedLibrary], dict[Label, dict[Label, list[Label]]]):
    linkable_nodes_graph = {target: value.deps + value.exported_deps for target, value in linkable_nodes.items()}
    shared_libs = {}
    shared_lib_overrides = {}
    transitive_linkable_cache = {}
    debug_link_deps = {}
    for target in post_order_traversal(linkable_nodes_graph):
        node = linkable_nodes[target]
        if node.preferred_linkage == Linkage("static") or not _has_linkable(node):
            continue

        if _is_prebuilt_shared(node):
            shared_lib = _shared_lib_for_prebuilt_shared(ctx, cxx_toolchain, target, node, linkable_nodes, transitive_linkable_cache, platform)
        else:
            soname = node.default_soname
            output_path = "relinkable-libs/{}/{}".format(platform, soname)
            link_args, shlib_deps, link_deps_graph = _create_link_args(
                cxx_toolchain = cxx_toolchain,
                root_target = target,
                node = node,
                graph = linkable_nodes,
                shared_lib_overrides = shared_lib_overrides,
            )
            debug_link_deps[target] = link_deps_graph
            shared_lib = create_shared_lib(
                ctx,
                output_path = output_path,
                soname = soname,
                link_args = [link_args],
                cxx_toolchain = cxx_toolchain,
                shared_lib_deps = [shared_lib_overrides[lib].name for lib in shlib_deps if lib in shared_lib_overrides],
                label = target,
                can_be_asset = node.can_be_asset,
            )
        shared_lib_overrides[target] = LinkInfo(
            name = shared_lib.soname.ensure_str(),
            pre_flags = node.linker_flags.exported_flags,
            linkables = [
                SharedLibLinkable(
                    lib = shared_lib.lib.output,
                )
            ],
            post_flags = node.linker_flags.exported_post_flags,
        )
        shared_libs[shared_lib.soname.ensure_str()] = shared_lib

    return {lib.soname.ensure_str(): lib for lib in shared_libs.values()}, debug_link_deps

def _create_link_args(
    *,
    cxx_toolchain: CxxToolchainInfo,
    root_target: Label,
    node: LinkableNode,
    graph: dict[Label, LinkableNode],
    shared_lib_overrides: dict[Label, LinkInfo] | None = None,
) -> (LinkArgs, list[Label], dict[Label, list[Label]]):
    if LinkOrdering(cxx_toolchain.linker_info.link_ordering) != LinkOrdering("topological"):
        fail("don't yet support link ordering {}".format(cxx_toolchain.linker_info.link_ordering))

    # TODO(cjhopman): verify picbehavior == pic
    link_strategy = node.default_link_strategy
    if not shared_lib_overrides:
        shared_lib_overrides = {}

    link_traversal_cache = {}

    def link_traversal(label: Label) -> list[Label]:
        def link_traversal_deps(label):
            node = graph[label]
            if label == root_target:
                return node.deps + node.exported_deps
            actual_linkable_type = get_lib_output_style(link_strategy, node.preferred_linkage, PicBehavior("supported"))
            if actual_linkable_type == LibOutputStyle("shared_lib"):
                return node.exported_deps
            else:
                return node.deps + node.exported_deps

        res = link_traversal_cache.get(label, None)
        if res:
            return res
        res = link_traversal_deps(label)
        link_traversal_cache[label] = res
        return res

    links = []
    shlib_deps = []
    for target in rust_matching_topological_traversal(None, [root_target], link_traversal):
        is_root = target == root_target
        node = graph[target]
        preferred_linkable_type = get_lib_output_style(link_strategy, node.preferred_linkage, PicBehavior("supported"))

        if is_root:
            link_info = node.link_infos[LibOutputStyle("pic_archive")].default
            link_info = wrap_link_info(
                link_info,
                pre_flags = node.linker_flags.flags,
                post_flags = node.linker_flags.post_flags,
            )
            links.append(set_link_info_link_whole(link_info))
        elif preferred_linkable_type == LibOutputStyle("shared_lib"):
            if target in shared_lib_overrides:
                links.append(shared_lib_overrides[target])
            else:
                links.append(node.link_infos[LibOutputStyle("shared_lib")].default)
            shlib_deps.append(target)
        else:
            links.append(node.link_infos[preferred_linkable_type].default)

    extra_runtime_flags = cxx_toolchain.linker_info.shared_dep_runtime_ld_flags or []
    if extra_runtime_flags:
        links.append(LinkInfo(pre_flags = extra_runtime_flags))
    return LinkArgs(infos = links), shlib_deps, link_traversal_cache

# Equivalent to _create_link_args but for somerge
def _create_merged_link_args(
    *, cxx_toolchain: CxxToolchainInfo, root_target: LinkGroupMergeInfo, linkable_nodes: dict[GroupLabel, LinkGroupLinkableNode]
) -> (LinkArgs, list[GroupLabel], dict[GroupLabel, list[GroupLabel]]):
    if LinkOrdering(cxx_toolchain.linker_info.link_ordering) != LinkOrdering("topological"):
        fail("don't yet support link ordering {}".format(cxx_toolchain.linker_info.link_ordering))

    link_traversal_cache = {}

    def link_traversal(label: GroupLabel) -> list[GroupLabel]:
        def link_traversal_deps(label: GroupLabel):
            if label == root_target.label:
                return root_target.deps + root_target.exported_deps

            linkable_node = linkable_nodes[label]
            if linkable_node.shared_lib:
                return linkable_node.exported_deps
            else:
                return linkable_node.deps + linkable_node.exported_deps

        res = link_traversal_cache.get(label, None)
        if res:
            return res
        res = link_traversal_deps(label)
        link_traversal_cache[label] = res
        return res

    links = []
    shlib_deps = []
    for label in rust_matching_topological_traversal(None, [root_target.label], link_traversal):
        if label == root_target.label:
            links.extend(root_target.constituent_link_infos)
        else:
            linkable_node = linkable_nodes[label]
            links.append(linkable_node.link)
            if linkable_node.shared_lib:
                shlib_deps.append(label)

    extra_runtime_flags = cxx_toolchain.linker_info.shared_dep_runtime_ld_flags or []
    if extra_runtime_flags:
        links.append(LinkInfo(pre_flags = extra_runtime_flags))
    return LinkArgs(infos = links), shlib_deps, link_traversal_cache

def _bolt_libraries(
    ctx: AnalysisContext,
    libraries_by_platform: dict[str, dict[str, SharedLibrary]],
    bolt_args: dict[str, ArgLike],
    native_cmd_entries = None,
    record_underlying_link: bool = False,
) -> (
    dict[str, dict[str, SharedLibrary]],
    dict[str, dict[str, SharedLibrary]],
):
    bolted_libraries_by_platform = {}
    pre_bolt_libraries_by_platform = {}

    for platform, shared_libraries in libraries_by_platform.items():
        cxx_toolchain = ctx.attrs._cxx_toolchain[platform][CxxToolchainInfo]
        bolted_libraries = bolted_libraries_by_platform.setdefault(platform, {})
        pre_bolt_libraries = pre_bolt_libraries_by_platform.setdefault(platform, {})

        for soname, _ in shared_libraries.items():
            original_shared_library = shared_libraries[soname]
            if not soname in bolt_args:
                bolted_libraries[soname] = original_shared_library
                continue

            # On plain-link apps (record_underlying_link), record the bolted soname's underlying link
            # BEFORE the swap: the bolted/pre-bolt objects below are command-less, so finalization
            # skips them and this plain link would otherwise be lost. When merging/relinking, that
            # underlying command is already captured at the merge/relink site, so skip it here.
            if record_underlying_link:
                record_link_command(native_cmd_entries, "link", platform, soname, original_shared_library.lib)

            # Create pre-BOLT copy with .pre_bolt.so suffix
            pre_bolt_soname = soname.removesuffix(".so") + ".pre_bolt.so"
            pre_bolt_output_path = "pre-bolt-libs/{}/{}".format(platform, pre_bolt_soname)
            pre_bolt_copy = ctx.actions.copy_file(pre_bolt_output_path, original_shared_library.lib.output, has_content_based_path = False)

            # Create pre-BOLT SharedLibrary object
            pre_bolt_linked_object = LinkedObject(
                output = pre_bolt_copy,
                unstripped_output = pre_bolt_copy,
            )

            pre_bolt_shared_lib = create_shlib(
                lib = pre_bolt_linked_object,
                stripped_lib = strip_lib(ctx, cxx_toolchain, pre_bolt_linked_object.output),
                can_be_asset = original_shared_library.can_be_asset,
                for_primary_apk = False,
                soname = pre_bolt_soname,
                label = original_shared_library.label,
            )

            pre_bolt_libraries[pre_bolt_soname] = pre_bolt_shared_lib

            output_path = "bolt-libs/{}/{}".format(platform, soname)

            bolted_libraries[soname] = _create_bolt_lib(
                ctx = ctx,
                cxx_toolchain = cxx_toolchain,
                prebolt_lib = original_shared_library,
                output_path = output_path,
                bolt_args = bolt_args[soname],
                native_cmd_entries = native_cmd_entries,
                platform = platform,
            )

    return bolted_libraries_by_platform, pre_bolt_libraries_by_platform

# When linking shared libraries, by default, all symbols are exported from the library. In a
# particular application, though, many of those symbols may never be used. Ideally, in each apk,
# each shared library would only export the minimal set of symbols that are used by other libraries
# in the apk. This would allow the linker to remove any dead code within the library (the linker
# can strip all code that is unreachable from the set of exported symbols).
#
# The native relinker tries to remedy the situation. When enabled for an apk, the native
# relinker will take the set of libraries in the apk and relink them in reverse order telling the
# linker to only export those symbols that are referenced by a higher library.
#
# The way this works is that the relinker does a topological traversal of the linked libraries (i.e.
# top-down, visiting nodes before their dependencies, this is the opposite order of most things we do
# in a build) and does:
# 1. extract the set of global symbols by the original lib
# 2. intersect that with the set of undefined symbols in all transitive dependents (i.e. higher in the graph) and
#    rules for required symbols (Java_*, Jni_Onload, relinker blocklist)
# 3. write a version script that says to make public only those symbols from (2)
# 4. link the lib with the exact same link line + the version script. Note that this means that the relinked libraries each are
#    actually linked against non-relinked ones. This does mean there's some risk of not detecting missing symbols (though mostly
#    only if they are caused by the relinker changes themselves).
# 5. extract the list of undefined symbols in the relinked libs (i.e. those symbols needed from dependencies and what had been
#    used in (1) above from higher nodes).

def relink_libraries(
    ctx: AnalysisContext, libraries_by_platform: dict[str, dict[str, SharedLibrary]], native_cmd_entries = None
) -> dict[str, dict[str, SharedLibrary]]:
    relinker_extra_deps = getattr(ctx.attrs, "relinker_extra_deps", None)
    relinker_extra_args = getattr(ctx.attrs, "relinker_extra_args", {})
    relinker_extra_args_all = getattr(ctx.attrs, "relinker_extra_args_all", [])
    red_linkables = {}
    if relinker_extra_deps:
        for red_elem in relinker_extra_deps:
            for platform, red in red_elem.items():
                red_link_graph = red.get(LinkableGraph)
                expect(red_link_graph != None, "relinker_extra_deps (`{}`) should be a linkable target", red.label)
                red_linkable = red_link_graph.nodes.value.linkable
                expect(red_linkable != None, "relinker_extra_deps (`{}`) should be a linkable target", red.label)
                expect(red_linkable.preferred_linkage == Linkage("static"), "buck2 currently only supports preferred_linkage='static' relinker_extra_deps")
                if platform not in red_linkables:
                    red_linkables[platform] = []
                red_linkables[platform].append((red.label, red_linkable.link_infos[LibOutputStyle("pic_archive")].default))

    relinked_libraries_by_platform = {}
    for platform, shared_libraries in libraries_by_platform.items():
        cxx_toolchain = ctx.attrs._cxx_toolchain[platform][CxxToolchainInfo]

        relinked_libraries = relinked_libraries_by_platform.setdefault(platform, {})
        unsupported_libs = {}
        shlib_graph = {}
        rev_shlib_graph = {}
        for soname, solib in shared_libraries.items():
            shlib_graph[soname] = []
            rev_shlib_graph.setdefault(soname, [])
            if solib.shlib_deps == None or solib.link_args == None:
                unsupported_libs[soname] = True
            else:
                for dep in solib.shlib_deps:
                    shlib_graph[soname].append(dep)
                    rev_shlib_graph.setdefault(dep, []).append(soname)
        needed_symbols_files = {}
        for soname in pre_order_traversal(shlib_graph):
            if soname in unsupported_libs:
                relinked_libraries[soname] = shared_libraries[soname]
                continue

            original_shared_library = shared_libraries[soname]
            output_path = "xdso-dce-relinker-libs/{}/{}".format(platform, soname)

            provided_symbols_file = extract_provided_symbols(ctx, cxx_toolchain, original_shared_library.lib.output)
            needed_symbols_for_this = [needed_symbols_files.get(rdep) for rdep in rev_shlib_graph[soname]]
            relinker_version_script = ctx.actions.declare_output(output_path + ".relinker.version_script", has_content_based_path = False)
            create_relinker_version_script(
                ctx.actions,
                output = relinker_version_script,
                relinker_allowlist = [regex(s) for s in ctx.attrs.relinker_whitelist],
                provided_symbols = provided_symbols_file,
                needed_symbols = needed_symbols_for_this,
            )
            relinker_link_args = (
                original_shared_library.link_args
                + [LinkArgs(flags = relinker_extra_args_all)]
                + ([LinkArgs(infos = [set_link_info_link_whole(red_linkable[1]) for red_linkable in red_linkables[platform]])] if len(red_linkables) > 0 else [])
                +
                # Add per-library linker args from relinker_extra_args attribute
                ([LinkArgs(flags = relinker_extra_args[soname])] if soname in relinker_extra_args else [])
                +
                # We add the version script last to allow easy post-processing if needed.
                [LinkArgs(flags = [cmd_args(relinker_version_script, format = "-Wl,--version-script={}")])]
            )

            extra_args = {} # @oss-enable
            # @oss-disable[end= ]: extra_args = add_gatorade_relinker_args(ctx, cxx_toolchain, output_path, soname, platform)
            relinker_output_args = get_extra_relinker_args(ctx, output_path, soname)
            extra_args = merge_extra_linker_args([extra_args, relinker_output_args])
            shared_lib = create_shared_lib(
                ctx,
                output_path = output_path,
                soname = soname,
                link_args = relinker_link_args,
                cxx_toolchain = cxx_toolchain,
                shared_lib_deps = original_shared_library.shlib_deps,
                label = original_shared_library.label,
                can_be_asset = original_shared_library.can_be_asset,
                **extra_args,
            )
            needed_symbols_from_this = extract_undefined_symbols(ctx, cxx_toolchain, shared_lib.lib.output)
            unioned_needed_symbols_file = ctx.actions.declare_output(output_path + ".all_needed_symbols", has_content_based_path = False)
            union_needed_symbols(ctx.actions, unioned_needed_symbols_file, needed_symbols_for_this + [needed_symbols_from_this])
            needed_symbols_files[soname] = unioned_needed_symbols_file

            relinked_libraries[soname] = shared_lib
            record_link_command(native_cmd_entries, "relink", platform, soname, shared_lib.lib)

    return relinked_libraries_by_platform

def _relink_for_native_libs(
    ctx: AnalysisContext,
    libraries_by_platform: dict[str, dict[str, SharedLibrary]],
    native_cmd_entries = None,
    check_report: Artifact | None = None,
    monogatorade_products: dict[str, Artifact] | None = None,
    excluded_libraries_by_platform: dict[str, dict[str, SharedLibrary]] = {},
) -> tuple:
    return (relink_libraries(ctx, libraries_by_platform, native_cmd_entries), {}) # @oss-enable
    # @oss-disable[end= ]: relink = relink_for_native_libs
    # @oss-disable[end= ]: products = (monogatorade_products, excluded_libraries_by_platform)
    # @oss-disable[end= ]: return relink(ctx, libraries_by_platform, relink_libraries, create_shared_lib, native_cmd_entries, check_report, *products)

def extract_provided_symbols(ctx: AnalysisContext, toolchain: CxxToolchainInfo, lib: Artifact) -> Artifact:
    return extract_defined_syms(ctx, toolchain, lib, "relinker_extract_provided_symbols")

def create_relinker_version_script(
    actions: AnalysisActions, relinker_allowlist: list[regex], output: Artifact, provided_symbols: Artifact, needed_symbols: list[Artifact]
):
    def create_version_script(ctx, artifacts, outputs):
        all_needed_symbols = {}
        for symbols_file in needed_symbols:
            for line in artifacts[symbols_file].read_string().splitlines():
                all_needed_symbols[line] = True

        symbols_to_keep = []
        for symbol in artifacts[provided_symbols].read_string().splitlines():
            keep_symbol = False
            if symbol in all_needed_symbols:
                keep_symbol = True
            elif symbol == "JNI_OnLoad":
                keep_symbol = True
            elif symbol.startswith("Java_"):
                keep_symbol = True
            else:
                for pattern in relinker_allowlist:
                    if pattern.match(symbol):
                        keep_symbol = True
                        break

            if keep_symbol:
                symbols_to_keep.append(symbol)

        version_script = "{\n"
        # @oss-disable[end= ]: if is_late_gatorade_enabled(ctx):
            # @oss-disable[end= ]: symbols_to_keep.append("*_Gatorade_Thunk")
        if symbols_to_keep:
            version_script += "global:\n"
        for symbol in symbols_to_keep:
            version_script += "  {};\n".format(symbol)
        version_script += "local: *;\n"
        version_script += "};\n"
        ctx.actions.write(outputs[output], version_script)

    actions.dynamic_output(dynamic = needed_symbols + [provided_symbols], inputs = [], outputs = [output.as_output()], f = create_version_script)

def extract_undefined_symbols(ctx: AnalysisContext, toolchain: CxxToolchainInfo, lib: Artifact) -> Artifact:
    return extract_undefined_syms(ctx, toolchain, lib, "relinker_extract_undefined_symbols")

def union_needed_symbols(actions: AnalysisActions, output: Artifact, needed_symbols: list[Artifact]):
    def compute_union(ctx, artifacts, outputs):
        unioned_symbols = {}
        for symbols_file in needed_symbols:
            for line in artifacts[symbols_file].read_string().splitlines():
                unioned_symbols[line] = True
        symbols = sorted(unioned_symbols.keys())
        ctx.actions.write(outputs[output], symbols)

    actions.dynamic_output(dynamic = needed_symbols, inputs = [], outputs = [output.as_output()], f = compute_union)

def strip_lib(ctx: AnalysisContext, cxx_toolchain: CxxToolchainInfo, shlib: Artifact, output_path: [str, None] = None):
    strip_flags = cmd_args(get_strip_non_global_flags(cxx_toolchain))
    return strip_object(
        ctx,
        cxx_toolchain,
        shlib,
        strip_flags,
        output_path = output_path,
        allow_cache_upload = True,
    )

_SharedLibraryResult = record(
    shared_library = SharedLibrary,
    shared_library_interface = Artifact | None,
)

def create_shared_lib(
    ctx: AnalysisContext,
    *,
    output_path: str,
    soname: str,
    link_args: list[LinkArgs],
    cxx_toolchain: CxxToolchainInfo,
    shared_lib_deps: list[str],
    label: Label,
    can_be_asset: bool,
    extra_linker_outputs_factory: typing.Callable | None = None,
    extra_linker_outputs_flags_factory: typing.Callable | None = None,
) -> SharedLibrary:
    return _create_shared_lib(
        ctx,
        output_path = output_path,
        soname = soname,
        link_args = link_args,
        cxx_toolchain = cxx_toolchain,
        shared_lib_deps = shared_lib_deps,
        label = label,
        can_be_asset = can_be_asset,
        extra_linker_outputs_factory = extra_linker_outputs_factory,
        extra_linker_outputs_flags_factory = extra_linker_outputs_flags_factory,
    ).shared_library

def _create_shared_lib(
    ctx: AnalysisContext,
    *,
    output_path: str,
    soname: str,
    link_args: list[LinkArgs],
    cxx_toolchain: CxxToolchainInfo,
    shared_lib_deps: list[str],
    label: Label,
    can_be_asset: bool,
    extra_linker_outputs_factory: typing.Callable | None = None,
    extra_linker_outputs_flags_factory: typing.Callable | None = None,
    produce_shared_library_interface: bool = False,
) -> _SharedLibraryResult:
    for link_arg in link_args:
        flags = link_arg.flags or []
        for info in link_arg.infos or []:
            flags += info.pre_flags or []
            flags += info.post_flags or []
        for flag in flags:
            flag = str(flag)
            if flag.endswith("--exclude-libs,ALL") or flag.endswith("--exclude-libs=ALL"):
                fail(
                    "The behavior of --exclude-libs,ALL is not predictable when building Android binaries and may cause runtime crashes, remove it from {} (or its merged constituents)".format(
                        label
                    )
                )
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = output_path,
        name = soname,
        opts = link_options(
            links = link_args,
            link_execution_preference = LinkExecutionPreference("any"),
            identifier = output_path,
            strip = False,
            cxx_toolchain = cxx_toolchain,
            extra_linker_outputs_factory = extra_linker_outputs_factory,
            extra_linker_outputs_flags_factory = extra_linker_outputs_flags_factory,
            produce_shared_library_interface = produce_shared_library_interface,
            allow_cache_upload = True,
        ),
    )

    shlib = link_result.linked_object
    return _SharedLibraryResult(
        shared_library = create_shlib(
            lib = shlib,
            stripped_lib = strip_lib(ctx, cxx_toolchain, shlib.output),
            shlib_deps = shared_lib_deps,
            link_args = link_args,
            can_be_asset = can_be_asset,
            for_primary_apk = False,
            soname = soname,
            label = label,
            extra_outputs = link_result.extra_outputs,
        ),
        shared_library_interface = link_result.shared_library_interface,
    )

def _create_bolt_lib(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    prebolt_lib: SharedLibrary,
    output_path: str,
    bolt_args: list[ArgLike],
    native_cmd_entries = None,
    platform = None,
) -> SharedLibrary:
    soname = prebolt_lib.soname.ensure_str()
    bolt_output = ctx.actions.declare_output(output_path, has_content_based_path = False)
    action_execution_properties = get_action_execution_attributes(LinkExecutionPreference("any"))

    args = cmd_args(
        cmd_args(cxx_toolchain.binary_utilities_info.bolt),
        prebolt_lib.lib.output,
        "-o",
        bolt_output.as_output(),
        bolt_args,
    )

    ctx.actions.run(
        args,
        category = "bolt",
        prefer_local = action_execution_properties.prefer_local,
        prefer_remote = action_execution_properties.prefer_remote,
        local_only = action_execution_properties.local_only,
        identifier = output_path,
        allow_cache_upload = True,
    )

    # Record the bolt command for [native_build_commands], referencing the plain output artifact
    # (not `as_output()`, which cannot be re-serialized by write_json).
    if native_cmd_entries != None:
        native_cmd_entries.append(
            native_build_command_entry(
                kind = "bolt",
                category = "bolt",
                arch = platform,
                soname = soname,
                output = bolt_output.short_path,
                argv = cmd_args(
                    cmd_args(cxx_toolchain.binary_utilities_info.bolt),
                    prebolt_lib.lib.output,
                    "-o",
                    bolt_output,
                    bolt_args,
                ),
                identifier = output_path,
            )
        )

    linked_object = LinkedObject(
        output = bolt_output,
        unstripped_output = bolt_output,
    )

    return create_shlib(
        lib = linked_object,
        stripped_lib = strip_lib(ctx, cxx_toolchain, linked_object.output),
        can_be_asset = prebolt_lib.can_be_asset,
        for_primary_apk = False,
        soname = soname,
        label = prebolt_lib.label,
    )
