# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

def _derive_link(artifact):
    if artifact.is_source:
        return artifact.short_path

    # TODO(cjhopman): Reject cross-repo resources. Buck1 does that. It's probably
    # easier for us (compared to v1) to construct a scheme for them that is
    # correct, but not necessary yet.

    return paths.join(artifact.owner.package, artifact.owner.name)

def _generate_script(
    name: str,
    main: Artifact,
    resources: list[Artifact],
    append_script_extension: bool,
    actions: AnalysisActions,
    is_windows: bool,
    copy_resources: bool,
    has_content_based_path: bool,
) -> (Artifact, Artifact):
    main_path = main.short_path
    if not append_script_extension:
        main_link = main_path
    elif is_windows:
        main_link = main_path if main_path.endswith(".bat") or main_path.endswith(".cmd") else main_path + ".bat"
    else:
        main_link = main_path if main_path.endswith(".sh") else main_path + ".sh"
    links = {}
    for link, src in [(_derive_link(src), src) for src in resources] + [(main_link, main)]:
        if links.get(link, src) != src:
            fail("sh_binary `{}`: `{}` and `{}` would both be placed at `{}`".format(name, links[link], src, link))
        links[link] = src
    resources = links

    # windows isn't stable with resources passed in as symbolic links for
    # remote execution. Allow using copies instead.
    if copy_resources:
        resources_dir = actions.copied_dir("resources", resources, has_content_based_path = has_content_based_path)
    else:
        resources_dir = actions.symlinked_dir("resources", resources, has_content_based_path = has_content_based_path)

    script_name = name + (".bat" if is_windows else "")
    script = actions.declare_output(script_name, has_content_based_path = has_content_based_path)

    # This is much, much simpler than the buck1 sh_binary template. A couple reasons:
    # 1. we don't invoke the script through a symlink and so don't need to use and implement a cross-platform `readlink -e`
    # 2. we don't construct an invocation-specific sandbox. The implementation of
    # that in buck1 is pretty crazy and it shouldn't actually be necessary.
    # 3. we don't construct the cell symlinks. those were also strange. They were
    # used for the links in the invocation-specific sandbox (so things would
    # point through the cell symlinks to their original locations). Instead we
    # construct links directly to things (which buck1 actually also did for its
    # BUCK_DEFAULT_RUNTIME_RESOURCES).
    if not is_windows:
        script_content = cmd_args(
            "#!/usr/bin/env bash",
            "set -e",
            # If we access this sh_binary via a unhashed symlink we need to
            # update the relative source.
            '__SRC="${BASH_SOURCE[0]}"',
            '__SRC="$(realpath "$__SRC")"',
            '__SCRIPT_DIR=$(dirname "$__SRC")',
            # The format of the directory tree is different in v1 and v2. We
            # should unify the two, but prior to doing this we should also
            # identify what the right format is. For now, this variable lets
            # callees disambiguate (see D28960177 for more context).
            "export BUCK_SH_BINARY_VERSION_UNSTABLE=2",
            cmd_args('export BUCK_PROJECT_ROOT="$__SCRIPT_DIR/', resources_dir, '"', delimiter = ""),
            # Normalize backslashes to forward slashes for the Windows-host /
            # Linux-target (RE) case where relative_to produces Windows-style separators.
            'export BUCK_PROJECT_ROOT="${BUCK_PROJECT_ROOT//\\\\//}"',
            # In buck1, the paths for resources that are outputs of rules have
            # different paths in BUCK_PROJECT_ROOT and
            # BUCK_DEFAULT_RUNTIME_RESOURCES, but we use the same paths. buck1's
            # BUCK_PROJECT_ROOT paths would use the actual buck-out path rather
            # than something derived from the target and so to use that people
            # would need to hardcode buck-out paths into their scripts. For repo
            # sources, the paths are the same for both.
            'export BUCK_DEFAULT_RUNTIME_RESOURCES="$BUCK_PROJECT_ROOT"',
            'exec "$BUCK_PROJECT_ROOT/{}" "$@"'.format(main_link),
            relative_to = (script, 1),
        )
    else:
        script_content = cmd_args(
            "@echo off",
            "setlocal EnableDelayedExpansion",
            # Fully qualified script path.
            "set __SRC=%~f0",
            # Symbolic links on windows RE systems may not be stable depending on implementation.
            # Don't try to resolve the links when using copies.
            'for /f "tokens=2 delims=[]" %%a in (\'dir %__SRC% ^|%SYSTEMROOT%\\System32\\find.exe "<SYMLINK>"\') do set "__SRC=%%a"'
            if not copy_resources
            else "",
            # Get parent folder.
            'for %%a in ("%__SRC%") do set "__SCRIPT_DIR=%%~dpa"',
            "set BUCK_SH_BINARY_VERSION_UNSTABLE=2",
            cmd_args("set BUCK_PROJECT_ROOT=%__SCRIPT_DIR%\\", resources_dir, delimiter = ""),
            "set BUCK_DEFAULT_RUNTIME_RESOURCES=%BUCK_PROJECT_ROOT%",
            "%BUCK_PROJECT_ROOT%\\{} %*".format(main_link),
            relative_to = (script, 1),
        )
    actions.write(
        script,
        script_content,
        is_executable = True,
    )

    return (script, resources_dir)

