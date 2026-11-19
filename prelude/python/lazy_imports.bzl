# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:argfile.bzl", "argfile")
load(":toolchain.bzl", "PythonToolchainInfo")

def _analyzer_python_version(ctx: AnalysisContext) -> str | None:
    python_version = ctx.attrs._python_toolchain[PythonToolchainInfo].python_version
    # The analyzer rejects versions below 3.12; keep its default there.
    if python_version == None or python_version.version_tuple < (3, 12):
        return None
    return "{}.{}".format(python_version.version_tuple[0], python_version.version_tuple[1])

def _add_python_version_arg(ctx: AnalysisContext, cmd: cmd_args) -> None:
    python_version = _analyzer_python_version(ctx)
    if python_version != None:
        cmd.add("--python-version")
        cmd.add(python_version)

def get_lazy_imports_analyzer(ctx: AnalysisContext) -> RunInfo | None:
    """Resolve the lifeguard analyzer binary, checking the target attr first
    then falling back to the toolchain."""
    attr_analyzer = getattr(ctx.attrs, "lazy_imports_analyzer", None)
    if attr_analyzer != None:
        return attr_analyzer[RunInfo]
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    return python_toolchain.lazy_imports_analyzer

def _get_main_module(ctx: AnalysisContext) -> str | None:
    main_module = getattr(ctx.attrs, "main_module", None)
    if main_module != None:
        # A leading dot marks a par-root entry (python_binary derives ".<name>"
        # for main-file binaries); it never matches lifeguard's path-derived
        # module names, so leave the entry unknown instead of failing the build.
        if main_module.startswith("."):
            return None
        return main_module
    main_function = getattr(ctx.attrs, "main_function", None)
    if main_function != None:
        return ""
    return None

def run_lazy_imports_analyzer(ctx: AnalysisContext, resources, output: Artifact, dbg_source_db_output: Artifact) -> DefaultInfo:
    """
    Run the lazy imports analyzer (lifeguard) against a Python binary by
    ingesting the existing dbg-db.json.
    """
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    cmd = cmd_args(ctx.attrs.lazy_imports_analyzer[RunInfo], hidden = resources)
    cmd.add(dbg_source_db_output)  # First arg: <DB_PATH>
    cmd.add(output.as_output())  # Second arg: <OUTPUT_PATH>
    cmd.add("--buck_mode")
    cmd.add("buck-build")
    _add_python_version_arg(ctx, cmd)

    main_module = _get_main_module(ctx)
    if main_module != None:
        cmd.add("--main-module")
        cmd.add(main_module)

    ctx.actions.run(
        cmd,
        category = "py_lazy_import_analysis",
        error_handler = python_toolchain.python_error_handler,
    )

    return DefaultInfo(default_output = output)

def run_lazy_imports_library_analyzer(ctx: AnalysisContext, analyzer: RunInfo, output: Artifact, source_db: DefaultInfo) -> DefaultInfo:
    """
    Run analyze_library: analyze this library's own srcs and produce
    a cache file. Cross-library resolution is deferred to analyze-binary.
    """
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    cmd = cmd_args(analyzer, hidden = source_db.other_outputs)
    cmd.add("analyze-library")
    cmd.add(source_db.default_outputs[0])  # <DB_PATH>
    cmd.add(output.as_output())  # <CACHE_OUTPUT_PATH>
    _add_python_version_arg(ctx, cmd)

    ctx.actions.run(
        cmd,
        category = "py_lazy_import_library_analysis",
        error_handler = python_toolchain.python_error_handler,
    )

    return DefaultInfo(default_output = output)

def run_lazy_imports_cached_analysis(ctx: AnalysisContext, analyzer: RunInfo, output: Artifact, dep_caches: list[Artifact]) -> DefaultInfo:
    """
    Run analyze_binary: assemble the final Lifeguard .json from cached
    library outputs. No per-file analysis happens here.
    """
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    cmd = cmd_args(analyzer)
    cmd.add("analyze-binary")
    cmd.add(output.as_output())  # <OUTPUT_PATH>

    cache_args = cmd_args()
    for cache in dep_caches:
        cache_args.add(cache)
    cmd.add("--cache-manifest")
    cmd.add(argfile(actions = ctx.actions, name = "safer_lazy_imports/cache-manifest.txt", args = cache_args, allow_args = True))
    _add_python_version_arg(ctx, cmd)

    main_module = _get_main_module(ctx)
    if main_module != None:
        cmd.add("--main-module")
        cmd.add(main_module)

    ctx.actions.run(
        cmd,
        category = "py_lazy_import_binary_analysis",
        error_handler = python_toolchain.python_error_handler,
    )

    return DefaultInfo(default_output = output)
