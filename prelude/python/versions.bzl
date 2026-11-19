# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

LibraryVersion = record(value = str)
LibraryName = record(value = str)

VersionedDependency = record(
    default_version = LibraryVersion,
    name = LibraryName,
    versions = dict[LibraryVersion, Dependency],
)

VersionedDependenciesInfo = provider(
    doc = """
        Represents all available versioned libraries in the current target and its recursive dependencies.
    """,
    fields = {
        "dependencies": provider_field(dict[LibraryName, VersionedDependency]),
    },
)

def gather_versioned_dependencies(raw_deps: list[Dependency]) -> VersionedDependenciesInfo:
    vdeps = {}
    for dep in raw_deps:
        if VersionedDependenciesInfo not in dep:
            continue
        info = dep[VersionedDependenciesInfo]
        for name in info.dependencies:
            vdep = info.dependencies[name]

            # NOTE: versioned deps with the same name should always be the same
            if name in vdeps and vdeps[name] != vdep:
                fail("Found conflicting contents of versioned dependency '{}'".format(name))
            vdeps[name] = vdep
    return VersionedDependenciesInfo(dependencies = vdeps)

def resolve_versions(versioned_deps: VersionedDependenciesInfo, version_selections: dict[LibraryName, LibraryVersion]) -> list[Dependency]:
    deps = {}
    for name in versioned_deps.dependencies:
        vdep = versioned_deps.dependencies[name]
        ver = version_selections.get(name, vdep.default_version)
        if ver not in vdep.versions:
            fail(
                "No such version for dependency '{}': '{}'. Available versions: {}".format(
                    name,
                    ver.value,
                    vdep.versions.keys(),
                )
            )
        deps[name] = vdep.versions[ver]

    # validate version_selections
    for name in version_selections:
        if name not in deps:
            fail(
                "No such versioned dependency: '{}'. Available choices: {}".format(
                    name,
                    [name.value for name in deps.keys()],
                )
            )

    return deps.values()

def _versioned_library_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.label == None:
        fail("Versioned libraries are not supported for `dynamic_output`")
    name = LibraryName(value = ctx.label.name)
    default = LibraryVersion(value = ctx.attrs.default)
    versions = {LibraryVersion(value = key): ctx.attrs.versions[key] for key in ctx.attrs.versions}

    if default not in versions:
        fail("Default version ({}) is not present. Available versions: {}".format(default, versions.keys()))

    return [
        DefaultInfo(),
        VersionedDependenciesInfo(
            dependencies = {
                name: VersionedDependency(
                    name = name,
                    versions = versions,
                    default_version = default,
                ),
            }
        ),
    ]

versioned_library = rule(
    doc = """
        Represents multiple versions of the same library. This is a less flexible option than simply using [`select()`](https://buck2.build/docs/rule_authors/configurations/) in `deps` attributes of targets, and setting the right constraints on the build to allow [select resolution](https://buck2.build/docs/rule_authors/configurations_by_example/) to resolve the desired version.

        Most notably, `versioned_library` can only be used with rules that specifically support it, unlike `select()`s which work with every rule.
    """,
    impl = _versioned_library_impl,
    attrs = {
        "default": attrs.string(
            doc = "The default version to use, if nothing is specified. Must match a key in `versions`.",
        ),
        "versions": attrs.dict(
            key = attrs.string(),
            value = attrs.dep(),
            doc = "A dictionary that maps versions to existing targets.",
        ),
    },
)
