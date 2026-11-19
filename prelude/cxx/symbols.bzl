# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_library_utility.bzl", "EMPTY_DEFAULT_INFO")
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "CxxToolchainInfo",
    "LinkerType",
)
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

def _extract_symbol_names(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    name: str,
    objects: list[Artifact],
    category: str,
    identifier: [str, None] = None,
    defined_only: bool = False,
    undefined_only: bool = False,
    # Do not include undefined-weak symbols.
    undefined_weak: bool = True,
    dynamic: bool = False,
    prefer_local: bool = False,
    local_only: bool = False,
    global_only: bool = False,
    allow_cache_upload: bool = False,
) -> Artifact:
    """
    Generate a file with a sorted list of symbol names extracted from the given
    native objects.
    """

    if not objects:
        fail("no objects provided")

    if defined_only and undefined_only:
        fail("only one of defined_only and undefined_only should be True")

    if not undefined_weak and not undefined_only:
        fail("can only use undefined_weak with undefined_only")

    nm = cxx_toolchain.binary_utilities_info.nm
    output = ctx.actions.declare_output(paths.join("__symbols__", name), has_content_based_path = True)

    nm_flag_suffix = ""
    if global_only:
        nm_flag_suffix += "g"
    if undefined_only:
        nm_flag_suffix += "u"

    # darwin objects don't have dynamic symbol tables.
    if dynamic and cxx_toolchain.linker_info.type != LinkerType("darwin"):
        nm_flag_suffix += "D"

    # llvm-nm supports -U for this but gnu nm doesn't.
    if defined_only:
        nm_flag_suffix += " --defined-only"

    # -A: Prepend all lines with the name of the input file to which it
    # corresponds.  Added only to make parsing the output a bit easier.
    # -P: Generate portable output format
    nm_flags = "-AP" + nm_flag_suffix

    is_windows = hasattr(ctx.attrs, "_exec_os_type") and ctx.attrs._exec_os_type[OsLookup].os == Os("windows")

    if is_windows:
        # Each invocation handles one object, so omit -A. With -P, llvm-nm output
        # starts with the invariant "name type value size", making token 1 the symbol.
        windows_nm_flags = "-P" + nm_flag_suffix
        script = ctx.actions.write(
            paths.join("__symbols__", name + ".extract_symbols.bat"),
            """@echo off
set "OUT=%~1"
set "NM=%~2"
set "RAW=%OUT%.raw"
set "NM_RAW=%OUT%.nm.raw"
"""
            +
            # Buck does not pre-create parent directories for nested declared outputs.
            """for %%D in ("%OUT%") do (
  if not exist "%%~dpD" (
    mkdir "%%~dpD"
    if errorlevel 1 if not exist "%%~dpD" goto extract_failed
  )
)
if exist "%RAW%" (
  del /F /Q "%RAW%"
  if errorlevel 1 goto extract_failed
)
if exist "%NM_RAW%" (
  del /F /Q "%NM_RAW%"
  if errorlevel 1 goto extract_failed
)
shift
shift
:objects_loop
if "%~1"=="" goto objects_done
"""
            +
            # Capture nm output first so quoted paths stay intact and nm failures propagate.
            '"%NM%" '
            + windows_nm_flags
            + """ "%~1" > "%NM_RAW%"
if errorlevel 1 goto extract_failed
for /F "usebackq tokens=1 delims= " %%S in ("%NM_RAW%") do (
  for /F "tokens=1 delims=@" %%T in ("%%S") do (
    echo(%%T | findstr /C:"__odr_asan_gen_" >NUL
    if errorlevel 2 goto extract_failed
"""
            +
            # Prefix the redirection so a trailing 1 or 2 in a symbol name is not
            # parsed as a stream number.
            """    if errorlevel 1 (
      ver >NUL
      >>"%RAW%" echo(%%T
      if errorlevel 1 goto extract_failed
    )
  )
)
shift
goto objects_loop

:objects_done
if not exist "%RAW%" (
  type nul > "%RAW%"
  if errorlevel 1 goto extract_failed
)
"""
            +
            # Windows sort has no /unique switch. Keep duplicates rather than
            # dropping symbols that differ only by case; downstream symbol
            # aggregation does the final unique sort. Use shell redirection
            # because sort's /O option cannot open long Buck output paths. Use
            # the full path because CI hosts put a GNU sort earlier on PATH.
            """"%SystemRoot%\\System32\\sort.exe" /L C < "%RAW%" > "%OUT%"
if errorlevel 1 goto extract_failed
del /F /Q "%RAW%" "%NM_RAW%" >NUL 2>NUL
exit /b 0

:extract_failed
del /F /Q "%RAW%" "%NM_RAW%" >NUL 2>NUL
exit /b 1
""",
        )
        symbol_extraction_args = [
            "cmd.exe",
            "/e:on",
            "/v:off",
            "/d",
            "/c",
            script,
            output.as_output(),
        ]
    else:
        script = (
            "set -euo pipefail; "
            + '"$1" {} "${{@:2}}"'.format(nm_flags)
            + (' | (grep -v "\\sw\\s*$" || true)' if undefined_only and not undefined_weak else "")
            +
            # Grab only the symbol name field.
            ' | cut -d" " -f2 '
            +
            # Strip off ABI Version (@...) when using llvm-nm to keep compat with buck1
            " | cut -d@ -f1 "
            +
            # Remove ASAN ODR generated symbols: __odr_asan_gen_*. They are
            # handled by a separate asan_dynamic_list.txt list of asan patterns.
            # BUT MORE IMPORTANTLY, symbols like __odr_asan_XXX[abi:cxx11] force
            # lld into a code path that repeatedly does a linear scan of all
            # symbols for O(num_patterns_with_bracket * num_symbols) (because of
            # the [] being treated as a glob pattern). This totally tanks link
            # time for builds with sanitizers! Anecdotally, a binary with 3.7M
            # symbols and 2K __odr_asan_XXX[abi:cxx11] can spend 6 mins
            # processing patterns and 10s actually linking. We use sed instead
            # of grep -v here to avoid an error exit code when there's no input
            # symbols, which is not an error for us.
            ' | sed "/__odr_asan_gen_.*/d"'
            +
            # These symbols are meant to be weak locals in the binary, with
            # definitions found on the platform. When using open source
            # toolchain and declaring these symbols as weak, the symbols
            # get promoted to global weak and thus failed to link due to
            # undefined symbols.
            ' | sed "/__gmon_start__/d"'
            + ' | sed "/_ITM_deregisterTMCloneTable/d"'
            + ' | sed "/_ITM_registerTMCloneTable/d"'
            + ' | sed "/MallocExtension_Internal_GetNumericProperty/d"'
            + ' | sed "/_ZTHN3c104impl26raw_local_dispatch_key_setE/d"'
            +
            # Sort and dedup symbols.  Use the `C` locale and do it in-memory to
            # make it significantly faster. CAUTION: if ten of these processes
            # run in parallel, they'll have cumulative allocations larger than RAM.
            " | LC_ALL=C sort -S 10% -u > {}"
        )
        symbol_extraction_args = [
            "/usr/bin/env",
            "bash",
            "-c",
            cmd_args(output.as_output(), format = script),
            "",
        ]

    ctx.actions.run(
        symbol_extraction_args
        + [
            nm,
        ]
        + objects,
        category = category,
        identifier = identifier,
        prefer_local = prefer_local,
        local_only = local_only,
        weight_percentage = 15,  # 10% + a little padding
        allow_cache_upload = allow_cache_upload,
    )

    return output

