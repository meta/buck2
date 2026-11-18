# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:attrs.bzl", "BUILD_INFO_ATTR", "cxx_common")
load("@prelude//cxx:link_groups_types.bzl", "LINK_GROUP_MAP_ATTR")
load("@prelude//decls:common.bzl", "RuntimeDependencyHandling", "buck")
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//linking:execution_preference.bzl", "link_execution_preference_attr")
load("@prelude//rust:clippy_configuration.bzl", "ClippyConfiguration")
load("@prelude//rust:link_info.bzl", "RustProcMacroPlugin")
load("@prelude//rust:sources.bzl", "mapped_srcs_arg", "srcs_arg", "srcs_filegroup_arg")

def rust_target_dep(is_binary: bool) -> Attr:
    return attrs.dep(
        pulls_and_pushes_plugins = [] if is_binary else [RustProcMacroPlugin],
        pulls_plugins = [RustProcMacroPlugin] if is_binary else [],
    )

def _deps_arg(is_binary: bool):
    return {
        "deps": attrs.list(
            rust_target_dep(is_binary),
            default = [],
            doc = """
    The set of dependencies of this rule. Currently, this supports rust\\_library
     and prebuilt\\_rust\\_library rules.
""",
        ),
    }

def _features_arg():
    return {
        "features": attrs.list(
            attrs.string(),
            default = [],
            doc = """
    The set of features to be enabled for this rule.

     These are passed to `rustc` with `--cfg feature="{feature}"`, and can be used in the code with `#[cfg(feature = "{feature}")]`.
""",
        ),
    }

def _edition_arg():
    return {
        "edition": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    Set the language edition to be used for this rule. Can be set to any edition the
     compiler supports (`2018` right now). If unset it uses the compiler's default
     (`2015`).
""",
        ),
    }

def _rustc_flags_arg():
    return {
        "rustc_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    The set of additional compiler flags to pass to `rustc`.
""",
        ),
    }

def _miri_flags_arg():
    return {
        "miri_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    The set of additional flags to pass to the Miri interpreter when
    running this target via the `[miri]` sub-target. Accepts the same
    `-Zmiri-*` options as `cargo miri` (e.g. `-Zmiri-tree-borrows`).
""",
        ),
    }

def _linker_flags_arg():
    return {
        "linker_flags": attrs.list(
            attrs.arg(anon_target_compatible = True),
            default = [],
            doc = """
    The set of additional flags to pass to the linker.
""",
        ),
    }

def _exported_linker_flags_arg():
    return {
        "exported_linker_flags": attrs.list(
            attrs.arg(anon_target_compatible = True),
            default = [],
            doc = """
    A set of additional flag to pass before this item on the link line, even if
    this items is compiled to a DSO.
""",
        ),
        "exported_post_linker_flags": attrs.list(
            attrs.arg(anon_target_compatible = True),
            default = [],
            doc = """
    A set of additional flag to pass after this item on the link line, even if
    this items is compiled to a DSO.
""",
        ),
    }

def _crate(crate_type):
    return {
        "crate": crate_type,
    }

def _crate_root():
    return {
        "crate_root": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    Set the name of the top-level source file for the crate, which can be used to override the
     default (see `srcs`).
""",
        ),
    }

def _default_roots_arg():
    return {
        "default_roots": attrs.option(
            attrs.list(attrs.string()),
            default = None,
            doc = """
    Set the candidate source names to consider for crate root. Typically used to disambiguate between
     lib.rs or main.rs for rust_test, which may be declare a test suite for either library or binary
     rules. Has no effect if an explicit `crate_root` is provided.
""",
        ),
    }

def _env_arg():
    return {
        "env": attrs.dict(
            key = attrs.string(),
            value = attrs.arg(),
            sorted = False,
            default = {},
            doc = """
    Set environment variables for this rule's invocations of rustc. The environment variable
     values may include macros which are expanded.
""",
        ),
    }

