# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:compile_types.bzl", "HeadersDepFiles")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo", "LinkerType")
load("@prelude//cxx:hmap.bzl", "write_hmap")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:lazy.bzl", "lazy")
load("@prelude//utils:utils.bzl", "value_or")

# Defines the varying bits of implementation affecting on how the end user
# should include the headers.
# Given there are 2 headers which are defined:
# a) one header in a list, as ["foo/bar/foobar.h"]
# b) one header in a dict (aka named header), as {"wfh/baz.h": "path/header.h"}
#
# `apple`:
# 1) header from the list should be included as NAMESPACE/PATH_BASENAME:
# #include "namespace/foobar.h"
# 2) header from the dict should be included as DICT_KEY (aka header name):
# #include "wfh/baz.h"
# 3) it should be possible to include list header from the same target via basename:
# #include "foobar.h"
#
# `regular`:
# 1) header from the list should be included as NAMESPACE/PATH:
# #include "namespace/foo/bar/foobar.h"
# 2) header from the dict should be included as NAMESPACE/DICT_KEY:
# #include "namespace/wfh/baz.h"
CxxHeadersNaming = enum("apple", "regular")

# Modes supporting implementing the `raw_headers` parameter of C++ rules using
# symlink trees and/or header maps through `headers`.
RawHeadersAsHeadersMode = enum(
    "enabled",
    "disabled",
)

# Modes supporting implementing the `headers` parameter of C++ rules using raw
# headers instead of e.g. symlink trees.
HeadersAsRawHeadersMode = enum(
    # Require that all headers be implemented as raw headers, failing if this
    # is not possible.
    "required",
    # Attempt to implement headers via raw headers, falling to header maps or
    # symlink tress when raw headers cannot be used (e.g. rule contains a
    # generated header or remaps a header to an incompatible location in the
    # header namespace).
    "preferred",
    "disabled",
)

HeaderMode = enum(
    # Creates the header map that references the headers directly in the source
    # tree.
    "header_map_only",
    # Creates the tree of symbolic links of headers.
    "symlink_tree_only",
    # Creates the tree of symbolic links of headers and creates the header map
    # that references the symbolic links to the headers.
    "symlink_tree_with_header_map",
)

HeaderStyle = enum(
    "local",
    "system",
)

Headers = record(
    include_path = field(cmd_args),
    # The header map on the include path, when the header mode uses one.
    header_map = field(Artifact | None, None),
    # NOTE(agallagher): Used for module hack replacement.
    symlink_tree = field(Artifact | None, None),
    # -ffile-prefix-map args that map symlinked headers to source path. The compiler drivers
    # derive the prefix maps for debug info, macros and coverage data from them, so none of
    # those needs flags of its own.
    file_prefix_args = field([cmd_args, None], None),
)

CHeader = record(
    # `"artifact"` pointing to the actual header file
    artifact = Artifact,
    # Basename as it should appear in include directive
    name = str,
    # Prefix before the basename as it should appear in include directive
    namespace = str,
    # Whether or not this header is provided via dict, where the corresponding key is a new name
    named = bool,
)

def header_map_key(header: CHeader) -> str:
    """
    The include path a header map gives `header`: `namespace/name`, or `name`
    when the namespace is empty. Tools that index headers by include path load
    this rather than rebuild the key.
    """
    return paths.join(header.namespace, header.name)

# Parameters controlling the varying aspects of headers-related behavior.
# The contract on how headers could be used (i.e. end user inclusion rules)
# is different for `apple_library` and `cxx_library`. Those parameters
# allows generalizing the C++ rules implementation and are provided
# by top-level user-facing wrappers around those generalized methods.
CxxHeadersLayout = record(
    # Prefix part of the header path in the include statement. Header name might
    # not always be prepended by the namespace, `naming` parameter is controlling
    # that behavior. The value is ready to be used and abstracts different naming
    # for such prefix in user-facing attributes (e.g. `apple_binary.header_path_prefix`
    # vs `cxx_binary.header_namespace`) and different default values when those
    # attributes are omitted (package path for regular C++ rules vs target name for
    # Apple-specific rules).
    namespace = str,
    # Selects the behavior in the implementation to support the specific way of how
    # headers are allowed to be included (e.g. if header namespace is applied for
    # headers from dicts). For more information see comment for `CxxHeadersNaming`
    naming = CxxHeadersNaming,
)

CxxPrecompiledHeader = record(
    header = field(Artifact),
    basename = field(str),
    basename_src = field(str),
    path = field(str),
    namespace = field(str),
    clanguage = field(str),
)

