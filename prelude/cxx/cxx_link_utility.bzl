# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifact_tset.bzl", "project_artifacts")
load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_apple_linker_flags.bzl", "apple_extra_darwin_linker_flags")
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "CxxToolchainInfo",
    "LinkerType",
)
load("@prelude//cxx:debug.bzl", "SplitDebugMode")
load("@prelude//cxx:linker.bzl", "get_rpath_origin")
load("@prelude//cxx:target_sdk_version.bzl", "get_target_triple")
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "LinkOrdering",  # @unused Used as a type
    "link_info_to_args",
    "unpack_link_args",
    "unpack_link_args_excluding_object_files_and_lazy_archives",
    "unpack_link_args_object_files_and_lazy_archives_only",
)
load("@prelude//linking:lto.bzl", "LtoMode")
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",  # @unused Used as a type
    "create_shlib_dwp_tree",
    "create_shlib_symlink_tree",
)
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type
load(
    "@prelude//utils:utils.bzl",
    "map_val",
)

def generates_split_debug(toolchain: CxxToolchainInfo):
    """
    Whether linking generates split debug outputs.
    """

    if toolchain.split_debug_mode == SplitDebugMode("none"):
        return False

    if toolchain.linker_info.lto_mode == LtoMode("none"):
        return False

    return True

def linker_supports_linker_maps(linker_type: LinkerType) -> bool:
    """
    Returns whether the given linker type supports generating linker maps.
    """
    return linker_type in [LinkerType("darwin"), LinkerType("gnu")]

def linker_map_args(toolchain: CxxToolchainInfo, linker_map) -> LinkArgs:
    linker_type = toolchain.linker_info.type
    if linker_type == LinkerType("darwin"):
        flags = [
            "-Xlinker",
            "-map",
            "-Xlinker",
            linker_map,
        ]
    elif linker_type == LinkerType("gnu"):
        flags = [
            "-Xlinker",
            "-Map",
            "-Xlinker",
            linker_map,
        ]
    else:
        fail("Linker type {} not supported for linker maps".format(linker_type))
    return LinkArgs(flags = flags)

def gc_sections_args(toolchain: CxxToolchainInfo, gc_sections_output) -> LinkArgs:
    """Generate linker args for gc-sections output file."""
    linker_type = toolchain.linker_info.type
    if linker_type == LinkerType("gnu"):
        # The linker wrapper (ld.py) handles --write-gc-sections-output flag
        flags = [
            "--write-gc-sections-output",
            gc_sections_output,
        ]
    else:
        fail("gc-sections output is only supported for GNU linker, got: {}".format(linker_type))
    return LinkArgs(flags = flags)

LinkArgsOutput = record(
    link_args = ArgLike,
    hidden = list[typing.Any],
    pdb_artifact = Artifact | None,
)

def get_extra_darwin_linker_flags(ctx: AnalysisContext) -> cmd_args:
    target_triple = get_target_triple(ctx)
    return cmd_args(apple_extra_darwin_linker_flags(target_triple))

def make_link_args(
    ctx: AnalysisContext,
    actions: AnalysisActions,
    cxx_toolchain_info: CxxToolchainInfo,
    links: list[LinkArgs],
    output_short_path: [str, None] = None,
    link_ordering: [LinkOrdering, None] = None,
    has_content_based_path: bool = False,
) -> LinkArgsOutput:
    """
    Merges LinkArgs. Returns the args, files that must be present for those
    args to work when passed to a linker, and optionally an artifact where DWO
    outputs will be written to.
    """
    args = cmd_args()
    hidden = []

    linker_info = cxx_toolchain_info.linker_info
    linker_type = linker_info.type

    link_ordering = link_ordering or map_val(LinkOrdering, linker_info.link_ordering)

    if linker_type == LinkerType("darwin"):
        args.add(get_extra_darwin_linker_flags(ctx))

    pdb_artifact = None
    if linker_info.is_pdb_generated and output_short_path != None:
        pdb_filename = paths.replace_extension(output_short_path, ".pdb")
        pdb_artifact = actions.declare_output(pdb_filename, has_content_based_path = has_content_based_path)
        hidden.append(pdb_artifact.as_output())

    if linker_type == LinkerType("darwin"):
        # We order inputs partially for binary size, and partially for historical
        # reasons. It is important that lazy object file (non link-whole archives)
        # be listed last on the command line so they are not unnecessarily loaded.
        # Some applications implicitly rely upon this ordering, so it is difficult
        # to change.  Note link_ordering configuration is not respected. The
        # link_metadata_flag is also ignored as that is a flag to the GNU linker wrapper,
        # which does not apply to Darwin.
        args.add(filter(None, [unpack_link_args_excluding_object_files_and_lazy_archives(link) for link in links]))
        args.add(filter(None, [unpack_link_args_object_files_and_lazy_archives_only(link) for link in links]))
    else:
        args.add(filter(None, [unpack_link_args(link, link_ordering = link_ordering, link_metadata_flag = linker_info.link_metadata_flag) for link in links]))

    return LinkArgsOutput(
        link_args = args,
        hidden = [args] + hidden,
        pdb_artifact = pdb_artifact,
    )

