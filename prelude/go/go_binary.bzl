# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifacts.bzl", "single_artifact")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:resources.bzl", "create_relocatable_resources_info", "create_resource_db")
load("@prelude//cxx:attrs.bzl", "BUILD_INFO_ATTR", "cxx_common")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//dist:dist_info.bzl", "DistInfo")
load("@prelude//go/transitions:defs.bzl", "build_tags_attr", "cgo_enabled_attr", "coverage_mode_attr", "go_binary_transition")
load(
    "@prelude//linking:link_info.bzl",
    "LinkStyle",
)
load(
    "@prelude//utils:utils.bzl",
    "from_named_set",
    "map_val",
    "value_or",
)
load(":attrs.bzl", "BuildMode", "go_common")
load(":cgo_builder.bzl", "get_cgo_build_context")
load(":compile.bzl", "GoTestInfo")
load(":coverage.bzl", "GoCoverageMode")
load(":link.bzl", "GoBuildMode", "link")
load(":package_builder.bzl", "GoBuildConfig", "GoSourceInputs", "declare_package_build")
load(":packages.bzl", "go_attr_pkg_name")
load(":toolchain.bzl", "evaluate_cgo_enabled")

def go_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    pkg_import_path = go_attr_pkg_name(ctx)
    cgo_enabled = evaluate_cgo_enabled(ctx.attrs._cgo_enabled)
    coverage_mode = GoCoverageMode(ctx.attrs._coverage_mode) if ctx.attrs._coverage_mode else None
    cgo_build_context = get_cgo_build_context(ctx)

    lib, pkg_info, _ = declare_package_build(
        ctx = ctx,
        pkg_import_path = pkg_import_path,
        main = True,
        sources = GoSourceInputs(
            srcs = ctx.attrs.srcs + ctx.attrs.headers,
            embed_srcs = from_named_set(ctx.attrs.embed_srcs),
            package_root = ctx.attrs.package_root,
        ),
        cgo_build_context = cgo_build_context,
        config = GoBuildConfig(
            compiler_flags = ctx.attrs.compiler_flags,
            build_tags = ctx.attrs._build_tags,
            cgo_enabled = cgo_enabled,
            coverage_enabled = ctx.attrs.coverage_enabled,
            coverage_mode = coverage_mode,
        ),
        deps = ctx.attrs.deps,
    )
    (bin, runtime_files, external_debug_info) = link(
        ctx,
        lib,
        cgo_enabled = cgo_enabled,
        deps = ctx.attrs.deps,
        link_style = value_or(map_val(LinkStyle, ctx.attrs.link_style), LinkStyle("static")),
        build_mode = GoBuildMode(value_or(ctx.attrs.build_mode, "exe")),
        linker_flags = ctx.attrs.linker_flags,
        link_mode = ctx.attrs.link_mode,
        external_linker_flags = ctx.attrs.external_linker_flags,
    )

    # runtime_files are all the artifacts that must be present in order for this
    # binary to be runnable. Notably, all of its shared library dependencies.
    # This is materialized when a Go binary is executed as a genrule.
    #
    # other_outputs is a superset of runtime_files, adding external debuginfo
    # which is necessary for a user to run this binary in a debugger. This is
    # materialized when a Go binary is the end result of a build.
    runtime_files = list(runtime_files)
    other_outputs = runtime_files + external_debug_info

    named_resources = from_named_set(ctx.attrs.resources)
    resources = {}
    for alias, resource in named_resources.items():
        resource = single_artifact(resource)
        resources[alias] = resource

        runtime_files.append(resource.default_output)
        runtime_files.extend(resource.nondebug_runtime_files)

        other_outputs.append(resource.default_output)
        other_outputs.extend(resource.other_outputs)

    # Relocatable manifest + contents dir so distributors (e.g. fbpkg with
    # package_resources=True) can ship resources alongside the binary, plus
    # the working-dir resource DB (<name>.resources.json) so runtimes that
    # execute the binary with its runtime files materialized (buck2 run,
    # genrules) resolve aliases exe-adjacent. Mirrors
    # rust_binary/cxx_executable. Exposed as the "resources" subtarget so the
    # mapping is inspectable.
    relocatable_resources_json = None
    relocatable_resources_contents = None
    sub_targets = {}
    if resources:
        resource_db = create_resource_db(
            ctx = ctx,
            name = ctx.label.name + ".resources.json",
            binary = bin,
            resources = resources,
        )
        runtime_files.append(resource_db)
        other_outputs.append(resource_db)
        relocatable_resources_json, relocatable_resources_contents = create_relocatable_resources_info(
            ctx = ctx,
            name = ctx.label.name,
            resources = resources,
        )
        sub_targets["resources"] = [
            DefaultInfo(
                default_output = relocatable_resources_json,
                other_outputs = [relocatable_resources_contents],
            ),
        ]

    return [
        DefaultInfo(
            default_output = bin,
            other_outputs = other_outputs,
            sub_targets = sub_targets,
        ),
        RunInfo(args = cmd_args(bin, hidden = other_outputs)),
        DistInfo(
            nondebug_runtime_files = runtime_files,
            relocatable_resources_json = relocatable_resources_json,
            relocatable_resources_contents = relocatable_resources_contents,
        ),
        GoTestInfo(
            deps = ctx.attrs.deps,
            srcs = ctx.attrs.srcs,
            pkg_import_path = pkg_import_path,
            coverage_enabled = ctx.attrs.coverage_enabled,
        ),
        pkg_info,
    ]

