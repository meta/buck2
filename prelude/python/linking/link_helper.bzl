# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:attrs.bzl", "BUILD_INFO_ATTR")
load(
    "@prelude//cxx:link_groups_types.bzl",
    "LINK_GROUP_MAP_ATTR",
)
load("@prelude//cxx:transformation_spec.bzl", "TransformationResultProvider")
load("@prelude//python:internal_tools.bzl", "PythonInternalToolsInfo")
load("@prelude//python:toolchain.bzl", "PackageStyle", "PythonToolchainInfo")
load("@prelude//python/linking:native.bzl", "process_native_linking")

LinkProviders = provider(
    fields = {
        "build_info_manifest_entries": provider_field(typing.Any, default = None),
        "extensions": provider_field(typing.Any, default = None),
        "extra": provider_field(typing.Any, default = None),
        "extra_artifacts": provider_field(typing.Any, default = None),
        "gc_sections_data": provider_field(typing.Any, default = None),
        "link_args": provider_field(typing.Any, default = None),
        "linker_map_data": provider_field(typing.Any, default = None),
        "runtime_files": provider_field(typing.Any, default = None),
        "shared_libraries": provider_field(typing.Any, default = None),
    },
)

cxx_implicit_attrs = {
    "allow_cache_upload": attrs.bool(default = False),
    "anonymous_link_groups": attrs.bool(default = True),
    "binary_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
    "bolt_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
    "bolt_profile": attrs.option(attrs.source(), default = None),
    "compiler_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
    "cxx_main": attrs.source(default = "prelude//python/tools:embedded_main.cpp"),
    "defaults": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
    "enable_distributed_thinlto": attrs.bool(default = False),
    "executable_deps": attrs.list(attrs.dep()),
    "executable_name": attrs.option(attrs.string(), default = None),
    "frameworks": attrs.list(attrs.string(), default = []),
    "header_namespace": attrs.option(attrs.string(), default = None),
    "headers": attrs.set(attrs.source(), sorted = True, default = []),
    "include_directories": attrs.set(attrs.string(), sorted = True, default = []),
    "lang_compiler_flags": attrs.any(default = {}),
    "lang_preprocessor_flags": attrs.any(default = {}),
    "libraries": attrs.list(attrs.string(), default = []),
    "link_group": attrs.option(attrs.string(), default = None),
    "link_group_deps": attrs.list(attrs.dep(), default = []),
    "link_group_map": LINK_GROUP_MAP_ATTR,
    "link_group_min_binary_node_count": attrs.option(attrs.int(), default = None),
    "link_group_public_deps_label": attrs.option(attrs.string(), default = None),
    "link_ordering": attrs.option(attrs.any(), default = None),
    "link_style": attrs.option(attrs.any(), default = None),
    "linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
    "precompiled_header": attrs.option(attrs.dep(), default = None),
    "prefer_stripped_native_objects": attrs.option(attrs.any(), default = None),
    "preload_deps": attrs.list(attrs.dep(), default = []),
    "preprocessor_flags": attrs.any(default = []),
    "raw_headers": attrs.set(attrs.source(), sorted = True, default = []),
    "_gen_build_info": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
    "_gen_build_info_no_config": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
    "_generated_build_info_enabled": attrs.bool(default = False),
    "_generated_build_info_mode": attrs.enum(["full", "stable"], default = "stable"),
    "_generated_build_info_spec": BUILD_INFO_ATTR,
    "_late_build_info_stamping": attrs.any(default = None),
}

python_implicit_attrs = {
    "static_extension_finder": attrs.source(default = "prelude//python/tools:static_extension_finder.py"),
    "use_oss_python": attrs.bool(default = False),
}

def _process_native_linking_rule_impl(ctx):
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    python_internal_tools = ctx.attrs._python_internal_tools[PythonInternalToolsInfo]
    raw_deps = ctx.attrs.deps
    shared_libs, extensions, link_args, extra, extra_artifacts, linker_map_data, gc_sections_data, runtime_files, build_info_manifest_entries = (
        process_native_linking(
            ctx,
            raw_deps,
            python_toolchain,
            python_internal_tools,
            PackageStyle(ctx.attrs.package_style),
            ctx.attrs.allow_cache_upload,
            generated_build_info_invalidation_deps = ctx.attrs.generated_build_info_invalidation_deps,
            generated_build_info_invalidation_sources = ctx.attrs.generated_build_info_invalidation_sources,
        )
    )
    return [
        LinkProviders(
            build_info_manifest_entries = build_info_manifest_entries,
            shared_libraries = shared_libs,
            link_args = link_args,
            extensions = extensions,
            extra = extra,
            extra_artifacts = extra_artifacts,
            gc_sections_data = gc_sections_data,
            linker_map_data = linker_map_data,
            runtime_files = runtime_files,
        ),
        DefaultInfo(),
    ]

process_native_linking_rule = rule(
    impl = _process_native_linking_rule_impl,
    attrs = {
        "deps": attrs.list(attrs.dep()),  # Note: cxx-only deps here
        "dlopen_deps": attrs.list(attrs.dep(), default = []),
        "generated_build_info_invalidation_deps": attrs.list(attrs.dep(), default = []),
        "generated_build_info_invalidation_sources": attrs.list(attrs.source(), default = []),
        "package_style": attrs.string(),
        "rpath": attrs.string(),
        "shared_only_deps": attrs.list(attrs.dep(), default = []),
        "static_extension_utils": attrs.source(),
        "transformation_spec": attrs.option(
            attrs.dep(providers = [TransformationResultProvider]),
            default = None,
        ),
        "use_anon_target_for_analysis": attrs.bool(default = True),
        "_cxx_toolchain": attrs.dep(),
        "_generate_build_info_after_link": attrs.bool(default = False),
        "_python_internal_tools": attrs.dep(),
        "_python_toolchain": attrs.dep(),
    }
    | cxx_implicit_attrs
    | python_implicit_attrs,
)
