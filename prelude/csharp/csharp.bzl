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
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(":csharp_providers.bzl", "DllDepTSet", "DllReference", "DotNetLibraryInfo", "generate_target_tset_children")
load(":toolchain.bzl", "CSharpToolchainInfo")

FrameworkVersion = ["net35", "net40", "net45", "net46"]

_CSHARP_LIBRARY_OR_EXE_ATTRIBUTES = {
    "compiler_flags": attrs.list(
        attrs.arg(),
        default = [],
        doc = """
              The set of additional compiler flags, in which macros such as `$(location //some:target)`
              are expanded. A `/target` among them has no effect: the rule passes its own after them.
          """,
    ),
    "deps": attrs.list(
        attrs.one_of(attrs.dep(providers = [DotNetLibraryInfo]), attrs.string()),
        default = [],
        doc = """
              The set of targets or system-provided assemblies this target depends on. Any
              values that are targets must be either csharp\\_library or `prebuilt_dotnet_library`
              instances.
          """,
    ),
    "framework_ver": attrs.enum(
        FrameworkVersion,
        doc = """
              The version of the .Net framework that the assembly targets. This is
              one of """
        + ", ".join(FrameworkVersion)
        + """.
          """,
    ),
    "srcs": attrs.list(
        attrs.source(),
        default = [],
        doc = """
              The set of C# source files to be compiled, and assembled by this rule.
              Each element may be either a literal string (representing the path within this package), or a target.
          """,
    ),
}

# One csc invocation: the assembly it produces, the dependencies it was compiled
# against (as the children handed to it and as one set), and its response file.
_CscCompilation = record(
    output = Artifact,
    dep_children = list[DllDepTSet],
    deps = DllDepTSet,
    argsfile = Artifact,
)

# One argument per line of a response file. csc splits a line at whitespace
# outside double quotes and takes a leading `#` as the start of a comment, so
# an argument with either is quoted. The backslashes before the closing quote
# are doubled, as a single one would escape it; csc keeps both, which is the
# same path. The arguments are paths and file names, which cannot contain a
# double quote.
def _csc_response_file_args(args) -> cmd_args:
    return cmd_args(
        args,
        replace_regex = [("^((?:#|.*\\s).*?)(\\\\*)$", '"${1}${2}${2}"')],
    )

def _compile(ctx: AnalysisContext, output_name: str, target_type: str) -> _CscCompilation:
    toolchain = ctx.attrs._csharp_toolchain[CSharpToolchainInfo]

    # Declare that this rule will produce a dll or exe.
    output = ctx.actions.declare_output(output_name, has_content_based_path = False)

    # Create a command invoking a wrapper script that calls csc.exe to compile the .dll or the .exe.
    cmd = [toolchain.csc]

    # Add caller specified compiler flags.
    cmd.append(ctx.attrs.compiler_flags)

    # Set the kind of assembly to produce.
    cmd.append("/target:" + target_type)
    cmd.append(
        cmd_args(
            output.as_output(),
            format = "/out:{}",
        )
    )

    # Don't include any default .NET framework assemblies like "mscorlib" or "System" unless
    # explicitly requested with `/reference:{}`. This flag also stops injection of other
    # default compiler flags.
    cmd.append("/noconfig")

    # Don't reference mscorlib.dll unless asked for. This is required for targets that target
    # embedded platforms such as Silverlight or WASM. (Originally for Buck1 compatibility.)
    cmd.append("/nostdlib")

    # Don't search any paths for .NET libraries unless explicitly referenced with `/lib:{}`.
    cmd.append("/nosdkpath")

    # The arguments below can be many, so they are written to a response file to avoid command
    # line too long errors.
    response_file_args = []

    # Let csc know the directory path where it can find system assemblies. This is the path
    # that is searched by `/reference:{libname}` if `libname` is just a DLL name.
    response_file_args.append(cmd_args(toolchain.framework_dirs[ctx.attrs.framework_ver], format = "/lib:{}"))

    # Add a `/reference:{name}` argument for each dependency.
    # Buck target refs should be absolute paths and system assemblies just the DLL name.
    dep_children = generate_target_tset_children(ctx.attrs.deps, ctx)
    deps = ctx.actions.tset(DllDepTSet, children = dep_children)

    response_file_args.append(deps.project_as_args("reference"))

    # Specify the C# source code files that should be compiled into this target.
    # NOTE: This must happen after /out and /target!
    response_file_args.append(ctx.attrs.srcs)

    argsfile = ctx.actions.write(
        "csc_args.rsp",
        _csc_response_file_args(response_file_args),
        with_inputs = True,
        has_content_based_path = False,
    )
    cmd.append(cmd_args(argsfile, format = "@{}"))

    # Run the C# compiler to produce the output artifact.
    ctx.actions.run(cmd, category = "csharp_compile")

    return _CscCompilation(
        output = output,
        dep_children = dep_children,
        deps = deps,
        argsfile = argsfile,
    )

def _sub_targets(compilation: _CscCompilation) -> dict[str, list[Provider]]:
    return {
        "argsfile": [DefaultInfo(default_output = compilation.argsfile)],
    }

def csharp_library_impl(ctx: AnalysisContext) -> list[Provider]:
    # Automatically set the output dll_name to this target's name if the caller did not specify a
    # custom name.
    dll_name = "{}.dll".format(ctx.attrs.name) if not ctx.attrs.dll_name else ctx.attrs.dll_name

    compilation = _compile(ctx, dll_name, "library")

    return [
        DefaultInfo(default_output = compilation.output, sub_targets = _sub_targets(compilation)),
        DotNetLibraryInfo(
            name = compilation.output.basename,
            object = compilation.output,
            dll_deps = ctx.actions.tset(
                DllDepTSet,
                value = DllReference(reference = compilation.output, label = ctx.label),
                children = compilation.dep_children,
            ),
        ),
    ]

