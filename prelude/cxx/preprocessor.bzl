# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_utility.bzl", "cxx_attrs_get_allow_cache_upload")
load("@prelude//cxx:target_sdk_version.bzl", "get_target_sdk_version_flags")
load(
    "@prelude//utils:utils.bzl",
    "filter_and_map_idx",
    "map_val",
    "value_or",
)
load(":attr_selection.bzl", "cxx_by_language_ext")
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(":cxx_toolchain_types.bzl", "CXX_COMPILER_TYPES")
load(
    ":headers.bzl",
    "CHeader",  # @unused Used as a type
    "CxxHeadersLayout",  # @unused Used as a type
    "CxxHeadersNaming",
    "HeaderMode",
    "HeaderStyle",
    "HeadersAsRawHeadersMode",
    "RawHeadersAsHeadersMode",
    "as_headers",
    "as_raw_headers",
    "cxx_attr_exported_header_style",
    "cxx_attr_exported_headers",
    "cxx_attr_headers",
    "prepare_headers",
)

SystemIncludeDirs = record(
    # Compiler type to infer correct include flags
    compiler_type = field(str),
    #  Directories to be included via [-isystem | /external:I] [arglike things]
    include_dirs = field(list[CellPath]),
)

CPreprocessorArgs = record(
    # The arguments, [arglike things]
    args = field(list[typing.Any], []),
    # -fdebug-prefix-map args that map symlinks to source file location
    file_prefix_args = field(list[typing.Any], []),
    # -fcoverage-prefix-map args (clang-only, separated so non-clang compilers can skip them)
    coverage_prefix_args = field(list[typing.Any], []),
    # Arguments used for module precompilation, replacing args
    precompile_args = field(list[typing.Any], []),
    # Header maps named by `args`, without the header trees behind them. Tools
    # that resolve a header map by reading it need these and nothing else.
    header_maps = field(list[Artifact], []),
)

HeaderUnit = record(
    name = field(str),
    module = field(Artifact),
    stub = field(Artifact | None),
    include_dir = field(Artifact),
    import_include = field(str | None),
    clang_trace = field(Artifact | None),
    diagnostics = field(Artifact | None, None),
)

# Note: Any generic attributes are assumed to be relative.
CPreprocessor = record(
    # Relative path args to be used for build operations.
    args = field(CPreprocessorArgs, CPreprocessorArgs()),
    # Header specs
    headers = field(list[CHeader], []),
    # Those should be mutually exclusive with normal headers as per documentation
    raw_headers = field(list[Artifact], []),
    # Directories to be included via -I, [arglike things]
    include_dirs = field(list[CellPath], []),
    # Directories to be included via -isystem, [arglike things]
    system_include_dirs = field([SystemIncludeDirs, None], None),
    # Whether to compile with modules support
    uses_modules = field(bool, False),
    # Modular args to set when modules are in use, [arglike things]
    modular_args = field(list[typing.Any], []),
    # Modulemap artifact
    modulemap_artifact = field([Artifact, None], None),
    # Header units to load transitively and supporting args.
    header_units = field(list[HeaderUnit], []),
    # Raw include directory strings from public_include_directories attr.
    raw_include_dirs = field(list[str], []),
    # Raw include directory strings from public_system_include_directories attr.
    raw_system_include_dirs = field(list[str], []),
)

_EMPTY_CPREPROCESSOR_ARGS = CPreprocessorArgs()
EMPTY_CPREPROCESSOR = CPreprocessor()

_EMPTY_SYSTEM_INCLUDE_DIRS_BY_COMPILER_TYPE = {
    compiler_type: SystemIncludeDirs(compiler_type = compiler_type, include_dirs = []) for compiler_type in CXX_COMPILER_TYPES
}

def _system_include_dirs(compiler_type: str, include_dirs: list[CellPath]) -> SystemIncludeDirs:
    if not include_dirs:
        shared = _EMPTY_SYSTEM_INCLUDE_DIRS_BY_COMPILER_TYPE.get(compiler_type)
        if shared != None:
            return shared
    return SystemIncludeDirs(compiler_type = compiler_type, include_dirs = include_dirs)

# Methods for transitive_sets must be declared prior to their use.

def _cpreprocessor_args(pres: list[CPreprocessor]):
    args = cmd_args()
    for pre in pres:
        args.add(pre.args.args)
    return args

def _cpreprocessor_modular_args(pres: list[CPreprocessor]):
    args = cmd_args()
    for pre in pres:
        args.add(pre.modular_args)
    return args

