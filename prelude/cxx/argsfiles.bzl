# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

ARGSFILES_SUBTARGET = "argsfiles"

# Information on argsfiles created for compilation.
CompileArgsfile = record(
    # The generated argsfile (does not contain dependent inputs).
    file = field(Artifact),
    # This argsfile as a command form that would use the argsfile (includes dependent inputs).
    cmd_form = field(cmd_args),
    # Command form for the NVCC dry run: the argsfile, the argsfiles it
    # references and the header maps the flags name, but not the header trees
    # or other artifacts behind the flags. Only built for CUDA sources.
    cmd_form_cuda_dryrun = field(cmd_args | None, None),
    # Args as written to the argsfile (with shell quoting applied).
    args = field(cmd_args),
    # Args aggregated for the argsfile excluding file prefix args (excludes shell quoting).
    args_without_file_prefix_args = field(cmd_args),
    # The generated GCC specs file carrying the file-prefix-map flags, when the
    # compiler takes them that way. None for compilers given the flags directly.
    file_prefix_specs = field(Artifact | None, None),
    # The argsfile's arguments by the layer that contributes them, in the order the argsfile
    # concatenates them: `toolchain` (the compiler's own flags), `compiler_type`, `cxx_flags`
    # (the `flags` targets' projection), `deps` (the preprocessor set's `args`, the target's own
    # exported preprocessor first, then its dependencies', and its `modular_args` when the set
    # uses modules), `target` (`_target_compile_flags`, the include directories and the
    # `-include` of a prefix header or an uncompiled precompiled header). The file-prefix
    # arguments (`-ffile-prefix-map`, `-fcoverage-prefix-map`, the symlink-tree header modes'
    # last argsfile) are in no layer on purpose: they map the paths of this compile's own
    # symlink trees, which a compile run from elsewhere never sees. Kept for Xcode argsfiles
    # alone, for the `xcode-compiler-flags` sub-target; None otherwise.
    layers = field(dict[str, cmd_args] | None, None),
)

CompileArgsfiles = record(
    # Relative path argsfiles used for build actions, mapped by extension.
    relative = field(dict[str, CompileArgsfile], default = {}),
    # Argsfiles used for Xcode integration, mapped by extension.
    xcode = field(dict[str, CompileArgsfile], default = {}),
)

def get_argsfiles_output(ctx: AnalysisContext, argsfile_by_ext: dict[str, CompileArgsfile], summary_name: str) -> DefaultInfo:
    argsfiles = []
    dependent_outputs = []
    for _, argsfile in argsfile_by_ext.items():
        argsfiles.append(argsfile.file)

        # To materialize the dependent `Artifact`s of `CompileArgsfile#file`,
        # `CompileArgsfile#cmd_form` is returned in `DefaultInfo#other_outputs`,
        # because it tracks the dependents through the `cmd_args` API.
        dependent_outputs.append(argsfile.cmd_form)

    argsfiles_summary = ctx.actions.write(summary_name, argsfiles, has_content_based_path = False)

    return DefaultInfo(default_outputs = [argsfiles_summary], other_outputs = argsfiles + dependent_outputs)

def get_xcode_compiler_flags_output(ctx: AnalysisContext, argsfile_by_ext: dict[str, CompileArgsfile], output_name: str) -> DefaultInfo:
    """
    The compile flags of every Xcode argsfile as one JSON file, by source extension and by the
    layer that contributes them (`CompileArgsfile.layers`), each layer a list of arguments as
    the argsfile spells them. A project generator reads the layers Buck composed instead of
    recomposing them from the target's attributes. The write depends on the content-based
    artifacts the arguments name (the header maps of the target and of its dependencies, and
    what they name in turn), which Buck builds but does not materialize for it; the output
    itself is one write per library.
    """
    content = {ext: argsfile.layers for ext, argsfile in argsfile_by_ext.items() if argsfile.layers != None}
    output = ctx.actions.write_json(output_name + ".json", content, pretty = True)
    return DefaultInfo(default_output = output)
