# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load("@prelude//tests:re_utils.bzl", "get_re_executors_from_props")
load(":nodejs_binary.bzl", "build_nodejs_command", "launcher_providers", "nodejs_command", "nodejs_manifest_name", "nodejs_tree_infos", "stage_nodejs_entries")
load(":nodejs_providers.bzl", "get_nodejs_dep_infos", "get_nodejs_tree_info")
load(":nodejs_toolchain.bzl", "NodejsToolchainInfo")
load(":nodejs_typescript.bzl", "compile_typescript")

# TPX reads per-test results from `TEST_RESULTS_OUTPUT_FILE` without copying the run's output into each.
_PER_TEST_RESULT_LABELS = ["tpx:supports-test-result-output-spec", "tpx:bundled-tests-have-per-test-details"]

# `node --test` with no arguments runs `**/{test,test/**/*,test-*,*[._-]test}.{js,mjs,cjs}` outside
# `node_modules`, and the TypeScript extensions too unless type stripping is off.
_NODE_TEST_JS_EXTENSIONS = ["js", "mjs", "cjs"]
_NODE_TEST_TS_EXTENSIONS = ["ts", "mts", "cts"]

def _is_node_test_file(short_path: str, strip_types: bool) -> bool:
    parts = short_path.split("/")
    dirs = parts[:-1]
    if "node_modules" in dirs:
        return False
    stem, dot, ext = parts[-1].rpartition(".")
    if not dot or not (ext in _NODE_TEST_JS_EXTENSIONS or (strip_types and ext in _NODE_TEST_TS_EXTENSIONS)):
        return False
    return "test" in dirs or stem == "test" or stem.startswith("test-") or stem.endswith((".test", "-test", "_test"))

def _node_test_files(ctx: AnalysisContext, srcs: dict[str, Artifact]) -> dict[str, Artifact]:
    if ctx.attrs.library != None:
        return {}
    strip_types = "--no-strip-types" not in ctx.attrs.node_args and "--no-experimental-strip-types" not in ctx.attrs.node_args
    test_files = {path: src for path, src in srcs.items() if _is_node_test_file(path, strip_types)}
    if not test_files:
        fail(
            "{}: no `srcs` entry is a node:test test file; node:test runs `**/{{test,test/**/*,test-*,*[._-]test}}.{{js,mjs,cjs{}}}` outside `node_modules`".format(
                ctx.label,
                ",ts,mts,cts" if strip_types else "",
            )
        )
    return test_files

# `.tsx` assumes `jsx` is not `preserve`.
_TS_EMITS = {".cts": ".cjs", ".mts": ".mjs", ".ts": ".js", ".tsx": ".js"}

def _emitted_path(short_path: str) -> str | None:
    for declaration in (".d.ts", ".d.mts", ".d.cts"):
        if short_path.endswith(declaration):
            return None
    stem, dot, ext = short_path.rpartition(".")
    emitted = _TS_EMITS.get(dot + ext)
    return stem + emitted if dot and emitted else None

def _staged_srcs(ctx: AnalysisContext) -> (dict[str, Artifact], dict[str, str]):
    """The `srcs` by the path each is staged at, and the source path of each compiled one."""
    if ctx.attrs.tsconfig == None:
        by_path = {}
        for src in ctx.attrs.srcs:
            existing = by_path.get(src.short_path)
            if existing != None and existing != src:
                fail("{}: `srcs` entries '{}' and '{}' both have the short path '{}'".format(ctx.label, existing, src, src.short_path))
            by_path[src.short_path] = src
        return by_path, {}
    # JSON sources go to `tsc` for the modules that import them.
    compiled = [
        src for src in ctx.attrs.srcs if _emitted_path(src.short_path) != None or (src.short_path.endswith(".json") and src.short_path != "package.json")
    ]
    package_json = [src for src in ctx.attrs.srcs if src.short_path == "package.json"]
    dep_infos = get_nodejs_dep_infos(ctx.attrs.deps, consumer_label = ctx.label, allow_data = True)
    emit = compile_typescript(
        actions = ctx.actions,
        label = ctx.label,
        tsc = ctx.attrs._nodejs_toolchain[NodejsToolchainInfo].tsc,
        srcs = compiled,
        src_root = "",
        tsconfig = ctx.attrs.tsconfig,
        package_json = package_json[0] if package_json else ctx.actions.write_json("ts/t.json", {"name": nodejs_manifest_name(ctx.label.name)}),
        dep_infos = dep_infos,
        identifier = ctx.attrs.name,
        trees = nodejs_tree_infos(ctx, dep_infos),
        rule = "nodejs_test",
    )
    staged = {}
    origins = {}
    sources = {}
    for src in ctx.attrs.srcs:
        emitted = _emitted_path(src.short_path)
        path = emitted or src.short_path
        existing = origins.get(path)
        if existing != None and existing != src:
            fail("{}: `srcs` entries '{}' and '{}' both stage to '{}'".format(ctx.label, existing, src, path))
        origins[path] = src
        staged[path] = emit.project(emitted) if emitted else src
        if emitted:
            sources[path] = src.short_path
    return staged, sources

