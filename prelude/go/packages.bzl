# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//cxx:headers.bzl", "prepare_headers")
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
)
load("@prelude//utils:utils.bzl", "value_or")
load(
    ":coverage.bzl",
    "GoCoverageMode",  # @Unused used as type
)

# Information about a package for GOPACKAGESDRIVER
GoPackageInfo = provider(
    fields = {
        "build_out": provider_field(Artifact),
        "cgo_gen_dir": provider_field(Artifact),
        "go_list_value": provider_field(DynamicValue),  # GoListDynamicValue inside
        "pkg_import_path": provider_field(str),
        # Full list of package source files
        # including generated and files of the package under test
        "srcs": provider_field(list[Artifact]),
    },
)

GoPkg = record(
    # We have to produce allways shared (PIC) and non-shared (non-PIC) archives
    archive_file = field(Artifact),
    archive_file_shared = field(Artifact),
    # The content of export_file and export_file_shared is likely to be the same
    # but we need to produce both to avoid running compilation twice.
    export_file = field(Artifact),
    export_file_shared = field(Artifact),
    coverage_instrumented = field(bool),
)

GoStdlibDynamicValue = provider(
    fields = {
        "pkgs": provider_field(dict[str, GoPkg]),
    },
)

GoStdlib = provider(
    fields = {
        "dynamic_value": provider_field(DynamicValue),  # GoStdlibDynamicValue inside
    },
)

def go_attr_pkg_name(ctx: AnalysisContext) -> str:
    """
    Return the Go package name for the given context corresponding to a rule.
    """
    return value_or(ctx.attrs.package_name, ctx.label.package)

def merge_pkgs(pkgss: list[dict[str, GoPkg]]) -> dict[str, GoPkg]:
    """
    Merge mappings of packages into a single mapping, throwing an error on
    conflicts.
    """

    all_pkgs = {}

    for pkgs in pkgss:
        for name, pkg in pkgs.items():
            if name in all_pkgs and all_pkgs[name] != pkg:
                fail("conflict for package {!r}: {} and {}".format(name, pkg, all_pkgs[name]))
            all_pkgs[name] = pkg

    return all_pkgs

def pkg_artifacts(pkgs: dict[str, GoPkg], shared: bool) -> dict[str, Artifact]:
    """
    Return a map package name to a `shared` or `static` package artifact.
    """
    return {name: pkg.archive_file_shared if shared else pkg.archive_file for name, pkg in pkgs.items()}

def export_files(pkgs: dict[str, GoPkg], shared: bool) -> dict[str, Artifact]:
    """
    Return a map package name to a `shared` or `static` package artifact.
    """
    return {name: pkg.export_file_shared if shared else pkg.export_file for name, pkg in pkgs.items()}

# Keep in sync with: https://github.com/golang/go/blob/go1.26.0/src/cmd/go/internal/load/pkg.go#L1718C5
_cgo_syscall_exclude = set([
    "runtime/cgo",
    "runtime/race",
    "runtime/msan",
    "runtime/asan",
])

def implicit_imports(
    pkg_name: str, pkg_import_path: str, standard: bool, has_cgo_files: bool, coverage_enabled: bool, coverage_mode: GoCoverageMode | None
) -> set[str]:
    imports = set()
    if has_cgo_files:
        if not standard or pkg_import_path != "runtime/cgo":
            imports.add("runtime/cgo")

        if not standard or pkg_import_path not in _cgo_syscall_exclude:
            imports.add("syscall")

    if pkg_name == "main":
        if coverage_enabled and coverage_mode != None:
            imports.add("runtime/coverage")

    if coverage_enabled and coverage_mode == GoCoverageMode("atomic"):
        imports.add("sync/atomic")

    return imports

def make_compile_importcfg(
    actions: AnalysisActions, pkg_import_path: str, deps: dict[str, GoPkg], imports: set[str], import_map: dict[str, str] | None, shared: bool
) -> Artifact:
    required_pkgs = set(imports)  # copy set to avoid modifying it

    # remove fake packages, build system should never try to provide them
    required_pkgs -= set(["unsafe", "builtin", "C"])

    provided_pkgs = set()
    content, a_files = [], []
    for name_, pkg_ in export_files(deps, shared).items():
        # skip packages not used by the current package
        if name_ not in required_pkgs:
            continue

        # Hack: we use cmd_args get "artifact" valid path and write it to a file.
        content.append(cmd_args("packagefile ", name_, "=", pkg_, delimiter = "", hidden = [pkg_]))
        a_files.append(pkg_)
        provided_pkgs.add(name_)

    if len(provided_pkgs) != len(required_pkgs):
        message = "cannot find package(s) when building '{}' (is your BUCK target missing deps?)\n".format(pkg_import_path)
        for imp in required_pkgs.difference(provided_pkgs):
            message += "  - " + imp + "\n"

        fail(message)

    if import_map:
        # add import map entries
        for import_as, import_path in import_map.items():
            content.append(cmd_args("importmap ", import_as, "=", import_path, delimiter = ""))

        unused_importmap_entries = set(import_map.values()).difference(provided_pkgs)
        if len(unused_importmap_entries) > 0:
            message = "found importmap entries for non-existent dependencies:\n"
            for imp in unused_importmap_entries:
                message += "  - " + imp + "\n"

            fail(message)

    importcfg = actions.declare_output("{}.importcfg".format("shared" if shared else "non_shared"), has_content_based_path = True)
    actions.write(importcfg, content)

    return importcfg.with_associated_artifacts(a_files)

def make_link_importcfg(actions: AnalysisActions, deps: dict[str, GoPkg], shared: bool) -> Artifact:
    content, a_files = [], []
    for name_, pkg_ in pkg_artifacts(deps, shared).items():
        content.append(cmd_args("packagefile ", name_, "=", pkg_, delimiter = "", hidden = [pkg_]))
        a_files.append(pkg_)

    importcfg = actions.declare_output("{}.importcfg".format("shared" if shared else "non_shared"), has_content_based_path = True)
    actions.write(importcfg, content)

    return importcfg.with_associated_artifacts(a_files)

# Return "_cgo_export.h" to expose exported C declarations to non-Go rules
def cgo_exported_preprocessor(ctx: AnalysisContext, pkg_info: GoPackageInfo) -> CPreprocessor:
    cxx_toolchain_info = ctx.attrs._cxx_toolchain[CxxToolchainInfo]
    return CPreprocessor(
        args = CPreprocessorArgs(
            args = [
                "-I",
                prepare_headers(
                    ctx.actions,
                    cxx_toolchain_info,
                    {"{}/{}.h".format(ctx.label.package, ctx.label.name): pkg_info.cgo_gen_dir.project("_cgo_export.h")},
                    "cgo-exported-headers",
                    uses_content_based_paths = True,
                ).include_path,
            ]
        )
    )