def _cpreprocessor_precompile_args(pres: list[CPreprocessor]):
    args = cmd_args()
    for pre in pres:
        args.add(pre.args.precompile_args)
    return args

def _cpreprocessor_header_maps(pres: list[CPreprocessor]):
    return cmd_args(hidden = [pre.args.header_maps for pre in pres])

def _cpreprocessor_header_units_args_impl(pres: list[CPreprocessor], stub: bool):
    args = cmd_args()
    for pre in pres:
        for h in pre.header_units:
            artifact = h.stub if stub else h.module
            if artifact:
                args.add(cmd_args("-fmodule-file=", h.name, "=", artifact, delimiter = ""))
                args.add(cmd_args(h.include_dir, format = "-I{}"))
                args.add(cmd_args(h.include_dir, format = "-fmodule-map-file={}/module.modulemap"))
                if h.import_include:
                    args.add(["-include", h.import_include])
    return args

def _cpreprocessor_header_units_args(pres: list[CPreprocessor]):
    return _cpreprocessor_header_units_args_impl(pres, stub = False)

def _cpreprocessor_header_unit_stubs_args(pres: list[CPreprocessor]):
    return _cpreprocessor_header_units_args_impl(pres, stub = True)

def _cpreprocessor_has_header_units_args(children: list[bool], pres: [list[CPreprocessor], None]):
    if pres:
        for pre in pres:
            if pre and pre.header_units and len(pre.header_units) > 0:
                return True
    return any(children)

def _cpreprocessor_file_prefix_args(pres: list[CPreprocessor]):
    args = cmd_args()
    for pre in pres:
        args.add(pre.args.file_prefix_args)
    return args

def _cpreprocessor_coverage_prefix_args(pres: list[CPreprocessor]):
    args = cmd_args()
    for pre in pres:
        args.add(pre.args.coverage_prefix_args)
    return args

def _cpreprocessor_has_file_prefix_args(children: list[bool], pres: [list[CPreprocessor], None]):
    if pres:
        for pre in pres:
            if pre.args.file_prefix_args or pre.args.coverage_prefix_args:
                return True
    return any(children)

def _cpreprocessor_include_dirs(pres: list[CPreprocessor]):
    args = cmd_args()
    for pre in pres:
        for d in pre.include_dirs:
            args.add(cmd_args(d, format = "-I{}"))
        if pre.system_include_dirs != None:
            for d in pre.system_include_dirs.include_dirs:
                system_include_args = format_system_include_arg(cmd_args(d), pre.system_include_dirs.compiler_type)
                args.add(system_include_args)
    return args

def _cpreprocessor_uses_modules(children: list[bool], pres: [list[CPreprocessor], None]):
    if pres:
        for pre in pres:
            if pre.uses_modules:
                return True
    return any(children)

# Set of [CPreprocessor]. Most nodes have just a single value, but we
# allow > 1 for cxx compilation commands where it we do want > 1 (one for
# exported pp info and one for not-exported).
CPreprocessorTSet = transitive_set(
    args_projections = {
        "args": _cpreprocessor_args,
        "coverage_prefix_args": _cpreprocessor_coverage_prefix_args,
        "file_prefix_args": _cpreprocessor_file_prefix_args,
        "header_maps": _cpreprocessor_header_maps,
        "header_unit_stubs_args": _cpreprocessor_header_unit_stubs_args,
        "header_units_args": _cpreprocessor_header_units_args,
        "include_dirs": _cpreprocessor_include_dirs,
        "modular_args": _cpreprocessor_modular_args,
        "precompile_args": _cpreprocessor_precompile_args,
    },
    reductions = {
        "has_file_prefix_args": _cpreprocessor_has_file_prefix_args,
        "has_header_units_args": _cpreprocessor_has_header_units_args,
        "uses_modules": _cpreprocessor_uses_modules,
    },
)

CPreprocessorInfo = provider(
    fields = {
        "set": provider_field(typing.Any, default = None),  # "CPreprocessorTSet"
    }
)

# Defines the provider exposed by libraries to test targets,
# so that tests can have access to the private headers of
# the first order deps (for testing purposes).
CPreprocessorForTestsInfo = provider(
    # @unsorted-dict-items
    fields = {
        # [str] - list of targets in "tests"
        "test_names": provider_field(typing.Any, default = None),  #
        # CPreprocessor - the private preprocessor
        # for the target which is _only_ exposed to any
        # test targets defined in `test_names`
        "own_non_exported_preprocessor": provider_field(typing.Any, default = None),
    },
)