_SymbolsInfo = provider(
    fields = {
        "artifact": provider_field(typing.Any, default = None),  # "artifact"
    }
)

def _anon_extract_symbol_names_impl(ctx):
    output = _extract_symbol_names(
        ctx = ctx,
        cxx_toolchain = ctx.attrs._cxx_toolchain[CxxToolchainInfo],
        category = ctx.attrs.category,
        dynamic = ctx.attrs.dynamic,
        global_only = ctx.attrs.global_only,
        identifier = ctx.attrs.identifier,
        local_only = ctx.attrs.local_only,
        name = ctx.attrs.output,
        objects = ctx.attrs.objects,
        prefer_local = ctx.attrs.prefer_local,
        undefined_only = ctx.attrs.undefined_only,
        undefined_weak = ctx.attrs.undefined_weak,
        allow_cache_upload = ctx.attrs.allow_cache_upload,
    )
    return [EMPTY_DEFAULT_INFO, _SymbolsInfo(artifact = output)]

# Anonymous wrapper for `extract_symbol_names`.
_anon_extract_symbol_names_impl_rule = anon_rule(
    impl = _anon_extract_symbol_names_impl,
    attrs = {
        "allow_cache_upload": attrs.bool(default = False),
        "category": attrs.string(),
        "dynamic": attrs.bool(default = False),
        "global_only": attrs.bool(default = False),
        "identifier": attrs.option(attrs.string(), default = None),
        "local_only": attrs.bool(default = False),
        "objects": attrs.list(attrs.source()),
        "output": attrs.string(),
        "prefer_local": attrs.bool(default = False),
        "undefined_only": attrs.bool(default = False),
        "undefined_weak": attrs.bool(default = True),
        "_cxx_toolchain": attrs.dep(providers = [CxxToolchainInfo]),
    },
    artifact_promise_mappings = {
        "symbols": lambda p: p[_SymbolsInfo].artifact,
    },
)

