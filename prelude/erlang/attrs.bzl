# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load(":erlang_info.bzl", "ErlangAppIncludeInfo", "ErlangAppInfo", "ErlangAppOrTestInfo", "StartTypeValues")

common_attributes = (
    buck.labels_arg()
    | buck.contacts_arg()
    | {
        "os_env": attrs.option(
            attrs.dict(key = attrs.string(), value = attrs.string()),
            default = None,
            doc = """
                This attribute allows to set additional values for the operating system environment for invocations to the
                Erlang toolchain.
            """,
        ),
    }
)

common_shell_attributes = {
    "shell_configs": attrs.set(
        attrs.dep(),
        default = read_root_config("erlang", "shell_configs", "").split(),
        doc = """
            This attribute allows to set config files for the shell. The dependencies that are typically used
            here are `export_file` targets.
        """,
    ),
    "shell_libs": attrs.set(
        attrs.dep(providers = [ErlangAppInfo]),
        default = ["prelude//erlang/shell:buck2_shell_utils"],
        doc = """
            This attribute allows to define additional dependencies for the shell. By default this is
            set to `["prelude//erlang/shell:buck2_shell_utils"]` which includes a `user_default` module
            that loads and compiles modules with buck2 mechanisms.
        """,
    ),
}

common_application_attributes = {
    "applications": attrs.list(
        attrs.dep(providers = [ErlangAppInfo]),
        default = [],
        doc = """
        Equivalent to the corresponding `applications` and `included_applications`
        fields you will find in `*.app.src` or `*.app` files and specify the application dependencies. Contrary to the
        fields in the `*.app.src` or `*.app` files, **it is necessary to use target paths to the application** where a
        dependency is desired. These fields will be used to construct equally named fields in the generated `*.app` file
        for the application.

        OTP applications are specified with the target path `prelude//erlang/applications:<application>`.

        **NOTE**: _If you use the `app_src` field and the references application resource file template specifies
        `applications` or `included_applications` buck2 checks that the target definitions and information in the template are
        equivalent to prevent these definitions from drifting apart during migration._
    """,
    ),
    "included_applications": attrs.list(
        attrs.dep(providers = [ErlangAppInfo]),
        default = [],
        doc = """
        Check the documentation for `applications`.
    """,
    ),
    "version": attrs.string(
        default = "1.0.0",
        doc = """
        The `version` field specifies the applications version that is materialized as `vsn` field in the generated `*.app`
        file. If you use the the `app_src` field and specify a version in the referenced template in addition to the version
        field, the versions need to be identical.

        If no version is specified in either the `app_src` template or the `version` field, a fallback version string of
        `"1.0.0"` is used.
    """,
    ),
    "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
} | common_shell_attributes

