# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerInfo", "LinkerType")
load("@prelude//utils:arglike.bzl", "ArgLike")
load("@prelude//utils:expect.bzl", "expect")

# Platform-specific linker flags handling.  Modeled after the `Linker` abstraction
# in v1 (https://fburl.com/diffusion/kqd2ylcy).
# TODO(T110378136): It might make more sense to pass these in via the toolchain.
Linker = record(
    # Given the soname for the shared library, how to format the install name argument
    shared_library_install_name_format = str,
    # The extension to use for the shared library if not set in the toolchain.
    default_shared_library_extension = str,
    # The format to use for the versioned shared library extension if not set in the toolchain.
    default_shared_library_versioned_extension_format = str,
    # How to format arguments to the linker to set a shared lib name.
    shared_library_name_linker_flags_format = list[str],
    # Flags to pass to the linker to make it generate a shared library.
    shared_library_flags = list[str],
)

# Allows overriding the default shared library flags.
# e.g. when building Apple tests, we want to link with `-bundle` instead of `-shared` to allow
# linking against the bundle loader.
SharedLibraryFlagOverrides = record(
    # How to format arguments to the linker to set a shared lib name.
    shared_library_name_linker_flags_format = list[str],
    # Flags to pass to the linker to make it generate a shared library.
    shared_library_flags = list[ArgLike],
)

DARWIN_SHARED_LIBRARY_INSTALL_NAME_FORMAT_STRING = "@rpath/{}"

LINKERS = {
    LinkerType("darwin"): Linker(
        shared_library_install_name_format = DARWIN_SHARED_LIBRARY_INSTALL_NAME_FORMAT_STRING,
        default_shared_library_extension = "dylib",
        default_shared_library_versioned_extension_format = "{}.dylib",
        shared_library_name_linker_flags_format = ["-install_name", DARWIN_SHARED_LIBRARY_INSTALL_NAME_FORMAT_STRING],
        shared_library_flags = ["-shared"],
    ),
    LinkerType("gnu"): Linker(
        shared_library_install_name_format = "{}",
        default_shared_library_extension = "so",
        default_shared_library_versioned_extension_format = "so.{}",
        shared_library_name_linker_flags_format = ["-Wl,-soname,{}"],
        shared_library_flags = ["-shared"],
    ),
    LinkerType("wasm"): Linker(
        shared_library_install_name_format = "{}",
        default_shared_library_extension = "wasm",
        default_shared_library_versioned_extension_format = "{}.wasm",
        shared_library_name_linker_flags_format = [],
        # lld supports this, at least.
        # See https://github.com/WebAssembly/tool-conventions/blob/main/DynamicLinking.md#llvm-implementation
        shared_library_flags = ["-shared"],
    ),
    LinkerType("windows"): Linker(
        shared_library_install_name_format = "{}",
        default_shared_library_extension = "dll",
        default_shared_library_versioned_extension_format = "dll",
        # NOTE(agallagher): I *think* windows doesn't support a flag to set the
        # library name, and relies on the basename.
        shared_library_name_linker_flags_format = [],
        shared_library_flags = ["/DLL"],
    ),
}

IMPORT_LIBRARY_SUB_TARGET = "implib"
PDB_SUB_TARGET = "pdb"

def _sanitize(s: str) -> str:
    return s.replace("/", "_")

# NOTE(agallagher): Does this belong in the native/shared_libraries.bzl?
def get_shared_library_name(linker_info: LinkerInfo, short_name: str, apply_default_prefix: bool, version: [str, None] = None):
    """
    Generate a platform-specific shared library name based for the given rule.
    """
    if version == None:
        full_name = linker_info.shared_library_name_format.format(short_name)
    else:
        full_name = linker_info.shared_library_versioned_name_format.format(short_name, version)

    if apply_default_prefix:
        full_name = linker_info.shared_library_name_default_prefix + full_name

    return full_name

