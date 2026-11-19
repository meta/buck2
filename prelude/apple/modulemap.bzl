# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")
load(
    "@prelude//apple/swift:swift_incremental_support.bzl",
    "get_uses_content_based_paths",
)
load("@prelude//apple/swift:swift_toolchain.bzl", "get_swift_toolchain_info")
load("@prelude//apple/swift:swift_toolchain_types.bzl", "SwiftToolchainInfo")  # @unused Used as a type
load(
    "@prelude//cxx:headers.bzl",
    "CHeader",  # @unused Used as a type
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
)

def create_modulemap(
    ctx: AnalysisContext,
    name: str,
    module_name: str,
    headers: list[CHeader],
    swift_header: Artifact | None = None,
    is_framework: bool = False,
    swift_toolchain: SwiftToolchainInfo | None = None,
    exec_group: ExecGroup | None = None,
) -> CPreprocessor:
    # We don't want to name this module.modulemap to avoid implicit importing
    if name == "module" and not is_framework:
        fail("Don't use the name `module` for modulemaps, this will allow for implicit importing.")

    swift_toolchain = swift_toolchain or get_swift_toolchain_info(ctx)
    uses_content_based_paths = get_uses_content_based_paths(ctx, swift_toolchain)
    use_relative_paths = swift_toolchain.supports_modulemaps_with_hmaps

    # Create a map of header import path to artifact location
    header_map = {}
    for h in headers:
        if h.namespace:
            header_map["{}/{}".format(h.namespace, h.name)] = h.artifact
        else:
            header_map[h.name] = h.artifact

    swift_header_name = None
    if swift_header:
        # We need to include the Swift header in the symlink tree too
        swift_header_name = "{}/{}-Swift.h".format(module_name, module_name)
        header_map[swift_header_name] = swift_header

    if use_relative_paths:
        symlink_tree = None
    else:
        # Create a symlink dir for the headers to import
        symlink_tree_name = name.replace(".", "_") + "_symlink_tree"
        symlink_tree = ctx.actions.symlinked_dir(symlink_tree_name, header_map, has_content_based_path = uses_content_based_paths)

    output = ctx.actions.declare_output(name + ".modulemap", has_content_based_path = uses_content_based_paths)
    cmd = cmd_args(ctx.attrs._apple_tools[AppleToolsInfo].make_modulemap)
    cmd.add(
        "--output",
        output.as_output(),
        "--name",
        module_name,
    )

    if swift_header:
        cmd.add(
            "--swift-header",
            swift_header if use_relative_paths else cmd_args(symlink_tree, format = "{}/" + swift_header_name),
        )

    if getattr(ctx.attrs, "use_submodules", False):
        cmd.add("--use-submodules")

    if is_framework:
        cmd.add("--framework")

    for include_path in sorted(header_map.keys()):
        # Don't include the Swift header in the mappings, this is handled separately.
        if include_path == swift_header_name:
            continue

        if is_framework:
            # Framework headers are the same as their include paths, they are
            # compiled relative to the framework's Headers folder.
            cmd.add(include_path)
            cmd.add(include_path)
        elif use_relative_paths:
            # The path to the header artifact is relative to the containing
            # folder of the modulemap. We can't get the paths in the prelude,
            # the relativizing is done in the generation tool.
            cmd.add(include_path)
            cmd.add(header_map[include_path])
        else:
            # With symlink trees the include path should match the symlink name
            # nested in the symlink tree root.
            cmd.add(include_path)
            cmd.add(cmd_args(symlink_tree, format = "{}/" + include_path))

    ctx.actions.run(cmd, category = "modulemap", identifier = name, exec_group = exec_group)

    header_artifacts = header_map.values()
    modular_args = [cmd_args(output, format = "-fmodule-map-file={}", hidden = header_artifacts)]
    if not use_relative_paths and not is_framework:
        # When using relative paths we rely on the hmaps provided by regular
        # cxx preprocessor.
        modular_args.append(cmd_args(symlink_tree, format = "-I{}"))
        header_artifacts.append(symlink_tree)

    return CPreprocessor(
        modular_args = modular_args,
        modulemap_artifact = output.with_associated_artifacts(header_artifacts),
    )