go_binary = prelude_rule(
    name = "go_binary",
    impl = go_binary_impl,
    docs = """
        A go\\_binary() rule builds a native executable from the supplied set of Go source files
         and dependencies. The files supplied are expected to be in the main package, implicitly.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/features/go/testdata).

        ```
        go_binary(
          name='greet',
          srcs=[
            'main.go',
          ],
          deps=[
            ':greeting',
          ],
        )

        go_library(
          name='greeting',
          srcs=[
            'greeting.go',
          ],
          deps=[
            ':join',
          ],
        )

        go_library(
          name='join',
          srcs=[
            'join.go',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        go_common.package_name_arg()
        | go_common.srcs_arg()
        | go_common.deps_arg()
        | go_common.link_style_arg()
        | go_common.link_mode_arg()
        | go_common.compiler_flags_arg()
        | go_common.assembler_flags_arg()
        | go_common.linker_flags_arg()
        | go_common.external_linker_flags_arg()
        | go_common.embed_srcs_arg()
        | {
            "build_mode": attrs.option(
                attrs.enum(BuildMode),
                doc = """
                Determines the build mode (equivalent of `-buildmode`). Can be
                 one of the following values: `exe`, `pie`.`
            """,
                default = None,
            ),
        }
        | go_common.package_root_arg()
        | go_common.cgo_enabled_arg()
        | go_common.build_tags_arg()
        | go_common.coverage_enabled_arg()
        | cxx_common.headers_arg()
        | cxx_common.header_namespace_arg()
        | go_common.cxx_preprocessor_flags_arg()
        | go_common.cxx_compiler_flags_arg()
        | {
            "platform": attrs.option(attrs.string(), default = None),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "coverage_mode": attrs.option(
                attrs.enum(GoCoverageMode.values()),
                default = None,
                doc = """
                Coverage instrumentation mode for the binary. When set, the binary is built with
                coverage instrumentation. Set GOCOVERDIR to a directory path when running the binary
                to collect coverage data.
            """,
            ),
            "resources": attrs.named_set(
                attrs.one_of(attrs.dep(), attrs.source(allow_directory = True)),
                sorted = True,
                default = [],
                doc = """
                Static files provided as runtime files of the binary, materialized
                 alongside its outputs. Accepts a list, with entries named by their
                 short path, or a dict mapping an alias to each entry; aliases
                 identify the entry in the `<binary>.resources.json` manifest,
                 resolved relative to the directory containing the binary.
            """,
            ),
            "_build_info": BUILD_INFO_ATTR,
            "_build_tags": build_tags_attr,
            "_cgo_enabled": cgo_enabled_attr,
            "_coverage_mode": coverage_mode_attr,
            "_cxx_toolchain": toolchains_common.cxx(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_go_stdlib": attrs.default_only(attrs.dep(default = "prelude//go/tools:stdlib")),
            "_go_toolchain": toolchains_common.go(),
        }
    ),
    cfg = go_binary_transition,
)
