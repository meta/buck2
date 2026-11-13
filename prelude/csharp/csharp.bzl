# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//utils:argfile.bzl", "at_argfile")
load(":csharp_providers.bzl", "DllDepTSet", "DllReference", "DotNetLibraryInfo", "generate_target_tset_children")
load(":dotnet_common.bzl", "FrameworkVersion")
load(":toolchain.bzl", "CSharpToolchainInfo")

_CSHARP_LIBRARY_OR_EXE_ATTRIBUTES = {
    "add_hermetic_arguments": attrs.bool(
        default = True,
        doc = """
              If true, the following arguments are passed to the compiler: "/noconfig", "/nostdlib" and "/nosdkpath".
              These attributes prevent loading of default assemblies by the compiler so that they can be explicitly
              controlled in Buck2.
          """,
    ),
    "compiler_flags": attrs.list(
        attrs.arg(),
        default = [],
        doc = """
              The set of additional compiler flags.
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
              The version of the .Net framework that this library targets. This is
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

def _csharp_library_or_exe_artifact(ctx: AnalysisContext, library_or_exe_name: str, target_type: str) -> (Artifact, list[DllDepTSet]):
    toolchain = ctx.attrs._csharp_toolchain[CSharpToolchainInfo]

    # Declare that this rule will produce a dll or exe.
    library_or_exe_artifact = ctx.actions.declare_output(library_or_exe_name, has_content_based_path = False)

    # Create a command invoking a wrapper script that calls csc.exe to compile the .dll or the .exe.
    cmd = [toolchain.csc]

    # Arguments written to a response file to avoid command line too long errors.
    response_file_args = []

    # Add caller specified compiler flags.
    response_file_args.append(ctx.attrs.compiler_flags)

    # Set the output target as a .NET library.
    response_file_args.append("/target:" + target_type)
    response_file_args.append(
        cmd_args(
            library_or_exe_artifact.as_output(),
            format = "/out:{}",
        )
    )

    # NOTE: These are passed directly on the command line since /noconfig is not allowed in a
    # response file.
    if ctx.attrs.add_hermetic_arguments:
        # Don't include any default .NET framework assemblies like "mscorlib" or "System" unless
        # explicitly requested with `/reference:{}`. This flag also stops injection of other
        # default compiler flags.
        cmd.append("/noconfig")

        # Don't reference mscorlib.dll unless asked for. This is required for targets that target
        # embedded platforms such as Silverlight or WASM. (Originally for Buck1 compatibility.)
        cmd.append("/nostdlib")

        # Don't search any paths for .NET libraries unless explicitly referenced with `/lib:{}`.
        cmd.append("/nosdkpath")

    # Let csc know the directory path where it can find system assemblies. This is the path
    # that is searched by `/reference:{libname}` if `libname` is just a DLL name.
    response_file_args.append(cmd_args(toolchain.framework_dirs[ctx.attrs.framework_ver], format = "/lib:{}"))

    # Add a `/reference:{name}` argument for each dependency.
    # Buck target refs should be absolute paths and system assemblies just the DLL name.
    child_deps = generate_target_tset_children(ctx.attrs.deps, ctx)
    deps_tset = ctx.actions.tset(DllDepTSet, children = child_deps)

    response_file_args.append(deps_tset.project_as_args("reference"))

    # Specify the C# source code files that should be compiled into this target.
    # NOTE: This must happen after /out and /target!
    response_file_args.append(ctx.attrs.srcs)

    cmd.append(
        at_argfile(
            actions = ctx.actions,
            name = "csc_args.rsp",
            args = cmd_args(response_file_args, quote = "shell"),
            allow_args = True,
        )
    )

    # Run the C# compiler to produce the output artifact.
    ctx.actions.run(cmd, category = "csharp_compile")

    return library_or_exe_artifact, child_deps

def csharp_library_impl(ctx: AnalysisContext) -> list[Provider]:
    # Automatically set the output dll_name to this target's name if the caller did not specify a
    # custom name.
    dll_name = "{}.dll".format(ctx.attrs.name) if not ctx.attrs.dll_name else ctx.attrs.dll_name

    library_or_exe_artifact, child_deps = _csharp_library_or_exe_artifact(ctx, dll_name, "library")

    return [
        DefaultInfo(default_output = library_or_exe_artifact),
        DotNetLibraryInfo(
            name = library_or_exe_artifact.basename,
            object = library_or_exe_artifact,
            dll_deps = ctx.actions.tset(DllDepTSet, value = DllReference(reference = library_or_exe_artifact), children = child_deps),
        ),
    ]

def csharp_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    exe_name = "{}.exe".format(ctx.attrs.name) if not ctx.attrs.exe_name else ctx.attrs.exe_name

    library_or_exe_artifact, child_deps = _csharp_library_or_exe_artifact(ctx, exe_name, "exe")

    new_runtime_files = []
    for child in child_deps:
        new_runtime_files.extend([ctx.actions.symlink_file(dll_dep.reference.basename, dll_dep.reference) for dll_dep in child.traverse()])

    return [
        DefaultInfo(default_output = library_or_exe_artifact, other_outputs = new_runtime_files),
        RunInfo(args = cmd_args(library_or_exe_artifact, hidden = new_runtime_files)),
    ]

def prebuilt_dotnet_library_impl(ctx: AnalysisContext) -> list[Provider]:
    # Prebuilt libraries are just passed through since they are already built.
    return [
        DefaultInfo(default_output = ctx.attrs.assembly),
        DotNetLibraryInfo(
            name = ctx.attrs.name,
            object = ctx.attrs.assembly,
            dll_deps = ctx.actions.tset(DllDepTSet, value = DllReference(reference = ctx.attrs.assembly)),
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
            'System.dll',
          ],
        )

        prebuilt_dotnet_library(
          name = 'other',
          assembly = 'other-1.0.dll',
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
        A csharp\\_binary() rule builds a .Net library from the supplied set of C# source files
         and dependencies by invoking csc.
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
            'System.dll',
          ],
        )

        prebuilt_dotnet_library(
          name = 'other',
          assembly = 'other-1.0.dll',
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