def _run_env_arg():
    return {
        "run_env": attrs.dict(
            key = attrs.string(),
            value = attrs.arg(),
            sorted = False,
            default = {},
            doc = """
    Set environment variables during test execution. The environment variable values may
     include macros which are expanded.
""",
        ),
    }

def _build_and_run_env_arg():
    # Same as env_arg(), but with different documentation.
    return {
        "env": attrs.dict(
            key = attrs.string(),
            value = attrs.arg(),
            sorted = False,
            default = {},
            doc = """
    Set environment variables for this rule's invocations of rustc *and* during execution of
     the tests. The environment variable values may include macros which are expanded.
""",
        ),
    }

def _named_deps_arg(is_binary: bool):
    return {
        "named_deps": attrs.one_of(
            attrs.dict(key = attrs.string(), value = rust_target_dep(is_binary), sorted = False),
            attrs.list(attrs.tuple(attrs.arg(), rust_target_dep(is_binary))),
            default = {},
            doc = """
    Add crate dependencies and define a local name by which to use that dependency by. This
     allows a crate to have multiple dependencies with the same crate name. For example:
     `named_deps = {"local_name", ":some_rust_crate" }`.
     The dependencies may also be non-Rust, but the alias is ignored. It has no effect on the
     symbols provided by a C/C++ library.
""",
        ),
    }

def _rust_toolchain_arg():
    return {
        "_rust_internal_tools_toolchain": attrs.default_only(
            attrs.toolchain_dep(default = "prelude//rust/tools:internal_tools_toolchain"),
        ),
        "_rust_toolchain": toolchains_common.rust(),
    }

def _cxx_toolchain_arg():
    return {
        "_cxx_toolchain": toolchains_common.cxx(),
    }

def _workspaces_arg():
    return {
        "_workspaces": attrs.list(
            attrs.label(),
            default = [],
            doc = """
    Internal implementation detail of Rust workspaces. This should not be set manually and will be
     replaced in favor of metadata in a future version of buck2.
""",
        ),
    }

rust_common = struct(
    deps_arg = _deps_arg,
    srcs_arg = srcs_arg,
    srcs_filegroup_arg = srcs_filegroup_arg,
    features_arg = _features_arg,
    edition_arg = _edition_arg,
    rustc_flags_arg = _rustc_flags_arg,
    miri_flags_arg = _miri_flags_arg,
    linker_flags_arg = _linker_flags_arg,
    exported_linker_flags_arg = _exported_linker_flags_arg,
    crate = _crate,
    crate_root = _crate_root,
    default_roots_arg = _default_roots_arg,
    env_arg = _env_arg,
    run_env_arg = _run_env_arg,
    build_and_run_env_arg = _build_and_run_env_arg,
    mapped_srcs_arg = mapped_srcs_arg,
    named_deps_arg = _named_deps_arg,
    rust_toolchain_arg = _rust_toolchain_arg,
    cxx_toolchain_arg = _cxx_toolchain_arg,
    workspaces_arg = _workspaces_arg,
)

# Values of the `_rust_analyzer_target_kind` attribute, which tells rust-analyzer
# what kind of crate a target produces.
RustAnalyzerTargetKind = enum("bin", "lib", "test")

def rust_common_attributes(is_binary: bool):
    return (
        buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "clippy_configuration": attrs.option(attrs.dep(providers = [ClippyConfiguration]), default = None),
            "clippy_flags": attrs.list(
                attrs.arg(),
                default = [],
                doc = "Flags passed to clippy but not `rustc`, ahead of `rustc_flags` so those can override them.",
            ),
            "coverage": attrs.bool(default = False),
            "default_platform": attrs.option(attrs.string(), default = None),
            "flagged_deps": attrs.list(attrs.tuple(rust_target_dep(is_binary), attrs.list(attrs.string())), default = []),
            "incremental_enabled": attrs.bool(default = False),
            "resources": attrs.named_set(attrs.one_of(attrs.dep(), attrs.source()), sorted = True, default = []),
            "rustdoc_flags": attrs.list(attrs.arg(), default = []),
            "separate_debug_info": attrs.bool(default = False),
            "use_content_based_paths": attrs.bool(default = True),
            "uses_restricted_rustc_flags": attrs.bool(default = False),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_target_os_type": buck.target_os_type_arg(),
        }
        | cxx_common.default_deps_arg()
    )