def cxx_attr_exported_preprocessor_flags(ctx: AnalysisContext) -> list[typing.Any]:
    return ctx.attrs.exported_preprocessor_flags + _by_language_cxx(ctx.attrs.exported_lang_preprocessor_flags)

def cxx_inherited_preprocessor_infos(first_order_deps: list[Dependency]) -> list[CPreprocessorInfo]:
    # We filter out nones because some non-cxx rule without such providers could be a dependency, for example
    # cxx_binary "fbcode//one_world/cli/util/process_wrapper:process_wrapper" depends on
    # python_library "fbcode//third-party-buck/$platform/build/glibc:__project__"
    return filter_and_map_idx(CPreprocessorInfo, first_order_deps)

def cxx_merge_cpreprocessors(actions: AnalysisActions, own: list[CPreprocessor], xs: list[CPreprocessorInfo]) -> CPreprocessorInfo:
    kwargs = {"children": [x.set for x in xs]}
    if own:
        kwargs["value"] = own
    return CPreprocessorInfo(
        set = actions.tset(CPreprocessorTSet, **kwargs),
    )

# Module globals are recursively frozen before callers can observe them.
_INCLUDE_FLAG_ARGS = {
    "-I": cmd_args("-I"),
    "-isystem": cmd_args("-isystem"),
}

def _format_include_arg(flag: str, path: cmd_args, compiler_type: str) -> list[cmd_args]:
    if compiler_type == "windows":
        return [cmd_args(path, format = flag + "{}")]
    else:
        shared = _INCLUDE_FLAG_ARGS.get(flag)
        return [shared if shared != None else cmd_args(flag), path]

def format_system_include_arg(path: cmd_args, compiler_type: str) -> list[cmd_args]:
    if compiler_type in ["windows", "clang_cl"]:
        return [cmd_args(path, format = "/external:I{}")]
    else:
        return [_INCLUDE_FLAG_ARGS["-isystem"], path]

def cxx_exported_preprocessor_info(ctx: AnalysisContext, headers_layout: CxxHeadersLayout, extra_preprocessors: list[CPreprocessor] = []) -> CPreprocessor:
    """
    This rule's preprocessor info which is both applied to the compilation of
    its source and propagated to the compilation of dependent's sources.
    """
    exported_headers = cxx_attr_exported_headers(ctx, headers_layout)

    # Add any headers passed in via constructor params
    for pre in extra_preprocessors:
        exported_headers += pre.headers

    exported_header_map = {paths.join(h.namespace, h.name): h.artifact for h in exported_headers}
    raw_headers = []
    include_dirs = []
    system_include_dirs = []

    style = cxx_attr_exported_header_style(ctx)
    compiler_type = get_cxx_toolchain_info(ctx).cxx_compiler_info.compiler_type

    # If headers-as-raw-headers is enabled, convert exported headers to raw
    # headers, with the appropriate include directories.
    raw_headers_mode = _attr_headers_as_raw_headers_mode(ctx)
    inferred_inc_dirs = as_raw_headers(ctx, exported_header_map, raw_headers_mode)
    if inferred_inc_dirs != None:
        raw_headers.extend(exported_header_map.values())
        if style == HeaderStyle("local"):
            include_dirs.extend(inferred_inc_dirs)
        else:
            system_include_dirs.extend(inferred_inc_dirs)
        exported_header_map.clear()

    # Add in raw headers and include dirs from attrs.
    raw_headers.extend(value_or(ctx.attrs.raw_headers, []))

    # if raw-headers-as-headers is enabled, convert raw headers to headers
    if _attr_raw_headers_as_headers_mode(ctx) != RawHeadersAsHeadersMode("disabled"):
        if raw_headers:
            exported_headers = as_headers(ctx, raw_headers, ctx.attrs.public_include_directories + ctx.attrs.public_system_include_directories)
            exported_header_map = {paths.join(h.namespace, h.name): h.artifact for h in exported_headers}
            raw_headers.clear()

            # Force system exported header style if any system include directories are set.
            if ctx.attrs.public_system_include_directories:
                style = HeaderStyle("system")
    else:
        include_dirs.extend([ctx.label.path.add(x) for x in ctx.attrs.public_include_directories])
        system_include_dirs.extend([ctx.label.path.add(x) for x in ctx.attrs.public_system_include_directories])

    args = get_exported_preprocessor_args(ctx, exported_header_map, style, compiler_type, raw_headers, extra_preprocessors)

    modular_args = []
    for pre in extra_preprocessors:
        modular_args.extend(pre.modular_args)

    header_units = []
    for pre in extra_preprocessors:
        header_units.extend(pre.header_units)

    return CPreprocessor(
        args = args,
        headers = exported_headers,
        raw_headers = raw_headers,
        include_dirs = include_dirs,
        system_include_dirs = _system_include_dirs(compiler_type, system_include_dirs),
        modular_args = modular_args,
        header_units = header_units,
        raw_include_dirs = ctx.attrs.public_include_directories,
        raw_system_include_dirs = ctx.attrs.public_system_include_directories,
    )

