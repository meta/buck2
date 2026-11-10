# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:attrs.bzl", "BUILD_INFO_ATTR")
load("@prelude//cxx:link_groups_types.bzl", "LINK_GROUP_MAP_ATTR")
load("@prelude//decls:common.bzl", "LinkableDepType", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//python:python_runtime_bundle.bzl", "PythonRuntimeBundleInfo")
load("@prelude//python:toolchain.bzl", "NativeLinkStrategy")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
PythonBuckConfigPackageStyle = ["standalone", "inplace", "outplace"]

def _srcs_arg():
    return {
        "srcs": attrs.named_set(
            attrs.source(),
            sorted = True,
            default = [],
            doc = """
    The set of Python (`.py`) files to include in this library.
""",
        ),
    }

def _resources_arg():
    return {
        "resources": attrs.named_set(
            attrs.source(),
            sorted = True,
            default = [],
            doc = """
    Static files to be packaged along with the Python sources.
     These resources can be accessed at runtime using
     the [importlib.resources](https://docs.python.org/3/library/importlib.resources.html) module
     from the Python standard library.
""",
        ),
    }

def _base_module_arg():
    return {
        "base_module": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    The package in which the specified source files and resources should reside in their final
     location in the top-level binary. If unset, Buck uses the project-relative directory
     that contains the BUCK file.
""",
        ),
    }

def _linker_flags_arg():
    return {
        "linker_flags": attrs.list(
            attrs.arg(anon_target_compatible = True),
            default = [],
            doc = """
    Additional linker flags that should be applied to any linking which is specific to this rule.
     Note that whether these flags are used is dependent on the native link strategy selected in
     `.buckconfig` and currently applies only to the merged `.buckconfig`;
     the `separate` link strategy pulls in shared libraries that are linked in the
     context of the rules that own them, such as `cxx_library()`.
""",
        ),
    }

def _package_style_arg():
    return {
        "package_style": attrs.option(
            attrs.enum(PythonBuckConfigPackageStyle),
            default = None,
            doc = """
    Used to override the global packaging style that is set in `[`.buckconfig`
    ]`.
""",
        ),
    }

def _preload_deps_arg():
    return {
        "preload_deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    A list of C/C++ library dependencies that need to be loaded before any
     other libraries when the PEX starts up. This requires dynamic loader support,
     such as `LD_PRELOAD`, found on most systems. This list is order-
     sensitive and the preload libraries listed here are passed down to the dynamic
     linker in the same order.
""",
        ),
    }

def _exclude_deps_from_merged_linking_arg():
    return {
        "exclude_deps_from_merged_linking": attrs.bool(
            default = False,
            doc = """
    When linking the top-level binary with a `merged` ``.buckconfig``,
     do not merge or re-link any native transitive deps of this library. This is useful if
     this library wraps prebuilt native extensions which cannot be re-linked as part of
     library merging.
""",
        ),
    }

def _deduplicate_merged_link_roots():
    return {
        "deduplicate_merged_link_roots": attrs.bool(
            default = True,
            doc = """
    When linking multiple top-level binaries with the `merged` ``.buckconfig``,
     coalesce root link rules which are identical across independent merged links.
""",
        ),
    }

def _version_selections_arg():
    return {
        "version_selections": attrs.dict(
            key = attrs.string(),
            value = attrs.string(),
            default = {},
            doc = """A mapping from library name to desired version. Versioned dependencies from the entire dependency tree with the given name will be used at the specified version, if available.
            Versioned libraries that are unspecified will be used at their respective default version.""",
        ),
    }

def _executable_deps_arg():
    return {
        "executable_deps": attrs.list(attrs.dep(), default = []),
    }