def rust_analyzer_target_kind(kind: str) -> dict[str, Attr]:
    return {
        "_rust_analyzer_target_kind": attrs.default_only(
            attrs.enum(RustAnalyzerTargetKind.values(), default = kind),
        ),
    }

def miri_interpreted_attr() -> dict[str, Attr]:
    return {
        "miri_interpreted": attrs.bool(
            default = False,
            doc = """
                Makes this target itself the Miri-interpreted run: no
                native artifact and no `[miri]` sub-target. Requires a
                toolchain providing the Miri driver.
            """,
        ),
    }

def rust_binary_attrs_group(prefix: str) -> dict[str, Attr]:
    attrs = rust_common.deps_arg(is_binary = True) | rust_common.named_deps_arg(is_binary = True) | rust_common.linker_flags_arg() | native_common.link_style()
    return {prefix + name: v for name, v in attrs.items()}

RUST_EXECUTABLE_ATTRIBUTES = {
    "anonymous_link_groups": attrs.bool(default = True),
    # Unlike cxx which supports pre-defined link groups, we only support
    # auto_link_groups in rust
    "auto_link_groups": attrs.bool(default = True),
    # BOLT post-link optimization only applies when cxx performs the link
    # (advanced_unstable_linking toolchains; see rust_link_binary).
    "bolt_flags": attrs.list(attrs.arg(), default = []),
    "bolt_profile": attrs.option(attrs.source(), default = None),
    # Contents to embed into the linked executable as named ELF sections, added
    # after BOLT and before build info stamping. Only applies when cxx performs
    # the link (advanced_unstable_linking toolchains; see rust_link_binary).
    "elf_sections": attrs.dict(
        key = attrs.string(),
        value = attrs.source(),
        sorted = True,
        default = {},
    ),
    "enable_distributed_thinlto": attrs.bool(default = False),
    "extra_dwp_flags": attrs.list(attrs.string(), default = []),
    # Opt the final executable output into content-based (immutable) buck-out
    # pathing (default off). A bare static binary becomes a content-based exe.
    # A binary with adjacent files (`resources` and/or a shared-lib tree)
    # ships as a content-addressed `assembled_dir` dist bundle instead (exe
    # copied in, adjacent files symlinked; `[dist]` subtarget). Bundle
    # immutability is per-entry: copied entries always; symlinked entries only
    # when their target is itself content-based -- config-based resources and
    # the (deliberately config-based) shlib tree remain rewrite-in-place.
    "has_content_based_path": attrs.bool(default = False),
    "link_execution_preference": link_execution_preference_attr(),
    # Required by the rules but not supported, since Rust is auto-link groups only
    "link_group": attrs.default_only(attrs.option(attrs.string(), default = None)),
    "link_group_map": LINK_GROUP_MAP_ATTR,
    "link_group_min_binary_node_count": attrs.option(attrs.int(), default = None),
    "rpath": attrs.bool(
        default = False,
        doc = """
              Set the "rpath" in the executable when using a shared link style.
          """,
    ),
    "runtime_dependency_handling": attrs.option(
        attrs.enum(RuntimeDependencyHandling),
        default = None,
        doc = """
              Controls how shared library dependencies are handled at runtime. If not set,
              falls back to the CXX toolchain default. If ``symlink`` is specified then shared
              library dependencies with ``preferred_linkage = "shared"`` are automatically
              detected and included in a symlink tree alongside the executable. If ``no_symlink``
              is specified then shared library dependencies are not included in the symlink tree.
          """,
    ),
    "_build_info": BUILD_INFO_ATTR,
}