def shared_libs_symlink_tree_name(output: Artifact | str) -> str:
    if isinstance(output, Artifact):
        output = output.short_path
    return "__{}__shared_libs_symlink_tree".format(output)

def _dwp_symlink_tree_name(output: Artifact) -> str:
    return "__{}__dwp_symlink_tree".format(output.short_path)

ExecutableSharedLibArguments = record(
    extra_link_args = field(list[ArgLike], []),
    # Files that must be present for the executable to run successfully. These
    # are always materialized, whether the executable is the output of a build
    # or executed as a host tool.
    runtime_files = field(list[ArgLike], []),
    # Files needed to debug the executable. These need to be materialized when
    # this executable is the output of a build, but not when it is used by other
    # rules.
    external_debug_info = field(list[TransitiveSetArgsProjection], []),
    # Optional shared libs symlink tree symlinked_dir action.
    shared_libs_symlink_tree = field(list[Artifact] | Artifact | None, None),
    dwp_symlink_tree = field(list[Artifact] | Artifact | None, None),
)

CxxSanitizerRuntimeArguments = record(
    extra_link_args = field(list[ArgLike], []),
    sanitizer_runtime_files = field(list[Artifact], []),
)

# @executable_path/Frameworks

def cxx_sanitizer_runtime_arguments(ctx: AnalysisContext, cxx_toolchain: CxxToolchainInfo, output: Artifact) -> CxxSanitizerRuntimeArguments:
    linker_info = cxx_toolchain.linker_info
    target_sanitizer_runtime_enabled = ctx.attrs.sanitizer_runtime_enabled if hasattr(ctx.attrs, "sanitizer_runtime_enabled") else None
    sanitizer_runtime_enabled = target_sanitizer_runtime_enabled if target_sanitizer_runtime_enabled != None else linker_info.sanitizer_runtime_enabled
    if not sanitizer_runtime_enabled:
        return CxxSanitizerRuntimeArguments()

    if not linker_info.sanitizer_runtime_files:
        fail("C++ sanitizer runtime enabled but there are no runtime files")

    if linker_info.type == LinkerType("darwin"):
        # ignore_artifacts as the runtime directory is not required at _link_ time
        runtime_rpath = cmd_args(ignore_artifacts = True)
        runtime_files = linker_info.sanitizer_runtime_files
        for runtime_shared_lib in runtime_files:
            # Rpath-relative dylibs have an install name of `@rpath/libName.dylib`,
            # which means we need to add the parent dir of the dylib as an rpath.
            runtime_shared_lib_dir = cmd_args(runtime_shared_lib, parent = 1)

            # The parent dir of the runtime shared lib must appear as a path
            # relative to the parent dir of the binary. `@executable_path`
            # represents the parent dir of the binary, not the binary itself.
            runtime_shared_lib_rpath = cmd_args(runtime_shared_lib_dir, format = "-Wl,-rpath,@executable_path/{}", relative_to = (output, 1))
            runtime_rpath.add(runtime_shared_lib_rpath)

        return CxxSanitizerRuntimeArguments(
            extra_link_args = [runtime_rpath],
            sanitizer_runtime_files = runtime_files,
        )

    return CxxSanitizerRuntimeArguments()

def cxx_runtime_library_arguments(cxx_toolchain: CxxToolchainInfo) -> list[ArgLike]:
    # Runtime-library files (e.g. compiler-rt builtins) the toolchain provides are
    # linked here, toward the end of the link line, to match the Clang driver's
    # placement of compiler-rt. The consumer is responsible for stopping the driver
    # from also injecting these (e.g. `-nodefaultlibs`).
    linker_info = cxx_toolchain.linker_info
    link_infos = linker_info.runtime_library_files
    if not link_infos:
        return []
    if linker_info.type != LinkerType("gnu"):
        fail("runtime_library_files is only supported for the gnu linker, got {}".format(linker_info.type))
    return [link_info_to_args(link_info) for link_info in link_infos]

