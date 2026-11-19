# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:arglike.bzl", "ArgLike")

# Create an argument file.
# Return `cmd_args` which is single string containing `@path/to/argfile`.
# Returned `cmd_args` contains given files as hidden artifacts.
def at_argfile(
    *,
    # ctx.actions
    actions,
    # name of the argument file
    name: str | Artifact,
    # the arguments to write to the argument file
    args,
    # pass to `ctx.actions.write`
    allow_args: bool = False,
    has_content_based_path: bool = False,
) -> cmd_args:
    if allow_args:
        args_file, _ = actions.write(name, args, allow_args = True, with_inputs = True, has_content_based_path = has_content_based_path)
    else:
        args_file = actions.write(name, args, with_inputs = True, has_content_based_path = has_content_based_path)
    return cmd_args(args_file, format = "@{}", hidden = args)

# An argsfile that renders as its path and also carries the artifacts
# referenced by `args`, the flags written into it.
#
# TODO(jtbraun): if "associated artifacts" ever become inheritable/transitive,
# attaching `args` to `argsfile` would be a better solution.
def argsfile_with_artifacts(argsfile: Artifact, args: ArgLike) -> cmd_args:
    return cmd_args(argsfile, hidden = args)

# Write arguments to a file and return an `argsfile_with_artifacts()` cmd_args
# value pairing the file with everything its contents reference.
def mk_argsfile(
    *,
    actions: AnalysisActions,
    name: str | Artifact,
    args: ArgLike,
    has_content_based_path: bool = False,
) -> cmd_args:
    # Write actions may spill additional file artifacts out to the side for
    # certain string macros, which are returned in `macro_files`. These have to be
    # carried forward to consumers, just as `args` is.
    argsfile, macro_files = actions.write(name, args, allow_args = True, has_content_based_path = has_content_based_path)
    return argsfile_with_artifacts(argsfile, [args] + macro_files)

# Write arguments to a file, and return the file path as `cmd_args`
# with args attached as hidden artifacts.
def argfile(
    *,
    # ctx.actions
    actions,
    # name of the argument file
    name: str | Artifact,
    # the arguments to write to the argument file
    args,
    # pass to `ctx.actions.write`
    allow_args: bool = False,
    has_content_based_path: bool = False,
) -> cmd_args:
    if allow_args:
        args_file, _ = actions.write(name, args, allow_args = True, with_inputs = True, has_content_based_path = has_content_based_path)
    else:
        args_file = actions.write(name, args, with_inputs = True, has_content_based_path = has_content_based_path)
    return cmd_args(args_file, hidden = args)
