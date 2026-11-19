# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:common.bzl", "LinkableDepType")

BuildMode = ["exe", "c_shared", "c_archive", "pie"]

LinkMode = ["internal", "external"]

def _deps_arg():
    return {
        "deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    The set of dependencies of this rule. Currently, this only supports go\\_library rules.
""",
        ),
    }

def _srcs_arg():
    return {
        "srcs": attrs.list(
            attrs.source(),
            default = [],
            doc = """
    The set of source files to be compiled by this rule. .go files will be compiled with the Go
     compiler, .s files will be compiled with the assembler, and everything else is assumed to be
     files that may be `#include`d by the assembler.
""",
        ),
    }

def _package_root_arg():
    return {
        "package_root": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    Sets Go package direactory (relative to BUCK file).
    By default (or if None passes) package_root is being detected automatically.
    Empty string of Go package is on the same level as BUCK file otherwise the subdirectory name.
    Example for srcs = ["foo/bar.go"], package_root = "foo"
""",
        ),
    }

def _link_style_arg():
    return {
        "link_style": attrs.option(
            attrs.enum(LinkableDepType),
            default = None,
            doc = """
    Determines whether to build and link this rule's dependencies statically or dynamically. Can be
     one of the following values: `static`, `static_pic` or `shared`.
     This argument is relevant only if the cgo extension is enabled. Otherwise, Buck ignores this argument.
""",
        ),
    }

def _link_mode_arg():
    return {
        "link_mode": attrs.option(
            attrs.enum(LinkMode),
            default = None,
            doc = """
    Determines the link mode (equivalent of `-mode`). Can be one of the following
     values: `internal`, `external`.
     If no value is provided, the mode is set automatically depending on the other args.
""",
        ),
    }

def _package_name_arg():
    return {
        "package_name": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    Sets the full name of the package being compiled. This defaults to the path from the buck root.
     (e.g. given a ./.buckconfig, a rule in ./a/b/BUCK defaults to package "a/b")
""",
        ),
    }

def _compiler_flags_arg():
    return {
        "compiler_flags": attrs.list(
            attrs.string(),
            default = [],
            doc = """
    The set of additional compiler flags to pass to `go tool compile`.
""",
        ),
    }

def _assembler_flags_arg():
    return {
        "assembler_flags": attrs.list(
            attrs.string(),
            default = [],
            doc = """
    The set of additional assembler flags to pass to `go tool asm`.
""",
        ),
    }

def _linker_flags_arg():
    return {
        "linker_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    Extra linker flags passed to go link
""",
        ),
    }

def _external_linker_flags_arg():
    return {
        "external_linker_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    Extra external linker flags passed to go link via `-extld` argument.
""",
        ),
    }

def _embed_srcs_arg():
    return {
        "embed_srcs": attrs.named_set(
            attrs.source(),
            default = [],
            doc = """
    The set of files and directories to be embedded if match go:embed patterns, e.g. `//go:embed *.txt`

    Can be specified as either:
    - A list of source files (names derived from short_path)
    - A dict mapping custom names to source files (for stable naming of generated targets)

    Example (list):

    ```
    embed_srcs = ["data/config.json", "templates/index.html"]
    ```

    Example (dict):

    ```
    embed_srcs = {
        "config.json": ":generated_config",
        "index.html": "templates/index.html",
    }
    ```
""",
        ),
    }

def _cgo_enabled_arg():
    return {
        "cgo_enabled": attrs.option(
            attrs.bool(),
            default = None,
            doc = """
    Analog of CGO_ENABLED env-var, applies to this target and its dependencies.
    If None it will depend on the availability of CXX toolchain.
""",
        ),
    }

def _override_cgo_enabled_arg():
    return {
        "override_cgo_enabled": attrs.option(
            attrs.bool(),
            default = None,
            doc = """
    Per-target analog of CGO_ENABLED env-var, overrides its value for the target, but not for its dependencies.
""",
        ),
    }

def _build_tags_arg():
    return {
        "build_tags": attrs.list(
            attrs.string(),
            default = [],
            doc = """
    Build tags to apply to this target and its dependencies.
""",
        ),
    }

def _cxx_compiler_flags_arg():
    return {
        "cxx_compiler_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    GCC/Clang flags to use when compiling any of the above C/C++ sources (which require compilation).
""",
        ),
    }

def _cxx_preprocessor_flags_arg():
    return {
        "cxx_preprocessor_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    GCC/Clang flags to use when preprocessing any of the above C/C++ sources (which require preprocessing).
""",
        ),
    }

def _generate_exported_header():
    return {
        "generate_exported_header": attrs.bool(
            default = False,
            doc = """
    Generate header file with declaration for functions exported with `//export`
    The header name for target `cell//foo/bar:lib` will be `foo/bar/lib.h`
""",
        ),
    }

def _coverage_enabled():
    return {
        "coverage_enabled": attrs.bool(
            default = True,
            doc = """
    Enable test code coverage collection for this target.
""",
        ),
    }

go_common = struct(
    deps_arg = _deps_arg,
    srcs_arg = _srcs_arg,
    package_root_arg = _package_root_arg,
    link_style_arg = _link_style_arg,
    link_mode_arg = _link_mode_arg,
    package_name_arg = _package_name_arg,
    compiler_flags_arg = _compiler_flags_arg,
    assembler_flags_arg = _assembler_flags_arg,
    linker_flags_arg = _linker_flags_arg,
    external_linker_flags_arg = _external_linker_flags_arg,
    embed_srcs_arg = _embed_srcs_arg,
    cgo_enabled_arg = _cgo_enabled_arg,
    override_cgo_enabled_arg = _override_cgo_enabled_arg,
    build_tags_arg = _build_tags_arg,
    cxx_compiler_flags_arg = _cxx_compiler_flags_arg,
    cxx_preprocessor_flags_arg = _cxx_preprocessor_flags_arg,
    generate_exported_header = _generate_exported_header,
    coverage_enabled_arg = _coverage_enabled,
)
