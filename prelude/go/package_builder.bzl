# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//utils:utils.bzl", "dedupe_by_value")
load(
    ":cgo_builder.bzl",
    "CGoBuildContext",  # @Unused used as type
    "build_cgo",
)
load(":compile.bzl", "get_inherited_compile_pkgs", "infer_package_root")
load(
    ":coverage.bzl",
    "GoCoverageMode",  # @Unused used as type
    "cover_srcs",
)
load(":go_error_handler.bzl", "go_build_error_handler")
load(":go_list.bzl", "GoListDynamicValue", "GoListOut", "go_list")
load(":package_root.bzl", "strip_package_root")
load(":packages.bzl", "GoPackageInfo", "GoPkg", "GoStdlib", "GoStdlibDynamicValue", "implicit_imports", "make_compile_importcfg", "merge_pkgs")
load(":toolchain.bzl", "GoToolchainInfo", "get_toolchain_env_vars")

GoBuildConfig = record(
    compiler_flags = field(list[str], []),
    assembler_flags = field(list[str], []),
    build_tags = field(list[str], []),
    coverage_enabled = field(bool, False),
    coverage_mode = field(GoCoverageMode | None, None),
    cgo_enabled = field(bool, False),
    with_tests = field(bool, False),
)

GoSourceInputs = record(
    srcs = field(list[Artifact]),
    embed_srcs = field(dict[str, Artifact], {}),
    package_root = field(str | None, None),
)

def declare_package_build(
    ctx: AnalysisContext,
    pkg_import_path: str,
    main: bool,
    sources: GoSourceInputs,
    cgo_build_context: CGoBuildContext | None,
    config: GoBuildConfig,
    pkgs: dict[str, GoPkg] = {},
    deps: list[Dependency] = [],
    cgo_gen_dir_name: str = "cgo_gen",
) -> (GoPkg, GoPackageInfo, Artifact):
    actions = ctx.actions
    target_label = ctx.label
    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]
    go_stdlib = ctx.attrs._go_stdlib[GoStdlib]

    asan = go_toolchain.asan
    race = go_toolchain.race
    if race and asan:
        fail("`race=True` and `asan=True` are mutually exclusive")

    if race and config.coverage_mode not in [None, GoCoverageMode("atomic")]:
        fail("`coverage_mode` must be `atomic` when `race=True`")

    out_x = actions.declare_output(paths.basename(pkg_import_path) + "_non-shared.x", has_content_based_path = True)
    out_a = actions.declare_output(paths.basename(pkg_import_path) + "_non-shared.a", has_content_based_path = True)

    out_shared_x = actions.declare_output(paths.basename(pkg_import_path) + "_shared.x", has_content_based_path = True)
    out_shared_a = actions.declare_output(paths.basename(pkg_import_path) + "_shared.a", has_content_based_path = True)

    cgo_gen_dir = actions.declare_output(cgo_gen_dir_name, dir = True, has_content_based_path = True)

    srcs = dedupe_by_value(sources.srcs)

    package_root = sources.package_root if sources.package_root != None else infer_package_root(srcs)

    go_list_value = go_list(actions, go_toolchain, pkg_import_path, srcs, package_root, config.build_tags, config.cgo_enabled, with_tests = config.with_tests)

    test_go_files_argsfile = actions.declare_output(paths.basename(pkg_import_path) + "_test_go_files.go_package_argsfile", has_content_based_path = True)

    all_pkgs = merge_pkgs([
        pkgs,
        get_inherited_compile_pkgs(deps),
    ])

    actions.dynamic_output_new(
        _build_package_action(
            target_label = target_label,
            pkg_import_path = pkg_import_path,
            main = main,
            package_root = package_root,
            embed_srcs = sources.embed_srcs,
            cgo_build_context = cgo_build_context,
            config = config,
            deps_pkgs = all_pkgs,
            go_toolchain = go_toolchain,
            go_stdlib_value = go_stdlib.dynamic_value,
            go_list_value = go_list_value,
            cgo_gen_dir = cgo_gen_dir.as_output(),
            out_a = out_a.as_output(),
            out_shared_a = out_shared_a.as_output(),
            out_shared_x = out_shared_x.as_output(),
            out_x = out_x.as_output(),
            test_go_files_argsfile = test_go_files_argsfile.as_output(),
        )
    )
    return (
        GoPkg(
            archive_file = out_a,
            archive_file_shared = out_shared_a,
            export_file = out_x,
            export_file_shared = out_shared_x,
            coverage_instrumented = config.coverage_enabled and config.coverage_mode != None,
        ),
        GoPackageInfo(
            build_out = out_x,
            cgo_gen_dir = cgo_gen_dir,
            pkg_import_path = pkg_import_path,
            go_list_value = go_list_value,
            srcs = srcs,
        ),
        test_go_files_argsfile.with_associated_artifacts(srcs),
    )