def _jest_install(ctx: AnalysisContext) -> Artifact:
    if ctx.attrs.test_runner != None:
        return get_nodejs_tree_info(ctx.attrs.test_runner, consumer_label = ctx.label, attr = "test_runner").node_modules
    jest = ctx.attrs._nodejs_toolchain[NodejsToolchainInfo].jest
    if jest == None:
        fail('{}: framework = "jest" requires `jest` on the nodejs toolchain, or `test_runner`'.format(ctx.label))
    return jest

def _vitest_install(ctx: AnalysisContext) -> Artifact:
    if ctx.attrs.test_runner != None:
        return get_nodejs_tree_info(ctx.attrs.test_runner, consumer_label = ctx.label, attr = "test_runner").node_modules
    vitest = ctx.attrs._nodejs_toolchain[NodejsToolchainInfo].vitest
    if vitest == None:
        fail('{}: framework = "vitest" requires `vitest` on the nodejs toolchain, or `test_runner`'.format(ctx.label))
    return vitest

def _framework_command(ctx: AnalysisContext) -> struct:
    jest = None
    vitest = None
    if ctx.attrs.framework == "jest":
        if ctx.attrs.library != None:
            fail(
                '{}: `library` cannot be combined with framework = "jest", which ignores test files under `node_modules`, where `library` stages `entry_point`; list the test files in `srcs` and the library in `deps`'.format(
                    ctx.label
                )
            )
        jest = _jest_install(ctx)
    if ctx.attrs.framework == "vitest":
        if ctx.attrs.library != None:
            fail(
                '{}: `library` cannot be combined with framework = "vitest", which ignores test files under `node_modules`, where `library` stages `entry_point`; list the test files in `srcs` and the library in `deps`'.format(
                    ctx.label
                )
            )
        vitest = _vitest_install(ctx)
    if ctx.attrs.main != None:
        fail('{}: `main` requires framework = "none"; list the test files in `srcs`'.format(ctx.label))
    if ctx.attrs.args and jest == None and vitest == None:
        fail(
            '{}: `args` cannot be combined with framework = "{}", which takes no script arguments; use framework = "none" to pass `args` to `main`'.format(
                ctx.label, ctx.attrs.framework
            )
        )
    for attr in ("coverage_prefix", "coverage_paths"):
        if getattr(ctx.attrs, attr) != None and jest == None and vitest == None:
            fail('{}: `{}` requires framework = "jest" or "vitest"'.format(ctx.label, attr))
    for reserved in ("LIST_COMMAND", "TEST_COMMAND"):
        if reserved in ctx.attrs.env:
            fail('{}: `env` cannot set `{}` with framework = "{}", which uses it to list and run tests'.format(ctx.label, reserved, ctx.attrs.framework))
    by_path, sources = _staged_srcs(ctx)
    if jest != None or vitest != None:
        mains = by_path
        srcs = dict(by_path)
        if ctx.attrs.config != None:
            existing = srcs.get(ctx.attrs.config.short_path)
            if existing != None and existing != ctx.attrs.config:
                fail("{}: `config` '{}' and `srcs` entry '{}' both stage to '{}'".format(ctx.label, ctx.attrs.config, existing, ctx.attrs.config.short_path))
            srcs[ctx.attrs.config.short_path] = ctx.attrs.config
    else:
        mains = _node_test_files(ctx, by_path)
        srcs = by_path

    staged = stage_nodejs_entries(
        ctx, mains, mains_attr = "`srcs`", staged_attrs = "`srcs`", srcs = srcs, stage = jest != None or vitest != None or ctx.attrs.tsconfig != None
    )
    args = cmd_args(ctx.attrs._test_adapter, "--framework", ctx.attrs.framework, hidden = staged.hidden)
    for arg in ctx.attrs.node_args:
        args.add("--node-arg", arg)
    if jest != None:
        args.add("--jest", jest, "--root", staged.runtime)
        if ctx.attrs.config != None:
            args.add("--config", cmd_args(staged.runtime, format = "{}/" + ctx.attrs.config.short_path))
        if ctx.attrs.coverage_prefix != None:
            args.add("--coverage-prefix", ctx.attrs.coverage_prefix)
        if ctx.attrs.coverage_paths != None:
            args.add("--coverage-paths", json.encode(ctx.attrs.coverage_paths))
        for arg in ctx.attrs.args:
            args.add("--arg", arg)
    if vitest != None:
        args.add("--vitest", vitest, "--root", staged.runtime)
        if ctx.attrs.config != None:
            args.add("--config", cmd_args(staged.runtime, format = "{}/" + ctx.attrs.config.short_path))
        if ctx.attrs.coverage_prefix != None:
            args.add("--coverage-prefix", ctx.attrs.coverage_prefix)
        if ctx.attrs.coverage_paths != None:
            args.add("--coverage-paths", json.encode(ctx.attrs.coverage_paths))
        for arg in ctx.attrs.args:
            args.add("--arg", arg)
    for entry in staged.entries:
        args.add("--test-file", sources.get(entry.name, entry.name), entry.path)
    return struct(dir = staged.dir, merged = staged.runtime, output = nodejs_command(ctx, args, staged.env), env = staged.env, resources = staged.resources)

