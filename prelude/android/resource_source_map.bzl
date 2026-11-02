# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")

def _target_build_file_path(target):
    cell = "" if target.cell in ["", "fbsource"] else target.cell + "/"
    package = target.package + "/" if target.package else ""
    return cell + package + "BUCK"

def create_resource_source_map(ctx: AnalysisContext, resource_infos: list, tool, name: str = "resource_source_map", category = None) -> Artifact:
    resource_infos_with_res = [resource for resource in resource_infos if resource.res != None]
    resource_dirs = [
        cmd_args([resource.res, resource.source_res or resource.res, _target_build_file_path(resource.raw_target)], delimiter = "\t")
        for resource in resource_infos_with_res
    ]
    resource_dirs_file = ctx.actions.write(name + "_resource_dirs", resource_dirs, has_content_based_path = False)
    asset_infos = [(asset, _target_build_file_path(resource.raw_target)) for resource in resource_infos for asset in resource.assets]
    asset_dirs = [cmd_args([asset, owner_build_file], delimiter = "\t") for asset, owner_build_file in asset_infos]
    asset_dirs_file = ctx.actions.write(name + "_asset_dirs", asset_dirs, has_content_based_path = False)
    output = ctx.actions.declare_output(name + ".tsv", has_content_based_path = False)
    command = cmd_args([
        ctx.attrs._java_toolchain[JavaToolchainInfo].java[RunInfo],
        "-jar",
        tool,
        "--resource-dirs",
        resource_dirs_file,
        "--asset-dirs",
        asset_dirs_file,
        "--output",
        output.as_output(),
    ])
    command.add(
        cmd_args(
            hidden = [resource.res for resource in resource_infos_with_res]
            + [resource.source_res for resource in resource_infos_with_res if resource.source_res != None]
            + [asset for asset, _ in asset_infos]
        )
    )
    ctx.actions.run(command, category = category or name, allow_cache_upload = True)
    return output