def _parse_ext_macro(name: str) -> [(str, [str, None]), None]:
    """
    Parse the `$(ext[ <version>])` macro from a user-specific library name,
    which expands to a platform-specific suffix (e.g. `.so`, `.dylib`).  If an
    optional version argument is given (e.g. `$(ext 3.4)`) it expands to a
    platform-specific versioned suffix (e.g. `.so.3.4`, `.3.4.dylib`).
    """

    # If there's no macro, then there's nothing to do.
    if ".$(ext" not in name:
        return None
    expect(name.endswith(")"))

    # Otherwise, attempt to parse out the macro.
    base, rest = name.split(".$(ext")

    # If the macro is arg-less, then return w/o a version.
    if rest == ")":
        return (base, None)

    # Otherwise, extract the version from the arg.
    expect(rest.startswith(" "))
    return (base, rest[1:-1])

def get_shared_library_name_for_param(linker_info: LinkerInfo, name: str):
    """
    Format a user-provided shared library name, supporting v1's `$(ext)` suffix.
    """
    parsed = _parse_ext_macro(name)
    if parsed != None:
        base, version = parsed
        name = get_shared_library_name(
            linker_info,
            base,
            apply_default_prefix = False,
            version = version,
        )
    return name

# NOTE(agallagher): Does this belong in the native/shared_libraries.bzl?
def get_default_shared_library_name(linker_info: LinkerInfo, label: Label):
    """
    Generate a platform-specific shared library name based for the given rule.
    """

    # TODO(T110378119): v1 doesn't use the cell/repo name, so we don't here for
    # initial compatibility, but maybe we should?
    short_name = "{}_{}".format(_sanitize(label.package), _sanitize(label.name))
    return get_shared_library_name(linker_info, short_name, apply_default_prefix = True)

def get_shared_library_name_linker_flags(linker_type: LinkerType, soname: str, flag_overrides: [SharedLibraryFlagOverrides, None] = None) -> list[str]:
    """
    Arguments to pass to the linker to set the given soname.
    """
    if flag_overrides:
        shared_library_name_linker_flags_format = flag_overrides.shared_library_name_linker_flags_format
    else:
        shared_library_name_linker_flags_format = LINKERS[linker_type].shared_library_name_linker_flags_format

    return [f.format(soname) for f in shared_library_name_linker_flags_format]

def get_shared_library_flags(linker_type: LinkerType, flag_overrides: [SharedLibraryFlagOverrides, None] = None) -> list[ArgLike]:
    """
    Arguments to pass to the linker to link a shared library.
    """
    if flag_overrides:
        return flag_overrides.shared_library_flags

    return LINKERS[linker_type].shared_library_flags

def get_link_whole_args(linker_type: LinkerType, inputs: list[Artifact]) -> list[typing.Any]:
    """
    Return linker args used to always link all the given inputs.
    """

    args = []

    if linker_type == LinkerType("gnu"):
        args.append("-Wl,--whole-archive")
        args.extend(inputs)
        args.append("-Wl,--no-whole-archive")
    elif linker_type == LinkerType("darwin"):
        for inp in inputs:
            args.append("-Xlinker")
            args.append("-force_load")
            args.append("-Xlinker")
            args.append(inp)
    elif linker_type == LinkerType("windows"):
        for inp in inputs:
            args.append(cmd_args(inp, format = "/WHOLEARCHIVE:{}"))
    elif linker_type == LinkerType("wasm"):
        args.append("--whole-archive")
        args.extend(inputs)
        args.append("--no-whole-archive")
    else:
        fail("Linker type {} not supported".format(linker_type))

    return args

def get_objects_as_library_args(linker_type: LinkerType, objects: list[Artifact]) -> list[typing.Any]:
    """
    Return linker args used to link the given objects as a library.
    """

    args = []

    if linker_type == LinkerType("gnu"):
        args.append("-Wl,--start-lib")
        args.extend(objects)
        args.append("-Wl,--end-lib")
    elif linker_type == LinkerType("darwin") or linker_type == LinkerType("windows") or linker_type == LinkerType("wasm"):
        args.extend(objects)
    else:
        fail("Linker type {} not supported".format(linker_type))

    return args

def get_ignore_undefined_symbols_flags(linker_type: LinkerType) -> list[str]:
    """
    Return linker args used to suppress undefined symbol errors.
    """

    args = []

    if linker_type == LinkerType("gnu"):
        args.append("-Wl,--allow-shlib-undefined")
        args.append("-Wl,--unresolved-symbols=ignore-all")
    elif linker_type == LinkerType("darwin"):
        args.append("-Wl,-undefined,dynamic_lookup")
    else:
        fail("Linker type {} not supported".format(linker_type))

    return args