def _build_package_action_impl(
    actions: AnalysisActions,
    target_label: Label,
    pkg_import_path: str,
    main: bool,
    package_root: str,
    embed_srcs: dict[str, Artifact],
    cgo_build_context: None | CGoBuildContext,
    config: GoBuildConfig,
    deps_pkgs: dict[str, GoPkg],
    go_toolchain: GoToolchainInfo,
    go_list_value: ResolvedDynamicValue,
    go_stdlib_value: ResolvedDynamicValue,
    cgo_gen_dir: OutputArtifact,
    out_a: OutputArtifact,
    out_shared_a: OutputArtifact,
    out_shared_x: OutputArtifact,
    out_x: OutputArtifact,
    test_go_files_argsfile: OutputArtifact,
):
    go_list = go_list_value.providers[GoListDynamicValue].go_list_out

    if go_list.error != None:
        fail("Invalid go package: {}", go_list.error.err)

    if len(go_list.x_test_go_files) > 0:
        fail("External tests are not supported, remove suffix '_test' from package declaration '{}': {}", go_list.name, target_label)

    go_stdlib_value = go_stdlib_value.providers[GoStdlibDynamicValue]

    result = build_package(
        actions = actions,
        target_label = target_label,
        go_toolchain = go_toolchain,
        cgo_build_context = cgo_build_context,
        go_list = go_list_for_build(go_list, config.with_tests),
        params = BuildPackageParams(
            main = main,
            standard = False,
            pkg_import_path = pkg_import_path,
            package_root = package_root,
            embed_srcs = embed_srcs,
            compiler_flags = config.compiler_flags,
            assembler_flags = config.assembler_flags,
            coverage_enabled = config.coverage_enabled,
            coverage_mode = config.coverage_mode,
            deps = merge_pkgs([go_stdlib_value.pkgs, deps_pkgs]),
            import_map = None,
        ),
    )

    actions.write(test_go_files_argsfile, cmd_args((go_list.test_go_files if config.with_tests else []), ""))
    actions.copy_dir(cgo_gen_dir, result.cgo_gen_dir)

    actions.copy_file(out_x, result.x_file)
    actions.copy_file(out_a, result.a_file)

    actions.copy_file(out_shared_x, result.x_file_shared)
    actions.copy_file(out_shared_a, result.a_file_shared)

    return []

_build_package_action = dynamic_actions(
    impl = _build_package_action_impl,
    # @unsorted-dict-items
    attrs = {
        # Input Parameters
        "target_label": dynattrs.value(Label),
        "pkg_import_path": dynattrs.value(str),
        "main": dynattrs.value(bool),
        "package_root": dynattrs.value(str),
        "embed_srcs": dynattrs.value(dict[str, Artifact]),
        "cgo_build_context": dynattrs.value(CGoBuildContext | None),
        "config": dynattrs.value(GoBuildConfig),
        "deps_pkgs": dynattrs.value(dict[str, GoPkg]),
        "go_toolchain": dynattrs.value(GoToolchainInfo),
        # Dynamic Values
        "go_list_value": dynattrs.dynamic_value(),  # GoListDynamicValue
        "go_stdlib_value": dynattrs.dynamic_value(),  # GoStdlibDynamicValue
        # Outputs
        "cgo_gen_dir": dynattrs.output(),
        "out_a": dynattrs.output(),
        "out_shared_a": dynattrs.output(),
        "out_shared_x": dynattrs.output(),
        "out_x": dynattrs.output(),
        "test_go_files_argsfile": dynattrs.output(),
    },
)

