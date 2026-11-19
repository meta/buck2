# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//linking:link_info.bzl", "LinkedObject")
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo")
load("@prelude//utils:arglike.bzl", "ArgLike")
load(":interface.bzl", "PythonLibraryManifestsInterface")
load(":manifest.bzl", "ManifestInfo")
load(":toolchain.bzl", "PythonToolchainInfo")

NativeDepsInfoTSet = transitive_set()

NativeDepsInfo = record(
    native_deps = field(dict[Label, Dependency], dict()),
    dlopen_deps = field(dict[Label, Dependency], dict()),
    shared_only_deps = field(dict[Label, Dependency], dict()),
)

PythonLibraryManifests = record(
    label = field(Label),
    srcs = field([ManifestInfo, None]),
    src_types = field([ManifestInfo, None], None),
    default_resources = field([(ManifestInfo, list[ArgLike]), None]),
    standalone_resources = field([(ManifestInfo, list[ArgLike]), None]),
    outplace_resources = field([(ManifestInfo, list[ArgLike]), None]),
    bytecode = field([ManifestInfo, None]),
    extensions = field([dict[str, LinkedObject], None]),
)

def _bytecode_artifacts(value: PythonLibraryManifests) -> list[ArgLike]:
    return [] if value.bytecode == None else [a for a, _ in value.bytecode.artifacts]

def _bytecode_manifests(value: PythonLibraryManifests) -> list[None] | Artifact:
    return [] if value.bytecode == None else value.bytecode.manifest

def _hidden_resources_for(field_name: str) -> typing.Callable[[PythonLibraryManifests], list[ArgLike]]:
    return lambda value: [] if getattr(value, field_name) == None else getattr(value, field_name)[1]

def _has_hidden_resources_for(field_name: str) -> typing.Callable[[list[bool], [PythonLibraryManifests, None]], bool]:
    def impl(children: list[bool], value: [PythonLibraryManifests, None]) -> bool:
        if value:
            resources = getattr(value, field_name)
            if resources and len(resources[1]) > 0:
                return True
        return any(children)

    return impl

def _resource_manifests_for(field_name: str) -> typing.Callable[[PythonLibraryManifests], list[None] | Artifact]:
    return lambda value: [] if getattr(value, field_name) == None else getattr(value, field_name)[0].manifest

def _resource_artifacts_for(field_name: str) -> typing.Callable[[PythonLibraryManifests], list[ArgLike]]:
    return lambda value: [] if getattr(value, field_name) == None else [a for a, _ in getattr(value, field_name)[0].artifacts]

def _source_manifests(value: PythonLibraryManifests) -> list[None] | Artifact:
    if value.srcs == None:
        return []
    return value.srcs.manifest

def _source_artifacts(value: PythonLibraryManifests) -> list[ArgLike]:
    if value.srcs == None:
        return []
    return [a for a, _ in value.srcs.artifacts]

def _source_type_manifests(value: PythonLibraryManifests) -> list[None] | Artifact:
    if value.src_types == None:
        return []
    return value.src_types.manifest

def _source_type_artifacts(value: PythonLibraryManifests) -> list[ArgLike]:
    if value.src_types == None:
        return []
    return [a for a, _ in value.src_types.artifacts]

def _extension_artifacts(value: PythonLibraryManifests) -> list[Artifact]:
    if value.extensions == None:
        return []
    return [extension.output for extension in value.extensions.values()]

# Mode strings mirror PackageStyle.value
_RESOURCE_MODES = {
    "inplace": ("", "default_resources"),
    "outplace": ("outplace_", "outplace_resources"),
    "standalone": ("standalone_", "standalone_resources"),
}

args_projections = {
    "bytecode_artifacts": _bytecode_artifacts,
    "bytecode_manifests": _bytecode_manifests,
    "extension_artifacts": _extension_artifacts,
    "source_artifacts": _source_artifacts,
    "source_manifests": _source_manifests,
    "source_type_artifacts": _source_type_artifacts,
    "source_type_manifests": _source_type_manifests,
}
args_projections.update({"{}hidden_resources".format(prefix): _hidden_resources_for(field_name) for prefix, field_name in _RESOURCE_MODES.values()})
args_projections.update({"{}resource_manifests".format(prefix): _resource_manifests_for(field_name) for prefix, field_name in _RESOURCE_MODES.values()})
args_projections.update({"{}resource_artifacts".format(prefix): _resource_artifacts_for(field_name) for prefix, field_name in _RESOURCE_MODES.values()})

PythonLibraryManifestsTSet = transitive_set(
    args_projections = args_projections,
    reductions = {"{}has_hidden_resources".format(prefix): _has_hidden_resources_for(field_name) for prefix, field_name in _RESOURCE_MODES.values()},
)

LazyImportsCacheTSet = transitive_set()

PythonLibraryInfo = provider(
    fields = {
        # Shared libraries coming from cxx_python_extension targets
        "extension_shared_libraries": provider_field(SharedLibraryInfo),
        "is_native_dep": provider_field(bool),
        # Transitive lazy imports caches for incremental analysis
        "lazy_imports_caches": provider_field(LazyImportsCacheTSet | None, default = None),
        # See the docs for PythonLibraryManifestsInterface
        "manifests": provider_field(PythonLibraryManifestsTSet),
        # Native deps
        "native_deps": provider_field(NativeDepsInfoTSet),
        # Package style for python binaries (None for libraries). One of
        # "inplace", "standalone", "outplace". Mirrors PackageStyle from toolchain.bzl.
        "package_style": provider_field(str | None, default = None),
        # PAR style for python binaries (None for libraries)
        "par_style": provider_field(str | None, default = None),
        # Shared libraries coming from python_library and others
        "shared_libraries": provider_field(SharedLibraryInfo),
    }
)

def manifests_to_interface(manifests: PythonLibraryManifestsTSet) -> PythonLibraryManifestsInterface:
    return PythonLibraryManifestsInterface(
        src_manifests = lambda: [manifests.project_as_args("source_manifests")],
        src_artifacts = lambda: [manifests.project_as_args("source_artifacts")],
        bytecode_manifests = lambda: [manifests.project_as_args("bytecode_manifests")],
        bytecode_artifacts = lambda: [manifests.project_as_args("bytecode_artifacts")],
        resource_manifests = lambda mode = "inplace": [manifests.project_as_args("{}resource_manifests".format(_RESOURCE_MODES[mode][0]))],
        resource_artifacts = lambda mode = "inplace": [manifests.project_as_args("{}resource_artifacts".format(_RESOURCE_MODES[mode][0]))],
        has_hidden_resources = lambda mode = "inplace": manifests.reduce("{}has_hidden_resources".format(_RESOURCE_MODES[mode][0])),
        hidden_resources = lambda mode = "inplace": [manifests.project_as_args("{}hidden_resources".format(_RESOURCE_MODES[mode][0]))],
    )

# The exported dependencies, and the default deps (if selected)
def python_attr_preload_deps(ctx: AnalysisContext) -> list[Dependency]:
    deps = []

    from_attr = getattr(ctx.attrs, "preload_deps", None)
    if from_attr:
        deps.extend(from_attr)

    toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    if toolchain.preload_deps:
        deps.extend(toolchain.preload_deps)

    return deps