def get_no_as_needed_shared_libs_flags(linker_type: LinkerType) -> list[str]:
    """
    Return linker args used to prevent linkers from dropping unused shared
    library dependencies from the e.g. DT_NEEDED tags of the link.
    """

    args = []

    if linker_type == LinkerType("gnu"):
        args.append("-Wl,--no-as-needed")
    elif linker_type == LinkerType("darwin"):
        pass
    else:
        fail("Linker type {} not supported".format(linker_type))

    return args

def get_output_flags(linker_type: LinkerType, output: Artifact) -> list[ArgLike]:
    if linker_type == LinkerType("windows"):
        return ["/Brepro", cmd_args(output.as_output(), format = "/OUT:{}")]
    else:
        return ["-o", output.as_output()]

def get_import_library(ctx: AnalysisContext, linker_type: LinkerType, output_short_path: str) -> (Artifact | None, list[ArgLike]):
    if linker_type == LinkerType("windows"):
        import_library = ctx.actions.declare_output(output_short_path + ".imp.lib", has_content_based_path = False)
        return import_library, [cmd_args(import_library.as_output(), format = "/IMPLIB:{}")]
    else:
        return None, []

def get_deffile_flags(ctx: AnalysisContext, linker_type: LinkerType) -> list[ArgLike]:
    if linker_type == LinkerType("windows") and getattr(ctx.attrs, "deffile", None) != None:
        return [
            cmd_args(ctx.attrs.deffile, format = "/DEF:{}"),
        ]
    else:
        return []

def get_rpath_origin(linker_type: LinkerType) -> str:
    """
    Return the macro that runtime loaders resolve to the main executable at
    runtime.
    """

    if linker_type in (LinkerType("gnu"), LinkerType("wasm")):
        return "$ORIGIN"
    if linker_type == LinkerType("darwin"):
        return "@loader_path"

    fail("Linker type {} not supported".format(linker_type))

def is_pdb_generated(linker_type: LinkerType, linker_flags: list[[str, ResolvedStringWithMacros]]) -> bool:
    if linker_type != LinkerType("windows"):
        return False
    for flag in reversed(linker_flags):
        flag = str(flag).upper()
        if flag.startswith('"/DEBUG') or flag.startswith('"-DEBUG'):
            # The last one should be not /DEBUG:NONE
            return not flag.endswith('DEBUG:NONE"')
    return False

def get_pdb_providers(pdb: Artifact, binary: Artifact):
    return [DefaultInfo(default_output = pdb, other_outputs = [binary])]

DUMPBIN_SUB_TARGET = "dumpbin"

def get_dumpbin_providers(ctx: AnalysisContext, binary: Artifact, dumpbin_toolchain_path: Artifact) -> list[Provider]:
    dumpbin_headers_out = ctx.actions.declare_output(binary.short_path + ".dumpbin_headers", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            cmd_args(dumpbin_toolchain_path, format = "{}/dumpbin.exe"),
            # We could use /ALL to display all information in one action, but that's too
            # expensive in practice
            "/HEADERS",
            binary,
            cmd_args(dumpbin_headers_out.as_output(), format = "/OUT:{}"),
        ),
        category = "dumpbin_headers",
        identifier = binary.short_path,
    )
    return [
        DefaultInfo(
            sub_targets = {
                "headers": [
                    DefaultInfo(
                        default_output = dumpbin_headers_out,
                    )
                ],
            }
        )
    ]

def sandbox_exported_linker_flags(linker_info: LinkerInfo, flags: list[typing.Any], post_flags: list[typing.Any]) -> (list[typing.Any], list[typing.Any]):
    """
    Helper to wrap exported pre/post linker flags with sandboxing flags (e.g.
    `--push-state`/`--pop-state`) only if flags are actually non-empty.
    """

    # If we're exporting flags, wrap in push/pop state flags to provide some
    # level of sandboxing.
    if linker_info.push_pop_state_flags != None and (flags or post_flags):
        push_state_flags, pop_state_flags = linker_info.push_pop_state_flags
        flags = push_state_flags + flags
        post_flags = post_flags + pop_state_flags

    return (flags, post_flags)