def extract_symbol_names(ctx: AnalysisContext, cxx_toolchain: CxxToolchainInfo, name: str, anonymous: bool = False, **kwargs) -> Artifact:
    """
    Generate a file with a sorted list of symbol names extracted from the given
    native objects.
    """

    if anonymous:
        cxx_toolchain_from_attrs = ctx.attrs._cxx_toolchain[CxxToolchainInfo]
        if cxx_toolchain != cxx_toolchain_from_attrs:
            fail("anon symbol extraction requires that the cxx_toolchain be from the _cxx_toolchain attr")
        artifact = ctx.actions.anon_target(
            _anon_extract_symbol_names_impl_rule,
            dict(_cxx_toolchain = ctx.attrs._cxx_toolchain, output = name, **kwargs),
        ).artifact("symbols")
        artifact = ctx.actions.assert_has_content_based_path(artifact)

        return ctx.actions.assert_short_path(artifact, short_path = paths.join("__symbols__", name))
    else:
        return _extract_symbol_names(ctx = ctx, cxx_toolchain = cxx_toolchain, name = name, **kwargs)

def extract_defined_syms(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    output: Artifact,
    category_prefix: str,
    prefer_local: bool = False,
    anonymous: bool = False,
    allow_cache_upload: bool = False,
) -> Artifact:
    return extract_symbol_names(
        ctx = ctx,
        cxx_toolchain = cxx_toolchain,
        name = output.short_path + ".defined_syms.txt",
        objects = [output],
        dynamic = True,
        global_only = True,
        defined_only = True,
        category = "{}_defined_syms".format(category_prefix),
        identifier = output.short_path,
        prefer_local = prefer_local,
        anonymous = anonymous,
        allow_cache_upload = allow_cache_upload,
    )

def extract_undefined_syms(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    output: Artifact,
    category_prefix: str,
    weak: bool = True,
    prefer_local: bool = False,
    anonymous: bool = False,
    allow_cache_upload: bool = False,
    hash_counter = 0,
) -> Artifact:
    name = "extracted_symbol_names/{}-{}.undefined_syms.txt".format(str(hash(output.short_path)), str(hash_counter))
    return extract_symbol_names(
        ctx = ctx,
        cxx_toolchain = cxx_toolchain,
        name = name,
        objects = [output],
        dynamic = True,
        global_only = True,
        undefined_only = True,
        undefined_weak = weak,
        category = "{}_undefined_syms".format(category_prefix),
        identifier = output.short_path,
        prefer_local = prefer_local,
        anonymous = anonymous,
        allow_cache_upload = allow_cache_upload,
    )

def extract_global_syms(
    ctx: AnalysisContext,
    cxx_toolchain: CxxToolchainInfo,
    output: Artifact,
    category_prefix: str,
    prefer_local: bool = False,
    anonymous: bool = False,
    allow_cache_upload: bool = False,
) -> Artifact:
    return extract_symbol_names(
        ctx = ctx,
        cxx_toolchain = cxx_toolchain,
        name = output.short_path + ".global_syms.txt",
        objects = [output],
        dynamic = True,
        global_only = True,
        category = "{}_global_syms".format(category_prefix),
        identifier = output.short_path,
        prefer_local = prefer_local,
        anonymous = anonymous,
        allow_cache_upload = allow_cache_upload,
    )

def _create_symbols_file_from_script(
    actions: AnalysisActions,
    name: str,
    script: str,
    symbol_files: list[Artifact],
    category: str,
    prefer_local: bool,
    weight_percentage: int,
    identifier: [str, None] = None,
) -> Artifact:
    """
    Generate a symbols file from from the given objects and
    link args.
    """

    all_symbol_files = actions.write(name + ".symbols", symbol_files, has_content_based_path = False)
    all_symbol_files = cmd_args(all_symbol_files, hidden = symbol_files)
    output = actions.declare_output(name, has_content_based_path = True)
    cmd = [
        "/usr/bin/env",
        "bash",
        "-c",
        script,
        "",
        all_symbol_files,
        output.as_output(),
    ]
    actions.run(
        cmd,
        category = category,
        prefer_local = prefer_local,
        weight_percentage = weight_percentage,
        identifier = identifier,
    )
    return output