def get_exported_preprocessor_args(
    ctx: AnalysisContext,
    headers: dict[str, Artifact],
    style: HeaderStyle,
    compiler_type: str,
    raw_headers: list[Artifact],
    extra_preprocessors: list[CPreprocessor],
) -> CPreprocessorArgs:
    cxx_toolchain_info = get_cxx_toolchain_info(ctx)
    allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs)
    header_root = prepare_headers(
        ctx.actions,
        cxx_toolchain_info,
        headers,
        "buck-headers",
        map_val(HeaderMode, getattr(ctx.attrs, "header_mode", None)),
        allow_cache_upload = allow_cache_upload,
        uses_content_based_paths = True,
        header_namespace = getattr(ctx.attrs, "header_namespace", None),
    )
    precompile_root = prepare_headers(
        ctx.actions,
        cxx_toolchain_info,
        headers,
        "buck-pre-headers",
        HeaderMode("header_map_only"),
        allow_cache_upload = allow_cache_upload,
        uses_content_based_paths = True,
    )

    # Process args to handle the `$(cxx-header-tree)` macro.
    #
    # Note: said macro looks like it is used for user/macro-level modulemaps, which
    # aren't compatible with header units precompilation. The macro expansion relies on
    # the symlink tree, which isn't compatible with the hmap-only header mode used for
    # precompile_args anyway.
    args = []
    precompile_args = []
    for arg in cxx_attr_exported_preprocessor_flags(ctx):
        precompile_args.append(arg)
        if _needs_cxx_header_tree_hack(arg):
            if header_root == None or header_root.symlink_tree == None:
                fail("No headers")
            arg = _cxx_header_tree_hack_replacement(header_root.symlink_tree)
        args.append(arg)

    # Propagate the exported header tree.
    file_prefix_args = []
    coverage_prefix_args = []
    header_maps = []
    if header_root != None:
        args.extend(_header_style_args(style, header_root.include_path, compiler_type))
        precompile_args.extend(_header_style_args(style, precompile_root.include_path, compiler_type))
        if header_root.header_map != None:
            header_maps.append(header_root.header_map)
        if header_root.file_prefix_args != None:
            file_prefix_args.append(header_root.file_prefix_args)
        if header_root.coverage_prefix_args != None:
            coverage_prefix_args.append(header_root.coverage_prefix_args)

    # Embed raw headers as hidden artifacts in our args.  This means downstream
    # cases which use these args don't also need to know to add raw headers.
    if raw_headers:
        # NOTE(agallagher): It's a bit weird adding an "empty" arg, but this
        # appears to do the job (and not e.g. expand to `""`).
        raw_header_args = cmd_args(hidden = raw_headers)
        args.append(raw_header_args)
        precompile_args.append(raw_header_args)

    # Append any extra preprocessor info passed in via the constructor params
    for pre in extra_preprocessors:
        args.extend(pre.args.args)
        precompile_args.extend(pre.args.precompile_args)
        header_maps.extend(pre.args.header_maps)

    if not args and not file_prefix_args and not coverage_prefix_args and not precompile_args:
        return _EMPTY_CPREPROCESSOR_ARGS
    return CPreprocessorArgs(
        args = args, file_prefix_args = file_prefix_args, coverage_prefix_args = coverage_prefix_args, precompile_args = precompile_args, header_maps = header_maps
    )