# Attrs:
# "deps": attrs.list(attrs.dep(), default = []),
# "main": attrs.source(),
# "resources": attrs.list(attrs.source(), default = []),
# "copy_resources": attrs.bool(default = False),
def sh_binary_impl(ctx):
    # TODO: implement deps (not sure what those even do, though)
    if len(ctx.attrs.deps) > 0:
        fail("sh_binary deps unsupported. Got `{}`".format(repr(ctx.attrs)))

    is_windows = ctx.attrs._target_os_type[OsLookup].os == Os("windows")
    (script, resources_dir) = _generate_script(
        ctx.label.name,
        ctx.attrs.main,
        ctx.attrs.resources,
        ctx.attrs.append_script_extension,
        ctx.actions,
        is_windows,
        ctx.attrs.copy_resources,
        ctx.attrs.has_content_based_path,
    )

    script = script.with_associated_artifacts([resources_dir])

    return [
        DefaultInfo(default_output = script, other_outputs = [resources_dir]),
        RunInfo(
            # TODO(cjhopman): Figure out if we need to specify the link targets
            # as inputs. We shouldn't need to, but need to verify it.
            args = cmd_args(script, hidden = resources_dir),
        ),
    ]

sh_binary = prelude_rule(
    name = "sh_binary",
    impl = sh_binary_impl,
    docs = """
        An `sh_binary()` is used to execute a shell script.
    """,
    examples = """
        This sh\\_binary() just cats a sample data file back at the user.

        ```
        # $REPO/BUCK
        sh_binary(
            name = "script",
            main = "script.sh",
            resources = [
                "data.dat",
            ],
        )
        ```

        ```
        # Sample data file with data we need at runtime
        $ echo "I'm a datafile" > data.dat

        # Create a simple script that prints out the resource
        $ cat > script.sh
        #!/bin/sh
        cat $BUCK_DEFAULT_RUNTIME_RESOURCES/data.dat

        # Make sure the script is executable
        $ chmod u+x script.sh

        # Run the script, and see that it prints out the resource we provided
        $ buck run //:script
        Jobs completed: 4. Time elapsed: 0.2s.
        BUILD SUCCEEDED
        I'm a datafile
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "main": attrs.source(
                doc = """
                Either the path to the script (relative to the build file), or a `build target`.
                 This file must be executable in order to be run.
            """
            ),
            "resources": attrs.list(
                attrs.source(allow_directory = True),
                default = [],
                doc = """
                A list of files or build rules that this rule requires in order to run. These could be things such as
                 random data files.

                 When the script runs, the `$BUCK_DEFAULT_RUNTIME_RESOURCES`
                 environment variable specifies the directory that contains these resources.
                 This directory's location is determined entirely by Buck; the script should
                 not assume the directory's location.

                 The resources are also made available in a tree structure that mirrors
                 their locations in the source and `buck-out` trees. The
                 environment variable `$BUCK_PROJECT_ROOT` specifies a directory
                 that contains all the resources, laid out in their locations relative to
                 the original buck project root.
            """,
            ),
            "append_script_extension": attrs.bool(
                default = True,
                doc = """
                By default, sh_binary ensures that the script has an appropriate extension (e.g. `.sh` or `.bat`),
                  appending one itself if necessary. Setting this to False prevents that behavior and makes the caller
                  responsible for ensuring an existing appropriate extension.
            """,
            ),
            "copy_resources": attrs.bool(
                default = False,
                doc = """
                By default, sh_binary attempts to use symbolic links for the resources. This can be changed so,
                that copies are made instead.
            """,
            ),
            "deps": attrs.list(attrs.dep(), default = []),
            "_target_os_type": buck.target_os_type_arg(),
            "has_content_based_path": attrs.bool(default = True),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