def get_undefined_symbols_args(
    ctx: AnalysisContext, name: str, symbol_files: list[Artifact], category: [str, None] = None, identifier: [str, None] = None, prefer_local: bool = False
) -> cmd_args:
    argsfile = create_undefined_symbols_argsfile(
        ctx.actions,
        name,
        symbol_files,
        category,
        identifier,
        prefer_local,
    )
    return cmd_args(argsfile, format = "@{}")

def create_undefined_symbols_argsfile(
    actions: AnalysisActions, name: str, symbol_files: list[Artifact], category: [str, None] = None, identifier: [str, None] = None, prefer_local: bool = False
) -> Artifact:
    """
    Combine files with sorted lists of symbols names into an argsfile to pass
    to the linker to mark these symbols as undefined via `-u`.
    """
    return _create_symbols_file_from_script(
        actions = actions,
        name = name,
        script = """\
set -euo pipefail
tr '\\n' '\\0' < "$1" > "$2.files0.txt"
LC_ALL=C sort -S 10% -u -m --files0-from="$2.files0.txt" | sed "s/^/-u/" > "$2"
""",
        symbol_files = symbol_files,
        category = category,
        identifier = identifier,
        prefer_local = prefer_local,
        weight_percentage = 15,  # 10% + a little padding
    )

def create_undefined_symbols_linker_script(
    actions: AnalysisActions, name: str, symbol_files: list[Artifact], category: [str, None] = None, identifier: [str, None] = None, prefer_local: bool = False
) -> Artifact:
    """
    Combine files with sorted lists of symbols names into a linker script
    to mark these symbols as undefined via EXTERN.
    """
    return _create_symbols_file_from_script(
        actions = actions,
        name = name,
        script = """\
set -euo pipefail;
echo "EXTERN(" > "$2";
tr '\\n' '\\0' < "$1" > "$2.files0.txt"
LC_ALL=C sort -S 10% -u -m --files0-from="$2.files0.txt" >> "$2";
echo ")" >> "$2";
""",
        symbol_files = symbol_files,
        category = category,
        identifier = identifier,
        prefer_local = prefer_local,
        weight_percentage = 15,  # 10% + a little padding
    )

def create_global_symbols_version_script(
    actions: AnalysisActions, name: str, symbol_files: list[Artifact], identifier: [str, None] = None, category: [str, None] = None, prefer_local: bool = False
) -> Artifact:
    """
    Combine files with sorted lists of symbols names into an argsfile to pass
    to the linker to mark these symbols as undefined (e.g. `-m`).
    """
    return _create_symbols_file_from_script(
        actions = actions,
        name = name,
        script = """\
set -euo pipefail
echo "{" > "$2"
echo "  global:" >> "$2"
tr '\\n' '\\0' < "$1" > "$2.files0.txt"
LC_ALL=C sort -S 10% -u -m --files0-from="$2.files0.txt" | awk '{print "    \\""$1"\\";"}' >> "$2"
echo "  local: *;" >> "$2"
echo "};" >> "$2"
""",
        symbol_files = symbol_files,
        category = category,
        identifier = identifier,
        prefer_local = prefer_local,
        weight_percentage = 15,  # 10% + a little padding
    )

def create_dynamic_list_version_script(
    actions: AnalysisActions, name: str, symbol_files: list[Artifact], identifier: [str, None] = None, category: [str, None] = None, prefer_local: bool = False
) -> Artifact:
    """
    Combine files with sorted lists of symbols names into a dynamic list version
    file that can be passed to the linked (e.g. via `--dynamic-list=<file>`).
    """
    return _create_symbols_file_from_script(
        actions = actions,
        name = name,
        script = """\
set -euo pipefail
echo "{" > "$2"
tr '\\n' '\\0' < "$1" > "$2.files0.txt"
LC_ALL=C sort -S 10% -u -m --files0-from="$2.files0.txt" | awk '{print "    \\""$1"\\";"}' >> "$2"
echo "};" >> "$2"
""",
        symbol_files = symbol_files,
        category = category,
        identifier = identifier,
        prefer_local = prefer_local,
        weight_percentage = 15,  # 10% + a little padding
    )
