# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:arglike.bzl", "ArgLike")
load(":manifest.bzl", "ManifestInfo")

# The ways that Python executables handle native linkable dependencies.
NativeLinkStrategy = enum(
    # Statically links extensions into an embedded python binary
    "native",
    # Pull transitive native deps in as fully linked standalone shared libraries.
    # This is typically the fastest build-time link strategy, as it requires no
    # top-level context and therefore can shared build artifacts with all other
    # binaries using this strategy.
    "separate",
    # Statically link all transitive native deps, which don't have an explicit
    # dep from non-C/C++ code (e.g. Python), into a monolithic shared library.
    # Native dep roots, which have an explicit dep from non-C/C++ code, remain
    # as fully linked standalone shared libraries so that, typically, application
    # code doesn't need to change to work with this strategy. This strategy
    # incurs a relatively big build-time cost, but can significantly reduce the
    # size of native code and number of shared libraries pulled into the binary.
    "merged",
)

PackageStyle = enum(
    "inplace",
    "standalone",
    # Similar to inplace, but generate copies instead of symlinks
    "outplace",
)

StripLibparStrategy = enum(
    # Strip all binaries and libraries
    "full",
    # Extract debug symbols into separate files
    "extract",
    # Leave debug symbols intact
    "none",
)

# Structured version; `version` on PythonToolchainInfo is a display
# label and should not be parsed.
ToolchainPythonVersion = record(
    version = str,
    version_tuple = (int, int),
    flavor = str,
)

PythonToolchainInfo = provider(
    # @unsorted-dict-items
    fields = {
        "build_standalone_binaries_locally": provider_field(bool | None, default = None),
        "compile": provider_field(ArgLike | None, default = None),
        # Whether to precompile .pyc bytecode for this version. Disabled for
        # versions whose pyc magic number isn't yet frozen + fully deployed.
        "pyc_compilation_enabled": provider_field(bool, default = True),
        # The interpreter to use to compile bytecode.
        "host_interpreter": provider_field(ArgLike | None, default = None),
        "interpreter": provider_field(ArgLike | None, default = None),
        "version": provider_field(str | None, default = None),
        # None for toolchains that do not declare one.
        "python_version": provider_field(ToolchainPythonVersion | None, default = None),
        "native_link_strategy": provider_field(str | None, default = None),  # Should be `NativeLinkStrategy`.
        "linker_flags": provider_field(ArgLike, default = []),
        "binary_linker_flags": provider_field(ArgLike, default = []),
        "supports_content_based_paths": provider_field(bool | None, default = None),
        "extension_linker_flags": provider_field(ArgLike, default = []),
        "wheel_extension_linker_flags": provider_field(ArgLike, default = []),
        "wheel_linker_flags": provider_field(ArgLike, default = []),
        # site-packages-relative rpaths to embed into libs/bins in the wheel
        "wheel_rpaths": provider_field(ArgLike, default = []),
        "gen_lpar_bootstrap": provider_field(Dependency | None, default = None),
        "package_style": provider_field(str | None, default = None),  # Should be `PackageStyle`.
        "strip_libpar": provider_field(str | None, default = None),  # Should be `StripLibparStrategy`.
        "native_library_runtime_paths": provider_field(ArgLike, default = []),
        "native_library_env_var": provider_field(ArgLike | None, default = None),
        "make_py_package_live": provider_field(Dependency | None, default = None),
        "make_py_package_standalone": provider_field(ArgLike | None, default = None),
        "pex_extension": provider_field(str, default = ".par"),
        # An optional executable for per-target type checking. It is invoked as
        # `<type_checker> <config.json> --output <result.json>`.
        #
        # The input JSON contains `sources` and `dependencies` manifest lists,
        # a `typeshed` manifest, `py_version`, and `system_platform`. The checker
        # must write a Pyre-compatible JSON object containing an `errors` list.
        # Each error must include `code`; `name` and `severity` are optional.
        # Errors that fail validation must also include `path`, `line`, `column`,
        # and `description`. Diagnostics with code 0, name `unused-ignore` or
        # `unused-type-ignore`, or severity `info`, `ignore`, or `warn` do not
        # fail validation.
        #
        # The checker must exit successfully whenever it writes valid result
        # JSON, including when that result contains type errors. A nonzero exit
        # fails the type-checking action before Buck2 can convert the result to
        # validation output.
        "type_checker": provider_field(RunInfo | None, default = None),
        "typeshed_stubs": provider_field(ManifestInfo | None, default = None),
        "emit_omnibus_metadata": provider_field(bool | None, default = None),
        # The fully qualified name of a function that handles invoking the
        # executable's entry point
        "main_runner": provider_field(str, default = "__par__.bootstrap.run_as_main"),
        # Prefix to use when running a Python test/executable.
        "run_prefix": provider_field(ArgLike, default = []),
        "python_error_handler": provider_field(typing.Callable | None, default = None),
        "lazy_imports_analyzer": provider_field(RunInfo | None, default = None),
        "manifest_module_entries": provider_field(dict[str, list[str] | dict[str, typing.Any]] | None, default = None),
        "preload_deps": provider_field(list[Dependency], default = []),
    },
)

# Stores "platform"/flavor name used to resolve *platform_* arguments
PythonPlatformInfo = provider(
    fields = {
        "name": provider_field(typing.Any, default = None),
    }
)

def get_package_style(ctx: AnalysisContext) -> PackageStyle:
    if ctx.attrs.package_style != None:
        return PackageStyle(ctx.attrs.package_style.lower())
    return PackageStyle(ctx.attrs._python_toolchain[PythonToolchainInfo].package_style)

python = struct(
    PythonToolchainInfo = PythonToolchainInfo,
    PythonPlatformInfo = PythonPlatformInfo,
    ToolchainPythonVersion = ToolchainPythonVersion,
    PackageStyle = PackageStyle,
    NativeLinkStrategy = NativeLinkStrategy,
)