CPrecompiledHeaderInfo = provider(
    fields = {
        "basename": provider_field(typing.Any, default = None),
        "clanguage": provider_field(str | None, default = None),
        "compiled": provider_field(bool, default = False),
        # Actual precompiled header ready to be used during compilation.
        "header": Artifact,
        "path": provider_field(typing.Any, default = None),
    }
)

def cxx_attr_header_namespace(ctx: AnalysisContext) -> str:
    return value_or(ctx.attrs.header_namespace, ctx.label.package)

def cxx_attr_headers_list(_ctx: AnalysisContext, headers: typing.Any, headers_layout: CxxHeadersLayout) -> list[CHeader]:
    return _get_attr_headers(headers, headers_layout.namespace, headers_layout.naming)

def cxx_attr_exported_headers(ctx: AnalysisContext, headers_layout: CxxHeadersLayout) -> list[CHeader]:
    return cxx_attr_headers_list(ctx, ctx.attrs.exported_headers, headers_layout)

def cxx_attr_headers(ctx: AnalysisContext, headers_layout: CxxHeadersLayout) -> list[CHeader]:
    return cxx_attr_headers_list(ctx, ctx.attrs.headers, headers_layout)

def cxx_get_regular_cxx_headers_layout(ctx: AnalysisContext) -> CxxHeadersLayout:
    namespace = cxx_attr_header_namespace(ctx)
    return CxxHeadersLayout(namespace = namespace, naming = CxxHeadersNaming("regular"))

def cxx_attr_exported_header_style(ctx: AnalysisContext) -> HeaderStyle:
    return HeaderStyle(ctx.attrs.exported_header_style)

def _concat_inc_dir_with_raw_header(namespace, inc_dir, header) -> list[str] | None:
    namespace_parts = namespace.split("/")
    inc_dir_parts = inc_dir.split("/")
    header_parts = header.short_path.split("/")

    for part in inc_dir_parts:
        if part == ".":
            continue
        if part == "..":
            if not namespace_parts:
                # Too many .., would set include root out of cell
                return None
            header_parts = [namespace_parts.pop()] + header_parts
        elif part == header_parts[0]:
            header_parts = header_parts[1:]
        else:
            # Header not accessible under this folder
            return None
    return header_parts

def as_headers(ctx: AnalysisContext, raw_headers: list[Artifact], include_directories: list[str]) -> list[CHeader]:
    headers = []
    base_namespace = ctx.label.package
    for header in raw_headers:
        for inc_dir in include_directories:
            inc_dir = paths.normalize(inc_dir)
            mapped_header = _concat_inc_dir_with_raw_header(base_namespace, inc_dir, header)
            if mapped_header:
                headers.append(CHeader(artifact = header, name = "/".join(mapped_header), namespace = "", named = True))

    return headers

def cxx_attr_precompiled_headers(ctx: AnalysisContext, headers_layout: CxxHeadersLayout) -> CHeader | None:
    header = ctx.attrs.srcs[0]
    return _get_attr_headers([header], headers_layout.namespace, headers_layout.naming)[0]

def _get_attr_headers(xs: typing.Any, namespace: str, naming: CxxHeadersNaming) -> list[CHeader]:
    if type(xs) == type([]):
        return [CHeader(artifact = x, name = _get_list_header_name(x, naming), namespace = namespace, named = False) for x in xs]
    else:
        return [CHeader(artifact = xs[x], name = x, namespace = _get_dict_header_namespace(namespace, naming), named = True) for x in xs]

def as_raw_headers(ctx: AnalysisContext, headers: dict[str, Artifact], mode: HeadersAsRawHeadersMode) -> [list[CellPath], None]:
    """
    Return the include directories needed to treat the given headers as raw
    headers, depending on the given `HeadersAsRawHeadersMode` mode.

    Args:
      mode:
        disabled - always return `None`
        preferred - return `None` if conversion isn't possible
        required - fail if conversion isn't possible
    """

    # If we're not supporting raw header conversion, return `None`.
    if mode == HeadersAsRawHeadersMode("disabled"):
        return None

    return _as_raw_headers(
        ctx,
        headers,
        # Don't fail if conversion isn't required.
        no_fail = mode != HeadersAsRawHeadersMode("required"),
    )

def _header_mode(cxx_toolchain_info: CxxToolchainInfo, header_mode: HeaderMode | None) -> HeaderMode:
    toolchain_header_mode = cxx_toolchain_info.header_mode

    # If the toolchain disabled header maps, respect that since the compiler
    # simply cannot accept anything else.
    if toolchain_header_mode == HeaderMode("symlink_tree_only"):
        return toolchain_header_mode

    # If the target specifies a header mode, use that in case it needs
    # a symlink tree (even with header maps)
    if header_mode != None:
        return header_mode

    return toolchain_header_mode

