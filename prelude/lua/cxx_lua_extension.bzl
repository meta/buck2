# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "cxx_common")
load("@prelude//decls:common.bzl", "CxxRuntimeType", "CxxSourceType", "HeadersAsRawHeadersMode", "buck")
load(":attrs.bzl", "lua_common")

def cxx_lua_extension_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

cxx_lua_extension = prelude_rule(
    name = "cxx_lua_extension",
    impl = cxx_lua_extension_impl,
    docs = """
        A cxx\\_lua\\_extension() rule is a variant of a C/C++ library which is built as a Lua module. As such,
        it has a module name formed by the `base_module` parameter and the rule name and implicitly
        depends on Lua C library (configured via the `.buckconfig`
        parameter.
    """,
    examples = """
        ```
        # A rule that builds a Lua extension from a single .cpp file.
        cxx_lua_extension(
          name = 'mymodule',
          base_module = 'foo.bar',
          srcs = [
            'mymodule.cpp',
          ],
          compiler_flags = [
            '-fno-omit-frame-pointer',
          ],
        )

        # A library rule which has a single source importing the above extension.
        lua_library(
          name = 'utils',
          srcs = [
            'utils.lua',
          ],
          deps = [
            ':mymodule',
          ],
        )
        ```

        ```
        -- The `utils.lua` source, wrapped by the `utils` rule above.

        -- Import the C/C++ extension build above.
        require "foo.bar.mymodule"

        ...
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        lua_common.base_module_arg()
        | cxx_common.srcs_arg()
        | cxx_common.headers_arg()
        | cxx_common.header_namespace_arg()
        | cxx_common.preprocessor_flags_arg()
        | cxx_common.compiler_flags_arg()
        | cxx_common.linker_flags_arg()
        | {
            "cxx_runtime_type": attrs.option(attrs.enum(CxxRuntimeType), default = None),
            "default_platform": attrs.option(attrs.string(), default = None),
            "defaults": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
            "deps": attrs.list(attrs.dep(), default = []),
            "executable_name": attrs.option(attrs.string(), default = None),
            "frameworks": attrs.list(attrs.string(), default = []),
            "headers_as_raw_headers_mode": attrs.option(attrs.enum(HeadersAsRawHeadersMode), default = None),
            "include_directories": attrs.set(attrs.string(), sorted = True, default = []),
            "lang_compiler_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "lang_preprocessor_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "libraries": attrs.list(attrs.string(), default = []),
            "linker_extra_outputs": attrs.list(attrs.string(), default = []),
            "post_linker_flags": attrs.list(attrs.arg(), default = []),
            "precompiled_header": attrs.option(attrs.source(), default = None),
            "prefix_header": attrs.option(attrs.source(), default = None),
            "raw_headers": attrs.set(attrs.source(), sorted = True, default = []),
            "version_universe": attrs.option(attrs.string(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
