# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_library_utility.bzl", "cxx_inherited_link_info")
load(
    "@prelude//cxx:cxx_link_utility.bzl",
    "ExecutableSharedLibArguments",
    "executable_shared_lib_arguments",
    "make_link_args",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//cxx:linker.bzl", "get_default_shared_library_name", "get_shared_library_name_linker_flags")
load(
    "@prelude//linking:link_info.bzl",
    "LinkStyle",
    "get_link_args_for_strategy",
    "to_link_strategy",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//linking:stamp_build_info.bzl", "stamp_build_info")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load(
    "@prelude//utils:utils.bzl",
    "filter_and_map_idx",
)
load(
    ":packages.bzl",
    "GoPkg",  # @Unused used as type
    "GoStdlib",
    "GoStdlibDynamicValue",
    "make_link_importcfg",
    "merge_pkgs",
)
load(":toolchain.bzl", "GoToolchainInfo", "get_toolchain_env_vars")

# Provider wrapping packages used for linking.
GoPkgLinkInfo = provider(
    fields = {
        "pkgs": provider_field(typing.Any, default = None),  # {str: "artifact"}
    }
)

GoBuildMode = enum(
    "exe",  # non-pic executable
    "pie",  # pic executable
    "c_shared",  # pic C-shared library
    "c_archive",  # pic C-static library
)

def _build_mode_param(mode: GoBuildMode) -> str:
    if mode == GoBuildMode("exe"):
        return "exe"
    if mode == GoBuildMode("pie"):
        return "pie"
    if mode == GoBuildMode("c_shared"):
        return "c-shared"
    if mode == GoBuildMode("c_archive"):
        return "c-archive"
    fail("unexpected: {}", mode)

def get_inherited_link_pkgs(deps: list[Dependency]) -> dict[str, GoPkg]:
    return merge_pkgs([d[GoPkgLinkInfo].pkgs for d in deps if GoPkgLinkInfo in d])

# TODO(cjhopman): Is link_style a LibOutputStyle or a LinkStrategy here? Based
# on returning an empty thing for link_style != shared, it seems likely its
# intended to be LibOutputStyle, but it's called in places that are passing what
# appears to be a LinkStrategy.
def _process_shared_dependencies(ctx: AnalysisContext, artifact: Artifact, deps: list[Dependency], link_style: LinkStyle) -> ExecutableSharedLibArguments:
    """
    Provides files and linker args needed to for binaries with shared library linkage.
    - the runtime files needed to run binary linked with shared libraries
    - linker arguments for shared libraries
    """
    if link_style != LinkStyle("shared"):
        return ExecutableSharedLibArguments()

    shlib_info = merge_shared_libraries(
        ctx.actions,
        deps = filter_and_map_idx(SharedLibraryInfo, deps),
    )
    shared_libs = traverse_shared_library_info(shlib_info, transformation_provider = None)

    return executable_shared_lib_arguments(
        ctx,
        ctx.attrs._cxx_toolchain[CxxToolchainInfo],
        artifact,
        shared_libs,
    )

def link(
    ctx: AnalysisContext,
    main: GoPkg,
    cgo_enabled: bool,
    pkgs: dict[str, GoPkg] = {},
    deps: list[Dependency] = [],
    build_mode: GoBuildMode = GoBuildMode("exe"),
    link_mode: [str, None] = None,
    link_style: LinkStyle = LinkStyle("static"),
    linker_flags: list[typing.Any] = [],
    external_linker_flags: list[typing.Any] = [],
):
    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]

    if not cgo_enabled and (go_toolchain.asan or go_toolchain.race):
        fail("`race=True` and `asan=True` are only supported when `cgo_enabled=True`")

    if go_toolchain.env_go_os == "windows":
        executable_extension = ".exe"
        shared_extension = ".dll"
        archive_extension = ".lib"
    else:
        executable_extension = ""
        shared_extension = ".so"
        archive_extension = ".a"

    if build_mode == GoBuildMode("c_shared"):
        file_extension = shared_extension
        use_shared_code = True  # PIC
        link_style = LinkStyle("shared")
    elif build_mode == GoBuildMode("c_archive"):
        file_extension = archive_extension
        use_shared_code = True  # PIC
        link_style = LinkStyle("static_pic")
    elif build_mode == GoBuildMode("pie"):
        file_extension = executable_extension
        use_shared_code = True  # PIC
        link_style = LinkStyle("static_pic")
    else:  # GoBuildMode("exe")
        file_extension = executable_extension
        use_shared_code = False  # non-PIC
    final_output_name = ctx.label.name + file_extension
    output = ctx.actions.declare_output(ctx.label.name + "-tmp" + file_extension, has_content_based_path = True)

    cmd = cmd_args()

    cmd.add(go_toolchain.go_wrapper)
    cmd.add(["--go", go_toolchain.linker])

    # Give internally-linked Go binaries a content-hash NT_GNU_BUILD_ID note.
    # Buck has no artifact content at analysis time, so reserve a zeroed note here
    # (-B 0x00..00) that the wrapper fills with sha256 post-link -- rewriting only a
    # still-all-zero descriptor, so an external link keeps its C++-set build-id.
    elf_go_os = ["linux", "android", "freebsd", "netbsd", "openbsd", "dragonfly", "illumos", "solaris"]
    emit_build_id = build_mode in [GoBuildMode("exe"), GoBuildMode("pie")] and link_mode in [None, "internal"] and go_toolchain.env_go_os in elf_go_os
    if emit_build_id:
        cmd.add("--gnu-build-id")  # wrapper flag, must precede "--"

    cmd.add("--")
    cmd.add(go_toolchain.linker_flags)

    cmd.add("-buildmode=" + _build_mode_param(build_mode))
    cmd.add("-buildid=")  # Setting to a static buildid helps make the binary reproducible.
    if emit_build_id:
        cmd.add("-B", "0x" + "00" * 20)  # reserve a 20-byte NT_GNU_BUILD_ID slot in PT_NOTE

    if go_toolchain.race:
        cmd.add("-race")

    if go_toolchain.asan:
        cmd.add("-asan")

    # Add inherited Go pkgs to library search path.
    all_pkgs = merge_pkgs([
        pkgs,
        get_inherited_link_pkgs(deps),
    ])

    identifier_prefix = ctx.label.name + "_" + _build_mode_param(build_mode)

    go_stdlib = ctx.attrs._go_stdlib[GoStdlib]

    executable_args = _process_shared_dependencies(ctx, output, deps, link_style)

    if link_mode == None:
        if build_mode == GoBuildMode("c_shared"):
            link_mode = "external"
        if build_mode == GoBuildMode("c_archive"):
            link_mode = "external"

    if link_mode != None:
        cmd.add("-linkmode", link_mode)

    cxx_toolchain_available = CxxToolchainInfo in ctx.attrs._cxx_toolchain
    if cxx_toolchain_available:
        cxx_toolchain = ctx.attrs._cxx_toolchain[CxxToolchainInfo]
        is_win = ctx.attrs._exec_os_type[OsLookup].os == Os("windows")

        # Gather external link args from deps.
        ext_links = get_link_args_for_strategy(
            ctx.actions,
            ctx.label,
            cxx_toolchain.linker_info,
            cxx_inherited_link_info(deps),
            to_link_strategy(link_style),
            prefer_stripped = False,
            transformation_spec_context = None,
        )
        ext_link_args_output = make_link_args(
            ctx,
            ctx.actions,
            cxx_toolchain,
            [ext_links],
        )
        ext_link_args = cmd_args(hidden = ext_link_args_output.hidden, quote = "shell")

        # Toolchain linker_flags must precede the objects, not go via cmd/link's -extldflags (which
        # it emits last): otherwise -Wl,-Bsymbolic-functions overrides cmd/link's own -Wl,-Bsymbolic
        # for c-shared, leaving runtime cgo data symbols preemptible and breaking the link.
        ext_link_args.add(cxx_toolchain.linker_info.linker_flags)
        ext_link_args.add(executable_args.extra_link_args)
        ext_link_args.add(external_linker_flags)
        ext_link_args.add(ext_link_args_output.link_args)

        if build_mode == GoBuildMode("c_shared") and go_toolchain.env_go_os != "windows":
            soname = get_default_shared_library_name(cxx_toolchain.linker_info, ctx.label)
            soname_flags = get_shared_library_name_linker_flags(cxx_toolchain.linker_info.type, soname)
            ext_link_args.add(soname_flags)

        # Delegate to C++ linker...
        # TODO: It feels a bit inefficient to generate a wrapper file for every
        # link.  Is there some way to etract the first arg of `RunInfo`?  Or maybe
        # we can generate the platform-specific stuff once and re-use?
        ext_link_argfile, _ = ctx.actions.write(
            output.short_path + ".go_ext_link_argsfile",
            ext_link_args,
            allow_args = True,
            has_content_based_path = True,
        )
        cxx_link_cmd = cmd_args(
            [
                cxx_toolchain.linker_info.linker,
                cmd_args(ext_link_argfile, format = "@{}"),
                "%*" if is_win else '"$@"',
            ],
            delimiter = " ",
        )
        linker_wrapper, _ = ctx.actions.write(
            "__{}_cxx_link_wrapper__.{}".format(identifier_prefix, "bat" if is_win else "sh"),
            ([] if is_win else ["#!/bin/sh"]) + [cxx_link_cmd],
            allow_args = True,
            is_executable = True,
            has_content_based_path = True,
        )
        cmd.add("-extld", linker_wrapper, cmd_args(hidden = [cxx_link_cmd, ext_link_args, ext_link_args_output.hidden]))
        # Kept here (not in the argfile) because these carry libraries (-lc, -ldl, ...) that must
        # follow the objects, and cmd/link emits -extldflags last.
        cmd.add(
            "-extldflags",
            cmd_args(
                go_toolchain.external_linker_flags,
                delimiter = " ",
                quote = "shell",
            ),
        )

    cmd.add(linker_flags)

    env = get_toolchain_env_vars(go_toolchain)

    ctx.actions.dynamic_output_new(
        _link(
            go_stdlib_value = go_stdlib.dynamic_value,
            env_vars = env,
            link_args = cmd,
            main_pkg = main,
            deps_pkgs = all_pkgs,
            shared = use_shared_code,
            identifier = identifier_prefix,
            out = output.as_output(),
            allow_cache_upload = go_toolchain.allow_cache_upload,
        )
    )

    # Stamp only executable targets. An internal link never runs the ld
    # wrapper, so there is no fb_build_info placeholder and the stamper adds
    # the section; an external link runs the wrapper, which reserves the
    # placeholder when the rule stamps.
    if build_mode in [GoBuildMode("exe"), GoBuildMode("pie")]:
        output = stamp_build_info(ctx, output, has_content_based_path = True)

    final_output = ctx.actions.copy_file(final_output_name, output, has_content_based_path = False)

    return (final_output, executable_args.runtime_files, executable_args.external_debug_info)

