# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//cxx:argsfiles.bzl",
    "CompileArgsfile",  # @unused Used as a type
)
load(
    "@prelude//cxx:cxx_sources.bzl",
    "CxxSrcWithFlags",  # @unused Used as a type
)
load("@prelude//ide_integrations/xcode:data.bzl", "XcodeDataInfoKeys")

def cxx_populate_xcode_attributes(ctx, srcs: list[CxxSrcWithFlags], argsfiles: dict[str, CompileArgsfile], product_name: str) -> dict[str, typing.Any]:
    converted_srcs = {}
    for src in srcs:
        file_properties = _get_artifact_owner(src.file)
        if src.flags:
            # List of resolved_macros will encode as:
            # [['\"-some-flag\"'], ['\"-another-flag\"']]
            #
            # Convert it to a string and rip-out the quotes
            # so it appears as ["-some-flag", "-another-flag"]
            file_properties["flags"] = [str(flag).replace('"', "") for flag in src.flags]
        converted_srcs[src.file] = file_properties

    data = {
        XcodeDataInfoKeys.ARGSFILES_BY_EXT: {ext: argsfile.file for ext, argsfile in argsfiles.items()},
        XcodeDataInfoKeys.HEADERS: _get_artifacts_with_owners(ctx.attrs.headers),
        XcodeDataInfoKeys.PRODUCT_NAME: product_name,
        XcodeDataInfoKeys.SRCS: converted_srcs,
    }

    if hasattr(ctx.attrs, "exported_headers"):
        data[XcodeDataInfoKeys.EXPORTED_HEADERS] = _get_artifacts_with_owners(ctx.attrs.exported_headers)

    return data

def _get_artifacts_with_owners(files: typing.Any) -> dict[Artifact, dict[str, Label]]:
    if type(files) == "dict":
        return {artifact: _get_artifact_owner(artifact) for _, artifact in files.items()}
    else:
        return {file: _get_artifact_owner(file) for file in files}

def _get_artifact_owner(file: Artifact) -> dict[str, Label]:
    if file.owner:
        return {"target": file.owner}
    else:
        return {}