def prepare_headers(
    actions: AnalysisActions,
    cxx_toolchain_info: CxxToolchainInfo,
    srcs: dict[str, Artifact],
    name: str,
    header_mode: [HeaderMode, None] = None,
    allow_cache_upload: bool = False,
    uses_content_based_paths: bool = False,
    header_namespace: [str, None] = None,
) -> [Headers, None]:
    """
    Prepare all the headers we want to use, depending on the header_mode
    set on the target's toolchain.
        - In the case of a header map, we create a `name`.hmap file and
          return it as part of the include path.
        - In the case of a symlink tree, we create a directory of `name`
          containing the headers and return it as part of the include path.
    """
    if len(srcs) == 0:
        return None

    header_mode = _header_mode(cxx_toolchain_info, header_mode)

    # TODO(T110378135): There's a bug in clang where using header maps w/o
    # explicit `-I` anchors breaks module map lookups.  This will be fixed
    # by https://reviews.llvm.org/D103930 so, until it lands, disable header
    # maps when we see a module map.
    if header_mode == HeaderMode("symlink_tree_with_header_map") and lazy.is_any(lambda n: paths.basename(n) == "module.modulemap", srcs.keys()):
        header_mode = HeaderMode("symlink_tree_only")

    output_name = name

    if header_mode == HeaderMode("header_map_only"):
        headers = {h: (a, "") for h, a in srcs.items()}
        hmap = _mk_hmap(actions, cxx_toolchain_info, output_name, headers, allow_cache_upload, uses_content_based_paths)
        return Headers(
            include_path = cmd_args(hmap, hidden = srcs.values()),
            header_map = hmap,
        )
    symlink_dir = actions.symlinked_dir(
        output_name,
        _normalize_header_srcs(srcs),
        has_content_based_path = uses_content_based_paths,
    )
    prefix_map_flag = "-ffile-prefix-map"
    if header_mode == HeaderMode("symlink_tree_only"):
        replacement = _get_prefix_map_replacement(cxx_toolchain_info, symlink_dir, srcs, header_namespace)
        file_prefix_args = None
        if replacement != None:
            file_prefix_args = cmd_args(cmd_args(symlink_dir, format = prefix_map_flag + "={}="), replacement, delimiter = "")
        return Headers(
            include_path = cmd_args(symlink_dir),
            symlink_tree = symlink_dir,
            file_prefix_args = file_prefix_args,
        )
    if header_mode == HeaderMode("symlink_tree_with_header_map"):
        headers = {h: (symlink_dir, h) for h in srcs}
        hmap = _mk_hmap(actions, cxx_toolchain_info, output_name, headers, allow_cache_upload, uses_content_based_paths)
        replacement = _get_prefix_map_replacement(cxx_toolchain_info, symlink_dir, srcs, header_namespace)
        file_prefix_args = None
        if replacement != None:
            file_prefix_args = cmd_args(cmd_args(symlink_dir, format = prefix_map_flag + "={}="), replacement, delimiter = "")
        return Headers(
            include_path = cmd_args(hmap, hidden = symlink_dir),
            header_map = hmap,
            symlink_tree = symlink_dir,
            file_prefix_args = file_prefix_args,
        )
    fail("Unsupported header mode: {}".format(header_mode))

def _normalize_header_srcs(srcs: dict) -> dict:
    normalized_srcs = {}
    for key, val in srcs.items():
        normalized_key = paths.normalize(key)
        stored_val = normalized_srcs.get(normalized_key, None)
        expect(
            stored_val == None or stored_val == val,
            "Got different values {} and {} for the same normalized header {}".format(
                val,
                stored_val,
                normalized_key,
            ),
        )
        normalized_srcs[normalized_key] = val

    return normalized_srcs

def _as_raw_headers(
    ctx: AnalysisContext,
    headers: dict[str, Artifact],
    # Return `None` instead of failing.
    no_fail: bool = False,
) -> [list[CellPath], None]:
    """
    Return the include directories needed to treat the given headers as raw
    headers.
    """

    # Find the all the include dirs needed to treat the given headers as raw
    # headers.
    inc_dirs = {}
    for name, header in headers.items():
        inc_dir = _as_raw_header(
            ctx,
            name,
            header,
            no_fail = no_fail,
        )

        # If the conversion wasn't possible, `inc_dir` will be `None` and we
        # should bail now.
        if inc_dir == None:
            return None
        inc_dirs[inc_dir] = None

    return [ctx.label.path.add(p) for p in inc_dirs]