def cxx_private_preprocessor_info(
    ctx: AnalysisContext,
    headers_layout: CxxHeadersLayout,
    raw_headers: list[Artifact] = [],
    extra_preprocessors: list[CPreprocessor] = [],
    non_exported_deps: list[Dependency] = [],
    is_test: bool = False,
) -> (CPreprocessor, list[CPreprocessor]):
    private_preprocessor = _cxx_private_preprocessor_info(ctx, headers_layout, raw_headers, extra_preprocessors)

    test_preprocessors = []
    if is_test:
        for non_exported_dep in non_exported_deps:
            preprocessor_for_tests = non_exported_dep.get(CPreprocessorForTestsInfo)
            if preprocessor_for_tests and ctx.label.name in preprocessor_for_tests.test_names:
                test_preprocessors.append(preprocessor_for_tests.own_non_exported_preprocessor)

    return (private_preprocessor, test_preprocessors)

def _cxx_private_preprocessor_info(
    ctx: AnalysisContext, headers_layout: CxxHeadersLayout, raw_headers: list[Artifact], extra_preprocessors: list[CPreprocessor]
) -> CPreprocessor:
    """
    This rule's preprocessor info which is only applied to the compilation of
    its source, and not propagated to dependents.
    """
    compiler_type = get_cxx_toolchain_info(ctx).cxx_compiler_info.compiler_type
    headers = cxx_attr_headers(ctx, headers_layout)

    # `apple_*` rules allow headers to be included via only a basename if those
    # are headers (private or exported) from the same target.
    if headers_layout.naming == CxxHeadersNaming("apple"):
        headers.extend(
            _remap_headers_to_basename(
                headers + cxx_attr_exported_headers(ctx, headers_layout),
            ),
        )

    # Include any headers provided via constructor params and determine whether
    # to use modules.
    uses_modules = False
    for pp in extra_preprocessors:
        headers += pp.headers
        uses_modules = uses_modules or pp.uses_modules

    header_map = {paths.join(h.namespace, h.name): h.artifact for h in headers}

    all_raw_headers = []
    include_dirs = []

    # If headers-as-raw-headers is enabled, convert exported headers to raw
    # headers, with the appropriate include directories.
    raw_headers_mode = _attr_headers_as_raw_headers_mode(ctx)
    inferred_inc_dirs = as_raw_headers(ctx, header_map, raw_headers_mode)
    if inferred_inc_dirs != None:
        all_raw_headers.extend(header_map.values())
        include_dirs.extend(inferred_inc_dirs)
        header_map.clear()

    # Add in raw headers and include dirs from attrs.
    all_raw_headers.extend(raw_headers)

    # if raw-headers-as-headers is enabled, convert raw headers to headers
    if all_raw_headers and _attr_raw_headers_as_headers_mode(ctx) != RawHeadersAsHeadersMode("disabled"):
        # private headers are also accessible via public_include_directories
        # same as exported_preprocessor_flags apply to the target itself.
        headers = as_headers(
            ctx,
            all_raw_headers,
            ctx.attrs.include_directories + getattr(ctx.attrs, "public_include_directories", []) + getattr(ctx.attrs, "public_system_include_directories", []),
        )
        header_map = {paths.join(h.namespace, h.name): h.artifact for h in headers}
        all_raw_headers.clear()
    else:
        include_dirs.extend([ctx.label.path.add(x) for x in ctx.attrs.include_directories])

    args = _get_private_preprocessor_args(ctx, header_map, compiler_type, all_raw_headers)

    # The remaining CPreprocessor fields use their defaults below, so these
    # checks cover every field in the record.
    if not headers and not all_raw_headers and not include_dirs and not uses_modules and args == _EMPTY_CPREPROCESSOR_ARGS:
        return EMPTY_CPREPROCESSOR

    return CPreprocessor(
        args = args,
        headers = headers,
        raw_headers = all_raw_headers,
        include_dirs = include_dirs,
        uses_modules = uses_modules,
    )

def _get_private_preprocessor_args(
    ctx: AnalysisContext, headers: dict[str, Artifact], compiler_type: str, all_raw_headers: list[Artifact]
) -> CPreprocessorArgs:
    # Create private header tree and propagate via args.
    args = get_target_sdk_version_flags(ctx)
    cxx_toolchain_info = get_cxx_toolchain_info(ctx)
    file_prefix_args = []
    coverage_prefix_args = []
    header_maps = []
    header_mode = map_val(HeaderMode, getattr(ctx.attrs, "header_mode", None))
    allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs)
    header_root = prepare_headers(
        ctx.actions,
        cxx_toolchain_info,
        headers,
        "buck-private-headers",
        header_mode = header_mode,
        allow_cache_upload = allow_cache_upload,
        uses_content_based_paths = True,
        header_namespace = getattr(ctx.attrs, "header_namespace", None),
    )
    if header_root != None:
        args.extend(_format_include_arg("-I", header_root.include_path, compiler_type))
        if header_root.header_map != None:
            header_maps.append(header_root.header_map)
        if header_root.file_prefix_args != None:
            file_prefix_args.append(header_root.file_prefix_args)
        if header_root.coverage_prefix_args != None:
            coverage_prefix_args.append(header_root.coverage_prefix_args)

    # Embed raw headers as hidden artifacts in our args.  This means downstream
    # cases which use these args don't also need to know to add raw headers.
    if all_raw_headers:
        # NOTE(agallagher): It's a bit weird adding an "empty" arg, but this
        # appears to do the job (and not e.g. expand to `""`).
        args.append(cmd_args(hidden = all_raw_headers))

    if not args and not file_prefix_args and not coverage_prefix_args:
        return _EMPTY_CPREPROCESSOR_ARGS
    return CPreprocessorArgs(args = args, file_prefix_args = file_prefix_args, coverage_prefix_args = coverage_prefix_args, header_maps = header_maps)

