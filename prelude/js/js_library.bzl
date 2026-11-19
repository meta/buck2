# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//js:js_providers.bzl", "JsLibraryInfo", "get_transitive_outputs")
load("@prelude//js:js_utils.bzl", "TRANSFORM_PROFILES", "get_canonical_src_name", "get_flavors", "run_worker_commands")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "map_idx")
load(":attrs.bzl", "js_library_attrs")

# A group of sources that all have the same canonical name. The main_source is arbitrary but
# consistent (it is just the first source encountered when processing the src files).
GroupedSource = record(
    canonical_name = str,
    main_source = Artifact,
    additional_sources = list[Artifact],
)

def _get_grouped_srcs(ctx: AnalysisContext) -> list[GroupedSource]:
    grouped_srcs = {}
    for src in ctx.attrs.srcs:
        expect(
            isinstance(src, Artifact),
            "src {} is not an artifact, its type is: {}".format(src, type(src)),
        )
        canonical_src_name = get_canonical_src_name(src.short_path)
        if grouped_srcs.get(canonical_src_name, None) == None:
            grouped_srcs[canonical_src_name] = GroupedSource(
                canonical_name = canonical_src_name,
                main_source = src,
                additional_sources = [],
            )
        else:
            grouped_srcs[canonical_src_name].additional_sources.append(src)

    return grouped_srcs.values()

def _get_base_package(ctx: AnalysisContext) -> str:
    # The base package is identical for every source in the target, so callers resolve it once and
    # pass it into _get_virtual_path rather than re-deriving it on every call.
    package = ctx.label.package
    base_path = ctx.attrs.base_path
    if base_path and base_path not in ["", "."]:
        package = paths.join(package, base_path)

    return package

def _get_virtual_path(package: str, src: Artifact) -> str:
    return paths.join(package, src.short_path)

# A source's transform job args, minus the fields that depend on the transform profile
# (`outputFilePath` and `transformProfile`). These are built once per source and reused across
# all of TRANSFORM_PROFILES.
_PrecomputedJsFile = record(
    canonical_name = str,
    hidden_inputs = list[ArgLike],
    job_args = dict,
)

def _precompute_transform_job_args(ctx: AnalysisContext, flavors: list[str], grouped_srcs: list[GroupedSource]) -> list[_PrecomputedJsFile]:
    # `js_library_impl` runs the transform pipeline once per entry in TRANSFORM_PROFILES. Every
    # field of a source's transform job args is identical across profiles except `outputFilePath`
    # and `transformProfile`, so build the profile-independent portion exactly once here instead
    # of rebuilding it for every (source, profile) pair inside `_build_js_files`.
    extra_job_args = {}
    extra_hidden_inputs = []
    if ctx.attrs.extra_json:
        extra_job_args["extraData"] = cmd_args(ctx.attrs.extra_json, delimiter = "")
        extra_hidden_inputs.append(ctx.attrs.extra_json)

    if ctx.attrs.extra_babel_plugins:
        babel_plugin_configs = []
        for plugin_value in ctx.attrs.extra_babel_plugins:
            if type(plugin_value) == "tuple":
                plugin_dep, plugin_args_json = plugin_value
            else:
                plugin_dep = plugin_value
                plugin_args_json = None
            plugin_artifact = plugin_dep[DefaultInfo].default_outputs[0]
            config = {"modulePath": plugin_artifact}
            extra_hidden_inputs.append(plugin_artifact)
            if plugin_args_json != None:
                config["pluginArgs"] = cmd_args(plugin_args_json, delimiter = "")
                extra_hidden_inputs.append(plugin_args_json)
            babel_plugin_configs.append(config)
        extra_job_args["extraBabelPlugins"] = babel_plugin_configs

    base_package = _get_base_package(ctx)

    precomputed_js_files = []
    for grouped_src in grouped_srcs:
        job_args = {
            "additionalSources": [
                {
                    "sourcePath": additional_source,
                    "virtualPath": _get_virtual_path(base_package, additional_source),
                }
                for additional_source in grouped_src.additional_sources
            ],
            "command": "transform",
            "flavors": flavors,
            "release": ctx.attrs._is_release,
            "sourceJsFileName": _get_virtual_path(base_package, grouped_src.main_source),
            "sourceJsFilePath": grouped_src.main_source,
        }
        job_args.update(extra_job_args)
        precomputed_js_files.append(
            _PrecomputedJsFile(
                canonical_name = grouped_src.canonical_name,
                hidden_inputs = [grouped_src.main_source] + grouped_src.additional_sources + extra_hidden_inputs,
                job_args = job_args,
            )
        )

    return precomputed_js_files

