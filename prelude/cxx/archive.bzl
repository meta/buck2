# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerInfo", "LinkerType")
load("@prelude//linking:link_info.bzl", "Archive", "new_archive")
load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:utils.bzl", "value_or")
load(":cxx_context.bzl", "get_cxx_toolchain_info")

def archive_flags(archiver_type: str, linker_type: LinkerType, use_archiver_flags: bool, symbol_table: bool, thin: bool) -> list[str]:
    if not use_archiver_flags:
        return []

    if archiver_type == "windows":
        if thin:
            fail("'windows' archiver doesn't support thin archives")
        return ["/Brepro", "/d2threads1"]
    elif archiver_type == "windows_clang":
        return ["/llvmlibthin"] if thin else []
    elif archiver_type == "amdclang":
        # amdclang can be used to create archives with --emit-static-lib, so let's
        # prefer to let the toolchain define the args instead of hardcoding them here.
        return []
    flags = ""

    # Operate in quick append mode, so that objects with identical basenames
    # won't overwrite one another.
    flags += "q"

    # Suppress warning about creating a new archive.
    flags += "c"

    # Run ranlib to generate symbol index for faster linking if requested.
    flags += "s" if symbol_table else "S"

    # Generate thin archives.
    if thin:
        flags += "T"

    # GNU archivers support generating deterministic archives.
    if linker_type == LinkerType("gnu"):
        flags += "D"

    return [flags]

_ARCHIVE_ENV = {"ZERO_AR_DATE": "1"}

# Create a static library from a list of object files.
def _archive(
    ctx: AnalysisContext, name: str, args: cmd_args, thin: bool, prefer_local: bool, allow_cache_upload: bool, force_disable_content_based_path: bool = False
) -> Artifact:
    toolchain = get_cxx_toolchain_info(ctx)
    has_content_based_path = (toolchain.linker_info.supports_content_based_paths_for_archiving == True) and not force_disable_content_based_path
    archive_output = ctx.actions.declare_output(name, has_content_based_path = has_content_based_path)
    command = cmd_args(toolchain.linker_info.archiver)
    archiver_type = toolchain.linker_info.archiver_type
    command.add(
        archive_flags(
            archiver_type,
            toolchain.linker_info.type,
            toolchain.linker_info.use_archiver_flags,
            toolchain.linker_info.archive_symbol_table,
            thin,
        )
    )
    if archiver_type == "windows" or archiver_type == "windows_clang":
        command.add([cmd_args(archive_output.as_output(), format = "/OUT:{}")])
    elif archiver_type == "amdclang":
        command.add(["-o", archive_output.as_output()])
    else:
        command.add([archive_output.as_output()])

    if toolchain.linker_info.archiver_supports_argfiles:
        shell_quoted_args = cmd_args(args, quote = "shell")
        if toolchain.linker_info.use_archiver_flags and toolchain.linker_info.archiver_flags != None:
            shell_quoted_args.add(toolchain.linker_info.archiver_flags)

        command.add(
            at_argfile(
                actions = ctx.actions,
                name = name + ".cxx_archive_argsfile",
                args = shell_quoted_args,
                allow_args = True,
                has_content_based_path = has_content_based_path,
            )
        )
    else:
        command.add(args)

    # By default, the archive header produced by `ar q` embeds the current unix
    # timestamp. With the GNU archiver we use `ar qD` (above in _archive_flags)
    # to make it produce a deterministic archive by zeroing the timestamp, but
    # other archivers do not support such a flag. Some implementations, notably
    # Xcode's, instead support zeroing the timestamp by way of an environment
    # variable.
    env = _ARCHIVE_ENV

    category = "archive"
    if thin:
        category = "archive_thin"
    ctx.actions.run(
        command,
        category = category,
        identifier = name,
        env = env,
        prefer_local = prefer_local,
        allow_cache_upload = allow_cache_upload,
    )
    return archive_output

def _archive_locally(ctx: AnalysisContext, linker_info: LinkerInfo) -> bool:
    archive_locally = linker_info.archive_objects_locally
    if hasattr(ctx.attrs, "_archive_objects_locally_override"):
        return value_or(ctx.attrs._archive_objects_locally_override, archive_locally)
    return archive_locally

def _archive_allow_cache_upload(ctx: AnalysisContext) -> bool:
    return getattr(ctx.attrs, "archive_allow_cache_upload", False)

def _archive_contents(ctx: AnalysisContext, linker_info: LinkerInfo) -> str:
    return getattr(ctx.attrs, "archive_contents", None) or linker_info.archive_contents

# Creates a static library given a list of object files.
def make_archive(
    ctx: AnalysisContext, name: str, objects: list[Artifact], hidden: list[Artifact] = [], force_disable_content_based_path: bool = False
) -> Archive:
    if len(objects) == 0:
        fail("no objects to archive")

    linker_info = get_cxx_toolchain_info(ctx).linker_info
    archive_contents = _archive_contents(ctx, linker_info)
    thin = archive_contents == "thin"
    object_args = cmd_args(objects)
    args = cmd_args(object_args, hidden = hidden)
    archive = _archive(
        ctx,
        name,
        args,
        thin = thin,
        prefer_local = _archive_locally(ctx, linker_info),
        allow_cache_upload = _archive_allow_cache_upload(ctx),
        force_disable_content_based_path = force_disable_content_based_path,
    )

    # TODO(T110378125): use argsfiles for GNU archiver for long lists of objects.
    # TODO(T110378123): for BSD archiver, split long args over multiple invocations.
    # TODO(T110378100): We need to scrub the static library (timestamps, permissions, etc) as those are
    # sources of non-determinism. See `ObjectFileScrubbers.createDateUidGidScrubber()` in Buck v1.

    return new_archive(
        artifact = archive,
        archive_contents_type = archive_contents,
        external_objects = objects,
        hidden = hidden,
        object_args = object_args,
    )