def _deps_arg():
    return {
        "deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
            A list of `python_library()` rules that specify Python
                modules to include in the binary — including all transitive
                dependencies of these rules.
        """,
        ),
    }

def _internal_tools_arg():
    return attrs.default_only(
        attrs.toolchain_dep(
            default = "prelude//python/tools:python_internal_tools",
            doc = """
        Internal implementation details of the Python rules.
    """,
        )
    )

python_common = struct(
    srcs_arg = _srcs_arg,
    resources_arg = _resources_arg,
    base_module_arg = _base_module_arg,
    linker_flags_arg = _linker_flags_arg,
    package_style_arg = _package_style_arg,
    preload_deps_arg = _preload_deps_arg,
    exclude_deps_from_merged_linking_arg = _exclude_deps_from_merged_linking_arg,
    deduplicate_merged_link_roots = _deduplicate_merged_link_roots,
    version_selections_arg = _version_selections_arg,
    executable_deps_arg = _executable_deps_arg,
    deps_arg = _deps_arg,
    internal_tools_arg = _internal_tools_arg,
)

StripLibparStrategy = ["full", "extract", "none"]

def create_manifest_for_source_dir_attr() -> Attr:
    return attrs.exec_dep(default = "prelude//python/tools:create_manifest_for_source_dir")

# Attrs common between python binary/test
def python_executable_attrs() -> dict[str, Attr]:
    python_executable_attrs = {}

    python_executable_attrs.update(constraint_overrides.attributes)

    # allow non-default value for the args below
    python_executable_attrs.update(
        {
            "anonymous_link_groups": attrs.bool(default = False),
            "binary_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "bolt_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "bolt_profile": attrs.option(attrs.source(), default = None),
            "compiler_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "cxx_main": attrs.source(default = "prelude//python/tools:embedded_main.cpp"),
            "enable_distributed_thinlto": attrs.bool(default = False),
            "exe_allow_cache_upload": attrs.bool(
                default = False,
                doc = """
            Allow uploading native executable for caching. Only meaningful for native link strategy.
        """,
            ),
            "executable_name": attrs.option(attrs.string(), default = None),
            "inplace_build_args": attrs.list(attrs.arg(), default = []),
            "interpreter_args": attrs.list(attrs.string(), default = []),
            "lazy_imports_analyzer": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "link_group": attrs.option(attrs.string(), default = None),
            "link_group_map": LINK_GROUP_MAP_ATTR,
            "link_group_min_binary_node_count": attrs.option(attrs.int(), default = None),
            "link_style": attrs.enum(LinkableDepType, default = "static"),
            "main_function": attrs.option(
                attrs.string(),
                default = None,
                doc = """
            Name of a Python function that will serve as the main entry point of
            the binary. The name is either a fully qualified name like
            `foo.bar.baz` or it starts with a `.` like `.bar.baz`, in which case
            it is relative to the package containing the target. This should
            usually be a function defined within one of the dependencies of this
            target. This attribute should be preferred over `main_module` or
            `main`, and it is an error to specify more than one of these.
        """,
            ),
            "make_py_package": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "manifest_module_entries": attrs.option(
                attrs.dict(
                    key = attrs.string(),
                    value = attrs.one_of(
                        attrs.dict(key = attrs.string(), value = attrs.option(attrs.any())),
                        attrs.list(attrs.string()),
                    ),
                ),
                default = None,
                doc = """If present, it should be a `string` -> `entry` mapping that
            gets generated into a `__manifest__` module in the executable. Top
            level string keys will be the names of variables in this module (so
            they must be valid Python identifiers). An `entry` can be a list of
            `string`s, or a further `string`-keyed dictionary.""",
            ),
            "native_link_strategy": attrs.option(attrs.enum(NativeLinkStrategy.values()), default = None),
            "omnibus_linking": attrs.option(
                attrs.enum(["always", "static_link_style"]),
                default = None,
                doc = """
                When the executable links native deps through omnibus (`merged`): `always`, or only when the
                target configuration uses a static link style. The python transition switches such executables to
                the large code model.
            """,
            ),
            "opt_by_default_enabled": attrs.bool(default = False),
            "optimize_for_action_throughput": attrs.bool(default = False),
            "package_split_dwarf_dwp": attrs.bool(default = False),
            "par_style": attrs.option(attrs.string(), default = None),
            "resources": attrs.named_set(attrs.one_of(attrs.dep(), attrs.source(allow_directory = True)), sorted = True, default = []),
            "run_with_inplace": attrs.bool(default = False),
            "runtime_bundle": attrs.option(attrs.dep(providers = [PythonRuntimeBundleInfo]), default = None),
            "runtime_bundle_full": attrs.bool(default = False),
            "runtime_env": attrs.option(attrs.dict(key = attrs.string(), value = attrs.string()), default = None),
            "runtime_libs": attrs.dict(
                key = attrs.string(),
                value = attrs.dep(),
                default = {},
                doc = """
                A dictionary mapping destination filenames to dependencies whose
                default outputs will be installed into the runtime/lib directory
                of the PAR file. Only relevant for native python.
            """,
            ),
            "standalone_build_args": attrs.list(attrs.arg(), default = []),
            "static_extension_finder": attrs.source(default = "prelude//python/tools:static_extension_finder.py"),
            "static_extension_utils": attrs.source(default = "prelude//python/tools:static_extension_utils.cpp"),
            "strip_libpar": attrs.enum(StripLibparStrategy, default = "none"),
            "strip_stapsdt": attrs.bool(default = False),
            "use_anon_target_for_analysis": attrs.bool(
                default = False
            ),  # TODO(dcssiva) Delete this when we change the default analysis method to use anon targets
            "use_lifeguard_incremental": attrs.bool(default = False),
            "use_oss_python": attrs.bool(default = False),
            "use_rust_make_par": attrs.bool(default = False),  # TODO(rishiarora) Delete this when we change the default build style
            "use_rust_make_par_optimizations": attrs.bool(default = False),
            "_build_info": BUILD_INFO_ATTR,
            "_create_manifest_for_source_dir": create_manifest_for_source_dir_attr(),
            "_cxx_hacks": attrs.default_only(attrs.dep(default = "prelude//cxx/tools:cxx_hacks")),
            "_cxx_toolchain": toolchains_common.cxx(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_gen_build_info": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "_generated_build_info_enabled": attrs.bool(default = False),
            "_generated_build_info_mode": attrs.enum(["full", "stable"], default = "stable"),
            "_generated_build_info_spec": BUILD_INFO_ATTR,
            "_late_build_info_stamping": attrs.any(default = None),
            "_python_internal_tools": python_common.internal_tools_arg(),
            "_python_toolchain": toolchains_common.python(),
            "_target_os_type": buck.target_os_type_arg(),
        }
    )

    return python_executable_attrs

def typing_arg() -> dict[str, Attr]:
    return {
        "py_version_for_type_checking": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    This option will force the type checker to perform checking under a specific version of Python interpreter.
""",
        ),
        "shard_typing": attrs.option(
            attrs.bool(),
            default = None,
            doc = """
    Determines if sharding should be enabled on a given target.
""",
        ),
        "typing": attrs.one_of(
            attrs.bool(),
            attrs.exec_dep(providers = [RunInfo]),
            default = False,
            doc = """
    Determines whether to perform type checking on the given target. A RunInfo-producing target enables typing and overrides the Python toolchain's type checker for this target.
""",
        ),
        "typing_validation": attrs.bool(
            default = False,
            doc = """
    When True (and typing is also True), Pyre type checking runs as a build
    validation during normal buck build, failing the build on type errors.
""",
        ),
    }
