# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""`lean_library`: compiles Lean modules and the C that `lean` generates.

The modules are the files of `srcs` and the `.lean` files below the
directories of `src_dirs`, usually generated, which `lean_deps` lists at build
time, so their number need not be known in BUCK.

One action runs `lean --deps-json` on all sources to read their imports. A
dynamic action then declares one `lean` action per module, in import order,
and one C compile action per module. The objects go into one static archive;
`link_whole` says whether it links whole. When the toolchain has
`c_export_flags`, a second C compile action per module makes the object that
exports every symbol; `LeanModule.export_object` carries it for the rules
that link shared libraries of Lean code. The `[lean]` sub-target is the
directory of the `.olean` files, `[modules]` the module names, one per line,
in build order: a module after the modules it imports.

Imports resolve through `LEAN_PATH`: every module of a library writes its
`.olean` below one directory of the library, and the `LEAN_PATH` of a module
lists that directory for its own library and for every library it depends on,
each once, in dependency order. Two libraries cannot have a module of the same
name. The inputs of a module are only the `.olean` and IR files of the modules
it imports, transitively, and the `resources` of the target: files that the
modules read at compile time with `include_str`.

`covers` names the files that the modules model, for example the code that a
specification is about. No action that compiles the modules reads them. They
are sources of the target, so queries find the target from a file:
`owner(<file>)` returns it, and `attrregexfilter(covers, '<file>$',
owner(<file>))` returns only the targets that name the file in `covers`, not
in `srcs` or `resources`. The `[covers]` sub-target is a directory of symlinks
to the files.
"""

load("@prelude//cxx:archive.bzl", "archive_flags")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//cxx:preprocessor.bzl", "cxx_merge_cpreprocessors")
load("@prelude//cxx:target_sdk_version.bzl", "get_target_sdk_version_flags")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//linking:link_groups.bzl", "merge_link_group_lib_info")
load(
    "@prelude//linking:link_info.bzl",
    "Archive",
    "ArchiveLinkable",
    "LibOutputStyle",
    "LinkInfo",
    "LinkInfos",
    "MergedLinkInfo",
    "create_merged_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo", "merge_shared_libraries")
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:graph_utils.bzl", "post_order_traversal")
load("@prelude//utils:utils.bzl", "filter_and_map_idx")
load(
    ":providers.bzl",
    "LeanArtifactsTSet",
    "LeanLibraryInfo",
    "LeanModule",
    "LeanModulesValue",
    "LeanToolchainInfo",
)
load(":transition.bzl", "lean_transition")

def lean_module_name(src: Artifact, package: str, root: str) -> str:
    """The module name of `src`: its path below `root`, with `.` for `/`."""
    path = src.short_path
    if package and path.startswith(package + "/"):
        path = path[len(package) + 1 :]
    if root:
        if not path.startswith(root + "/"):
            fail("Lean source `{}` is not below `root` `{}`".format(path, root))
        path = path[len(root) + 1 :]
    if not path.endswith(".lean"):
        fail("Lean source `{}` does not end with `.lean`".format(path))
    return path[: -len(".lean")].replace("/", ".")

def _import_arts(direct: list[(str, bool, bool, bool)], modules: dict[str, LeanModule]) -> (dict[str, list[list[Artifact]]], list[typing.Any]):
    """The import artifacts of a module of the module system, as Lean loads
    them on the command line (`importModulesCore`, and Lake's
    `fetchTransImportArts`): the exported `.olean` of every module reachable
    through `public import`, all `.olean` levels for `import all`, and the IR
    for `meta import`, transitively. Returns the `importArts` of the module's
    `--setup` file and the transitive sets of the modules that resolve
    through `LEAN_PATH` instead: those of libraries without `import_cutoff`
    (the core library, third-party packages), which rarely change, and
    modules without the module system, whose imports Lean loads at every
    level.
    """
    levels = {}
    order = []
    fallback = {}
    queue = [(m, a, meta) for m, a, _e, meta in reversed(direct)]

    # Starlark has no `while`. The walk ends: a module is visited once and
    # widened at most twice, and only a visit or a widening queues imports.
    for _ in range(0x7FFFFFFF):
        if not queue:
            break
        name, import_all, meta = queue.pop()
        module = modules[name]
        if not module.import_cutoff or not module.is_module:
            fallback[name] = module.artifacts
            continue
        old = levels.get(name)
        if old != None:
            if (old[0] or not import_all) and (old[1] or not meta):
                continue
            import_all = import_all or old[0]
            meta = meta or old[1]
        else:
            order.append(name)
        levels[name] = (import_all, meta)
        for child, child_all, child_exported, child_meta in reversed(module.imports):
            reachable = import_all or child_exported
            child_meta = meta or (reachable and child_meta)
            if reachable or child_meta:
                queue.append((child, import_all and child_all, child_meta))

    arts = {}
    for name in order:
        module = modules[name]
        import_all, meta = levels[name]
        if import_all:
            arts[name] = [[module.olean, module.olean_server, module.olean_private], [module.ir_sig, module.ir]]
        elif meta:
            arts[name] = [[module.olean], [module.ir_sig, module.ir]]
        else:
            arts[name] = [[module.olean]]
    return arts, fallback.values()

def _compile_impl(
    actions: AnalysisActions,
    label: Label,
    deps_json: ArtifactValue,
    src_names: list[str],
    src_files: list[Artifact],
    src_dirs: list[Artifact],
    resources: list[Artifact],
    dep_values: list[ResolvedDynamicValue],
    lean: RunInfo,
    lean_flags: list[typing.Any],
    lean_path_separator: str,
    c_compile: cmd_args,
    c_export_flags: list[typing.Any],
    object_extension: str,
    import_cutoff: bool,
    archive_cmd: cmd_args,
    archiver_supports_argfiles: bool,
    archive: OutputArtifact,
    lean_dir: OutputArtifact,
    module_list: OutputArtifact,
) -> list[Provider]:
    deps = deps_json.read_json()

    # The modules of `src_dirs` come after those of `srcs`, in the order of
    # `lean_deps`, which is the order of `imports`.
    src_names = list(src_names)
    src_files = list(src_files)
    for src_dir, paths in zip(src_dirs, deps["dirs"]):
        for path in paths:
            src_names.append(path[: -len(".lean")].replace("/", "."))
            src_files.append(src_dir.project(path))

    headers = deps["imports"]
    if len(headers) != len(src_files):
        fail("`lean --deps-json` returned {} results for {} files".format(len(headers), len(src_files)))

    # Modules from the dependencies. Two dependencies may bring the same
    # library (`Init` through `Std` and through `Lean`), but not two different
    # libraries with the same module name.
    library = str(label.raw_target())
    modules = {}
    dep_roots = {}
    for value in dep_values:
        info = value.providers[LeanModulesValue]
        for name, module in info.modules.items():
            other = modules.get(name)
            if other != None and other.library != module.library:
                fail("{}: module `{}` is in two dependencies, `{}` and `{}`".format(label, name, other.library, module.library))
            modules[name] = module
        for key, dep_root in info.roots.items():
            if key not in dep_roots:
                dep_roots[key] = dep_root

    own = {}
    imports = {}
    import_flags = {}
    is_module = {}
    for name, src, header in zip(src_names, src_files, headers):
        if header["errors"]:
            fail("{}: cannot read the imports of `{}`:\n{}".format(label, src.short_path, "\n".join(header["errors"])))
        if name in own:
            fail("{}: two sources have the module name `{}`".format(label, name))
        if name in modules:
            fail("{}: `{}` has the module name `{}`, which is also in the dependency `{}`".format(label, src.short_path, name, modules[name].library))
        own[name] = src
        is_module[name] = header["result"]["isModule"]
        own_imports = []
        flags = []
        for imp in header["result"]["imports"]:
            module = imp["module"]
            if module not in own_imports:
                own_imports.append(module)
            flags.append((module, imp.get("importAll", False), imp.get("isExported", True), imp.get("isMeta", False)))
        imports[name] = own_imports
        import_flags[name] = flags

    for name in src_names:
        for module in imports[name]:
            if module not in own and module not in modules:
                fail("{}: module `{}` imports `{}`, which is neither in `srcs` nor in a dependency".format(label, name, module))

    own_imports_only = {name: [m for m in imports[name] if m in own] for name in src_names}

    objects = []
    lean_dir_files = {}
    # Every module after the modules it imports. `post_order_traversal` fails
    # on an import cycle.
    order = post_order_traversal(own_imports_only)
    actions.write(module_list, "".join([name + "\n" for name in order]))
    for name in order:
        src = own[name]
        path = name.replace(".", "/")
        depth = len(name.split("."))
        olean = actions.declare_output("lean", path + ".olean", has_content_based_path = False)
        ilean = actions.declare_output("lean", path + ".ilean", has_content_based_path = False)
        c_file = actions.declare_output("c", path + ".c", has_content_based_path = False)
        parts = []
        if is_module[name]:
            # With the module system, `lean -o X.olean` also writes these.
            parts = [
                actions.declare_output("lean", path + ".olean.server", has_content_based_path = False),
                actions.declare_output("lean", path + ".olean.private", has_content_based_path = False),
                actions.declare_output("lean", path + ".ir.sig", has_content_based_path = False),
                actions.declare_output("lean", path + ".ir", has_content_based_path = False),
            ]

        lean_dir_files[path + ".olean"] = olean
        lean_dir_files[path + ".ilean"] = ilean
        for part, extension in zip(parts, [".olean.server", ".olean.private", ".ir.sig", ".ir"]):
            lean_dir_files[path + extension] = part

        import_sets = [modules[m].artifacts for m in imports[name]]
        root = cmd_args(olean, parent = depth, ignore_artifacts = True)

        # With `import_cutoff`, a module of the module system gets only the
        # levels of its imports that it can see, so a change that keeps them
        # (a proof, a private declaration, a position) does not rerun it:
        # Buck keys the action on those files alone.
        if is_module[name] and import_cutoff:
            import_arts, fallback = _import_arts(import_flags[name], modules)
            setup = actions.write_json(
                "setup/" + path + ".json",
                {"dynlibs": [], "importArts": import_arts, "isModule": True, "name": name, "options": {}, "plugins": []},
            )
            setup_args = ["--setup", setup]
            inputs = [f for levels in import_arts.values() for files in levels for f in files] + [s.project_as_args("artifacts") for s in fallback]
        else:
            setup_args = []
            inputs = [s.project_as_args("artifacts") for s in import_sets]

        actions.run(
            cmd_args(
                lean,
                src,
                "-R",
                cmd_args(src, parent = depth),
                "-o",
                olean.as_output(),
                "-i",
                ilean.as_output(),
                "-c",
                c_file.as_output(),
                setup_args,
                lean_flags,
                hidden = [part.as_output() for part in parts] + inputs + resources,
            ),
            env = {"LEAN_PATH": cmd_args([root] + dep_roots.values(), delimiter = lean_path_separator)},
            category = "lean",
            identifier = name,
        )

        obj = actions.declare_output("obj", path + "." + object_extension, has_content_based_path = False)
        actions.run(
            cmd_args(c_compile, "-c", c_file, "-o", obj.as_output()),
            category = "lean_cc",
            identifier = name,
        )
        objects.append(obj)

        export_obj = None
        if c_export_flags:
            export_obj = actions.declare_output("obj-export", path + "." + object_extension, has_content_based_path = False)
            actions.run(
                cmd_args(c_compile, c_export_flags, "-c", c_file, "-o", export_obj.as_output()),
                category = "lean_cc_export",
                identifier = name,
            )

        modules[name] = LeanModule(
            name = name,
            library = library,
            root = root,
            artifacts = actions.tset(
                LeanArtifactsTSet,
                value = [olean] + parts,
                children = import_sets,
            ),
            object = obj,
            export_object = export_obj,
            is_module = is_module[name],
            olean = olean,
            olean_server = parts[0] if parts else None,
            olean_private = parts[1] if parts else None,
            ir_sig = parts[2] if parts else None,
            ir = parts[3] if parts else None,
            imports = import_flags[name],
            import_cutoff = import_cutoff,
        )

    if archiver_supports_argfiles:
        objects_args = at_argfile(
            actions = actions,
            name = "objects.argsfile",
            args = cmd_args(objects, quote = "shell"),
        )
    else:
        objects_args = cmd_args(objects)
    actions.run(
        cmd_args(archive_cmd, archive, objects_args),
        env = {"ZERO_AR_DATE": "1"},
        category = "archive",
    )

    actions.copied_dir(lean_dir, lean_dir_files)

    roots = {}
    if src_names:
        roots[library] = modules[src_names[0]].root
    roots.update(dep_roots)
    return [LeanModulesValue(modules = modules, roots = roots)]

_compile = dynamic_actions(
    impl = _compile_impl,
    attrs = {
        "archive": dynattrs.output(),
        "archive_cmd": dynattrs.value(cmd_args),
        "archiver_supports_argfiles": dynattrs.value(bool),
        "c_compile": dynattrs.value(cmd_args),
        "c_export_flags": dynattrs.value(list[typing.Any]),
        "dep_values": dynattrs.list(dynattrs.dynamic_value()),
        "deps_json": dynattrs.artifact_value(),
        "import_cutoff": dynattrs.value(bool),
        "label": dynattrs.value(Label),
        "lean": dynattrs.value(RunInfo),
        "lean_dir": dynattrs.output(),
        "lean_flags": dynattrs.value(list[typing.Any]),
        "lean_path_separator": dynattrs.value(str),
        "module_list": dynattrs.output(),
        "object_extension": dynattrs.value(str),
        "resources": dynattrs.value(list[Artifact]),
        "src_dirs": dynattrs.value(list[Artifact]),
        "src_files": dynattrs.value(list[Artifact]),
        "src_names": dynattrs.value(list[str]),
    },
)

def _c_compile_cmd(ctx: AnalysisContext, cxx: CxxToolchainInfo, toolchain: LeanToolchainInfo) -> cmd_args:
    pre = cxx_merge_cpreprocessors(ctx.actions, [], toolchain.c_preprocessor_infos)
    c_compiler = cxx.c_compiler_info
    return cmd_args(
        c_compiler.compiler,
        c_compiler.preprocessor_flags,
        c_compiler.compiler_flags,
        # The Apple toolchains name the target with `-target`, as the cxx
        # rules do; without it, a cross build from Linux gets Linux objects.
        get_target_sdk_version_flags(ctx),
        pre.set.project_as_args("args"),
        pre.set.project_as_args("include_dirs"),
        toolchain.c_compiler_flags,
        ctx.attrs.c_compiler_flags,
    )

def _archive_cmd(cxx: CxxToolchainInfo) -> cmd_args:
    linker_info = cxx.linker_info
    cmd = cmd_args(linker_info.archiver)
    cmd.add(
        archive_flags(
            linker_info.archiver_type,
            linker_info.type,
            linker_info.use_archiver_flags,
            linker_info.archive_symbol_table,
            False,
        )
    )
    if linker_info.use_archiver_flags and linker_info.archiver_flags != None:
        cmd.add(linker_info.archiver_flags)
    return cmd

def compile_lean_library(ctx: AnalysisContext, link_whole: bool) -> struct:
    """Compiles the sources of a `lean_library`, `lean_binary` or `lean_test`.

    Returns a struct with `archive`, `lean_dir`, `module_list` (the module
    names, one per line, in build order), `modules` (the `LeanLibraryInfo`
    dynamic value), `link_infos`, `merged_link_info` and `native_deps`.
    """
    toolchain = ctx.attrs.lean_toolchain[LeanToolchainInfo]
    cxx = get_cxx_toolchain_info(ctx)

    src_files = ctx.attrs.srcs
    src_names = [lean_module_name(src, ctx.label.package, ctx.attrs.root) for src in src_files]

    # A copy of each source directory, at a path that does not change with
    # the directory's content. A generated directory usually has a
    # content-based path, which changes when any file in it changes; the
    # path of every module is in its `lean` command, so with the original
    # directory one changed module would miss the cache for all of them.
    src_dirs = []
    for i, src_dir in enumerate(ctx.attrs.src_dirs):
        src_dirs.append(ctx.actions.copy_dir("src_dirs/{}".format(i), src_dir, has_content_based_path = False))

    # `lean_deps` lists the modules and runs `lean --deps-json` on them, with
    # the file names on stdin, so their number has no limit.
    deps_json = ctx.actions.declare_output("deps.json", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            ctx.attrs._lean_deps[RunInfo],
            cmd_args(deps_json.as_output(), format = "--out={}"),
            at_argfile(actions = ctx.actions, name = "deps.argsfile", args = cmd_args(src_files, format = "--src={}")),
            cmd_args(src_dirs, format = "--src-dir={}"),
            toolchain.lean,
            "--deps-json",
        ),
        category = "lean_deps",
    )

    # `lean` runs on the execution platform and reads `LEAN_PATH` with the
    # path separator of that OS.
    lean_path_separator = ";" if ctx.attrs._exec_os_type[OsLookup].os == Os("windows") else ":"

    lean_deps = toolchain.stdlib + ctx.attrs.deps
    archive = ctx.actions.declare_output("lib{}.{}".format(ctx.label.name, cxx.linker_info.static_library_extension), has_content_based_path = False)
    lean_dir = ctx.actions.declare_output("lean", dir = True, has_content_based_path = False)
    module_list = ctx.actions.declare_output("modules.txt", has_content_based_path = False)
    modules = ctx.actions.dynamic_output_new(
        _compile(
            label = ctx.label,
            deps_json = deps_json,
            src_names = src_names,
            src_files = src_files,
            src_dirs = src_dirs,
            resources = ctx.attrs.resources,
            dep_values = [dep[LeanLibraryInfo].modules for dep in lean_deps],
            lean = toolchain.lean,
            lean_flags = toolchain.lean_flags + ctx.attrs.lean_flags,
            lean_path_separator = lean_path_separator,
            c_compile = _c_compile_cmd(ctx, cxx, toolchain),
            c_export_flags = toolchain.c_export_flags,
            object_extension = cxx.linker_info.object_file_extension,
            import_cutoff = ctx.attrs.import_cutoff,
            archive_cmd = _archive_cmd(cxx),
            archiver_supports_argfiles = bool(cxx.linker_info.archiver_supports_argfiles),
            archive = archive.as_output(),
            lean_dir = lean_dir.as_output(),
            module_list = module_list.as_output(),
        )
    )

    native_deps = lean_deps + toolchain.runtime
    link_infos = {}
    for output_style in LibOutputStyle:
        link_infos[output_style] = LinkInfos(
            default = LinkInfo(
                linkables = [
                    ArchiveLinkable(
                        archive = Archive(artifact = archive),
                        linker_type = cxx.linker_info.type,
                        link_whole = link_whole,
                    )
                ],
            ),
        )

    return struct(
        archive = archive,
        lean_dir = lean_dir,
        module_list = module_list,
        modules = modules,
        link_infos = link_infos,
        merged_link_info = create_merged_link_info(
            ctx,
            pic_behavior = cxx.pic_behavior,
            link_infos = link_infos,
            preferred_linkage = Linkage("static"),
            exported_deps = filter_and_map_idx(MergedLinkInfo, native_deps),
        ),
        native_deps = native_deps,
    )

def lean_default_info(ctx: AnalysisContext, default_output: Artifact, sub_targets: dict[str, list[Provider]] = {}) -> DefaultInfo:
    """The `DefaultInfo` of a `lean_library`, `lean_binary` or `lean_test`:
    `default_output`, `sub_targets` and, for a target with `covers`, the
    `[covers]` sub-target."""
    covers = ctx.attrs.covers
    if covers == None:
        return DefaultInfo(default_output = default_output, sub_targets = sub_targets)
    if not covers:
        fail("{}: `covers` names no file".format(ctx.label))
    files = {}
    for file in covers:
        # A label would add the target as a dependency. Its output can also be
        # a file of another package, which `owner()` does not give to this
        # target, at a path that a file of this package has too.
        if isinstance(file, Dependency):
            fail("{}: `covers` takes files of the package, and `{}` is a target".format(ctx.label, file.label.raw_target()))
        files[file.short_path] = file

    # Named after the target: `covers` alone is the path of the executable of
    # a `lean_binary` or `lean_test` with that name.
    covers_dir = ctx.actions.symlinked_dir(ctx.label.name + ".covers", files)
    return DefaultInfo(
        default_output = default_output,
        # Buck accepts the name of a file that does not exist until an action
        # reads the file, and no other action of the target reads these. As
        # an other output, the directory makes a build of the target fail on
        # such a name.
        other_outputs = [covers_dir],
        sub_targets = sub_targets | {"covers": [DefaultInfo(default_output = covers_dir)]},
    )

def lean_library_impl(ctx: AnalysisContext) -> list[Provider]:
    library = compile_lean_library(ctx, link_whole = ctx.attrs.link_whole)
    return [
        lean_default_info(
            ctx,
            library.archive,
            {
                "lean": [DefaultInfo(default_output = library.lean_dir)],
                "modules": [DefaultInfo(default_output = library.module_list)],
            },
        ),
        LeanLibraryInfo(modules = library.modules),
        library.merged_link_info,
        merge_link_group_lib_info(deps = library.native_deps),
        merge_shared_libraries(ctx.actions, deps = filter_and_map_idx(SharedLibraryInfo, library.native_deps)),
        create_linkable_graph(
            ctx,
            node = create_linkable_graph_node(
                ctx,
                linkable_node = create_linkable_node(
                    ctx,
                    default_soname = None,
                    preferred_linkage = Linkage("static"),
                    exported_deps = library.native_deps,
                    link_infos = library.link_infos,
                ),
            ),
            deps = library.native_deps,
        ),
    ]

# The attributes of every Lean rule that compiles sources.
lean_library_attrs = {
    "c_compiler_flags": attrs.list(attrs.arg(), default = [], doc = "Extra flags for the compilation of the generated C."),
    "covers": attrs.option(
        # `attrs.source()` alone takes a label too, and gives the output of
        # the target. With `attrs.dep()` first, a label is a `Dependency`,
        # which `lean_default_info` refuses.
        attrs.list(attrs.one_of(attrs.dep(), attrs.source())),
        default = None,
        doc = 'Source files of the package that the modules model, for example the code that a specification is about. They make the target an owner of the files for queries: `buck2 uquery "owner(<file>)"` returns it. No action that compiles the modules reads them, so a change to one recompiles nothing. A list without files is an error, so that a `glob` that matches nothing does not pass unseen; so are the label of a target, and a file that does not exist, when the target is built. The `[covers]` sub-target is a directory of symlinks to the files.',
    ),
    "deps": attrs.list(attrs.dep(providers = [LeanLibraryInfo]), default = [], doc = "`lean_library` targets whose modules `srcs` import."),
    "import_cutoff": attrs.bool(
        default = False,
        doc = "Compile each module of the module system against only the import levels it can see: the exported `.olean` of a `public import` chain, all levels for `import all`, the IR for `meta import`, passed with `lean --setup`. A change to a module that keeps its exported `.olean` (a proof, a private or unexposed body, a position) then reruns no module that imports it. Imports from libraries without the attribute, and modules without the module system, keep their full transitive files.",
    ),
    "labels": attrs.list(attrs.string(), default = []),
    "lean_flags": attrs.list(attrs.arg(), default = [], doc = "Extra flags for `lean`, for example `-DmaxHeartbeats=0`."),
    "lean_toolchain": attrs.toolchain_dep(
        providers = [LeanToolchainInfo],
        default = "toolchains//:lean",
        doc = "The Lean toolchain. Bootstrap builds of the core library set it to an earlier stage.",
    ),
    "resources": attrs.list(
        attrs.source(),
        default = [],
        doc = "Files that the modules read at compile time, with `include_str`. Every `lean` action of the target gets them as inputs, at their source paths, so a module reaches them with a path relative to its own file. They are not runtime resources of a `lean_binary`.",
    ),
    "root": attrs.string(
        default = "",
        doc = "The directory, relative to the package, that module names start at. `<root>/Foo/Bar.lean` is module `Foo.Bar`.",
    ),
    "src_dirs": attrs.list(
        attrs.source(allow_directory = True),
        default = [],
        doc = "Directories of `.lean` files, usually generated, for modules whose number is known only at build time. A module's name is its path below the directory: `<dir>/Foo/Bar.lean` is module `Foo.Bar`; `root` does not apply. Files that do not end in `.lean` are ignored. The modules join those of `srcs` in the library.",
    ),
    "srcs": attrs.list(attrs.source(), default = [], doc = "The `.lean` files."),
    "_cxx_toolchain": attrs.toolchain_dep(default = "toolchains//:cxx", providers = [CxxToolchainInfo]),
    "_exec_os_type": buck.exec_os_type_arg(),
    "_lean_deps": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//lean/tools:lean_deps")),
}

lean_library = rule(
    impl = lean_library_impl,
    attrs = lean_library_attrs
    | {
        "link_whole": attrs.bool(
            default = False,
            doc = "Link all objects of the library into binaries. The `lean` compiler needs this for the core library, because its interpreter finds native code with `dlsym`.",
        ),
    },
    cfg = lean_transition,
)