def _by_language_cxx(x: dict[typing.Any, typing.Any]) -> list[typing.Any]:
    return cxx_by_language_ext(x, ".cpp")

def _header_style_args(style: HeaderStyle, path: cmd_args, compiler_type: str) -> list[cmd_args]:
    if style == HeaderStyle("local"):
        return _format_include_arg("-I", path, compiler_type)
    if style == HeaderStyle("system"):
        return format_system_include_arg(path, compiler_type)
    fail("unsupported header style: {}".format(style))

def _attr_raw_headers_as_headers_mode(ctx: AnalysisContext) -> RawHeadersAsHeadersMode:
    """
    Return the `RawHeadersAsHeadersMode` setting to use for this rule.
    """

    mode = get_cxx_toolchain_info(ctx).raw_headers_as_headers_mode

    # If the platform hasn't set a raw headers translation mode, we don't do anything.
    if mode == None:
        return RawHeadersAsHeadersMode("disabled")

    # Otherwise use the rule-specific setting, if provided (not available on prebuilt_cxx_library).
    if getattr(ctx.attrs, "raw_headers_as_headers_mode", None) != None:
        return RawHeadersAsHeadersMode(ctx.attrs.raw_headers_as_headers_mode)

    # Fallback to platform default.
    return mode

def _attr_headers_as_raw_headers_mode(ctx: AnalysisContext) -> HeadersAsRawHeadersMode:
    """
    Return the `HeadersAsRawHeadersMode` setting to use for this rule.
    """

    mode = get_cxx_toolchain_info(ctx).headers_as_raw_headers_mode

    # If the platform hasn't set a raw headers translation mode, we don't do anything.
    if mode == None:
        return HeadersAsRawHeadersMode("disabled")

    # Otherwise use the rule-specific setting, if provided (not available on prebuilt_cxx_library).
    if getattr(ctx.attrs, "headers_as_raw_headers_mode", None) != None:
        return HeadersAsRawHeadersMode(ctx.attrs.headers_as_raw_headers_mode)

    # Fallback to platform default.
    return mode

def _needs_cxx_header_tree_hack(arg: typing.Any) -> bool:
    # The macro $(cxx-header-tree) is used in exactly once place, and its a place which isn't very
    # Buck v2 compatible. We replace $(cxx-header-tree) with HACK-CXX-HEADER-TREE at attribute time,
    # then here we substitute in the real header tree.
    return "HACK-CXX-HEADER-TREE" in repr(arg)

def _cxx_header_tree_hack_replacement(header_tree: Artifact) -> cmd_args:
    # Unfortunately, we can't manipulate flags very precisely (for good reasons), so we rely on
    # knowing the form it takes.
    # The source is: -fmodule-map-file=$(cxx-header-tree)/module.modulemap
    return cmd_args(header_tree, format = "-fmodule-map-file={}/module.modulemap")

# Remap the given headers to be includable via their basenames (for use with
# "apple" style header naming).
def _remap_headers_to_basename(headers: list[CHeader]) -> list[CHeader]:
    remapped_headers = []
    for header in headers:
        if not header.named:
            remapped_headers.append(
                CHeader(
                    artifact = header.artifact,
                    name = paths.basename(header.name),
                    namespace = "",
                    named = False,
                )
            )
    return remapped_headers

def get_flags_for_compiler_type(compiler_type: str) -> list[str]:
    # MSVC requires this flag to enable external headers
    if compiler_type in ["windows"]:
        return ["/experimental:external", "/external:W0"]
    else:
        return []