def _as_raw_header(
    ctx: AnalysisContext,
    # The full name used to include the header.
    name: str,
    header: Artifact,
    # Return `None` instead of failing.
    no_fail: bool = False,
) -> [str, None]:
    """
    Return path to pass to `include_directories` to treat the given header as
    a raw header.
    """
    name = paths.normalize(name)

    # We can't handle generated headers.
    if not header.is_source:
        if no_fail:
            return None
        fail("generated headers cannot be used as raw headers ({})".format(header))

    # To include the header via its name using raw headers and include dirs,
    # it needs to be a suffix of its original path, and we'll strip the include
    # name to get the include dir used to include it.
    path = paths.join(ctx.label.package, header.short_path)
    path = paths.normalize(path)
    base = paths.strip_suffix(path, name)
    if base == None:
        if no_fail:
            return None
        fail("header name must be a path suffix of the header path to be " + "used as a raw header ({} => {})".format(name, header))

    # If the include dir is underneath our package, then just relativize to find
    # out package-relative path.
    if len(base) >= len(ctx.label.package):
        return paths.relativize(base, ctx.label.package)

    # Otherwise, this include dir needs to reference a parent dir.
    expect(ctx.label.package.startswith(base))
    num_parents = len(ctx.label.package.split("/")) - (0 if not base else len(base.split("/")))
    return "/".join([".."] * num_parents)

def _get_list_header_name(header: Artifact, naming: CxxHeadersNaming) -> str:
    if naming.value == "regular":
        return header.short_path
    elif naming.value == "apple":
        return header.basename
    else:
        fail("Unsupported header naming: {}".format(naming))

def _get_dict_header_namespace(namespace: str, naming: CxxHeadersNaming) -> str:
    if naming.value == "regular":
        return namespace
    elif naming.value == "apple":
        return ""
    else:
        fail("Unsupported header naming: {}".format(naming))

def _infer_include_prefix(srcs: dict[str, Artifact], header_namespace: [str, None], package: [str, None] = None) -> [str, None]:
    """
    Compute the cell-relative path prefix that, when prepended to a symlink
    tree key, reconstructs the correct source path within the cell.
    Returns a cell-relative prefix string, or None when no match could be
    determined (caller should fall back to the package path).
    """
    return _infer_include_prefix_from_paths(
        {k: v.short_path for k, v in srcs.items()},
        header_namespace,
        package,
    )

def _ends_with_path(path: str, suffix: str) -> bool:
    # Whole path components only: `config/linux-config.h` does not end with `config.h`.
    return path == suffix or path.endswith("/" + suffix)

def _infer_include_prefix_from_paths(srcs_paths: dict[str, str], header_namespace: [str, None], package: [str, None] = None) -> [str, None]:
    if header_namespace != "":
        return None
    for key, short in srcs_paths.items():
        if _ends_with_path(short, key):
            if len(short) > len(key):
                prefix = paths.normalize(short[: len(short) - len(key)])
                return paths.join(package, prefix) if package else prefix
            # Exact match (short == key) is uninformative — this entry
            # sits directly in the package dir. Skip it and check more
            # entries, since others may reveal a subdirectory prefix
            # (e.g. "include/") that applies to the majority of headers.
            continue
        if package:
            full_path = paths.join(package, short)
            if _ends_with_path(full_path, key):
                if len(full_path) > len(key):
                    return paths.normalize(full_path[: len(full_path) - len(key)])
                # full_path == key means the key already encodes the
                # package-relative path (e.g. via subdir_glob prefix).
                # Return "" so the caller uses just the cell prefix.
                return ""
        break
    return None

def _get_prefix_map_replacement(
    cxx_toolchain_info: CxxToolchainInfo, header_dir: Artifact, srcs: dict[str, Artifact], header_namespace: [str, None]
) -> [cmd_args, None]:
    """Compute the replacement path for prefix-map flags."""

    # NOTE(@christylee): Do we need to enable debug-prefix-map for darwin and windows?
    if cxx_toolchain_info.linker_info.type != LinkerType("gnu"):
        return None

    package = header_dir.owner.package
    source_root = None
    if read_root_config("cxx", "header_prefix_map_source_root", "false") == "true":
        source_root = _common_source_root({k: (v.short_path if v.is_source else None) for k, v in srcs.items()}, package)
    if source_root != None:
        # Rendered from a header's source artifact, so that it includes the
        # cell's location in the repo. Single-component paths render with a
        # leading "./".
        tree_path, parents = source_root
        return cmd_args(srcs[tree_path], parent = parents, ignore_artifacts = True, replace_regex = ("^\\./", ""))

    cell = value_or(header_dir.owner.cell, ".")
    return cmd_args(
        _prefix_map_replacement(
            cell = cell,
            cell_prefix = cxx_toolchain_info.cell_to_path_prefix_map.get(cell, ""),
            package = package,
            header_namespace = header_namespace,
            include_prefix = _infer_include_prefix(srcs, header_namespace, package = package),
        )
    )

