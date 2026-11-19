# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:genrule.bzl", "genrule_attributes", "genrule_impl")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:genrule_common.bzl", "genrule_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")

cxx_genrule = prelude_rule(
    name = "cxx_genrule",
    impl = genrule_impl,
    docs = """
        A `cxx_genrule()` enables you to run shell commands as part
        of the Buck build process. A `cxx_genrule()` exposes - through
        a set of string parameter macros and variables - information about the
        tools and configuration options used by the
        Buck environment, specifically those related to the C/C++ toolchain.

        The information exposed through these tools and configuration options is a reflection of:
        Buck's built-in settings,
        the settings in `.buckconfig`
        and `.buckconfig.local`,
        and the result of various command-line overrides specified through
        the `common_parameters` command-line option.

        This information is available only
        to the shell commands specified in the `cxx_genrule`.
        The information is not available to other arguments of the rule.

        A `cxx_genrule()` can be an input to
        another `cxx_genrule()`.

        Note that if you specify the `cxx_genrule` as a command-line
        target to `buck build`, you must include a platform flavor.
        For example:

        ```
        buck build :cxx_gr_name#iphonesimulator-x86_64
        ```

        You could also just specify the default platform flavor explicitly:

        ```
        buck build :cxx_gr_name#default
        ```
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        genrule_common.srcs_arg()
        | {
            "cmd": attrs.option(
                attrs.arg(),
                default = None,
                doc = """
                The shell command to run to generate the output file. It is the fallback of `bash`
                 and `cmd_exe`. The shell command can access information
                 about the buck build environment through a set
                 of *macros*, *parameterized macros*, and *variables*.

                 #### Macros

                 The following macros are available to the shell command and are
                 accessed using the following syntax.

                ```
                $(<macro>)
                ```

                 Example:

                ```
                $(cc)
                ```

                `$(cc)`
                Path to the C compiler.

                `$(cxx)`
                Path to the C++ compiler.

                `$(cflags)`
                Flags passed to the C compiler.

                `$(cppflags)`
                Flags passed to the C preprocessor.

                `$(cxxflags)`
                Flags passed to the C++ compiler.

                `$(cxxppflags)`
                Flags to pass to the C++ preprocessor.

                `$(ld)`
                Path to the linker.

                `$(ldflags-pic)`
                Flags passed to the linker for binaries that use
                 position-independent code (PIC).

                `$(ldflags-pic-filter <pattern>)`
                Flags passed to the linker for binaries that use position-independent code (PIC).
                 Use the *pattern* parameter to specify a regular expression that matches the build targets that use these flags.

                `$(ldflags-shared)`
                Flags passed to the linker for shared libraries, such as dynamic-link libraries (DLLs).

                `$(ldflags-shared-filter <pattern>)`
                Flags passed to the linker for shared libraries, such as dynamic-link libraries (DLLs).
                 Use the *pattern* parameter to specify a regular expression that matches the build targets that use these flags.

                `$(ldflags-static)`
                Flags passed to the linker for statically-linked libraries.

                `$(ldflags-static-filter <pattern>)`
                Flags passed to the linker for statically-linked libraries.
                 Use the *pattern* parameter to specify a regular expression that matches the build targets that use these flags.

                #### Parameterized Macros

                 It is also possible to expand references to other rules within the
                 shell command, using the following subset of the
                 builtin `string parameter macros`
                .
                 Note that all build rules expanded in the command are automatically
                 considered to be dependencies of the `genrule()`.

                 Note that the paths returned by these macros are *absolute* paths. You should convert these paths to be relative paths before
                 embedding them in, for example, a shell script or batch file. Using
                 relative paths ensures that your builds are *hermetic*, that
                 is, they are reproducible across different machine environments.

                 Additionally, if you embed these paths in a shell script, you should
                 execute that script using the `sh_binary()` rule and include
                 the targets for these paths in the `resources` argument of
                 that `sh_binary` rule. These are the same targets that you
                 pass to the string parameter macros.

                `$(exe //path/to:target)`
                Expands to the commands necessary to run the executable
                 generated by the specified build rule. For a C++ executable, this
                 will typically just be the name of the output executable itself,
                 such as `main`. If the specified build rule does not generate an
                 executable output, an exception will be thrown and the build will
                 fail.

                `$(location //path/to:target)`
                Expands to the path of the output of the build rule. This
                 means that you can refer to these without needing to be aware of
                 how Buck is storing data on the disk mid-build.

                #### Variables

                 Finally, Buck adds the following variables to the environment in
                 which the shell command runs. They are accessed using the following syntax.
                 Note the use of braces rather than parentheses.

                ```
                ${<variable>}
                ```

                 Example:

                ```
                ${SRCS}
                ```

                `${SRCS}`
                A string expansion of the `srcs` argument delimited by
                 the `environment_expansion_separator` argument where each element
                 of `srcs` will be translated into an absolute path.

                `${SRCDIR}`
                The absolute path to the to which sources are copied
                 prior to running the command.

                `${OUT}`
                The output file for the `genrule()`. The file
                 specified by this variable must always be written by this
                 command. If not, the execution of this rule will be considered a
                 failure, halting the build process.

                `${TMP}`
                A temporary directory which can be used for intermediate results and will not be
                 bundled into the output.
            """,
            ),
        }
        | genrule_common.bash_arg()
        | genrule_common.cmd_exe_arg()
        | genrule_common.type_arg()
        | genrule_common.weight_arg()
        | genrule_common.out_arg()
        | genrule_common.env_arg()
        | genrule_common.environment_expansion_separator()
        | {
            "cacheable": attrs.option(attrs.bool(), default = None),
            "default_outs": attrs.option(attrs.set(attrs.string(), sorted = False), default = None),
            "enable_sandbox": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Whether this target should be executed in a sandbox or not.
            """,
            ),
            "outs": attrs.option(attrs.dict(key = attrs.string(), value = attrs.set(attrs.string(), sorted = False), sorted = False), default = None),
            "remote": attrs.option(attrs.bool(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | buck.optional_has_content_based_path_attr()
        | genrule_attributes()
        | {
            "_cxx_toolchain": toolchains_common.cxx(),
            "_exec_os_type": buck.exec_os_type_arg(),
        }
    ),
)
