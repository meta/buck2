# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:command_alias.bzl", "CommandAliasOutput", "command_alias", "command_alias_providers")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused: Used as a type
load("@prelude//zip_file:zip_file_toolchain.bzl", "ZipFileToolchainInfo")
load(":nodejs_closure.bzl", "compose_node_modules")
load(":nodejs_package.bzl", "get_bundled_node", "package_standalone")
load(
    ":nodejs_providers.bzl",
    "NodejsLibraryInfo",
    "NodejsTreeInfo",
    "check_relative_path",
    "collect_packages",
    "get_nodejs_data_deps",
    "get_nodejs_dep_infos",
    "get_nodejs_library_info",
    "get_nodejs_tree_info",
    "get_transitive_outputs",
    "get_transitive_trees",
    "get_trees",
)
load(":nodejs_toolchain.bzl", "NodejsToolchainInfo")

NodejsEntry = record(
    # The file's short path, or the `library`'s entry.
    name = field(str),
    path = field(Artifact | cmd_args),
)

def _library_entry(label: Label, entry_point: str | None, lib_info: NodejsLibraryInfo) -> str:
    if entry_point != None:
        check_relative_path(entry_point, "{}: invalid `entry_point`".format(label))
        return entry_point
    if len(lib_info.bins) == 1:
        return lib_info.bins.values()[0]
    if lib_info.main != None:
        return lib_info.main
    fail("{}: `library` requires `entry_point` when the library sets neither a single `bin` nor `main` attribute".format(label))

def nodejs_tree_infos(ctx: AnalysisContext, dep_infos: list[NodejsLibraryInfo]) -> list[NodejsTreeInfo]:
    """The third-party trees a target runs with: its `node_modules`, or else those of `dep_infos`."""
    if ctx.attrs.node_modules == None:
        return get_trees(get_transitive_trees(ctx.actions, deps = dep_infos))
    return [get_nodejs_tree_info(dep, consumer_label = ctx.label) for dep in ctx.attrs.node_modules]

def nodejs_manifest_name(name: str) -> str:
    chars = []
    for c in name.lower().elems():
        chars.append(c if c.isalnum() or c in "-._~" else "-")
    return "".join(chars).lstrip("._")[:214] or "app"