def executable_shared_lib_arguments(
    ctx: AnalysisContext, cxx_toolchain: CxxToolchainInfo, output: Artifact, shared_libs: list[SharedLibrary]
) -> ExecutableSharedLibArguments:
    def create_external_debug_info() -> list[TransitiveSetArgsProjection]:
        return project_artifacts(
            actions = ctx.actions,
            tsets = [shlib.lib.external_debug_info for shlib in shared_libs],
        )

    def create_shared_libs_symlink_tree_windows() -> list[Artifact]:
        return [
            ctx.actions.symlink_file(
                shlib.lib.output.basename,
                shlib.lib.output,
                has_content_based_path = False,
            )
            for shlib in shared_libs
        ]

    def create_shared_libs_symlink_trees(shared_libs_symlink_tree_name_arg: str, dwp_symlink_tree_name_arg: str) -> (Artifact, Artifact) | None:
        if not shared_libs:
            return None

        shared_libs_symlink_tree = create_shlib_symlink_tree(
            actions = ctx.actions,
            out = shared_libs_symlink_tree_name_arg,
            shared_libs = shared_libs,
        )
        dwp_symlink_tree = create_shlib_dwp_tree(ctx.actions, dwp_symlink_tree_name_arg, shared_libs)
        return (shared_libs_symlink_tree, dwp_symlink_tree)

    return executable_shared_lib_arguments_template(
        cxx_toolchain,
        output,
        create_external_debug_info,
        create_shared_libs_symlink_trees,
        create_shared_libs_symlink_tree_windows,
    )

def executable_shared_lib_arguments_template(
    cxx_toolchain: CxxToolchainInfo,
    output: Artifact,
    create_external_debug_info: typing.Callable[[], list[TransitiveSetArgsProjection]],
    create_shared_libs_symlink_trees: typing.Callable[[str, str], (Artifact, Artifact) | None],
    create_shared_libs_symlink_tree_windows: typing.Callable[[], list[Artifact]],
) -> ExecutableSharedLibArguments:
    """A generic/templated version of `executable_shared_lib_arguments` that takes in `Callable`s for constructing
    the shared libs symlink tree and external debug info.

    The shared libs symlink tree is constructed differently depending on if the SharedLibraries are stored in a `record`
    or in a `TransitiveSet`.
    """

    extra_link_args = []
    runtime_files = []
    shared_libs_symlink_tree = None

    # External debug info is materialized only when the executable is the output
    # of a build. Do not add to runtime_files.
    external_debug_info = create_external_debug_info()

    linker_type = cxx_toolchain.linker_info.type

    dwp_symlink_tree = None
    if linker_type == LinkerType("windows"):
        shared_libs_symlink_tree = create_shared_libs_symlink_tree_windows()
        runtime_files.extend(shared_libs_symlink_tree)
        # Windows doesn't support rpath.

    else:
        symlink_trees = create_shared_libs_symlink_trees(
            shared_libs_symlink_tree_name(output),
            _dwp_symlink_tree_name(output),
        )

        if symlink_trees:
            shared_libs_symlink_tree, dwp_symlink_tree = symlink_trees
            runtime_files.append(shared_libs_symlink_tree)
            rpath_reference = get_rpath_origin(linker_type)

            # We ignore_artifacts here since we don't want the symlink tree to actually be there for the link.
            rpath_arg = cmd_args(
                shared_libs_symlink_tree,
                format = "-Wl,-rpath,{}/{{}}".format(rpath_reference),
                ignore_artifacts = True,
                relative_to = (output, 1),
            )
            extra_link_args.append(rpath_arg)

    return ExecutableSharedLibArguments(
        extra_link_args = extra_link_args,
        runtime_files = runtime_files,
        external_debug_info = external_debug_info,
        shared_libs_symlink_tree = shared_libs_symlink_tree,
        dwp_symlink_tree = dwp_symlink_tree,
    )

LinkCmdParts = record(
    linker = [RunInfo, cmd_args],
    linker_flags = cmd_args,
    post_linker_flags = cmd_args,
    # linker + linker_flags, for convenience
    link_cmd = cmd_args,
)

def cxx_link_cmd_parts(toolchain: CxxToolchainInfo, executable: bool) -> LinkCmdParts:
    # `toolchain_linker_flags` can either be a list of strings, `cmd_args` or `None`,
    # so we need to do a bit more work to satisfy the type checker
    toolchain_linker_flags = toolchain.linker_info.linker_flags
    toolchain_post_linker_flags = toolchain.linker_info.post_linker_flags
    if toolchain_linker_flags == None:
        toolchain_linker_flags = cmd_args()
    elif not type(toolchain_linker_flags) == "cmd_args":
        toolchain_linker_flags = cmd_args(toolchain_linker_flags)

    if executable:
        toolchain_linker_flags = cmd_args(
            toolchain_linker_flags,
            toolchain.linker_info.executable_linker_flags,
        )

    if toolchain_post_linker_flags == None:
        toolchain_post_linker_flags = cmd_args()
    elif not type(toolchain_post_linker_flags) == "cmd_args":
        toolchain_post_linker_flags = cmd_args(toolchain_post_linker_flags)

    link_cmd = cmd_args(toolchain.linker_info.linker)
    link_cmd.add(toolchain_linker_flags)

    return LinkCmdParts(
        linker = toolchain.linker_info.linker,
        linker_flags = toolchain_linker_flags,
        post_linker_flags = toolchain_post_linker_flags,
        link_cmd = link_cmd,
    )