rules_attributes = {
    "erlang_app": {
        "app_name": attrs.option(
            attrs.string(),
            default = None,
            doc = """
                This attribute allows the user to overwrite the Erlang application name, which otherwise defaults to the target name.
            """,
        ),
        "app_src": attrs.option(
            attrs.source(),
            default = None,
            doc = """
                The `app_src` field allows to optionally reference a `*.app.src` template file. This template file will then be used by
                buck2 to generate the `*.app` output file in the applications `ebin/` directory. This is useful during the migration from
                rebar3 to buck2 to avoid duplicated entries, of e.g. the `version`.

                Buck2 will use or check all fields present in the template, and fill out the fields with the information provided in the
                target, e.g. if the `version` is specified in both, buck2 will check that they are identical. Otherwise, it uses the
                information from the template if the target doesn't specify it, and vice versa.

                **NOTE**: _If you use the `app_src` field and the references application resource file template specifies `applications`
                or `included_applications` buck2 checks that the target definitions and information in the template are equivalent to
                prevent these definitions from drifting apart during migration._
            """,
        ),
        "app_src_vsn": attrs.string(
            default = "git",
            doc = """
                The version placeholder used in the `app_src` and `appup_src` templates. When the `vsn` field of `app_src` (or the
                version element of `appup_src`) matches this placeholder, either as an atom or as a string, buck2 substitutes the
                value from the `version` field. Defaults to `"git"`, matching rebar3's convention.
            """,
        ),
        "appup_src": attrs.option(
            attrs.source(),
            default = None,
            doc = """
                The `appup_src` field allows to optionally reference a `*.appup.src` template file used to generate the application's
                `*.appup` file in the `ebin/` directory. The template's first element is the application version, which buck2 substitutes
                using the same placeholder convention as `app_src_vsn`.
            """,
        ),
        "docs": attrs.list(
            attrs.source(),
            default = [],
            doc = """
                Documentation files referenced by `-moduledoc {file, "..."}` or `-doc {file, "..."}` annotations
                in source files. These files are made available to `erlc` at compile time, preserving their path
                relative to the application root.
            """,
        ),
        "env": attrs.option(
            attrs.dict(key = attrs.string(), value = attrs.string()),
            default = None,
            doc = """
                The `env` field allows to set the application env variables. The key value pairs will materialise in the application's `.app`
                file and can then be accessed by [`application:get_env/2`](https://www.erlang.org/doc/man/application.html#get_env-2).
           """,
        ),
        "erl_opts": attrs.option(
            attrs.list(attrs.string()),
            default = None,
            doc = """
                Typically compile options are managed by global config files, however, sometimes it is
                desirable to overwrite the pre-defined compile options. The `erl_opts` field allows developers to do so for individual
                applications.
            """,
        ),
        "extra_includes": attrs.list(
            attrs.dep(providers = [ErlangAppIncludeInfo]),
            default = [],
            doc = """
                In some cases we might have the situation, where an application `app_a` depends through the `applications` and
                `included_applications` fields on application `app_b` and a source file in `app_b` includes a header file from `app_a`
                (e.g. `-include_lib("app_a/include/header.hrl`). This technically creates circular dependency from `app_a` to `app_b`
                (e.g. via `applications` field) and back from `app_b` to `app_a` (via `-include_lib`). To break the dependency
                developers can specify targets in the `extra_includes` field, whose public include files are accessible to the
                application target during build time.

                Only the includes of the specified application are available and eventual transitive dependencies need to be managed
                manually.

                **NOTE**: _It is not possible (or even desired) to add OTP applications with this field._

                **NOTE**: _This mechanism is added to circumvent unclean dependency relationships and the goal for
                developers should be to reduce usages of this field._ **DO NOT ADD ANY MORE USAGES!!**
            """,
        ),
        "extra_properties": attrs.option(
            attrs.dict(key = attrs.string(), value = attrs.one_of(attrs.string(), attrs.list(attrs.string()))),
            default = None,
            doc = """
                The extra_properties field can be used to specify extra key-value pairs which is are not defined in
                [application_opt()](https://www.erlang.org/doc/man/application.html#load-2). The key-value pair will be stored in the
                applications `.app` file and can be accessed by `file:consult/1`.
            """,
        ),
        "include_src": attrs.bool(
            default = True,
            doc = """
                This field controls if the generated application directory contains a src/ directory with the Erlang code or not.
            """,
        ),
        "includes": attrs.list(
            attrs.source(),
            default = [],
            doc = """
                The public header files accessible via `-include_lib("appname/include/header.hrl")` from other erlang files.
            """,
        ),
        "mod": attrs.option(
            attrs.tuple(attrs.string(), attrs.string()),
            default = None,
            doc = """
                The `mod` field specifies the equivalent field in the generated `*.app` files. The format is similar, with the
                difference, that the module name, and the individual start arguments need to be given as the string representation
                of the corresponding Erlang terms.
             """,
        ),
        "peek_private_includes": attrs.bool(
            default = False,
            doc = """
                This attribute allows you to use the private includes of the application's dependencies. This can be useful for
                test applications, to create shared abstractions for tests. It's not advisable to use this attribute for prodution
                code. All private includes transitively must be non-ambiguous.
            """,
        ),
        "resources": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
                The `resources` field specifies targets whose default output are placed in the applications `priv/` directory. For
                regular files this field is typically combined with `export_file`, `filegroup`, or similar targets. However, it
                is general, and any target can be used, e.g. if you want to place a built escript in the `priv/` directory, you can use
                an `erlang_escript` target.
            """,
        ),
        "srcs": attrs.list(
            attrs.source(),
            default = [],
            doc = """
                A list of `*.erl`, `*.hrl`, `*.xrl`, or `*.yrl` source inputs that are typically located
                in an application's `src/` folder. Header files (i.e. `*.hrl` files) specified in this field are considered application
                private headers, and can only be accessed by the `*.erl` files of the application itself. `*.xrl` and `*.yrl` files are
                processed into `*.erl` files before all `*.erl` files are compiled into `*.beam` files.
            """,
        ),
        "use_global_parse_transforms": attrs.bool(
            default = True,
            doc = """
                This field indicates if global parse_tranforms should be applied to this application as well. It often makes sense
                for third-party dependencies to not be subjected to global parse_transforms, similar to OTP applications.
            """,
        ),
        "xrl_includefile": attrs.option(
            attrs.source(),
            default = None,
            doc = """
                Customised prologue file to replace the default. See [`includefile` option](https://www.erlang.org/doc/apps/parsetools/leex.html#file/2) for details.
        """,
        ),
        "yrl_includefile": attrs.option(
            attrs.source(),
            default = None,
            doc = """
                Customised prologue file to replace the default. See [`includefile` option](https://www.erlang.org/doc/apps/parsetools/yecc.html#file/2) for details.
        """,
        ),
        "_includes_target": attrs.option(
            attrs.dep(providers = [ErlangAppIncludeInfo]),
            default = None,
            doc = """
                Internal, used by the `erlang_application` macro to link the proper application target and include_only targets.
            """,
        ),
    }
    | common_application_attributes,
    "erlang_app_includes": {
        "app_name": attrs.string(),
        "includes": attrs.list(attrs.source(), default = []),
        "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
    },
    "erlang_escript": {
        "configs": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
            This attribute allows to set config files for the escript. The dependencies that are typically used
            here are `export_file` targets.""",
        ),
        "deps": attrs.list(
            attrs.dep(providers = [ErlangAppInfo]),
            doc = """
                List of Erlang applications that are bundled in the escript. This includes all transitive dependencies as well.
            """,
        ),
        "emu_args": attrs.list(
            attrs.string(),
            default = [],
            doc = """
                This field specifies the emulator flags that the escript uses on execution. It is often desirable to specify the number
                of threads and schedulers the escript uses. Please refer to the
                [OTP documentation](https://www.erlang.org/doc/man/erl.html#emu_flags) for details.
            """,
        ),
        "include_priv": attrs.bool(
            default = False,
            doc = """
                Setting this flag, will package the applications `priv` directory in the escript. Similar to files added through the
                `resources` field, the `priv` folders files can then be accessed by `escript"extract/2`.
            """,
        ),
        "main_module": attrs.option(
            attrs.string(),
            default = None,
            doc = """
                Overrides the default main module. Instead of deferring the main module from the scripts filename, the specified module
                is used. That module needs to export a `main/1` function that is called as entry point.
            """,
        ),
        "resources": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
                This adds the targets default output to the escript archive. To access these files, you need to use `escript:extract/2`,
                which will extract the entire escript in memory. The relevant files can then be accessed through the `archive`
                section.

                Please refer to the [`escript:extract/2`](https://www.erlang.org/doc/man/escript.html) for more details.
            """,
        ),
        "script_name": attrs.option(
            attrs.string(),
            default = None,
            doc = """
                Overrides the filename of the produced escript.
            """,
        ),
        "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
    },
    "erlang_otp_binaries": {
        "erl": attrs.arg(
            doc = """
                Reference to `erl` binary
            """
        ),
        "erlc": attrs.arg(
            doc = """
                Reference to `erlc` binary
            """
        ),
        "escript": attrs.arg(
            doc = """
                Reference to `escript` binary
            """
        ),
    },
    "erlang_release": {
        "applications": attrs.list(
            attrs.one_of(attrs.dep(providers = [ErlangAppInfo]), attrs.tuple(attrs.dep(providers = [ErlangAppInfo]), attrs.enum(StartTypeValues))),
            doc = """
                This field specifies the list of applications that the release should start in the given order, and optionally the start
                type. Top-level applications without given start type are started with type
                [`permanent`](https://www.erlang.org/doc/man/application.html#type-restart_type).
            """,
        ),
        "bootscript_builders": attrs.dict(
            key = attrs.string(),
            value = attrs.dep(),
            default = {},
            doc = """
                The mapping listed here maps from boot script name (e.g. "start.boot") to a binary (e.g. "$(location ...)") generating the boot script.
                The binary receives four input arguments:
                1. path of to a json file containing the applications, i.e. `["app1", ("app2", "load"), ...]`
                2. the location of the lib dir containing the `<app>_<version>` folders
                3. the output location of the boot script
                4. the output location of the script file
                followed by the values of `extra_bootscript_builder_args`, if any.
                """,
        ),
        "default_bootscript_name": attrs.string(
            default = "start",
            doc = """
                The boot script a runnable release boots, named without its `.boot` extension and looked up in
                `releases/<version>/`. The default is `start`, the release's default boot script, produced by
                `generate_default_bootscript`. When the release also contains
                `releases/<version>/<default_bootscript_name>.vm.args`, the launcher passes it as `-args_file`.
            """,
        ),
        "extra_bootscript_builder_args": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
                Additional arguments that are appended, in order, to every `bootscript_builders` invocation
                after the four positional arguments. This lets a custom boot script builder be parameterised
                by the release that uses it. Macros such as `$(location //some:target)` are expanded, and the
                targets they name are made available to the invocation.
            """,
        ),
        "generate_default_bootscript": attrs.bool(
            default = True,
            doc = """
                This attribute controls wether the default bootscript should be generated with OTP's systools.
                """,
        ),
        "include_erts": attrs.bool(
            default = False,
            doc = """
                This field controls whether OTP applications and the Erlang runtime system should be included as part of the release.
                Please note, that at the moment the erts folder is just `erts/`.
            """,
        ),
        "is_executable": attrs.bool(
            default = False,
            doc = """
                This field controls whether the release is runnable. If set, a launcher is generated at
                `bin/<release_name>` that boots the release with the bundled emulator, which is why it
                requires `include_erts = True`. The boot script it uses is `default_bootscript_name`.

                When the release contains `releases/<version>/sys.config`, the launcher passes it as `-config`.
            """,
        ),
        "overlays": attrs.dict(
            key = attrs.string(),
            value = attrs.list(attrs.dep()),
            default = {},
            doc = """
                Overlays can be used to add files to the release. They are specified as mapping from path (from the release
                root) to list of targets. The targets files are places **flat** at the target location with their basename.
            """,
        ),
        "release_name": attrs.option(
            attrs.string(),
            default = None,
            doc = """
                The release name can explicitly be set by this field. This overwrites the default from the target name.
            """,
        ),
        "version": attrs.string(
            default = "1.0.0",
            doc = """
                The `version` field specifies the release version. The release version is used in the release resource file, and
                is part of the path for the folder containing the boot scripts.
            """,
        ),
        "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
    },
    "erlang_test": {
        "common_app_env": attrs.dict(
            key = attrs.string(),
            value = attrs.string(),
            default = {},
            doc = """
                Application environment variables for the `common` application.
            """,
        ),
        "config_files": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
                Will specify what config files the erlang beam machine running test with should load, for reference look at
                [OTP documentation](https://www.erlang.org/doc/man/config.html). These ones should consist of default_output of
                some targets. In general, this field is filled with target coming from then `export_file` rule, as in the example below.
            """,
        ),
        "deps": attrs.list(
            attrs.dep(providers = [ErlangAppOrTestInfo]),
            default = [],
            doc = """
                The set of dependencies needed for all suites included in the target
                to compile and run. They could be either `erlang_app(lication)` or `erlang_test`
                targets, although the latter is discouraged. If some suites need to access common methods,
                a common helper file should be created and included in the `srcs` field of the `erlang_tests` target.
                If some applications are included as dependencies of this target, their private include will automatically
                be pulled and made available for the test. That allows tests to access the private header files from the
                applications under test.
            """,
        ),
        "env": attrs.dict(
            key = attrs.string(),
            value = attrs.string(),
            default = {},
            doc = """
                Add the given values to the environment variables with which the test is executed.
            """,
        ),
        "erl_opts": attrs.option(
            attrs.list(attrs.string()),
            default = None,
            doc = """
                Typically compile options are managed by global config files, however, sometimes it is
                desirable to overwrite the pre-defined compile options. The `erl_opts` field allows developers to do so for
                the compilation of the test suites in this target.
            """,
        ),
        "extra_ct_hooks": attrs.list(
            attrs.string(),
            default = [],
            doc = """
                List of additional Common Test hooks. The strings are interpreted as Erlang terms.
            """,
        ),
        "extra_erl_flags": attrs.list(
            attrs.string(),
            default = [],
            doc = """
                List of additional command line arguments given to the erl command invocation. These
                arguments are added to the front of the argument list.
            """,
        ),
        "resources": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
                The `resources` field specifies targets whose default output are placed in the test `data_dir` directory for
                all the suites present in the macro target. Additionally, if data directory are present in the directory along
                the suite, this one will be pulled automatically for the relevant suite.

                Any target can be used, e.g. if you want to place a built escript in the `data_dir` directory, you can use
                an `erlang_escript` target.
            """,
        ),
        "suite": attrs.source(
            doc = """
                The source file for the test suite. If you are using the macro, you should use the `suites` attribute instead.

                The suites attribute specifies which erlang_test targets should be generated. For each suite "path_to_suite/suite_SUITE.erl" an
                implicit 'erlang_test' target suite_SUITE will be generated.
            """
        ),
        "supports_test_execution_caching": attrs.bool(default = True),
        "_artifact_annotation_mfa": attrs.string(default = "artifact_annotations:default_annotation/1"),
        "_cli_lib": attrs.dep(providers = [ErlangAppInfo], default = "prelude//erlang/common_test/test_cli_lib:test_cli_lib"),
        "_ct_opts": attrs.string(default = read_root_config("erlang", "erlang_test_ct_opts", "")),
        "_inner_trampolines": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
            trampolines that are used for the test node
        """,
        ),
        "_providers": attrs.string(default = ""),
        "_test_binary_lib": attrs.dep(providers = [ErlangAppInfo], default = "prelude//erlang/common_test/test_binary:test_binary"),
        "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
        "_trampolines": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
            trampolines that are used for the test binary
        """,
        ),
    }
    | common_shell_attributes
    | test_common.attributes()
    | re_test_common.test_args(),
}

erlang_rule_attributes = {name: dict(rules_attributes[name], **common_attributes) for name in rules_attributes}