def nodejs_test_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs.framework not in ("jest", "vitest"):
        for attr in ("config", "test_runner"):
            if getattr(ctx.attrs, attr) != None:
                fail('{}: `{}` requires framework = "jest" or "vitest"'.format(ctx.label, attr))
    if ctx.attrs.tsconfig != None and ctx.attrs.framework == "none":
        fail('{}: `tsconfig` requires framework = "node", "jest" or "vitest"; framework = "none" runs `main` as it is'.format(ctx.label))
    command = build_nodejs_command(ctx) if ctx.attrs.framework == "none" else _framework_command(ctx)
    cmd = cmd_args(command.output.cmd, hidden = command.resources)
    if ctx.attrs.framework == "none":
        test_type = "custom"
        env = command.env
        labels = ctx.attrs.labels
    else:
        test_type = "js_test"
        env = command.env | {
            "LIST_COMMAND": cmd_args(cmd, "--list"),
            "TEST_COMMAND": cmd_args(cmd, "--write-test-results"),
        }
        labels = ctx.attrs.labels + [label for label in _PER_TEST_RESULT_LABELS if label not in ctx.attrs.labels]

    re_executors = get_re_executors_from_props(ctx)
    use_project_relative_paths = "buck2_run_from_project_root" in ctx.attrs.labels or re_executors.use_project_relative_paths

    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = test_type,
            command = [cmd],
            env = env,
            labels = labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executors.default_executor,
            executor_overrides = re_executors.executor_overrides,
            run_from_project_root = True,
            use_project_relative_paths = use_project_relative_paths or None,
            supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
        ),
    ) + [launcher_providers(command)[0]]

