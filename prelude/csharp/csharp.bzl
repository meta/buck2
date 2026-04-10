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
load(":csharp_providers.bzl", "DllDepTSet", "DllReference", "DotNetLibraryInfo", "generate_target_tset_children")
load(":toolchain.bzl", "CSharpToolchainInfo")
load(":dotnet_common.bzl", "dotnet_common")

def _csharp_library_or_exe_artifact(ctx: AnalysisContext, library_or_exe_name: str, target_type: str) -> (Artifact, list[DllDepTSet]):
    toolchain = ctx.attrs._csharp_toolchain[CSharpToolchainInfo]

    # Declare that this rule will produce a dll or exe.
    library_or_exe_artifact = ctx.actions.declare_output(library_or_exe_name, has_content_based_path = False)

    # Create a command invoking a wrapper script that calls csc.exe to compile the .dll or the .exe.
    cmd = [toolchain.csc]

    # Add caller specified compiler flags.
    cmd.append(ctx.attrs.compiler_flags)

    # Set the output target as a .NET library.
    cmd.append("/target:" + target_type)
    cmd.append(
        cmd_args(
            library_or_exe_artifact.as_output(),
            format = "/out:{}",
        )
    )

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
    cmd.append(cmd_args(toolchain.framework_dirs[ctx.attrs.framework_ver], format = "/lib:{}"))

    # Add a `/reference:{name}` argument for each dependency.
    # Buck target refs should be absolute paths and system assemblies just the DLL name.
    child_deps = generate_target_tset_children(ctx.attrs.deps, ctx)
    deps_tset = ctx.actions.tset(DllDepTSet, children = child_deps)

    cmd.append(deps_tset.project_as_args("reference"))

    # Specify the C# source code files that should be compiled into this target.
    # NOTE: This must happen after /out and /target!
    cmd.append(ctx.attrs.srcs)

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

    library_or_exe_artifact, _ = _csharp_library_or_exe_artifact(ctx, exe_name, "exe")

    return [
        DefaultInfo(default_output = library_or_exe_artifact),
        RunInfo(args = cmd_args(library_or_exe_artifact)),
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
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/rust/testdata/).

        ```
        csharp_library(
          name = 'simple',
          dll_name = 'Cake.dll',
          framework_ver = 'net46',
          srcs = [
            'Hello.cs',
          ],
          resources = {
            'greeting.txt': '//some:target',
          },
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
        | dotnet_common.srcs_arg()
        | dotnet_common.resources_arg()
        | dotnet_common.framework_ver_arg()
        | dotnet_common.deps_arg()
        | dotnet_common.compiler_flags_arg()
        | dotnet_common.add_hermetic_arguments_arg()
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
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/rust/testdata/).


        ```

        csharp_binary(
          name = 'simple',
          exe_name = 'Cake.exe',
          framework_ver = 'net46',
          srcs = [
            'Hello.cs',
          ],
          resources = {
            'greeting.txt': '//some:target',
          },
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
            "exe_name": attrs.string(default = "", doc = """
                The output name of the dll. This allows you to specify the name of
                 the dll exactly. When this is not set, the dll will be named after
                 the short name of the target.
            """),
        }
        | dotnet_common.srcs_arg()
        | dotnet_common.resources_arg()
        | dotnet_common.framework_ver_arg()
        | dotnet_common.deps_arg()
        | dotnet_common.compiler_flags_arg()
        | dotnet_common.add_hermetic_arguments_arg()
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
        ```
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
