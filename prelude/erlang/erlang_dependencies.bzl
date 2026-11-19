# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":erlang_info.bzl",
    "CodePathTSet",
    "ErlangAppIncludeInfo",
    "ErlangAppInfo",
    "ErlangDependencyInfo",
    "ErlangTestInfo",
)

def _erlang_deps_impl(ctx: AnalysisContext) -> list[Provider]:
    dependencies = {}
    _flatten_dependencies(ctx.attrs.deps, dependencies)
    _flatten_dependencies(ctx.attrs.applications, dependencies)
    _flatten_dependencies(ctx.attrs.included_applications, dependencies)
    _flatten_dependencies(ctx.attrs.extra_includes, dependencies)

    all_includes = {}
    includes = {}
    include_dirs = {}
    header_deps_files = {}
    private_includes = {}
    private_include_dirs = {}
    beams = {}
    all_beams = {}

    for name in dependencies:
        dep = dependencies[name]

        if ErlangAppInfo in dep:
            dep_info = dep[ErlangAppInfo]

            if dep_info.virtual:
                # virtual applications don't directories we need to include
                # we skip this entire step
                continue

            # collect beams
            if dep_info.beams:
                new_beams = dep_info.beams
                for mod in new_beams:
                    if mod in all_beams:
                        fail_dep_conflict("module", mod, all_beams[mod], name)
                    all_beams[mod] = name
                beams[name] = new_beams

            # collect private includes
            if dep_info.private_include_dir:
                private_include_dirs[name] = dep_info.private_include_dir
                new_includes = dep_info.private_includes
                for hrl in new_includes:
                    if hrl in all_includes:
                        fail_dep_conflict("header", hrl, all_includes[hrl], name)
                    all_includes[hrl] = name
                private_includes[name] = new_includes

        elif ErlangAppIncludeInfo in dep:
            dep_info = dep[ErlangAppIncludeInfo]

        elif ErlangTestInfo in dep:
            # for test deps we only care about code path
            continue
        else:
            fail("invalid dep {}", dep)

        # collect includes
        if dep_info.include_dir:
            include_dirs[name] = dep_info.include_dir
            new_includes = dep_info.includes
            for hrl in new_includes:
                if hrl in all_includes:
                    fail_dep_conflict("header", hrl, all_includes[hrl], name)
                all_includes[hrl] = name
            includes[name] = new_includes

        # collect header_deps_files
        if dep_info.header_deps_file:
            header_deps_files[name] = dep_info.header_deps_file

    code_path_tset = ctx.actions.tset(
        CodePathTSet,
        children = _direct_code_path_tsets(
            ctx.attrs.deps + ctx.attrs.applications + ctx.attrs.included_applications,
        ),
    )

    dependency_info = ErlangDependencyInfo(
        dependencies = dependencies,
        includes = includes,
        include_dirs = include_dirs,
        private_includes = private_includes,
        private_include_dirs = private_include_dirs,
        header_deps_files = header_deps_files,
        beams = beams,
        code_path_tset = code_path_tset,
    )

    return [DefaultInfo(), dependency_info]

def _direct_code_path_tsets(deps: list[Dependency]) -> list[CodePathTSet]:
    """Code path sets of the given dependencies, skipping the ones that contribute nothing.

    Virtual applications carry no directory and no dependencies of their own, and
    include-only targets are headers, so neither has a set.
    """
    tsets = []
    for dep in deps:
        if ErlangAppInfo in dep:
            tset = dep[ErlangAppInfo].code_path_tset
        elif ErlangTestInfo in dep:
            tset = dep[ErlangTestInfo].code_path_tset
        else:
            tset = None
        if tset != None:
            tsets.append(tset)
    return tsets

def fail_dep_conflict(kind: str, name: str, app1: str, app2: str) -> None:
    fail("conflicting {} `{}` found, defined in applications '{}' and '{}'".format(kind, name, app1, app2))

erlang_deps_rule = anon_rule(
    impl = _erlang_deps_impl,
    attrs = {
        "applications": attrs.set(attrs.dep(), default = []),
        "deps": attrs.set(attrs.dep(), default = []),
        "extra_includes": attrs.set(attrs.dep(), default = []),
        "included_applications": attrs.set(attrs.dep(), default = []),
    },
    artifact_promise_mappings = {},
)

ErlAppDependencies = dict[str, Dependency]

def flatten_dependencies(deps: list[Dependency]) -> ErlAppDependencies:
    dependencies = {}
    _flatten_dependencies(deps, dependencies)
    return dependencies

# mutates dependencies in place
def _flatten_dependencies(deps: list[Dependency], dependencies: ErlAppDependencies):
    """collect transitive dependencies

    Flatten all transitive dependencies and merge together with the direct
    ones. This is done at every step (with each `erlang_application_impl ` call),
    so we only need to look one hop away.
    """
    for dep in deps:
        if ErlangAppInfo in dep:
            # handle transitive deps
            for trans_dep_val in dep[ErlangAppInfo].dependencies.values():
                dependencies = _safe_add_dependency(dependencies, trans_dep_val)
        elif ErlangTestInfo in dep:
            for trans_dep_val in dep[ErlangTestInfo].dependencies.values():
                dependencies = _safe_add_dependency(dependencies, trans_dep_val)
        dependencies = _safe_add_dependency(dependencies, dep)

    return dependencies

# mutates dependencies in place
def _safe_add_dependency(dependencies: ErlAppDependencies, dep: Dependency):
    """Adds ErlangAppInfo and ErlangAppIncludeInfo dependencies

    ErlangAppInfo (full) application dependencies overwrite include_only dependencies,
    while include_only dependencies are only added if no other dependency is already
    present.

    ErlangAppInfo applications fail, if there is already another full application with the
    same name added.
    """
    if ErlangAppInfo in dep:
        name = dep[ErlangAppInfo].name
        if name in dependencies and ErlangAppInfo in dependencies[name] and dep.label != dependencies[name].label:
            fail(("duplicated application `%s` in dependency tree:\n" + "    %s\n" + "    %s") % (name, str(dep.label), str(dependencies[name].label)))
        else:
            dependencies[name] = dep
    elif ErlangTestInfo in dep:
        dependencies[dep[ErlangTestInfo].name] = dep
    elif dep[ErlangAppIncludeInfo].name not in dependencies:
        dependencies[dep[ErlangAppIncludeInfo].name] = dep
    return dependencies
