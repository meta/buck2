# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Attributes for OCaml build rules.

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxPlatformInfo", "CxxToolchainInfo")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//ocaml:ocaml_toolchain_types.bzl", "OCamlPlatformInfo", "OCamlToolchainInfo")

def _toolchain(lang: str, providers: list[typing.Any]) -> Attr:
    return attrs.default_only(attrs.toolchain_dep(default = "toolchains//:" + lang, providers = providers))

def _cxx_toolchain() -> Attr:
    return _toolchain("cxx", [CxxToolchainInfo, CxxPlatformInfo])

def _ocaml_toolchain() -> Attr:
    return _toolchain("ocaml", [OCamlToolchainInfo, OCamlPlatformInfo])

def _toolchains() -> dict[str, Attr]:
    return {
        "_cxx_toolchain": _cxx_toolchain(),
        "_ocaml_toolchain": _ocaml_toolchain(),
    }

def _deps_arg():
    return {
        "deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    The set of dependencies of this rule. It could include references to ocaml\\_library and cxx\\_library rules.
""",
        ),
    }

def _srcs_arg():
    return {
        "srcs": attrs.option(
            attrs.named_set(attrs.source(), sorted = False),
            default = None,
            doc = """
    The set of source files to be compiled by this rule. It supports \\*.ml, \\*.mli, \\*.mly, \\*.mll, and \\*.c files.
     (see  [this test](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/features/ocaml/testdata/ocaml/clib/BUCK.fixture)  as C interop example and
      [this test](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/features/ocaml/testdata/ocaml/calc/BUCK.fixture)  as parser and lexer example).
""",
        ),
    }

def _compiler_flags_arg():
    return {
        "compiler_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    The set of additional compiler flags to pass to ocaml compiler. It supports
     specifying ppx (see [for example](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/features/ocaml/testdata/compiler_flag_macros/BUCK.fixture)).
""",
        ),
    }

def ocaml_binary_attributes() -> dict[str, Attr]:
    return (
        _srcs_arg()
        | _deps_arg()
        | _compiler_flags_arg()
        | {
            "bytecode_only": attrs.option(attrs.bool(), default = None),
            "linker_flags": attrs.list(attrs.string(), default = []),
            "ocamldep_flags": attrs.list(attrs.arg(), default = []),
            "platform": attrs.option(attrs.string(), default = None),
            "warnings_flags": attrs.option(attrs.string(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | _toolchains()
    )

def ocaml_library_attributes() -> dict[str, Attr]:
    return (
        _srcs_arg()
        | _deps_arg()
        | _compiler_flags_arg()
        | {
            "bytecode_only": attrs.bool(default = False),
            "linker_flags": attrs.list(attrs.arg(), default = []),
            "native_plugin": attrs.bool(default = False),
            "ocamldep_flags": attrs.list(attrs.arg(), default = []),
            "warnings_flags": attrs.option(attrs.string(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | _toolchains()
    )

def ocaml_object_attributes() -> dict[str, Attr]:
    return {
        "bytecode_only": attrs.option(attrs.bool(), default = None),
        "compiler_flags": attrs.list(attrs.arg(), default = []),
        "contacts": attrs.list(attrs.string(), default = []),
        "deps": attrs.list(attrs.dep(), default = []),
        "labels": attrs.list(attrs.string(), default = []),
        "licenses": attrs.list(attrs.source(), default = []),
        "linker_flags": attrs.list(attrs.string(), default = []),
        "ocamldep_flags": attrs.list(attrs.arg(), default = []),
        "platform": attrs.option(attrs.string(), default = None),
        "srcs": attrs.option(attrs.named_set(attrs.source(), sorted = False), default = None),
        "warnings_flags": attrs.option(attrs.string(), default = None),
    } | _toolchains()

def ocaml_shared_attributes() -> dict[str, Attr]:
    return {
        "bytecode_only": attrs.option(attrs.bool(), default = None),
        "compiler_flags": attrs.list(attrs.arg(), default = []),
        "contacts": attrs.list(attrs.string(), default = []),
        "deps": attrs.list(attrs.dep(), default = []),
        "labels": attrs.list(attrs.string(), default = []),
        "licenses": attrs.list(attrs.source(), default = []),
        "linker_flags": attrs.list(attrs.string(), default = []),
        "ocamldep_flags": attrs.list(attrs.arg(), default = []),
        "platform": attrs.option(attrs.string(), default = None),
        "srcs": attrs.option(attrs.named_set(attrs.source(), sorted = False), default = None),
        "warnings_flags": attrs.option(attrs.string(), default = None),
    } | _toolchains()

def prebuilt_ocaml_library_attributes() -> dict[str, Attr]:
    return (
        {
            "bytecode_c_libs": attrs.list(attrs.source(), default = []),
            "bytecode_lib": attrs.option(attrs.source(), default = None),
            "bytecode_only": attrs.bool(default = False),
            "c_libs": attrs.list(attrs.source(), default = []),
            "deps": attrs.list(attrs.dep(), default = []),
            "include_dir": attrs.option(attrs.source(allow_directory = True), default = None),
            "lib_dir": attrs.string(default = ""),
            "lib_name": attrs.string(default = ""),
            "native_c_libs": attrs.list(attrs.source(), default = []),
            "native_lib": attrs.option(attrs.source(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    )