BuildPackageGoList = record(
    pkg_name = field(str),
    go_files = field(list[Artifact]),
    cgo_files = field(list[Artifact]),
    s_files = field(list[Artifact]),
    h_files = field(list[Artifact]),
    c_cxx_files = field(list[Artifact]),
    syso_files = field(list[Artifact]),
    imports = field(set[str]),
    embed_patterns = field(list[str]),
    cgo_cflags = field(list[str]),
    cgo_cppflags = field(list[str]),
)

def go_list_for_build(go_list_out: GoListOut, with_tests: bool) -> BuildPackageGoList:
    imports = set(go_list_out.imports + (go_list_out.test_imports if with_tests else []))
    embed_patterns = go_list_out.embed_patterns + (go_list_out.test_embed_patterns if with_tests else [])
    go_files = go_list_out.go_files + (go_list_out.test_go_files if with_tests else [])
    c_cxx_files = go_list_out.c_files + go_list_out.cxx_files
    return BuildPackageGoList(
        pkg_name = go_list_out.name,
        go_files = go_files,
        cgo_files = go_list_out.cgo_files,
        s_files = go_list_out.s_files,
        h_files = go_list_out.h_files,
        c_cxx_files = c_cxx_files,
        syso_files = go_list_out.syso_files,
        imports = imports,
        embed_patterns = embed_patterns,
        cgo_cflags = go_list_out.cgo_cflags,
        cgo_cppflags = go_list_out.cgo_cppflags,
    )

BuildPackageParams = record(
    main = field(bool),
    standard = field(bool),
    pkg_import_path = field(str),
    package_root = field(str),
    embed_srcs = field(dict[str, Artifact]),
    compiler_flags = field(list[str]),
    assembler_flags = field(list[str]),
    coverage_enabled = field(bool),
    coverage_mode = field(GoCoverageMode | None),
    deps = field(dict[str, GoPkg]),
    # Importmap is used when the import path we use in the code different
    # from the actual import path our dependency has.
    # For now, it's only used for stdlib's deps.
    # - key: import path in current package's source code
    # - value: actual import path of a dependency
    import_map = field(dict[str, str] | None),
)
BuildPackageResult = record(
    a_file = field(Artifact),
    x_file = field(Artifact),
    a_file_shared = field(Artifact),
    x_file_shared = field(Artifact),
    cgo_gen_dir = field(Artifact),
)

# Packages with forward declarations resolved at runtime.
# Keep in sync with https://github.com/golang/go/blob/go1.26.0/src/cmd/go/internal/work/gc.go#L90
_incomplete_pkgs_allow_list = set([
    "bytes",
    "internal/poll",
    "net",
    "os",
    "runtime/metrics",
    "runtime/pprof",
    "runtime/trace",
    "sync",
    "syscall",
    "time",
])