def _build_js_files(ctx: AnalysisContext, transform_profile: str, precomputed_js_files: list[_PrecomputedJsFile]) -> list[Artifact]:
    if not precomputed_js_files:
        return []

    all_output_paths = []
    all_command_hidden_artifacts = []
    all_job_args = []
    for precomputed_js_file in precomputed_js_files:
        identifier = "{}/{}".format(transform_profile, precomputed_js_file.canonical_name)

        output_path = ctx.actions.declare_output(
            "transform-out/{}.jsfile".format(identifier),
            has_content_based_path = True,
        )

        # Copy the profile-independent base and fill in the two profile-specific fields.
        job_args = dict(precomputed_js_file.job_args)
        job_args["outputFilePath"] = output_path.as_output()
        job_args["transformProfile"] = "default" if transform_profile == "hermes-legacy" else transform_profile

        all_output_paths.append(output_path)
        all_command_hidden_artifacts.append(precomputed_js_file.hidden_inputs + [output_path.as_output()])
        all_job_args.append(job_args)

    command_args_file = ctx.actions.write_json(
        "{}.js_command_args".format(transform_profile),
        {
            "commands": all_job_args,
            "version": 1,
        },
        with_inputs = False,
        has_content_based_path = True,
    )

    batch_size = 25
    command_count = len(all_output_paths)
    for batch_number, start_index in enumerate(range(0, command_count, batch_size)):
        end_index = min(start_index + batch_size, command_count)
        hidden_artifacts = []
        for command_hidden_artifacts in all_command_hidden_artifacts[start_index:end_index]:
            hidden_artifacts.extend(command_hidden_artifacts)
        run_worker_commands(
            ctx = ctx,
            worker_tool = ctx.attrs.worker,
            command_args_file = command_args_file,
            command_args_file_range = (start_index, end_index),
            hidden_artifacts = hidden_artifacts,
            identifier = "{}_{}_batch{}".format(ctx.label.name, transform_profile, batch_number),
            category = "transform",
            has_content_based_path = True,
        )

    return all_output_paths

def _build_library_files(ctx: AnalysisContext, transform_profile: str, flavors: list[str], js_files: list[Artifact]) -> Artifact:
    output_path = ctx.actions.declare_output(
        "library-files-out/{}/library_files".format(transform_profile),
        has_content_based_path = True,
    )

    job_args = {
        "command": "library-files",
        "flavors": flavors,
        "outputFilePath": output_path.as_output(),
        "platform": ctx.attrs._platform,
        "release": ctx.attrs._is_release,
        "sourceFilePaths": js_files,
    }

    if ctx.attrs.extra_json:
        job_args["extraData"] = cmd_args(ctx.attrs.extra_json, delimiter = "")

    if ctx.attrs._asset_dest_path_resolver:
        job_args["assetDestPathResolver"] = ctx.attrs._asset_dest_path_resolver

    command_args_file = ctx.actions.write_json(
        "library_files_{}_command_args".format(transform_profile),
        job_args,
        with_inputs = True,
        has_content_based_path = True,
    )

    run_worker_commands(
        ctx = ctx,
        worker_tool = ctx.attrs.worker,
        command_args_file = command_args_file,
        identifier = transform_profile,
        category = "library_files",
        has_content_based_path = True,
    )
    return output_path

def _build_js_library(ctx: AnalysisContext, transform_profile: str, library_files: Artifact, flavors: list[str], dep_library_files: list[Artifact]) -> Artifact:
    output_path = ctx.actions.declare_output(
        "library-dependencies-out/{}.jslib".format(transform_profile),
        has_content_based_path = True,
    )
    job_args = {
        "aggregatedSourceFilesFilePath": library_files,
        "command": "library-dependencies",
        "dependencyLibraryFilePaths": dep_library_files,
        "flavors": flavors,
        "outputPath": output_path.as_output(),
        "platform": ctx.attrs._platform,
        "release": ctx.attrs._is_release,
    }

    if ctx.attrs.extra_json:
        job_args["extraData"] = cmd_args(ctx.attrs.extra_json, delimiter = "")

    command_args_file = ctx.actions.write_json(
        "library_deps_{}_args".format(transform_profile),
        job_args,
        with_inputs = True,
        has_content_based_path = True,
    )

    run_worker_commands(
        ctx = ctx,
        worker_tool = ctx.attrs.worker,
        command_args_file = command_args_file,
        identifier = transform_profile,
        category = "library_dependencies",
        has_content_based_path = True,
    )

    return output_path

def js_library_impl(ctx: AnalysisContext) -> list[Provider]:
    grouped_srcs = _get_grouped_srcs(ctx)
    flavors = get_flavors(ctx)
    precomputed_js_files = _precompute_transform_job_args(ctx, flavors, grouped_srcs)
    sub_targets = {}

    for transform_profile in TRANSFORM_PROFILES:
        built_js_files = _build_js_files(ctx, transform_profile, precomputed_js_files)
        library_files = _build_library_files(ctx, transform_profile, flavors, built_js_files)

        js_library_deps = dedupe(
            map_idx(
                JsLibraryInfo,
                [dep[DefaultInfo].sub_targets[transform_profile] for dep in ctx.attrs.deps],
            )
        )
        # Dependency resolution only needs each dep's own module map (file
        # paths, haste IDs and package info), all of which is present in the
        # dep's `library_files` output. Depending on that instead of the dep's
        # resolved `.jslib` output keeps the depth of the action graph constant
        # instead of proportional to the depth of the js_library target graph.
        js_library = _build_js_library(
            ctx,
            transform_profile,
            library_files,
            flavors,
            [js_library_dep.library_files for js_library_dep in js_library_deps],
        )

        transitive_outputs = get_transitive_outputs(
            ctx.actions,
            value = js_library,
            deps = js_library_deps,
        )

        sub_targets[transform_profile] = [
            DefaultInfo(default_output = js_library),
            JsLibraryInfo(
                library_files = library_files,
                output = js_library,
                transitive_outputs = transitive_outputs,
            ),
        ]

    return [
        DefaultInfo(default_output = None, sub_targets = sub_targets),
    ]

js_library = prelude_rule(
    name = "js_library",
    impl = js_library_impl,
    docs = """
        A `js_library()` rule groups together JavaScript source files and assets so they
        can be consumed by a `js_bundle()`.
    """,
    examples = None,
    further = None,
    attrs = js_library_attrs(),
)
