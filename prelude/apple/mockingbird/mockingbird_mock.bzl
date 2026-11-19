# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":mockingbird_types.bzl",
    "MockingbirdLibraryInfo",
    "MockingbirdLibraryRecord",
    "MockingbirdSourcesInfo",
    "MockingbirdToolchainInfo",
)

def mockingbird_mock_impl(ctx: AnalysisContext) -> list[Provider]:
    if not MockingbirdLibraryInfo in ctx.attrs.module:
        fail("This module does not contain any swift files. Mockingbird only creates generated mock files for swift code.")

    mockingbird_info = ctx.attrs.module[MockingbirdLibraryInfo]

    dep_names = [dep[MockingbirdLibraryInfo].name for dep in ctx.attrs.deps]
    included_srcs = [src.basename for src in ctx.attrs.srcs]
    excluded_srcs = [src.basename for src in ctx.attrs.excluded_srcs]

    for src_name in included_srcs:
        if not src_name.endswith(".swift"):
            fail("srcs should only specify Swift files. Other source files, such as {}, do not need to be included.".format(src_name))

    for src_name in excluded_srcs:
        if not src_name.endswith(".swift"):
            fail("excluded_srcs should only specify Swift files. Other source files, such as {}, do not need to be included.".format(src_name))

    (json_project_description, src_dirs) = _get_mockingbird_json_project_description(
        info = mockingbird_info, included_srcs = included_srcs, excluded_srcs = excluded_srcs, dep_names = dep_names
    )
    json_project_description_output = ctx.actions.declare_output("mockingbird_project.json", has_content_based_path = True)
    ctx.actions.write_json(json_project_description_output.as_output(), json_project_description)

    mockingbird_source = ctx.actions.declare_output(mockingbird_info.name + "Mocks.generated.swift", dir = False, has_content_based_path = True)
    cmd = cmd_args(
        hidden = src_dirs,
    )

    params = [
        ctx.attrs._mockingbird_toolchain[MockingbirdToolchainInfo].bin,
        "generate",
        "--target",
        mockingbird_info.name,
        "--project",
        json_project_description_output,
        "--output",
        mockingbird_source.as_output(),
        "--support",
        ctx.attrs._mockingbird_toolchain[MockingbirdToolchainInfo].support,
        "--verbose",
        "--disable-cache",
    ]

    if ctx.attrs.only_protocols:
        params.append("--only-protocols")

    cmd.add(params)

    ctx.actions.run(
        cmd,
        category = "mockingbird",
        local_only = True,  # Mockingbird creates sockets for interprocess communication, which is deliberately blocked on RE.
        allow_cache_upload = True,
    )

    return [
        DefaultInfo(mockingbird_source),
        MockingbirdSourcesInfo(srcs = [mockingbird_source]),
    ]

def mockingbird_mock_attrs():
    attribs = {
        ## If the superclass for an object being mocked is in another module add it as a dep so mockingbird can find the implementation.
        "deps": attrs.list(attrs.dep(), default = []),
        ## The list of source files to exclude. Only the name of the file, excluding the path, should be set. If set, the srcs attribute will be ignored.
        "excluded_srcs": attrs.set(attrs.source(), sorted = True, default = []),
        ## The module to generate mocks for.
        "module": attrs.dep(),
        ## Whether to only generate mocks for Swift protocols.
        "only_protocols": attrs.bool(default = False),
        ## A list of source files to include. Only the name of the file, excluding the path, should be set. By default all source files are included and this doesn't need to be specified.
        "srcs": attrs.set(attrs.source(), sorted = True, default = []),
        "_mockingbird_toolchain": attrs.toolchain_dep(providers = [MockingbirdToolchainInfo], default = "toolchains//:mockingbird"),
    }
    return attribs

# Produce JSON project description for Mockingbird codegen
# https://mockingbirdswift.com/json-project-description
# {
#   "targets": [
#     {
#       "name": "MyLibrary",
#       "type": "library",
#       "path": "/path/to/MyLibrary",
#       "dependencies": [],
#       "sources": [
#         "SourceFileA.swift",
#         "SourceFileB.swift"
#       ]
#     },
#     {
#       "name": "MyOtherLibrary",
#       "type": "library",
#       "path": "/path/to/MyOtherLibrary",
#       "dependencies": [
#         "MyLibrary"
#       ],
#       "sources": [
#         "SourceFileA.swift",
#         "SourceFileB.swift"
#       ]
#     },
#     {
#       "name": "MyLibraryTests",
#       "type": "test",
#       "path": "/path/to/MyLibraryTests",
#       "dependencies": [
#         "MyLibrary"
#       ],
#       "sources": [
#         "SourceFileA.swift",
#         "SourceFileB.swift"
#       ]
#     }
#   ]
# }
def _get_mockingbird_json_project_description(
    info: MockingbirdLibraryInfo, included_srcs: list[str], excluded_srcs: list[str], dep_names: list[str]
) -> (dict, list):
    targets = []
    src_dirs = []
    for record in info.tset.traverse():
        if record.name == info.name:
            targets.append(
                _target_dict_for_mockingbird_record(record = record, included_srcs = included_srcs, excluded_srcs = excluded_srcs, include_non_exported_deps = True)
            )
            src_dirs.append(record.src_dir)
        elif record.name in dep_names:
            targets.append(_target_dict_for_mockingbird_record(record = record, included_srcs = [], excluded_srcs = [], include_non_exported_deps = False))
            src_dirs.append(record.src_dir)
    json = {
        "targets": targets,
    }

    return (json, src_dirs)

def _target_dict_for_mockingbird_record(
    record: MockingbirdLibraryRecord, included_srcs: list[str], excluded_srcs: list[str], include_non_exported_deps: bool
) -> dict:
    srcs = []
    if len(included_srcs) > 0 and len(excluded_srcs) > 0:
        fail("Included srcs and excluded srcs cannot both be set at the same time")

    record_src_names = [src.basename for src in record.srcs]

    for specified_src in included_srcs + excluded_srcs:
        if specified_src not in record_src_names:
            fail("The source file {} does not exist in target {}".format(specified_src, record.name))

    if len(included_srcs) > 0:
        for src_name in record_src_names:
            if src_name in included_srcs:
                srcs.append(src_name)
    elif len(excluded_srcs) > 0:
        for src_name in record_src_names:
            if src_name not in excluded_srcs:
                srcs.append(src_name)
    else:
        srcs = record_src_names

    deps = record.exported_dep_names

    if include_non_exported_deps:
        deps = deps + record.dep_names

    return {
        "dependencies": deps,
        "name": record.name,
        "path": record.src_dir,
        "sources": srcs,
        "type": record.type,
    }