def build_package(
    actions: AnalysisActions,
    target_label: Label,
    go_toolchain: GoToolchainInfo,
    cgo_build_context: CGoBuildContext | None,
    go_list: BuildPackageGoList,
    params: BuildPackageParams,
) -> BuildPackageResult:
    covered_go_files, covered_cgo_files, coveragecfg = cover_srcs(
        actions = actions,
        go_toolchain = go_toolchain,
        pkg_name = go_list.pkg_name,
        pkg_import_path = params.pkg_import_path,
        go_files = go_list.go_files,
        cgo_files = go_list.cgo_files,
        coverage_enabled = params.coverage_enabled,
        coverage_mode = params.coverage_mode,
    )

    # A go package can can contain CGo or Go ASM files, but not both.
    # If CGo and ASM files are present, we process ASM files together with C files with CxxToolchain.
    # The `go build` command does additional check here and throws an error if both CGo and Go-ASM files are present.
    c_cxx_files = [] + go_list.c_cxx_files
    s_files = go_list.s_files
    if len(go_list.cgo_files) > 0:
        # the only exception when we have both types of ASM files is "runtime/cgo"
        if params.standard and params.pkg_import_path == "runtime/cgo":
            go_s_files = []
            for s_file in s_files:
                if paths.basename(s_file.short_path).startswith("gcc_"):
                    c_cxx_files.append(s_file)
                else:
                    go_s_files.append(s_file)

            s_files = go_s_files
        else:
            c_cxx_files += s_files
            s_files = []

    # Generate CGO and C sources.
    transformed_cgo_files, cgo_o_files, cgo_gen_dir = build_cgo(
        actions = actions,
        target_label = target_label,
        go_toolchain_info = go_toolchain,
        cgo_build_context = cgo_build_context,
        pkg_import_path = params.pkg_import_path,
        package_root = params.package_root,
        standard = params.standard,
        cgo_files = covered_cgo_files,
        h_files = go_list.h_files,
        c_files = c_cxx_files,
        c_flags = go_list.cgo_cflags,
        cpp_flags = go_list.cgo_cppflags,
        anon_targets_allowed = False,
    )

    symabis = _symabis(actions, go_toolchain, params.pkg_import_path, params.main, s_files, go_list.h_files, params.assembler_flags)

    embedcfg = _embedcfg(actions, go_toolchain, params.pkg_import_path, params.package_root, params.embed_srcs, go_list.embed_patterns)

    # Use -complete flag when compiling Go code only
    complete_flag = len(go_list.cgo_files) + len(s_files) + len(c_cxx_files) + len(go_list.syso_files) == 0
    if params.standard and params.pkg_import_path in _incomplete_pkgs_allow_list:
        complete_flag = False

    go_files_to_compile = covered_go_files + transformed_cgo_files

    # Use argsfile to avoid command length limit on Windows. The source list
    # is identical for the shared and non-shared variants, so they share it.
    srcs_argsfile = actions.write("srcs.go_package_argsfile", go_files_to_compile, has_content_based_path = True) if go_files_to_compile else None

    def build_variant(shared: bool) -> (Artifact, Artifact):
        build_variant_id = "shared" if shared else "non-shared"  # use  tomake artifacts and actions unique

        required_imports = go_list.imports | implicit_imports(
            pkg_name = go_list.pkg_name,
            pkg_import_path = params.pkg_import_path,
            standard = params.standard,
            has_cgo_files = len(go_list.cgo_files) > 0,
            coverage_enabled = params.coverage_enabled,
            coverage_mode = params.coverage_mode,
        )

        importcfg = make_compile_importcfg(
            actions = actions,
            pkg_import_path = params.pkg_import_path,
            deps = params.deps,
            imports = required_imports,
            import_map = params.import_map,
            shared = shared,
        )
        go_x_file, go_a_file, asmhdr = _compile(
            actions = actions,
            go_toolchain = go_toolchain,
            pkg_import_path = params.pkg_import_path,
            main = params.main,
            go_srcs = go_files_to_compile,
            srcs_argsfile = srcs_argsfile,
            importcfg = importcfg,
            compiler_flags = params.compiler_flags,
            shared = shared,
            race = go_toolchain.race,
            asan = go_toolchain.asan,
            build_variant_id = build_variant_id,
            complete = complete_flag,
            standard = params.standard,
            coveragecfg = coveragecfg,
            embedcfg = embedcfg,
            symabis = symabis,
            gen_asmhdr = len(s_files) > 0,
        )

        asm_o_files = _asssembly(
            actions, go_toolchain, params.pkg_import_path, params.main, s_files, go_list.h_files, asmhdr, params.assembler_flags, shared, build_variant_id
        )

        return go_x_file, _pack(actions, go_toolchain, params.pkg_import_path, go_a_file, cgo_o_files + asm_o_files + go_list.syso_files, build_variant_id)

    non_shared_x, non_shared_a = build_variant(shared = False)

    shared_x, shared_a = build_variant(shared = True)

    return BuildPackageResult(
        a_file = non_shared_a,
        x_file = non_shared_x,
        a_file_shared = shared_a,
        x_file_shared = shared_x,
        cgo_gen_dir = cgo_gen_dir,
    )

# Keep in sync with https://github.com/golang/go/blob/go1.26.0/src/cmd/go/internal/test/test.go#L941
_libfuzzer_exclude_list = set([
    "context",
    "internal/fuzz",
    "internal/godebug",
    "internal/runtime/maps",
    "internal/sync",
    "reflect",
    "runtime",
    "sync",
    "sync/atomic",
    "syscall",
    "testing",
    "time",
])