nodejs_test = prelude_rule(
    name = "nodejs_test",
    impl = nodejs_test_impl,
    docs = """A Node.js test, run with the toolchain's Node runtime and no
    custom loaders. `framework` decides what runs and what is reported.

    `"node"` (the default) runs each test file with Node's built-in test
    runner, `node:test` (Node 20 or later), in a process of its own. Its
    test files are the `srcs` that `node --test` with no arguments would
    run: short paths matching `**/*.test.<ext>`, `**/*-test.<ext>`,
    `**/*_test.<ext>`, `**/test-*.<ext>`, `**/test.<ext>` or
    `**/test/**/*.<ext>` outside `node_modules`, where `<ext>` is `js`,
    `mjs` or `cjs`, or `ts`, `mts` or `cts` unless `node_args` turns off
    type stripping (`--no-strip-types` or `--no-experimental-strip-types`).
    TypeScript needs Node 22.18+, or 22.6+ with `--experimental-strip-types`.
    With `library`, the test file is the `entry_point`. `args` cannot be
    set. Test files running at once prefix each line they print with
    `[<test file>] `.

    `"jest"` runs the test files with Jest from `test_runner` (a
    `nodejs_node_modules` target), or the nodejs toolchain's `jest`. Its
    test files are the `srcs` that the Jest config selects, as
    `jest --listTests` reports them; `library` cannot be set. `config`, a
    Jest config file, is staged at its short path, and Jest's `rootDir`
    defaults to the config file's directory; without it, `rootDir` is the
    staged directory. `args` are passed to Jest, except the watch modes,
    the flags the adapter sets (`--ci`, `--watchman`, `--config`,
    `--json`, `--outputFile`, `--listTests`, `--runTestsByPath`,
    `--testNamePattern`, `--coverage`/`--collectCoverage`,
    `--coverageReporters`, `--coverageDirectory`) and the flags that change
    which test files run (`--onlyChanged`, `--onlyFailures`,
    `--changedSince`, `--changedFilesWithAncestor`, `--lastCommit`,
    `--findRelatedTests`, `--shard`), which fail the test. Coverage is
    collected as for `"vitest"` below, with Jest's
    `--coverage --coverageReporters=json`; the Jest config decides which
    files are instrumented (`collectCoverageFrom`,
    `coveragePathIgnorePatterns`).

    `"vitest"` runs the test files with Vitest from `test_runner` (a
    `nodejs_node_modules` target), or the nodejs toolchain's `vitest`. Its
    test files are the `srcs` that the Vitest config selects, as
    `vitest list --filesOnly` reports them; `library` cannot be set.
    `config`, a Vitest config file, is staged at its short path and passed
    with `--config`; without it, Vitest's own config discovery under the
    staged directory applies. `args` are passed to Vitest, except the
    flags the adapter sets (`run`, `--root`, `--config`, `--reporter`,
    `--outputFile`, `--testNamePattern`, the `--coverage*` flags, and
    `list`/`--filesOnly`/`--json`) and
    the flags that change which test files run (`--changed`, `--related`,
    `--shard`, `--dir`, `--exclude`), which fail the test. Without
    `tsconfig`, the `srcs` are staged as they are and Vitest transpiles
    TypeScript itself; a type error then fails the test, not the build.
    When TPX coverage collection is active
    (`COVERAGE_COLLECTION_OUTPUT_FILE` is set, as it is for targets with
    the `tpx:supports_coverage` label), the adapter runs Vitest with
    `--coverage --coverage.reporter=json`, writing each Vitest run's
    report to a directory of its own under `COVERAGE_COLLECTION_TMPDIR`
    (or a temporary directory), then merges the reports and writes TPX coverage
    JSONL to `COVERAGE_COLLECTION_OUTPUT_FILE`: one
    `{"filepath", "covered_lines", "executable_lines"}` object per file
    (0-based lines, from the Istanbul `statementMap`), for files under
    the staged directory outside `node_modules`. `filepath` is the path
    relative to the staged directory, prefixed with `coverage_prefix`
    when it is set, so a caller whose staged files mirror a project
    subtree (for example `coverage_prefix =
    "xplat/wearable/tools/js/core_utils_common"`) gets paths relative
    to the repository root. `coverage_paths`, when set, keeps only the
    files whose `filepath` is one of its entries or under one of them, as
    for selective coverage, where the `fb_native` wrapper sets it from
    `code_coverage.file_path_filter` and `folder_path_filter`; `[]` keeps
    none. A missing or unreadable coverage report is a run failure.

    `tsconfig`, with `"node"`, `"jest"` or `"vitest"`, compiles the `.ts`, `.tsx`, `.mts`
    and `.cts` `srcs` (not declarations) together with the toolchain's
    `tsc`, as `nodejs_library` compiles its `srcs`, against the
    `nodejs_library` `deps`, the third-party trees and the `.json` `srcs`
    they import, then stages each TypeScript source as the file `tsc`
    emits for it:
    its short path with the extension `.js`, `.mjs` or `.cjs` (`.js` for
    `.tsx`, so `jsx` must not be `preserve`). A `package.json` in `srcs`
    decides the emitted module format. Test files are then selected among
    the emitted names (for `"jest"`, the config must match them) but keep
    their source paths in results. Other `srcs` are staged as they are. A
    type error fails the build rather than the test.

    The other `srcs` are helpers, not run, and `main` cannot be set.
    Without `library`, the test fails if no `srcs` entry is a test file: at
    analysis with `"node"`, when listing or running with `"jest"` or `"vitest"`.
    `node_args` go to the Node processes running the test files. Each test
    file gets a result named by its short path (by the `entry_point` for
    `library`), and each test that finishes gets one named
    `<test file> > <describe> > ... > <test>`. With `"node"`, a
    `describe` block gets one only when it is skipped or fails on its own,
    and a test that fails only because a subtest failed gets none.
    Same-named sibling tests share a result name. A test file
    fails if any of its tests fails. With `"jest"` or `"vitest"`, it also fails if it
    fails to load or the runner reports no result for it. With `"node"`, it also
    fails if its process exits non-zero or before node:test reports the
    end of the run, or its events cannot be read in full, naming tests
    that never finished. With `"jest"`, an unsuccessful Jest run with no
    failed test file, as for a missed coverage threshold, is a run
    failure. With `"vitest"`, an unsuccessful Vitest run with no failed
    test file is likewise a run failure.

    The test is an external runner test of type `js_test`. `LIST_COMMAND`
    writes one JSON line `{"testcase": <test file>}` per test file to the
    file named by an appended `--output <file>`. `TEST_COMMAND` runs
    the test files and tests named by appended `--filter <name>` arguments
    (every test file without one) and, when `TEST_RESULTS_OUTPUT_FILE` is
    set, writes results there in the test result output spec. A filter
    names a test file, a `describe` block (every test in it) or a test, and
    fails if it matches no test. With `"jest"`, a filter below a test file
    ignores case. With `"vitest"`, a filter below a test file is matched
    as a regular expression against the full test name, case-sensitively. With `"node"`, filtering below a test file
    needs Node 22 or later and cannot select a subtest of a `test()`. Run
    on its own, as by `buck2 run`, the test writes no results and exits non-zero
    on failure.

    `"none"` runs `main`, or the `library`'s `entry_point`, with `args` and
    `node_args`; its exit code is the only result. `srcs` are helpers.

    The `[dir]` sub-target, present unless `main` is a directory run with
    `entry_point`, is one directory, built as `nodejs_binary` builds it:
    a composed `node_modules`, `srcs` and `main` beside it at their short
    paths, which must not collide, and a root `package.json`. When
    `library`, `deps`, a third-party tree or a `resources` dict is
    involved, or with `"jest"` or `"vitest"`, the test runs from `[dir]`; with
    `library`, the `entry_point` runs in the library's package under
    `node_modules`. Otherwise, the test files run in place, and files they
    import relatively must be in `srcs` to be build inputs. `NODE_PATH` is
    never set; one set in `env` is passed through. The default output is
    the launcher, a `command_alias` wrapper script for the target platform
    that sets `env` on Unix and Windows. `library`, `entry_point` (and its
    default) and `node_modules` (unset inherits the trees of `library` and
    `deps`; a list replaces them; `[]` means none) follow the same rules as
    `nodejs_binary`, as do package staging, `.d.ts` stripping and `deps`
    that are not `nodejs_library` targets, which are data deps staged in
    `[dir]`.
    `resources`, and with `"none"` a directory `main` with `entry_point`,
    also follow `nodejs_binary`. `resources` that are not staged are hidden
    inputs of the test command.""",
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "main": attrs.option(attrs.source(allow_directory = True), default = None),
            "srcs": attrs.list(attrs.source(), default = []),
            "resources": attrs.named_set(attrs.source(allow_directory = True), sorted = False, default = []),
            "framework": attrs.enum(["node", "jest", "vitest", "none"], default = "node"),
            "config": attrs.option(attrs.source(allow_directory = False), default = None),
            "tsconfig": attrs.option(attrs.source(allow_directory = False), default = None),
            "test_runner": attrs.option(attrs.dep(), default = None),
            "coverage_prefix": attrs.option(attrs.string(), default = None),
            "coverage_paths": attrs.option(attrs.list(attrs.string()), default = None),
            "args": attrs.list(attrs.arg(), default = []),
            "node_args": attrs.list(attrs.string(), default = []),
            "env": attrs.dict(attrs.string(), attrs.arg(), default = {}),
            "deps": attrs.list(attrs.dep(), default = []),
            "library": attrs.option(attrs.dep(), default = None),
            "entry_point": attrs.option(attrs.string(), default = None),
            "node_modules": attrs.option(attrs.list(attrs.dep()), default = None),
            "supports_test_execution_caching": attrs.bool(default = True),
            "_nodejs_toolchain": toolchains_common.nodejs(),
            "_target_os_type": buck.target_os_type_arg(),
            "_test_adapter": attrs.default_only(attrs.source(default = "prelude//nodejs/tools:nodejs_test_adapter.cjs")),
        }
        | buck.labels_arg()
        | buck.contacts_arg()
        | buck.inject_test_env_arg()
        | buck.test_rule_timeout_ms()
        | test_common.attributes()
        | re_test_common.test_args()
    ),
)