_FILE_MAIN_EXTENSIONS = [".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".json"]

def _resource_paths(label: Label, resources: dict[str, Artifact] | list[Artifact]) -> dict[str, Artifact]:
    if not isinstance(resources, dict):
        by_path = {}
        for r in resources:
            existing = by_path.get(r.short_path)
            if existing != None and existing != r:
                fail("{}: `resources` entries '{}' and '{}' both stage to '{}'; use a dict to place them".format(label, existing, r, r.short_path))
            by_path[r.short_path] = r
        return by_path
    for path in resources:
        check_relative_path(path, "{}: invalid `resources` path".format(label))
    return resources

def _directory_main(ctx: AnalysisContext, mains: list[Artifact], srcs: list[Artifact], bundled_node: (str, Artifact) | None) -> struct:
    if len(mains) != 1 or mains[0].extension.lower() in _FILE_MAIN_EXTENSIONS:
        fail("{}: `entry_point` requires `library` or a directory `main`".format(ctx.label))
    entry_point = ctx.attrs.entry_point
    check_relative_path(entry_point, "{}: invalid `entry_point`".format(ctx.label))
    rejected = [
        ("`srcs`", srcs),
        ("`deps`", ctx.attrs.deps),
        ("`node_modules`", ctx.attrs.node_modules),
        ('package_style = "standalone"', bundled_node),
        ("a `resources` dict", isinstance(ctx.attrs.resources, dict)),
    ]
    for what, present in rejected:
        if present:
            fail("{}: {} cannot be combined with a directory `main` and `entry_point`, which run in place without staging".format(ctx.label, what))
    return struct(
        dir = None,
        runtime = None,
        entries = [NodejsEntry(name = entry_point, path = cmd_args(mains[0], format = "{}/" + entry_point))],
        dir_paths = [],
        hidden = [],
        resources = list(ctx.attrs.resources),
        env = dict(ctx.attrs.env),
    )

def stage_nodejs_entries(
    ctx: AnalysisContext,
    mains: list[Artifact] | dict[str, Artifact],
    *,
    mains_attr: str = "`main`",
    staged_attrs: str = "`main`/`srcs`",
    srcs: list[Artifact] | dict[str, Artifact] | None = None,
    stage: bool = False,
    bundled_node: (str, Artifact) | None = None,
    directory_main: bool = False,
) -> struct:
    """Stages the files a target runs beside a `node_modules` of `library`, `deps` and trees.

    `mains` are the files to run without `library`; `mains_attr` names them in errors, and
    `staged_attrs` names them together with `srcs` in staging conflicts. `srcs`
    (default: the `srcs` attribute) are staged with `mains`. A list stages each file at its short
    path; a dict maps paths to files, and its `mains` are named by those paths. `mains` run staged when there is a
    `library`, `deps`, a third-party tree, `bundled_node`, a `resources` dict or `stage`.
    `directory_main` lets `entry_point` without `library` run inside a directory `main`. Returns:
    - `dir`: the staged directory, if any.
    - `runtime`: `dir`, or `None` when `mains` run in place.
    - `entries`: a `NodejsEntry` per main, or for the `library`'s entry.
    - `dir_paths`: the path of each entry in `dir`.
    - `hidden`: the inputs of an in-place run.
    - `resources`: the `resources` of an in-place run.
    - `env`: the environment to run with.
    """
    if srcs == None:
        srcs = ctx.attrs.srcs
    lib_info = None
    entry_point = ""
    if ctx.attrs.library != None:
        if mains:
            fail("{}: {} cannot be combined with `library`; use `entry_point`".format(ctx.label, mains_attr))
        if srcs:
            fail("{}: `srcs` cannot be combined with `library`".format(ctx.label))
        lib_info = get_nodejs_library_info(ctx.attrs.library, consumer_label = ctx.label)
        entry_point = _library_entry(ctx.label, ctx.attrs.entry_point, lib_info)
    else:
        if ctx.attrs.entry_point != None:
            if not directory_main:
                fail("{}: `entry_point` requires `library`".format(ctx.label))
            return _directory_main(ctx, mains, srcs, bundled_node)
        if not mains:
            fail("{}: {} is required without `library`".format(ctx.label, mains_attr))
    if isinstance(mains, dict):
        mains_by_path = mains
    else:
        mains_by_path = {}
        for main in mains:
            existing = mains_by_path.get(main.short_path)
            if existing != None and existing != main:
                fail("{}: {} entries '{}' and '{}' both have the short path '{}'".format(ctx.label, mains_attr, existing, main, main.short_path))
            mains_by_path[main.short_path] = main
    dep_infos = ([lib_info] if lib_info != None else []) + get_nodejs_dep_infos(ctx.attrs.deps, consumer_label = ctx.label, allow_data = True)
    data_deps = get_nodejs_data_deps(ctx.attrs.deps, consumer_label = ctx.label)
    tree_infos = nodejs_tree_infos(ctx, dep_infos)
    staged = {}
    if lib_info == None:
        staged.update(mains_by_path)
        for path, src in srcs.items() if isinstance(srcs, dict) else [(src.short_path, src) for src in srcs]:
            existing = staged.get(path)
            if existing != None and existing != src:
                fail("{}: {} entries '{}' and '{}' both stage to '{}'".format(ctx.label, staged_attrs, existing, src, path))
            staged[path] = src
    resources = _resource_paths(ctx.label, ctx.attrs.resources)
    for path, resource in resources.items():
        existing = staged.get(path)
        if existing != None and existing != resource:
            fail("{}: `resources` entry '{}' and {} entry '{}' both stage to '{}' in `[dir]`".format(ctx.label, resource, staged_attrs, existing, path))
        staged[path] = resource
    for data_path, data in data_deps.items():
        for path, existing in staged.items():
            if path == data_path or path.startswith(data_path + "/") or data_path.startswith(path + "/"):
                fail("{}: data dep '{}' at '{}' and {}/`resources` entry '{}' at '{}' overlap".format(ctx.label, data, data_path, staged_attrs, existing, path))
    staged.update(data_deps)
    name = "app"
    if bundled_node:
        node_name, node_dist = bundled_node
        if node_name in staged:
            fail(
                "{}: {}/`resources` entry '{}' and the bundled Node binary '{}' both stage to '{}'".format(
                    ctx.label, staged_attrs, staged[node_name], node_dist, node_name
                )
            )
        staged[node_name] = node_dist
        name = "__standalone__/" + ctx.label.name
    packages = collect_packages(get_transitive_outputs(ctx.actions, deps = dep_infos))
    if "package.json" not in staged:
        # Tools that find the project root beside `node_modules`, such as `module-alias/register`, read it.
        staged["package.json"] = ctx.actions.write_json("app.json", {"name": nodejs_manifest_name(ctx.label.name), "private": True, "version": "0.0.0"})
    staged_dir = compose_node_modules(
        ctx.actions,
        name,
        {n: p.runtime_dir for n, p in packages.items()},
        {n: p.label for n, p in packages.items()},
        tree_infos,
        root_files = staged,
        root_file_desc = "{}: {}/`resources` entry '{{artifact}}' at '{{path}}'".format(ctx.label, staged_attrs),
    )
    runtime = staged_dir if dep_infos or data_deps or tree_infos or bundled_node or stage or isinstance(ctx.attrs.resources, dict) else None

    hidden = []
    dir_paths = list(mains_by_path)
    if lib_info != None:
        dir_paths = ["node_modules/" + lib_info.dir_name + "/" + entry_point]
        entries = [NodejsEntry(name = entry_point, path = cmd_args(runtime, format = "{}/" + dir_paths[0]))]
    elif runtime != None:
        entries = [NodejsEntry(name = path, path = cmd_args(runtime, format = "{}/" + path)) for path in mains_by_path]
    else:
        entries = [NodejsEntry(name = path, path = main) for path, main in mains_by_path.items()]
        hidden = list(srcs.values()) if isinstance(srcs, dict) else srcs

    return struct(
        dir = staged_dir,
        runtime = runtime,
        entries = entries,
        dir_paths = dir_paths,
        hidden = hidden,
        resources = list(ctx.attrs.resources) if runtime == None else [],
        env = dict(ctx.attrs.env),
    )

def nodejs_command(ctx: AnalysisContext, args: cmd_args, env: dict[str, ArgLike]) -> CommandAliasOutput:
    """Runs the toolchain's Node with `args` and `env`, through a trampoline script for the target platform."""
    return command_alias(
        actions = ctx.actions,
        path = None,
        target_os = ctx.attrs._target_os_type[OsLookup],
        base = ctx.attrs._nodejs_toolchain[NodejsToolchainInfo].node,
        args = args,
        env = env,
        labels = ctx.attrs.labels,
    )

def build_nodejs_command(ctx: AnalysisContext, bundled_node: (str, Artifact) | None = None) -> struct:
    """Runs `main`, or the `library`'s entry, with `node_args` and `args`."""
    staged = stage_nodejs_entries(ctx, [ctx.attrs.main] if ctx.attrs.main != None else [], bundled_node = bundled_node, directory_main = True)
    args = cmd_args(ctx.attrs.node_args, staged.entries[0].path, ctx.attrs.args, hidden = staged.hidden)
    return struct(dir = staged.dir, merged = staged.runtime, output = nodejs_command(ctx, args, staged.env), env = staged.env, resources = staged.resources)

def _dir_sub_targets(command: struct) -> dict[str, list[Provider]]:
    return {"dir": [DefaultInfo(default_output = command.dir)]} if command.dir != None else {}

def launcher_providers(command: struct) -> list[Provider]:
    """The launcher as the default output, with the staged directory, if any, as `[dir]`, and `RunInfo`, as `command_alias` returns them."""
    return command_alias_providers(command.output, command.resources, sub_targets = _dir_sub_targets(command))

def nodejs_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    bundled_node = None
    if ctx.attrs.package_style == "standalone":
        bundled_node = get_bundled_node(ctx.label, ctx.attrs._nodejs_toolchain[NodejsToolchainInfo].node_dist, ctx.attrs._target_os_type)
    command = build_nodejs_command(ctx, bundled_node)
    providers = launcher_providers(command)
    if bundled_node:
        providers[0] = DefaultInfo(
            default_output = package_standalone(
                ctx.actions,
                ctx.attrs._zip_file_toolchain[ZipFileToolchainInfo].create_zip,
                command.merged,
                ctx.label.name,
            ),
            other_outputs = providers[0].other_outputs,
            sub_targets = _dir_sub_targets(command),
        )
    return providers

nodejs_binary = prelude_rule(
    name = "nodejs_binary",
    impl = nodejs_binary_impl,
    docs = """A runnable Node.js script, executed with the toolchain's
    Node runtime and no custom loaders. The `[dir]` sub-target, present
    unless `main` is a directory run with `entry_point` (below), is one
    directory holding a `node_modules` composed from the transitive
    `nodejs_library` packages and the packages of the third-party trees,
    `main` and `srcs` beside it at their short paths (distinct files
    staging to the same path fail, as does one under `node_modules` when
    any package is composed), and
    a `package.json` at its root: the one in `srcs`, or else
    `{"name": <target name>, "private": true, "version": "0.0.0"}`, for
    tools that look for the project root beside `node_modules` (the name
    is lowercased and cut to 214 characters, with characters npm rejects
    replaced by `-`). When
    `library`, `deps`, a third-party tree, a `resources` dict or
    `package_style = "standalone"` is involved, the staged `main` runs
    from `[dir]`, so Node's own resolution finds every package, for
    `import`, `require`, worker threads and child `node` processes alike.
    Otherwise, `main` runs at its original path and files it imports via
    relative paths must be listed in `srcs` to be declared as build
    inputs. `NODE_PATH` is never set; one set in `env` is passed through.
    The default output is the launcher: a `command_alias` trampoline for
    the target platform that runs the toolchain's `node` with `node_args`,
    the entry and `args`, and sets `env`; `$(location)` of the target runs
    it. `DefaultInfo` and `RunInfo` are the providers `command_alias`
    returns: `RunInfo` runs the same command, without the trampoline when
    `env` is empty. For an in-place target without `srcs`, the trampoline
    and `RunInfo` match those of `command_alias(exe = <toolchain node>,
    args = node_args + [<entry>] + args, env = env, resources = resources)`.
    `resources` are files and directories the program reads at run time:
    a list stages each at its short path in `[dir]`, a dict at its key.
    When `main` runs in place, a list's entries stay at their original
    paths as hidden inputs, as `command_alias`'s `resources` do; a dict
    always stages. `[dir]` stages a list even when a file `main` runs in
    place, so two list entries with the same short path, or an entry at
    the path of a distinct `main` or `srcs` file, fail either way.
    `main` may be a directory, such as a `filegroup` or a genrule with
    `out = "."`. With `entry_point` (and no `library`), node runs
    `<main>/<entry_point>` in place, with no `[dir]`; a `main` ending in
    `.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts` or `.json` is a file and
    fails. `srcs`, `deps`, `node_modules`, a `resources` dict and
    `package_style = "standalone"` are rejected. Without `entry_point`, node runs `main` itself, which for
    a directory means its `package.json` `main` or `index.js`.
    Deps must be loadable as regular `node_modules` packages: each
    package's `package.json` governs whether it is CommonJS or an ES
    module, and TypeScript must be compiled to JavaScript before it is
    packaged. Each `nodejs_library` package is staged without its
    `.d.ts`/`.d.mts`/`.d.cts` files and their maps (the library's
    `[runtime]`), because Node's type stripping refuses declarations under
    `node_modules`. Transitive deps' package directories must be unique,
    as for `nodejs_library`.
    A `deps` entry that is not a `nodejs_library` is a data dep: it must
    have exactly one default output, which is staged at the dep's package
    path in `[dir]`. A directory output, such as a `filegroup`'s, keeps its
    files at their repository-relative paths (for a test that reads another
    package's files); a single file becomes the file at that path. A data
    dep cannot be a `nodejs_node_modules` tree, a runnable target or in the
    root package, and its path must not be, contain or sit inside another
    data dep's or a staged `main`, `srcs` or `resources` entry's path.
    `node_modules`, when unset, takes the third-party trees of `library`
    and `deps` (see `nodejs_library`'s `node_modules`); a list of
    `nodejs_node_modules` targets replaces them, and `[]` means none.
    Third-party packages are composed one by one as installed (they keep
    their `.d.ts` files; dot entries such as `.bin` are dropped): a
    package that two sources provide fails the build and names both, and
    `exclude` on a `nodejs_node_modules` drops packages.
    `library`, when set, runs a file from inside a `nodejs_library`'s
    package instead of `main` (a library with only `deps` or
    `node_modules` stages none and fails here): `entry_point` names the file relative to the package root
    (e.g. `"main.js"`; a relative `/`-separated path with no `:` and
    no `.` or `..` segments), and defaults to the library's `bin` attribute
    when it has exactly one, else its `main` attribute; entries set only in
    the library's `package_json` or `prebuilt` manifest are not read, so
    such a library needs `entry_point`. `main`/`srcs` cannot be combined
    with `library`, and `entry_point` requires it or a directory `main`.
    `package_style` is `"inplace"` (the default) or `"standalone"`.
    `standalone` always stages, adds the toolchain's `node_dist` binary to
    the staged directory as `node` (`node.exe` on Windows), and replaces
    the default output with a zip of that directory under a top-level
    directory named after the target (which therefore cannot contain
    `/`), for hosts without Node. The package runs as
    `<name>/node <name>/<main>`, or with `library`, as
    `<name>/node <name>/node_modules/<dir>/<entry_point>`, where `<dir>`
    is the library's `out`, or its `package_name` when `out` is unset;
    `args`, `node_args`, `env` and any flags or env the toolchain's `node`
    wrapper adds are not applied there.""",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "main": attrs.option(attrs.source(allow_directory = True), default = None),
            "srcs": attrs.list(attrs.source(), default = []),
            "resources": attrs.named_set(attrs.source(allow_directory = True), sorted = False, default = []),
            "args": attrs.list(attrs.arg(), default = []),
            "node_args": attrs.list(attrs.string(), default = []),
            "env": attrs.dict(attrs.string(), attrs.arg(), default = {}),
            "deps": attrs.list(attrs.dep(), default = []),
            "library": attrs.option(attrs.dep(), default = None),
            "entry_point": attrs.option(attrs.string(), default = None),
            "node_modules": attrs.option(attrs.list(attrs.dep()), default = None),
            "package_style": attrs.enum(["inplace", "standalone"], default = "inplace"),
            "_nodejs_toolchain": toolchains_common.nodejs(),
            "_target_os_type": buck.target_os_type_arg(),
            "_zip_file_toolchain": toolchains_common.zip_file(),
        }
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