def _link_impl(
    actions: AnalysisActions,
    go_stdlib_value: ResolvedDynamicValue,
    env_vars: dict[str, str | cmd_args | Artifact],
    link_args: cmd_args,
    main_pkg: GoPkg,
    deps_pkgs: dict[str, GoPkg],
    shared: bool,
    identifier: str,
    out: OutputArtifact,
    allow_cache_upload: bool | None,
) -> list[Provider]:
    go_stdlib_value = go_stdlib_value.providers[GoStdlibDynamicValue]

    deps = merge_pkgs([go_stdlib_value.pkgs, deps_pkgs])
    importcfg = make_link_importcfg(actions, deps, shared)
    main_pkg_o = main_pkg.archive_file_shared if shared else main_pkg.archive_file

    cmd = [
        link_args,
        ["-importcfg", importcfg],
        ["-o", out],
        main_pkg_o,
    ]
    actions.run(cmd, env = env_vars, category = "go_link", identifier = identifier, allow_cache_upload = allow_cache_upload)
    return []

_link = dynamic_actions(
    impl = _link_impl,
    # @unsorted-dict-items
    attrs = {
        "go_stdlib_value": dynattrs.dynamic_value(),  # GoStdlibDynamicValue
        "env_vars": dynattrs.value(dict[str, str | cmd_args | Artifact]),
        "link_args": dynattrs.value(cmd_args),
        "main_pkg": dynattrs.value(GoPkg),
        "deps_pkgs": dynattrs.value(dict[str, GoPkg]),
        "shared": dynattrs.value(bool),
        "identifier": dynattrs.value(str),
        "out": dynattrs.output(),
        "allow_cache_upload": dynattrs.value(bool | None),
    },
)