def _common_source_root(srcs_paths: dict[str, [str, None]], package: str) -> [(str, int), None]:
    """
    The source directory that every source header in a symlink tree sits at
    under the same relative path as in the tree, if there is one.

    `srcs_paths` maps each path in the tree to the header's package-relative
    source path, or to None for generated headers. Returns `(tree_path,
    parents)`: the directory is `parents` levels above the source of the header
    at `tree_path`. Returns None when headers disagree, there are no source
    headers, or the directory would be the cell root (the empty path in the
    repo's root cell).
    """
    root = None
    example = None
    for tree_path, short_path in srcs_paths.items():
        if short_path == None:
            continue
        cell_path = paths.join(package, short_path)
        if len(cell_path) <= len(tree_path) + 1 or not cell_path.endswith("/" + tree_path):
            return None
        candidate = cell_path[: len(cell_path) - len(tree_path) - 1]
        if root == None:
            root = candidate
            example = tree_path
        elif candidate != root:
            return None
    if example == None:
        return None
    return (example, example.count("/") + 1)

def _prefix_map_replacement(cell: str, cell_prefix: str, package: str, header_namespace: [str, None], include_prefix: [str, None]) -> str:
    prefix_target = cell_prefix
    if header_namespace == "":
        if include_prefix != None:
            # include_prefix captures the path from cell root to above the
            # symlink tree key structure. Use it directly instead of the
            # package path to avoid duplication when keys already encode
            # the package path (e.g. via subdir_glob prefix).
            if include_prefix:
                prefix_target = paths.join(prefix_target, include_prefix) if prefix_target else include_prefix
        elif package:
            prefix_target = paths.join(prefix_target, package) if prefix_target else package
    return prefix_target if prefix_target else cell

def _mk_hmap(
    actions: AnalysisActions,
    cxx_toolchain_info: CxxToolchainInfo,
    name: str,
    headers: dict[str, (Artifact, str)],
    allow_cache_upload: bool,
    uses_content_based_paths: bool = False,
) -> Artifact:
    output = actions.declare_output(
        name + ".hmap",
        has_content_based_path = uses_content_based_paths,
    )

    if read_root_config("buck2", "native_hmap", "false") == "true":
        native_hmap = write_hmap(actions, output, headers)
        if native_hmap != None:
            return native_hmap

    header_args = cmd_args()
    for n, (path, subpath) in headers.items():
        header_args.add(n)
        header_args.add(cmd_args(path, format = "{}/" + subpath) if subpath else path)

    hmap_args_file = actions.write(
        output.basename + ".cxx_hmap_argsfile",
        cmd_args(header_args, quote = "shell"),
        has_content_based_path = uses_content_based_paths,
    )

    cmd = cmd_args(
        [cxx_toolchain_info.internal_tools.hmap_wrapper] + ["--output", output.as_output()] + ["--mappings-file", hmap_args_file],
    )
    actions.run(cmd, category = "generate_hmap", identifier = name, allow_cache_upload = allow_cache_upload)
    return output

def add_headers_dep_files(
    actions: AnalysisActions, cmd: cmd_args, headers_dep_files: HeadersDepFiles, src: Artifact, filename_base: str, action_dep_files: dict[str, ArtifactTag]
) -> cmd_args:
    dep_file = actions.declare_output(
        paths.join("__dep_files__", filename_base),
        has_content_based_path = True,
    ).as_output()
    processor_flags, compiler_flags = headers_dep_files.mk_flags(
        actions,
        filename_base,
        src,
    )
    cmd.add(compiler_flags)

    # API: First argument is the dep file source path, second is the
    # dep file destination path, other arguments are the actual compile
    # command.
    cmd = cmd_args([
        headers_dep_files.processor,
        headers_dep_files.dep_tracking_mode.value,
        processor_flags,
        headers_dep_files.tag.tag_artifacts(dep_file),
        cmd,
    ])

    action_dep_files["headers"] = headers_dep_files.tag
    return cmd

headers_for_tests = struct(
    common_source_root = _common_source_root,
    infer_include_prefix = _infer_include_prefix_from_paths,
    prefix_map_replacement = _prefix_map_replacement,
)
