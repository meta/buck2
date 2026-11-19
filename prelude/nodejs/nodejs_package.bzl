# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

def get_bundled_node(label: Label, node_dist: Artifact | None, target_os_type: Dependency) -> (str, Artifact):
    """The name (`node`, or `node.exe` on Windows) and artifact of the Node binary a `standalone` package stages at its root."""
    if "/" in label.name:
        fail("{}: a `standalone` package's target name names the zip's top-level directory, so it cannot contain `/`".format(label))
    if node_dist == None:
        fail("{}: a `standalone` package requires `node_dist` on the nodejs toolchain".format(label))
    is_windows = target_os_type[OsLookup].os == Os("windows")
    return ("node.exe" if is_windows else "node", node_dist)

def package_standalone(
    actions: AnalysisActions,
    create_zip,
    staged: Artifact,
    identifier: str,
) -> Artifact:
    """Zip the directory `staged` under a top-level directory named after its basename.

    File modes are kept, so the bundled Node binary stays executable.
    """
    entries_file = actions.write(
        "__pkg_entries__/" + identifier,
        # `artifact, short_path, is_source`: a non-source entry is zipped under its basename, so the short path is unused.
        cmd_args([staged, staged.basename, "false"]),
        has_content_based_path = True,
    )

    output = actions.declare_output(identifier + ".zip", has_content_based_path = True)

    cmd = cmd_args([
        create_zip,
        "--output_path",
        output.as_output(),
        "--on_duplicate_entry",
        "fail",
        "--entries_file",
        entries_file,
        cmd_args(hidden = staged),
    ])
    actions.run(cmd, category = "package_standalone", identifier = identifier)

    return output