# What the executable knows an assembly by: its file name without the extension.
# Windows compares names by their upper case.
def _assembly_name(file_name: str) -> str:
    return paths.split_extension(paths.basename(file_name))[0].upper()

def csharp_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    exe_name = "{}.exe".format(ctx.attrs.name) if not ctx.attrs.exe_name else ctx.attrs.exe_name

    compilation = _compile(ctx, paths.join("obj", paths.basename(exe_name)), "exe")

    # The executable looks for the assembly of a dependency in its own directory,
    # so it is copied to one directory with the assembly of every dependency that
    # is a target, where no two files can be the same assembly to it. System
    # assemblies come from the framework.
    exe_dir = paths.dirname(exe_name)
    files = {exe_name: compilation.output}
    references_by_assembly = {
        _assembly_name(exe_name): DllReference(reference = compilation.output, label = ctx.label),
    }
    for dep in compilation.deps.traverse():
        dll = dep.reference
        if not isinstance(dll, Artifact):
            continue
        assembly = _assembly_name(dll.basename)
        other = references_by_assembly.get(assembly)
        if other != None:
            if other.reference == dll:
                continue
            fail(
                "{}: `{}` produces `{}` and `{}` produces `{}`, which are the same assembly to the executable: it finds an assembly by its file name without the extension, ignoring case".format(
                    ctx.label,
                    other.label,
                    other.reference.basename,
                    dep.label,
                    dll.basename,
                )
            )
        references_by_assembly[assembly] = dep
        files[paths.join(exe_dir, dll.basename)] = dll

    bin_dir = ctx.actions.copied_dir("bin", files, has_content_based_path = False)
    exe = bin_dir.project(exe_name, hide_prefix = True).with_associated_artifacts([bin_dir])

    return [
        DefaultInfo(default_output = exe, other_outputs = [bin_dir], sub_targets = _sub_targets(compilation)),
        RunInfo(args = [exe]),
    ]

def prebuilt_dotnet_library_impl(ctx: AnalysisContext) -> list[Provider]:
    # Prebuilt libraries are just passed through since they are already built.
    return [
        DefaultInfo(default_output = ctx.attrs.assembly),
        DotNetLibraryInfo(
            name = ctx.attrs.name,
            object = ctx.attrs.assembly,
            dll_deps = ctx.actions.tset(DllDepTSet, value = DllReference(reference = ctx.attrs.assembly, label = ctx.label)),
        ),
    ]

csharp_library = prelude_rule(
    name = "csharp_library",
    impl = csharp_library_impl,
    docs = """
        A csharp\\_library() rule builds a .Net library from the supplied set of C# source files
         and dependencies by invoking csc.
    """,
    examples = """
        ```python
        csharp_library(
          name = 'simple',
          dll_name = 'Cake.dll',
          framework_ver = 'net46',
          srcs = [
            'Hello.cs',
          ],
          deps=[
            ':other',
            'mscorlib.dll',
            'System.dll',
          ],
        )

        prebuilt_dotnet_library(
          name = 'other',
          assembly = 'other.dll',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "dll_name": attrs.string(
                default = "",
                doc = """
                The output name of the dll. This allows you to specify the name of
                 the dll exactly. When this is not set, the dll will be named after
                 the short name of the target.
            """,
            ),
        }
        | _CSHARP_LIBRARY_OR_EXE_ATTRIBUTES
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "_csharp_toolchain": toolchains_common.csharp(),
        }
    ),
)

csharp_binary = prelude_rule(
    name = "csharp_binary",
    impl = csharp_binary_impl,
    docs = """
        A csharp\\_binary() rule builds a .Net executable from the supplied set of C# source files
         and dependencies by invoking csc. The executable and the assemblies of the targets it
         depends on are copied to one directory, where the executable finds each of them in a
         `.dll` or `.exe` file named after the assembly: the `assembly` of a
         `prebuilt_dotnet_library` has to be named that way. A `prebuilt_dotnet_library` cannot
         name the assemblies it needs, so the executable has to depend on those too.
    """,
    examples = """
        ```python
        csharp_binary(
          name = 'simple',
          exe_name = 'Cake.exe',
          framework_ver = 'net46',
          srcs = [
            'Hello.cs',
          ],
          deps=[
            ':other',
            'mscorlib.dll',
            'System.dll',
          ],
        )

        prebuilt_dotnet_library(
          name = 'other',
          assembly = 'other.dll',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "exe_name": attrs.string(
                default = "",
                doc = """
                The output name of the executable. This allows you to specify the name of
                 the executable exactly. When this is not set, the executable will be named after
                 the short name of the target, with the extension `.exe`.
            """,
            ),
        }
        | _CSHARP_LIBRARY_OR_EXE_ATTRIBUTES
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "_csharp_toolchain": toolchains_common.csharp(),
        }
    ),
)

prebuilt_dotnet_library = prelude_rule(
    name = "prebuilt_dotnet_library",
    impl = prebuilt_dotnet_library_impl,
    docs = """
        A `prebuilt_dotnet_library()` rule is used to include
        prebuilt .Net assembles into your .Net code.
    """,
    examples = """
        ```python
        prebuilt_dotnet_library(
          name = 'log4net',
          assembly = 'log4net.dll',
        )

        csharp_library(
          name = 'example',
          srcs = [
            'Hello.cs',
          ],
          framework_ver = 'net46',
          deps = [
            ':log4net',
            'mscorlib.dll',
            'System.dll',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "assembly": attrs.source(
                doc = """
                The path to the DLL that this rule provides.
            """
            ),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