def _compile(
    actions: AnalysisActions,
    go_toolchain: GoToolchainInfo,
    pkg_import_path: str,
    main: bool,
    go_srcs: list[Artifact],
    srcs_argsfile: Artifact | None,
    importcfg: Artifact,
    compiler_flags: list[str],
    shared: bool,
    race: bool,
    asan: bool,
    build_variant_id: str,
    complete: bool,
    standard: bool,
    coveragecfg: Artifact | None = None,
    embedcfg: Artifact | None = None,
    symabis: Artifact | None = None,
    gen_asmhdr: bool = False,
) -> (Artifact, Artifact, Artifact | None):
    env = get_toolchain_env_vars(go_toolchain)
    out_a = actions.declare_output("go_compile_out_{}.a".format(build_variant_id), has_content_based_path = True)
    out_x = actions.declare_output("go_compile_out_{}.x".format(build_variant_id), has_content_based_path = True)

    if len(go_srcs) == 0:
        actions.write(out_a.as_output(), "")
        actions.write(out_x.as_output(), "")
        return out_x, out_a, None

    asmhdr = actions.declare_output("__asmhdr__{}/go_asm.h".format(build_variant_id), has_content_based_path = True) if gen_asmhdr else None

    compile_cmd = cmd_args(
        [
            go_toolchain.go_wrapper,
            ["--go", go_toolchain.compiler],
            "--",
            go_toolchain.compiler_flags,
            compiler_flags,
            "-buildid=",
            "-nolocalimports",
            "-pack",
            ["-trimpath", "%cwd%"],
            ["-p", "main" if main else pkg_import_path],
            ["-importcfg", importcfg],
            ["-o", out_x.as_output()],
            ["-linkobj", out_a.as_output()],
            ["-race"] if race else [],
            ["-asan"] if asan else [],
            ["-d=libfuzzer"] if go_toolchain.fuzz and (not standard or pkg_import_path not in _libfuzzer_exclude_list) else [],
            ["-shared"] if shared else [],
            ["-coveragecfg", coveragecfg] if coveragecfg else [],
            ["-embedcfg", embedcfg] if embedcfg else [],
            ["-symabis", symabis] if symabis else [],
            ["-asmhdr", asmhdr.as_output()] if asmhdr else [],
            ["-complete"] if complete else [],
            ["-std"] if standard else [],
            cmd_args(srcs_argsfile, format = "@{}", hidden = go_srcs),
        ],
    )

    actions.run(
        compile_cmd,
        env = env,
        category = "go_compile",
        identifier = "{} [{}]".format(pkg_import_path, build_variant_id),
        error_handler = go_build_error_handler,
        allow_cache_upload = go_toolchain.allow_cache_upload,
    )

    return (out_x, out_a, asmhdr)

def _symabis(
    actions: AnalysisActions,
    go_toolchain: GoToolchainInfo,
    pkg_import_path: str,
    main: bool,
    s_files: list[Artifact],
    h_files: list[Artifact],
    assembler_flags: list[str],
) -> Artifact | None:
    if len(s_files) == 0:
        return None

    env = get_toolchain_env_vars(go_toolchain)

    # we have to supply "go_asm.h" with any content to make asm tool happy
    # its content doesn't matter if -gensymabis provided
    # https://github.com/golang/go/blob/3f8f929d60a90c4e4e2b07c8d1972166c1a783b1/src/cmd/go/internal/work/gc.go#L441-L443
    fake_asmhdr = actions.write("__fake_asmhdr__/go_asm.h", "", has_content_based_path = True)
    symabis = actions.declare_output("symabis", has_content_based_path = True)
    asm_cmd = [
        go_toolchain.go_wrapper,
        ["--go", go_toolchain.assembler],
        "--",
        go_toolchain.assembler_flags,
        assembler_flags,
        _asm_args(go_toolchain, pkg_import_path, main, False),  # flag -shared doesn't matter for symabis
        "-gensymabis",
        ["-o", symabis.as_output()],
        ["-I", cmd_args(fake_asmhdr, parent = 1)],
        [cmd_args(h_files, parent = 1, prepend = "-I")] if h_files else [],
        [cmd_args(s_files, parent = 1, prepend = "-I")],  # some .s files can include other .s files
        ["-trimpath", "%cwd%"],
        s_files,
    ]

    actions.run(asm_cmd, env = env, category = "go_symabis", identifier = pkg_import_path, allow_cache_upload = go_toolchain.allow_cache_upload)

    return symabis

def _asssembly(
    actions: AnalysisActions,
    go_toolchain: GoToolchainInfo,
    pkg_import_path: str,
    main: bool,
    s_files: list[Artifact],
    h_files: list[Artifact],
    asmhdr: Artifact | None,
    assembler_flags: list[str],
    shared: bool,
    build_variant_id: str,
) -> list[Artifact]:
    if len(s_files) == 0:
        return []

    env = get_toolchain_env_vars(go_toolchain)

    o_files = []
    for s_file in s_files:
        o_file = actions.declare_output("{}_{}.o".format(s_file.short_path, build_variant_id), has_content_based_path = True)
        o_files.append(o_file)

        asm_cmd = [
            go_toolchain.go_wrapper,
            ["--go", go_toolchain.assembler],
            "--",
            go_toolchain.assembler_flags,
            assembler_flags,
            _asm_args(go_toolchain, pkg_import_path, main, shared),
            ["-o", o_file.as_output()],
            ["-I", cmd_args(asmhdr, parent = 1)] if asmhdr else [],  # can it actually be None?
            [cmd_args(h_files, parent = 1, prepend = "-I")] if h_files else [],
            [cmd_args(s_files, parent = 1, prepend = "-I")],  # some .s files can include other .s files
            ["-trimpath", "%cwd%"],
            s_file,
        ]

        actions.run(
            asm_cmd,
            env = env,
            category = "go_assembly",
            identifier = "{}/{} [{}]".format(pkg_import_path, s_file.short_path, build_variant_id),
            allow_cache_upload = go_toolchain.allow_cache_upload,
        )

    return o_files

def _pack(
    actions: AnalysisActions, go_toolchain: GoToolchainInfo, pkg_import_path: str, a_file: Artifact, o_files: list[Artifact], build_variant_id: str
) -> Artifact:
    if len(o_files) == 0:
        # no need to repack .a file, if there are no .o files
        return a_file

    env = get_toolchain_env_vars(go_toolchain)

    pkg_file = actions.declare_output("pkg{}.a".format(build_variant_id), has_content_based_path = True)

    pack_cmd = [
        go_toolchain.packer,
        "c",
        pkg_file.as_output(),
        a_file,
        o_files,
    ]

    actions.run(
        pack_cmd,
        env = env,
        category = "go_pack",
        identifier = "{} [{}]".format(pkg_import_path, build_variant_id),
        allow_cache_upload = go_toolchain.allow_cache_upload,
    )

    return pkg_file

def _embedcfg(
    actions: AnalysisActions, go_toolchain: GoToolchainInfo, pkg_import_path: str, package_root: str, embed_srcs: dict[str, Artifact], embed_patterns: list[str]
) -> Artifact | None:
    if len(embed_patterns) == 0:
        return None

    embedcfg = actions.declare_output("embedcfg", has_content_based_path = True)

    srcs_dir = actions.copied_dir(
        "__embed_srcs_dir__",
        {strip_package_root(name, package_root): src for name, src in embed_srcs.items()},
        has_content_based_path = True,
    )

    embed_cmd = [
        go_toolchain.gen_embedcfg,
        ["-o", embedcfg.as_output()],
        ["-pkgdir", srcs_dir],
        embed_patterns,
    ]

    actions.run(embed_cmd, category = "go_embedcfg", identifier = pkg_import_path, allow_cache_upload = go_toolchain.allow_cache_upload)

    return embedcfg.with_associated_artifacts([srcs_dir])

def _asm_args(go_toolchain: GoToolchainInfo, pkg_import_path: str, main: bool, shared: bool):
    return [
        ["-p", "main" if main else pkg_import_path],
        ["-I", go_toolchain.env_go_root.project("pkg/include")] if go_toolchain.env_go_root else [],
        ["-D", "GOOS_" + go_toolchain.env_go_os] if go_toolchain.env_go_os else [],
        ["-D", "GOARCH_" + go_toolchain.env_go_arch] if go_toolchain.env_go_arch else [],
        ["-shared"] if shared else [],
        ["-std"] if go_toolchain.version and go_toolchain.version.minor >= 27 else [],  # todo: safe to remove once go1.28 is released
    ]
